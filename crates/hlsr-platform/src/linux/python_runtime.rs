//! Python для umu (`linux/PythonRuntime.js`): `umu-run` — zipapp с `#!/usr/bin/env python3`, ему
//! нужен Python ≥ 3.10 с `ssl`, `lzma` и `ctypes`. Сначала ищется подходящий в `PATH`, иначе
//! лаунчер один раз скачивает python-build-standalone 3.12.15 (размер и SHA-256 закреплены),
//! проверяет список архива и распаковывает в `<данные>/runtimes/python-3.12.15-20261001`.

use crate::linux::abort::Shared;
use crate::linux::prefix_command::ProtonError;
use crate::linux::proton_manager::{
    Fetch, FetchRules, GithubFetch, tar_program, validate_archive_listing_with,
};
use crate::linux::user_namespaces::exec_file;
use crate::spawn::{Env, PATH_DELIMITER};
use hlsr_delivery::ports::{Logger, TracingLogger};
use hlsr_delivery::providers::BoxFuture;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt as _;

/// Закреплённая сборка Python (`DISTRIBUTION`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Distribution {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub url: String,
}

/// `DISTRIBUTION`.
pub fn distribution() -> Distribution {
    Distribution {
        name: "cpython-3.12.15+20261001-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz"
            .to_owned(),
        size: 34_289_539,
        sha256: "7bb1659e3235077b7f63d5b6eb6ce653c6fcd6c5041e9d5f73b42ce10421464d".to_owned(),
        url: "https://github.com/astral-sh/python-build-standalone/releases/download/20261001/cpython-3.12.15%2B20261001-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz".to_owned(),
    }
}

/// `HOSTS`: откуда можно качать Python.
pub const HOSTS: [&str; 3] = [
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
];
/// `PROBE`.
pub const PROBE: &str = "import sys,ssl,lzma,ctypes; assert sys.version_info >= (3,10)";
/// Каталог своей копии (от папки данных).
pub const RUNTIME_DIR: [&str; 2] = ["runtimes", "python-3.12.15-20261001"];

/// Правила загрузки Python (`request` в `PythonRuntime.js`).
pub fn fetch_rules() -> FetchRules {
    FetchRules {
        hosts: &HOSTS,
        accept: None,
        untrusted: "Недоверенный адрес загрузки Python",
        timeout: "Тайм-аут загрузки Python",
        redirect_without_location: None,
        status: |code| format!("Загрузка Python: HTTP {code}"),
    }
}

/// `compatiblePython(file, env)`: интерпретатор запускается и проходит [`PROBE`].
pub async fn compatible_python(file: &Path, env: &Env) -> bool {
    let args = ["-I".to_owned(), "-c".to_owned(), PROBE.to_owned()];
    exec_file(
        &file.to_string_lossy(),
        &args,
        env,
        Duration::from_millis(60_000),
        4096,
    )
    .await
    .is_ok()
}

/// Проверка интерпретатора (`probe`).
pub type Probe = Arc<dyn Fn(PathBuf, Env) -> BoxFuture<'static, bool> + Send + Sync>;

/// Что от Python нужно запуску (`ProtonRunner` зовёт `prepareEnvironment(env)`).
pub trait PythonEnvironment: Send + Sync {
    fn prepare_environment<'a>(&'a self, env: Env) -> BoxFuture<'a, Result<Env, ProtonError>>;
}

/// Параметры `PythonRuntime`.
#[derive(Clone, Default)]
pub struct PythonOptions {
    pub data_dir: PathBuf,
    pub logger: Option<Arc<dyn Logger>>,
    /// `probe`; по умолчанию — [`compatible_python`]. Им же проверяется и распакованная копия.
    pub probe: Option<Probe>,
    /// Загрузка; по умолчанию — GitHub по HTTPS.
    pub fetch: Option<Arc<dyn Fetch>>,
    /// Закреплённая сборка; по умолчанию — [`distribution`] (тесты подставляют маленький архив).
    pub distribution: Option<Distribution>,
}

/// `PythonRuntime`.
pub struct PythonRuntime {
    root: PathBuf,
    logger: Arc<dyn Logger>,
    probe: Probe,
    fetch: Arc<dyn Fetch>,
    distribution: Distribution,
    pending: Mutex<Option<Shared<Result<PathBuf, ProtonError>>>>,
    selected: Mutex<Option<PathBuf>>,
}

