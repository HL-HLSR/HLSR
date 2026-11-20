//! Linux: архивная установка в `~/.local/opt/hlsr` и самообновление (`launcher-update/archive.js`,
//! `embedded-payload.js`, `ArchiveInstall.js`, `ArchiveDownload.js`, `linux/UpdateManager.js` 0.4.7),
//! интеграция с рабочим столом (`linux/DesktopIntegration.js`), установщик AppImage (режим `Bootstrap`,
//! `BootstrapInstaller.js`) и удаление (`uninstall/launcher-uninstall.js`, `linux/library-cleanup.js`).
//!
//! Модули собираются на любой ОС: разбор USTAR и `payload.json`, байты `.desktop` и CLI, проверка
//! распакованной версии, трассы службы обновления проверяются и в Windows. Ссылки `current`/`previous`,
//! права файлов, `/proc` и `boot_id` — под `cfg(unix)`/`cfg(target_os = "linux")`; их тесты бегут на Linux.

pub mod archive;
pub mod archive_download;
pub mod archive_install;
pub mod bootstrap_installer;
pub mod desktop_integration;
pub mod embedded_payload;
pub mod launcher_uninstall;
pub mod library_cleanup;
pub mod update_manager;

use crate::cancel::CancelToken;
use crate::updater::processes::{RunExit, Spawn, SystemLauncher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Окружение процесса: пары имя → значение (`process.env`).
pub type EnvVars = Vec<(String, String)>;

/// Значение переменной окружения (имя с учётом регистра, как в Linux).
pub fn env_get<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Каталоги пользователя, которые 0.4.7 брала у `os.homedir()`, `process.env` и `app.getPath`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxDirs {
    /// `os.homedir()`.
    pub home: PathBuf,
    /// `XDG_DATA_HOME`, если путь абсолютный, иначе `~/.local/share` (`defaultDataHome`).
    pub data_home: PathBuf,
    /// Рабочий стол (`app.getPath("desktop")`): `XDG_DESKTOP_DIR` из `user-dirs.dirs`, иначе `~/Desktop`.
    pub desktop: PathBuf,
    /// Каталог данных лаунчера (`~/.config/hlsr`): там `library.json`.
    pub config: PathBuf,
}

impl LinuxDirs {
    /// Каталоги по домашнему каталогу и окружению; `config` — каталог данных лаунчера
    /// (`hlsr_core::paths::Paths`).
    pub fn from_env(home: &Path, env: &[(String, String)], config: &Path) -> LinuxDirs {
        let absolute = |name: &str| {
            env_get(env, name)
                .filter(|value| Path::new(value).is_absolute())
                .map(PathBuf::from)
        };
        let config_home = absolute("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        LinuxDirs {
            home: home.to_owned(),
            data_home: absolute("XDG_DATA_HOME")
                .unwrap_or_else(|| home.join(".local").join("share")),
            desktop: xdg_user_dir(home, &config_home, "DESKTOP")
                .unwrap_or_else(|| home.join("Desktop")),
            config: config.to_owned(),
        }
    }
}

/// `xdg_user_dir_lookup` (xdg-user-dirs, им пользуется Chromium для `app.getPath("desktop")`):
/// последняя строка `XDG_<NAME>_DIR="$HOME/…"` или `"/…"` из `<config>/user-dirs.dirs`; обратная
/// косая экранирует следующий символ. `None` — записи нет.
pub fn xdg_user_dir(home: &Path, config_home: &Path, name: &str) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config_home.join("user-dirs.dirs")).ok()?;
    let key = format!("XDG_{name}_DIR");
    let mut found = None;
    for line in text.lines() {
        let rest = line.trim_start_matches([' ', '\t']);
        let Some(rest) = rest.strip_prefix(key.as_str()) else {
            continue;
        };
        let rest = rest.trim_start_matches([' ', '\t']);
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start_matches([' ', '\t']);
        let Some(rest) = rest.strip_prefix('"') else {
            continue;
        };
        let (relative, rest) = match rest.strip_prefix("$HOME/") {
            Some(rest) => (true, rest),
            None if rest.starts_with('/') => (false, rest),
            None => continue,
        };
        let mut value = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => {
                    if let Some(next) = chars.next() {
                        value.push(next);
                    }
                }
                other => value.push(other),
            }
        }
        found = Some(if relative {
            home.join(value)
        } else {
            PathBuf::from(value)
        });
    }
    found
}

/// `path.resolve(p)` без обращения к ФС: абсолютный путь с разобранными `.` и `..`.
pub fn resolve(path: &Path) -> PathBuf {
    let native = hlsr_core::js::path::NodePath::native();
    PathBuf::from(native.resolve(&[&path.to_string_lossy()]))
}

/// `crypto.randomBytes(n).toString("hex")`.
pub fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("случайные байты ОС");
    hlsr_core::js::text::hex(&buffer)
}

