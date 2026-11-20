//! Пространства имён пользователя для Steam Runtime (`linux/UserNamespaces.js`). Ubuntu 24.04+
//! запрещает их непривилегированным программам через AppArmor
//! (`kernel.apparmor_restrict_unprivileged_userns = 1`), и `srt-bwrap` из umu не запускается.
//! Тогда лаунчер один раз просит администратора (`pkexec`) поставить профиль AppArmor только для
//! `srt-bwrap` этого пользователя (`resources/linux/install-userns-profile.sh`); глобальная
//! политика не меняется.

use crate::linux::abort::Shared;
use crate::linux::prefix_command::ProtonError;
use crate::spawn::{Env, is_launcher_ui_var};
use hlsr_delivery::ports::{Logger, TracingLogger};
use hlsr_delivery::providers::BoxFuture;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Файл sysctl ограничения AppArmor.
pub const APPARMOR_SYSCTL: &str = "proc/sys/kernel/apparmor_restrict_unprivileged_userns";
/// `pkexec`.
pub const PKEXEC: &str = "/usr/bin/pkexec";
/// Помощник в ресурсах: `linux/install-userns-profile.sh`.
pub const PROFILE_HELPER: [&str; 2] = ["linux", "install-userns-profile.sh"];
/// Проверка `bwrap`: `--ro-bind / / --unshare-user -- /bin/true`.
pub const BWRAP_PROBE: [&str; 6] = ["--ro-bind", "/", "/", "--unshare-user", "--", "/bin/true"];

/// `umuRoot(env)`: `$UMU_FOLDERS_PATH/umu`, иначе `$XDG_DATA_HOME/umu` (во Flatpak —
/// `$HOST_XDG_DATA_HOME`), иначе `~/.local/share/umu`.
pub fn umu_root(env: &Env) -> PathBuf {
    let get = |name: &str| env.get(name).filter(|value| !value.is_empty());
    let data = if env.get("container").map(String::as_str) == Some("flatpak") {
        get("HOST_XDG_DATA_HOME")
    } else {
        get("XDG_DATA_HOME")
    };
    let base = get("UMU_FOLDERS_PATH")
        .or(data)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::home_dir()
                .unwrap_or_default()
                .join(".local")
                .join("share")
        });
    base.join("umu")
}

/// `appArmorRestricted()`: в sysctl под `root` (на Linux — `/`) записано `1`.
pub fn app_armor_restricted_in(root: &Path) -> bool {
    std::fs::read(root.join(APPARMOR_SYSCTL))
        .map(|bytes| hlsr_core::js::trim(&hlsr_core::js::utf8_string(&bytes)) == "1")
        .unwrap_or(false)
}

/// `appArmorRestricted()` на этой машине.
pub fn app_armor_restricted() -> bool {
    app_armor_restricted_in(Path::new("/"))
}

/// Аргументы `pkexec`: `/bin/bash <помощник> <uid> <каталог umu>`.
pub fn pkexec_args(helper: &Path, uid: u32, root: &Path) -> Vec<String> {
    vec![
        "/bin/bash".to_owned(),
        helper.to_string_lossy().into_owned(),
        uid.to_string(),
        root.to_string_lossy().into_owned(),
    ]
}

/// `srt-bwrap` первого `steamrt<N>` в каталоге umu.
pub fn find_bwrap(root: &Path) -> Option<PathBuf> {
    let names = hlsr_core::js::fs::read_dir_names(root).ok()?;
    names
        .iter()
        .filter(|name| is_steamrt(name))
        .map(|name| {
            root.join(name)
                .join("pressure-vessel")
                .join("libexec")
                .join("steam-runtime-tools-0")
                .join("srt-bwrap")
        })
        .find(|candidate| candidate.exists())
}

/// `/^steamrt\d+$/`.
fn is_steamrt(name: &str) -> bool {
    name.strip_prefix("steamrt")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Запуск команды (`execute(file, args, {env, timeout})`): `Err` — код выхода не 0, тайм-аут или
/// ошибка запуска; текст — сообщение и вывод ошибок.
pub trait Execute: Send + Sync {
    fn execute<'a>(
        &'a self,
        file: &'a str,
        args: &'a [String],
        env: &'a Env,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<String, ProtonError>>;
}

/// `execFile` (`maxBuffer` 8192).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemExecute;

impl Execute for SystemExecute {
    fn execute<'a>(
        &'a self,
        file: &'a str,
        args: &'a [String],
        env: &'a Env,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<String, ProtonError>> {
        Box::pin(exec_file(file, args, env, timeout, 8192))
    }
}