impl PythonRuntime {
    pub fn new(options: PythonOptions) -> Arc<PythonRuntime> {
        let root = options.data_dir.join(RUNTIME_DIR[0]).join(RUNTIME_DIR[1]);
        Arc::new(PythonRuntime {
            root,
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            probe: options.probe.unwrap_or_else(|| {
                Arc::new(|file: PathBuf, env: Env| {
                    Box::pin(async move { compatible_python(&file, &env).await })
                        as BoxFuture<'static, bool>
                })
            }),
            fetch: options
                .fetch
                .unwrap_or_else(|| Arc::new(GithubFetch::new(fetch_rules()))),
            distribution: options.distribution.unwrap_or_else(distribution),
            pending: Mutex::new(None),
            selected: Mutex::new(None),
        })
    }

    /// Своя копия: `<данные>/runtimes/python-3.12.15-20261001`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Идёт поиск или загрузка (`pending !== null`).
    pub fn is_pending(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    /// `ensure(env)`: найденный раньше интерпретатор; одновременные вызовы ждут один поиск.
    pub async fn ensure(self: &Arc<Self>, env: &Env) -> Result<PathBuf, ProtonError> {
        if let Some(selected) = self
            .selected
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            return Ok(selected);
        }
        let shared = {
            let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            match pending.as_ref() {
                Some(shared) => shared.clone(),
                None => {
                    let this = self.clone();
                    let env = env.clone();
                    let cleaner = self.clone();
                    let shared =
                        Shared::spawn(async move { this.find_or_install(&env).await }, move || {
                            *cleaner.pending.lock().unwrap_or_else(|p| p.into_inner()) = None;
                        });
                    *pending = Some(shared.clone());
                    shared
                }
            }
        };
        shared.wait().await
    }

    fn select(&self, file: PathBuf) -> PathBuf {
        *self.selected.lock().unwrap_or_else(|p| p.into_inner()) = Some(file.clone());
        file
    }

    async fn probe(&self, file: &Path, env: &Env) -> bool {
        (self.probe)(file.to_path_buf(), env.clone()).await
    }

    /// `findOrInstall(env)`: `python3` из того же `PATH`, что увидит `#!/usr/bin/env python3`
    /// umu; затем своя копия; затем загрузка.
    pub async fn find_or_install(&self, env: &Env) -> Result<PathBuf, ProtonError> {
        let path = env.get("PATH").map(String::as_str).unwrap_or("");
        for directory in path.split(PATH_DELIMITER).filter(|dir| !dir.is_empty()) {
            let candidate = Path::new(directory).join("python3");
            if self.probe(&candidate, env).await {
                return Ok(self.select(candidate));
            }
        }
        let managed = self.root.join("bin").join("python3");
        if self.probe(&managed, env).await {
            return Ok(self.select(managed));
        }
        self.logger
            .info("[runtime] Python 3.10+ отсутствует или неполон; скачиваю Python для umu");
        self.install(env).await?;
        if !self.probe(&managed, env).await {
            return Err(ProtonError::new(
                "Скачанный Python не запускается. Нужен Linux x86_64 с glibc.",
            ));
        }
        Ok(self.select(managed))
    }

    /// `install(env)`: загрузка во временную папку рядом, сверка размера и SHA-256, проверка
    /// списка архива, распаковка, проверка интерпретатора и замена своей копии.
    pub async fn install(&self, env: &Env) -> Result<(), ProtonError> {
        let parent = self.root.parent().unwrap_or(&self.root).to_path_buf();
        std::fs::create_dir_all(&parent).map_err(io_error)?;
        let staging = make_temp_dir(&parent, ".python-")?;
        let result = self.install_into(&staging, env).await;
        let _ = std::fs::remove_dir_all(&staging);
        result
    }