/// `fs.mkdtempSync(prefix)`: каталог `<prefix>XXXXXX` (шесть символов `[A-Za-z0-9]`).
pub fn mkdtemp(prefix: &Path) -> io::Result<PathBuf> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let base = prefix.as_os_str().to_owned();
    for _ in 0..100 {
        let mut random = [0u8; 6];
        getrandom::fill(&mut random).expect("случайные байты ОС");
        let suffix: String = random
            .iter()
            .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
            .collect();
        let mut name = base.clone();
        name.push(&suffix);
        let candidate = PathBuf::from(name);
        match make_dir(&candidate, 0o700) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "mkdtemp: нет свободного имени",
    ))
}

/// `fs.mkdirSync(dir, {mode})` одного каталога (права — только Unix).
fn make_dir(dir: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(mode).create(dir)
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        std::fs::create_dir(dir)
    }
}

/// `fs.mkdirSync(dir, {recursive: true, mode})`: создаёт недостающие каталоги с правами `mode`
/// (с учётом umask, как Node).
pub fn make_dirs(dir: &Path, mode: u32) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    builder.create(dir)
}

/// `fs.chmodSync(file, mode)`; на Windows прав нет, как и у Node (ничего не меняется).
pub fn chmod(file: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        std::fs::metadata(file).map(|_| ())
    }
}

/// `fs.writeFileSync(file, data, {mode, flag})`: `exclusive` — `wx` (файла быть не должно).
pub fn write_file(file: &Path, data: &[u8], mode: u32, exclusive: bool) -> io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if exclusive {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    options.open(file)?.write_all(data)
}

/// Текст ошибки `fs` Node: `ENOENT: no such file or directory, open '<путь>'`.
pub fn fs_error(error: &io::Error, syscall: &str, path: &Path) -> String {
    hlsr_core::js::errno::fs_message(error, syscall, &path.to_string_lossy(), None)
}

/// Текст ошибки `fs` Node с двумя путями (`rename '<откуда>' -> '<куда>'`).
pub fn fs_error2(error: &io::Error, syscall: &str, from: &Path, to: &Path) -> String {
    hlsr_core::js::errno::fs_message(
        error,
        syscall,
        &from.to_string_lossy(),
        Some(&to.to_string_lossy()),
    )
}

/// `fs.rmSync(path, {recursive: true, force: true})`: ссылки не разворачиваются, отсутствие — не ошибка.
pub fn remove_all(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
    }
}

// ------------------------------------------------------------------- процессы

/// `AbortError` Node у `execFile(..., {signal})`: процесс снят по отмене.
pub const RUN_ABORTED: &str = "The operation was aborted";

/// `spawnSync`/`execFile` с ожиданием: запустить и ждать выхода не дольше `timeout` (дольше — процесс
/// снимается). `cancel` — `signal` у `execFile`: отмена снимает процесс сразу, ошибка — [`RUN_ABORTED`].
/// Ошибка — процесс не запустился (`spawnSync … ENOENT`) или снят по отмене.
pub trait SyncRunner: Send + Sync {
    fn run(
        &self,
        command: &Spawn,
        timeout: Duration,
        cancel: Option<&CancelToken>,
    ) -> Result<RunExit, String>;
}

/// Процессы ОС; вывод отбрасывается (`stdio: "ignore"`), окружение — только из команды, `hidden` в
/// Windows — без окна (как [`SystemLauncher`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRunner;

impl SyncRunner for SystemRunner {
    fn run(
        &self,
        command: &Spawn,
        timeout: Duration,
        cancel: Option<&CancelToken>,
    ) -> Result<RunExit, String> {
        let mut child = SystemLauncher::build(command)
            .spawn()
            .map_err(|error| SystemLauncher::spawn_error(command, &error))?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(RunExit::Exited(status.code())),
                Ok(None) => {}
                Err(error) => return Err(error.to_string()),
            }
            if cancel.is_some_and(CancelToken::is_cancelled) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RUN_ABORTED.to_owned());
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(RunExit::TimedOut);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

type RunResponder =
    Box<dyn Fn(&Spawn, Option<&CancelToken>) -> Result<RunExit, String> + Send + Sync>;

/// Подделка: записывает команды и сроки; ответ задаёт тест (по умолчанию — код 0), ему видна и отмена.
pub struct FakeRunner {
    calls: Mutex<Vec<(Spawn, Duration)>>,
    respond: RunResponder,
}

impl Default for FakeRunner {
    fn default() -> FakeRunner {
        FakeRunner::new()
    }
}

impl FakeRunner {
    pub fn new() -> FakeRunner {
        FakeRunner {
            calls: Mutex::new(Vec::new()),
            respond: Box::new(|_, _| Ok(RunExit::Exited(Some(0)))),
        }
    }

    pub fn on_run(
        mut self,
        respond: impl Fn(&Spawn, Option<&CancelToken>) -> Result<RunExit, String>
        + Send
        + Sync
        + 'static,
    ) -> FakeRunner {
        self.respond = Box::new(respond);
        self
    }

    pub fn calls(&self) -> Vec<(Spawn, Duration)> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl SyncRunner for FakeRunner {
    fn run(
        &self,
        command: &Spawn,
        timeout: Duration,
        cancel: Option<&CancelToken>,
    ) -> Result<RunExit, String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((command.clone(), timeout));
        (self.respond)(command, cancel)
    }
}