/// `execFile(file, args, {env, encoding: "utf8", timeout, maxBuffer})`: вывод при коде 0;
/// иначе `Command failed: <команда>\n<stderr>`.
pub async fn exec_file(
    file: &str,
    args: &[String],
    env: &Env,
    timeout: Duration,
    max_buffer: usize,
) -> Result<String, ProtonError> {
    let mut command = tokio::process::Command::new(file);
    command
        .args(args)
        .env_clear()
        .envs(env.iter().filter(|(name, _)| !is_launcher_ui_var(name)))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let failed = |detail: &str| {
        let line = std::iter::once(file.to_owned())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        ProtonError::new(
            hlsr_core::js::trim(&format!("Command failed: {line}\n{detail}")).to_owned(),
        )
    };
    let output = match tokio::time::timeout(timeout, command.output()).await {
        Err(_) => return Err(failed("")),
        Ok(Err(error)) => {
            let spawn = crate::spawn::spawn_error(file, &error);
            return Err(ProtonError::from(spawn));
        }
        Ok(Ok(output)) => output,
    };
    if output.stdout.len() > max_buffer || output.stderr.len() > max_buffer {
        return Err(ProtonError::with_code(
            "ERR_CHILD_PROCESS_STDIO_MAXBUFFER",
            "stdout maxBuffer length exceeded",
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(stdout)
    } else {
        Err(failed(&String::from_utf8_lossy(&output.stderr)))
    }
}

/// Что умеет проверка пространств имён (`ProtonRunner` зовёт `ensure(env)`).
pub trait UserNamespacesCheck: Send + Sync {
    fn ensure<'a>(&'a self, env: &'a Env) -> BoxFuture<'a, Result<(), ProtonError>>;
}

/// Параметры `UserNamespaces`.
#[derive(Clone, Default)]
pub struct UserNamespacesOptions {
    /// `resourcesPath` (рядом с лаунчером).
    pub resources_path: PathBuf,
    pub logger: Option<Arc<dyn Logger>>,
    /// `isRestricted`; по умолчанию — sysctl этой машины.
    pub is_restricted: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// `execute`; по умолчанию — `execFile`.
    pub execute: Option<Arc<dyn Execute>>,
    /// `process.getuid()`; по умолчанию — пользователь процесса (не Linux — 0).
    pub uid: Option<u32>,
}

/// `UserNamespaces`.
pub struct UserNamespaces {
    resources_path: PathBuf,
    logger: Arc<dyn Logger>,
    is_restricted: Arc<dyn Fn() -> bool + Send + Sync>,
    execute: Arc<dyn Execute>,
    uid: u32,
    pending: Mutex<Option<Shared<Result<(), ProtonError>>>>,
}

impl UserNamespaces {
    pub fn new(options: UserNamespacesOptions) -> Arc<UserNamespaces> {
        Arc::new(UserNamespaces {
            resources_path: options.resources_path,
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            is_restricted: options
                .is_restricted
                .unwrap_or_else(|| Arc::new(app_armor_restricted)),
            execute: options.execute.unwrap_or_else(|| Arc::new(SystemExecute)),
            uid: options
                .uid
                .or_else(crate::linux::steam_locator::current_uid)
                .unwrap_or(0),
            pending: Mutex::new(None),
        })
    }

    /// `ensure(env)`: одновременные вызовы ждут одну проверку.
    pub async fn ensure(self: &Arc<Self>, env: &Env) -> Result<(), ProtonError> {
        let shared = {
            let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            match pending.as_ref() {
                Some(shared) => shared.clone(),
                None => {
                    let this = self.clone();
                    let env = env.clone();
                    let cleaner = self.clone();
                    let shared = Shared::spawn(async move { this.check(&env).await }, move || {
                        *cleaner.pending.lock().unwrap_or_else(|p| p.into_inner()) = None;
                    });
                    *pending = Some(shared.clone());
                    shared
                }
            }
        };
        shared.wait().await
    }

    /// `check(env)`.
    pub async fn check(&self, env: &Env) -> Result<(), ProtonError> {
        if !(self.is_restricted)() {
            return Ok(());
        }
        let root = umu_root(env);
        let Some(bwrap) = find_bwrap(&root) else {
            return Ok(());
        };
        let bwrap = bwrap.to_string_lossy().into_owned();
        let probe: Vec<String> = BWRAP_PROBE.iter().map(|arg| (*arg).to_owned()).collect();
        let short = Duration::from_millis(10_000);
        if self
            .execute
            .execute(&bwrap, &probe, env, short)
            .await
            .is_ok()
        {
            return Ok(());
        }
        let helper = self
            .resources_path
            .join(PROFILE_HELPER[0])
            .join(PROFILE_HELPER[1]);
        self.logger.info(
            "[runtime] AppArmor блокирует umu; ОС запросит разрешение администратора для профиля HLSR",
        );
        let granted = async {
            self.execute
                .execute(
                    PKEXEC,
                    &pkexec_args(&helper, self.uid, &root),
                    env,
                    Duration::from_millis(120_000),
                )
                .await?;
            self.execute.execute(&bwrap, &probe, env, short).await?;
            Ok::<(), ProtonError>(())
        };
        granted.await.map_err(|error| {
            ProtonError::with_code(
                "LINUX_USERNS_BLOCKED",
                format!(
                    "AppArmor блокирует Steam Runtime. Для подготовки профиля HLSR требуется подтверждение администратора через PolicyKit (pkexec). {}",
                    error.message
                ),
            )
        })
    }
}

impl UserNamespacesCheck for Arc<UserNamespaces> {
    fn ensure<'a>(&'a self, env: &'a Env) -> BoxFuture<'a, Result<(), ProtonError>> {
        Box::pin(UserNamespaces::ensure(self, env))
    }
}