    async fn install_into(&self, staging: &Path, env: &Env) -> Result<(), ProtonError> {
        let archive = staging.join("python.tar.gz");
        let mut body = self.fetch.request(&self.distribution.url, None).await?;
        let mut file = open_private(&archive).await?;
        let mut hash = Sha256::new();
        let mut size: u64 = 0;
        while let Some(chunk) = body.chunk().await? {
            size += chunk.len() as u64;
            hash.update(&chunk);
            if size > self.distribution.size {
                return Err(ProtonError::new("Архив Python слишком велик"));
            }
            file.write_all(&chunk).await.map_err(io_error)?;
        }
        file.flush().await.map_err(io_error)?;
        drop(file);
        if size != self.distribution.size
            || hlsr_delivery::file_ops::hex(&hash.finalize()) != self.distribution.sha256
        {
            return Err(ProtonError::new(
                "Размер или SHA-256 архива Python не совпал",
            ));
        }
        let tar = tar_program();
        let file_arg = archive.to_string_lossy().into_owned();
        let minute = Duration::from_millis(60_000);
        let limit = 16 * 1024 * 1024;
        let names_args = ["-tzf".to_owned(), file_arg.clone()];
        let verbose_args = ["-tvzf".to_owned(), file_arg.clone()];
        let (names, verbose) = tokio::join!(
            exec_file(&tar, &names_args, env, minute, limit),
            exec_file(&tar, &verbose_args, env, minute, limit),
        );
        validate_archive_listing_with(&names?, &verbose?, "python", |root| root == "python")?;
        let extract = [
            "-xzf".to_owned(),
            file_arg,
            "-C".to_owned(),
            staging.to_string_lossy().into_owned(),
            "--no-same-owner".to_owned(),
            "--no-same-permissions".to_owned(),
        ];
        exec_file(&tar, &extract, env, minute, limit).await?;
        let extracted = staging.join("python");
        if !self
            .probe(&extracted.join("bin").join("python3"), env)
            .await
        {
            return Err(ProtonError::new("Проверка скачанного Python не пройдена"));
        }
        // Заменяется только своя копия и только после проверки.
        let _ = std::fs::remove_dir_all(&self.root);
        std::fs::rename(&extracted, &self.root).map_err(io_error)?;
        self.logger
            .info("[runtime] Python 3.12.15 установлен для umu");
        Ok(())
    }

    /// `prepareEnvironment(env)`: папка Python первой в `PATH` и `PYTHONNOUSERSITE=1`.
    pub async fn prepare_environment(self: &Arc<Self>, env: Env) -> Result<Env, ProtonError> {
        let python = self.ensure(&env).await?;
        let mut next = env.clone();
        let dir = python
            .parent()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default();
        next.insert(
            "PATH".to_owned(),
            format!(
                "{dir}{PATH_DELIMITER}{}",
                env.get("PATH").map(String::as_str).unwrap_or("")
            ),
        );
        next.insert("PYTHONNOUSERSITE".to_owned(), "1".to_owned());
        Ok(next)
    }
}

impl PythonEnvironment for Arc<PythonRuntime> {
    fn prepare_environment<'a>(&'a self, env: Env) -> BoxFuture<'a, Result<Env, ProtonError>> {
        Box::pin(PythonRuntime::prepare_environment(self, env))
    }
}

/// Ошибка файловой системы: текст как у Node.
pub(crate) fn io_error(error: std::io::Error) -> ProtonError {
    ProtonError {
        code: hlsr_core::js::errno::code(&error).map(std::borrow::Cow::into_owned),
        message: error.to_string(),
        ..ProtonError::default()
    }
}

/// `fs.mkdtempSync(prefix)`: новая папка `<prefix>XXXXXX`.
pub(crate) fn make_temp_dir(parent: &Path, prefix: &str) -> Result<PathBuf, ProtonError> {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    for _ in 0..100 {
        let mut bytes = [0u8; 6];
        getrandom::fill(&mut bytes).expect("источник случайных чисел ОС");
        let suffix: String = bytes
            .iter()
            .map(|b| CHARS[usize::from(*b) % CHARS.len()] as char)
            .collect();
        let dir = parent.join(format!("{prefix}{suffix}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Err(ProtonError::new("EEXIST: временная папка не создана"))
}

/// Файл с правами 0600 (`createWriteStream(file, {mode: 0o600})`).
pub(crate) async fn open_private(file: &Path) -> Result<tokio::fs::File, ProtonError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(file).await.map_err(io_error)
}
