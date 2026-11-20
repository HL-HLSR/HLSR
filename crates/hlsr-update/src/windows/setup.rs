//! Режимы установщика в Windows-сборке HLSR (`windows-install/setup.js`):
//! - **setup** — `HLSR-Setup-<версия>.exe` (нагрузка в своём exe, Р1) или `HLSR.exe` с
//!   `PORTABLE_EXECUTABLE_FILE` (путь В 0.4.7): окно установки, а с `/S` — тихое обновление, как его
//!   запускают обновляторы прежних версий;
//! - **uninstall** — `HLSR.exe --hlsr-uninstall` из «Программ и компонентов».
//!
//! Режим выбирает `hlsr_core::args` (`LaunchMode::Setup`/`Uninstall`), запускает приложение
//! (задача 9). Здесь: тихие режимы целиком (коды выхода `0/1/3/4`, блокировка, ожидание прежней
//! версии до 60 с, запуск установленного `HLSR.exe`) и backend окна для задачи 10.

use super::file_list::{self, FileList};
use super::install::{self, InstallError, InstallOptions, sz_value};
use super::known_folders::KnownFolders;
use super::layout::{self, EnvLookup, Identity, win_path};
use super::payload::{self, Payload, Source};
use super::processes::{Command, Env, Processes, SpawnError, launch_environment};
use super::registry::{self, RegValue, Registry};
use super::shortcuts::{ShellLinks, Shortcuts};
use super::uninstall::{self, UninstallOptions, UninstallStatus};
use crate::channel::compare_semver;
use crate::errors::setup_exit as exit;
use crate::installer_window::{
    BackendError, Ctx, InstallerBackend, InstallerState, Phase, StatePatch, UninstallPlan,
};
use crate::legacy_traces;
use crate::uninstall::cleanup_cli::{self, RunOptions, UninstallTelemetry};
use crate::uninstall::library_cleanup::{self, CleanupOptions, SteamLibraries};
use hlsr_core::args::SetupArgs;
use hlsr_core::logger::Logger;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Ожидание выхода прежней версии в тихом режиме.
pub const LOCK_WAIT: Duration = Duration::from_secs(60);
/// Повторы запуска свежего `HLSR.exe` (антивирус держит exe): 6 × 1 с.
pub const LAUNCH_ATTEMPTS: u32 = 6;
pub const LAUNCH_RETRY: Duration = Duration::from_secs(1);
/// Флаги совместимости exe («Запускать от имени администратора»).
pub const COMPAT_LAYERS: &str =
    "Software\\Microsoft\\Windows NT\\CurrentVersion\\AppCompatFlags\\Layers";
/// Ошибка окна, пока лаунчер запущен.
pub const LOCK_ERROR: &str =
    "HLSR сейчас запущен. Закройте лаунчер и нажмите «Повторить установку».";

// ---------------------------------------------------------------------------------- блокировка

/// Блокировка единственного экземпляра (`app.requestSingleInstanceLock` 0.4.7): пока она у
/// установщика или удаления, HLSR не запустится; не взялась — HLSR работает и его файлы заняты.
pub trait InstanceLock: Send + Sync {
    fn try_acquire(&self) -> bool;
    fn release(&self);
    fn is_held(&self) -> bool;
}

/// Блокировка 0.5 плюс ожидание 0.4.7: пока Electron держит `<данные>\lockfile`
/// (`hlsr_core::instance::electron_running`), блокировка не берётся.
pub struct LegacyAwareLock {
    data_dir: PathBuf,
    electron_running: fn(&Path) -> bool,
    inner: Arc<dyn InstanceLock>,
}

impl LegacyAwareLock {
    pub fn new(data_dir: &Path, inner: Arc<dyn InstanceLock>) -> LegacyAwareLock {
        LegacyAwareLock::with_check(data_dir, inner, hlsr_core::instance::electron_running)
    }

    pub fn with_check(
        data_dir: &Path,
        inner: Arc<dyn InstanceLock>,
        electron_running: fn(&Path) -> bool,
    ) -> LegacyAwareLock {
        LegacyAwareLock {
            data_dir: data_dir.to_owned(),
            electron_running,
            inner,
        }
    }
}

impl InstanceLock for LegacyAwareLock {
    fn try_acquire(&self) -> bool {
        if self.inner.is_held() {
            return true;
        }
        !(self.electron_running)(&self.data_dir) && self.inner.try_acquire()
    }

    fn release(&self) {
        self.inner.release();
    }

    fn is_held(&self) -> bool {
        self.inner.is_held()
    }
}

/// Подделка: блокировка освобождается после `busy_attempts` неудачных попыток.
#[derive(Debug, Default)]
pub struct FakeLock {
    held: AtomicBool,
    busy_attempts: AtomicU32,
    attempts: AtomicU32,
    releases: AtomicU32,
}

impl FakeLock {
    pub fn new(busy_attempts: u32) -> FakeLock {
        FakeLock {
            busy_attempts: AtomicU32::new(busy_attempts),
            ..FakeLock::default()
        }
    }

    pub fn attempts(&self) -> u32 {
        self.attempts.load(Ordering::SeqCst)
    }

    pub fn releases(&self) -> u32 {
        self.releases.load(Ordering::SeqCst)
    }
}

impl InstanceLock for FakeLock {
    fn try_acquire(&self) -> bool {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt < self.busy_attempts.load(Ordering::SeqCst) {
            return false;
        }
        self.held.store(true, Ordering::SeqCst);
        true
    }

    fn release(&self) {
        self.releases.fetch_add(1, Ordering::SeqCst);
        self.held.store(false, Ordering::SeqCst);
    }

    fn is_held(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }
}

/// `acquireLock(app, waitMs)`: попытка сразу и затем раз в секунду, пока не пройдёт `wait`.
pub fn acquire_lock(lock: &dyn InstanceLock, wait: Duration, sleep: &dyn Fn(Duration)) -> bool {
    let attempts = wait.as_secs() + 1;
    for attempt in 0..attempts {
        if lock.try_acquire() {
            return true;
        }
        if attempt + 1 < attempts {
            sleep(Duration::from_secs(1));
        }
    }
    false
}

// ---------------------------------------------------------------------------------- сборка

/// Источник файлов установщика: нагрузка своего exe или, при `PORTABLE_EXECUTABLE_FILE`, каталог
/// exe (путь В). Ошибка — текст для «Установщик повреждён: …».
pub fn open_source(exe: &Path) -> Result<Source, String> {
    if payload::has_trailer(exe) {
        return Payload::open(exe)
            .map(|payload| Source::Payload(Box::new(payload)))
            .map_err(|error| error.to_string());
    }
    Ok(Source::Dir(
        exe.parent().map(Path::to_path_buf).unwrap_or_default(),
    ))
}

/// Сборка, которую ставит установщик или удаляет удаление (`buildOf`): её список файлов и
/// identity. Без списка удаление всё же работает — с ключами боевого HLSR.
#[derive(Debug, Clone)]
pub struct Build {
    pub list: Option<FileList>,
    pub identity: Identity,
    pub error: Option<String>,
}

impl Build {
    /// Версия, которая ставится (реестр, «готово»): из списка файлов нагрузки, без списка —
    /// `app.getVersion()`. У выпуска они равны, как в 0.4.9; пробная упаковка `--dev` кладёт exe
    /// `0.5.0-dev` в нагрузку `0.5.0`. Одна точка выбора для окна и тихой установки.
    pub fn version_or<'a>(&'a self, app_version: &'a str) -> &'a str {
        self.list.as_ref().map_or(app_version, |list| &list.version)
    }
}

fn production_identity() -> Identity {
    layout::identity(None).expect("боевое имя продукта допустимо")
}

/// `buildOf(dir, log)`.
pub fn build_of(source: &Result<Source, String>, log: &dyn Logger) -> Build {
    let list = match source {
        Ok(source) => source.read_list().map_err(|error| error.to_string()),
        Err(error) => Err(error.clone()),
    };
    let checked = list.and_then(|list| {
        layout::identity(Some(&list.app))
            .map(|identity| (list, identity))
            .map_err(|error| error.to_string())
    });
    match checked {
        Ok((list, identity)) => Build {
            list: Some(list),
            identity,
            error: None,
        },
        Err(error) => {
            log.warn(&format!("[setup] {error}"));
            Build {
                list: None,
                identity: production_identity(),
                error: Some(error),
            }
        }
    }
}

/// Имя приложения сборки, из которой запущен `exe` (`app.getName()` 0.4.7 — `name` упакованного
/// `package.json`), по `app` её списка файлов — из тех же мест, что у [`build_of`]: нагрузка
/// установщика (`HLSR-Setup-<v>.exe`) или `resources\install-files.json` рядом с exe (установленный
/// лаунчер, удаление, путь В). Нужно до журнала — от имени зависит его каталог, — поэтому без
/// записей; хеш нагрузки сверит сам установщик. Списка нет (отладочная сборка, Linux) или он
/// битый — боевое имя, как боевые ключи у [`build_of`].
pub fn app_name_of(exe: &Path) -> &'static str {
    let list = if payload::has_trailer(exe) {
        payload::read_list(exe).ok()
    } else {
        exe.parent().and_then(|dir| file_list::read(dir).ok())
    };
    list.map_or(layout::PRODUCTION_NAME, |list| list.app.name())
}

/// `Установщик повреждён: <причина>. Скачайте его заново.`
pub fn broken_setup(build: &Build) -> Option<String> {
    build
        .error
        .as_ref()
        .map(|error| format!("Установщик повреждён: {error}. Скачайте его заново."))
}

/// `resolveInstallDir(args, values, id)`: `/D=`, затем сохранённый каталог (если NSIS не пометил
/// его незаконченным и он допустим), затем каталог по умолчанию.
pub fn resolve_install_dir(
    args: &SetupArgs,
    values: &[(String, RegValue)],
    id: &Identity,
    env: EnvLookup<'_>,
) -> String {
    let path = win_path();
    if let Some(dir) = &args.dir {
        return path.resolve(&[dir]);
    }
    let saved = sz_value(values, "InstallLocation").filter(|saved| !saved.is_empty());
    if let Some(saved) = saved
        && sz_value(values, layout::SEED_FLAG).as_deref() != Some("1")
        && layout::refused_dir(&saved, env).is_none()
    {
        return path.resolve(&[&saved]);
    }
    id.default_install_dir.clone()
}

/// Корень библиотеки: `libraryPath` из `library.json` профиля, иначе `C:\HLSR`.
pub fn library_root(user_data: &Path) -> String {
    cleanup_cli::library_path_from(user_data).unwrap_or_else(|| layout::LIBRARY_ROOT.to_owned())
}

/// Язык окна: метка `-ru.exe`/`-en.exe` в имени установщика, иначе язык системы.
pub fn language(locale: &str, setup_file: Option<&str>) -> &'static str {
    if let Some(origin) = setup_file.and_then(registry::origin_of) {
        return origin;
    }
    if locale.starts_with("ru") { "ru" } else { "en" }
}

/// Установщик, к которому ведёт «Назад» после удаления (`returnSetupOf`): только существующий
/// `.exe` по абсолютному пути.
pub fn return_setup_of(file: Option<&str>) -> Option<String> {
    let value = file.unwrap_or_default();
    let exe = value.len() >= 4 && value[value.len() - 4..].eq_ignore_ascii_case(".exe");
    if !win_path().is_absolute(value) || !exe || value.contains(['\r', '\n', '\0']) {
        return None;
    }
    std::fs::metadata(value)
        .is_ok_and(|stat| stat.is_file())
        .then(|| value.to_owned())
}

// ---------------------------------------------------------------------------------- запуск

/// `\bRUNASADMIN\b` без учёта регистра в данных флага совместимости.
pub fn has_runasadmin(data: &str) -> bool {
    let lower = data.to_ascii_lowercase();
    let word = |byte: Option<&u8>| byte.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(found) = lower[from..].find("runasadmin") {
        let start = from + found;
        let end = start + "runasadmin".len();
        let before = start.checked_sub(1).and_then(|at| bytes.get(at));
        if !word(before) && !word(bytes.get(end)) {
            return true;
        }
        from = start + 1;
    }
    false
}

/// `runsAsAdmin(exe)`: у exe стоит «Запускать от имени администратора» — значение с путём exe в
/// `AppCompatFlags\Layers` (у пользователя или у всех), в данных `RUNASADMIN`. С ним `CreateProcess`
/// из обычного процесса не запустит exe.
pub fn runs_as_admin(exe: &str, registry: &dyn Registry) -> bool {
    ["HKCU", "HKLM"].iter().any(|hive| {
        registry
            .read_value(&format!("{hive}\\{COMPAT_LAYERS}"), exe)
            .is_some_and(|value| has_runasadmin(&value.data_text()))
    })
}

/// Запуск через оболочку Windows (`Start-Process` PowerShell — это `ShellExecute`: учитывает флаг
/// совместимости и сам спрашивает UAC, а в отличие от `shell.openPath` передаёт аргументы).
/// Окружение — без переменных распаковщика и окна. `true` — PowerShell вышел с кодом 0.
pub fn shell_start(
    processes: &dyn Processes,
    env: &Env,
    exe: &str,
    args: &[String],
    cwd: &str,
) -> bool {
    let mut command_env = launch_environment(env);
    command_env.set("HLSR_EXE", exe);
    command_env.set("HLSR_DIR", cwd);
    command_env.set("HLSR_ARGS", &args.join(" "));
    let mut script =
        "Start-Process -FilePath $env:HLSR_EXE -WorkingDirectory $env:HLSR_DIR".to_owned();
    if !args.is_empty() {
        script.push_str(" -ArgumentList $env:HLSR_ARGS");
    }
    let command = Command {
        file: uninstall::powershell_path(env),
        args: [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ]
        .iter()
        .map(|arg| (*arg).to_owned())
        .chain(Some(script))
        .collect(),
        cwd: None,
        env: command_env,
        hidden: true,
    };
    processes.run(&command, None) == Some(0)
}

/// Всё, что нужно запуску установленного `HLSR.exe`.
pub struct LaunchDeps<'a> {
    pub lock: &'a dyn InstanceLock,
    pub registry: &'a dyn Registry,
    pub processes: &'a dyn Processes,
    pub env: &'a Env,
    pub sleep: &'a dyn Fn(Duration),
    pub log: &'a dyn Logger,
    /// Есть ли запуск через оболочку (в 0.4.7 — только на win32).
    pub shell: bool,
}

/// `launchInstalled(app, installDir, deps, args)`: блокировка отпускается; при флаге
/// администратора — сразу оболочка; иначе `CreateProcess` до 6 раз по 1 с при `EACCES`/`EPERM`/
/// `EBUSY` (антивирус проверяет свежий exe), в конце — оболочка. Ошибка — исходная ошибка запуска.
pub fn launch_installed(
    deps: &LaunchDeps<'_>,
    install_dir: &str,
    args: &[String],
) -> Result<(), SpawnError> {
    let exe = win_path().join(&[install_dir, layout::EXE]);
    deps.lock.release();
    let open_with_shell = || {
        let ok = shell_start(deps.processes, deps.env, &exe, args, install_dir);
        if !ok {
            deps.log
                .warn("[setup] запуск HLSR.exe через оболочку не удался");
        }
        ok
    };
    if deps.shell && runs_as_admin(&exe, deps.registry) {
        deps.log.warn(
            "[setup] у HLSR.exe стоит «Запускать от имени администратора»: запуск через оболочку",
        );
        if open_with_shell() {
            return Ok(());
        }
    }
    let command = Command {
        file: exe.clone(),
        args: args.to_vec(),
        cwd: Some(install_dir.to_owned()),
        env: launch_environment(deps.env),
        hidden: false,
    };
    let mut last_error = None;
    for attempt in 1..=LAUNCH_ATTEMPTS {
        match deps.processes.spawn(&command) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let retry = matches!(error.code.as_deref(), Some("EACCES" | "EPERM" | "EBUSY"));
                if !retry {
                    return Err(error);
                }
                deps.log.warn(&format!(
                    "[setup] запуск HLSR.exe: {}, попытка {attempt} из {LAUNCH_ATTEMPTS}",
                    error.code.as_deref().unwrap_or_default()
                ));
                last_error = Some(error);
                if attempt < LAUNCH_ATTEMPTS {
                    (deps.sleep)(LAUNCH_RETRY);
                }
            }
        }
    }
    let error = last_error.expect("была хотя бы одна попытка");
    if !deps.shell || !open_with_shell() {
        return Err(error);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------- службы

/// Службы режимов установщика (`services(electron, identity)` и `app` 0.4.7).
#[derive(Clone)]
pub struct SetupServices {
    pub registry: Arc<dyn Registry>,
    pub links: Arc<dyn ShellLinks>,
    pub folders: Arc<dyn KnownFolders>,
    pub processes: Arc<dyn Processes>,
    pub lock: Arc<dyn InstanceLock>,
    pub log: Arc<dyn Logger>,
    /// `process.env` (снимок; dev-hooks подменяют папки пользователя).
    pub env: Env,
    pub sleep: Arc<dyn Fn(Duration) + Send + Sync>,
    /// `app.getPath("userData")`: `%APPDATA%\hlsr`, у QA-сборки — `%APPDATA%\hlsr-qa`.
    pub user_data: PathBuf,
    /// `app.getName()`: [`app_name_of`] своего exe.
    pub app_name: String,
    /// `app.getVersion()`.
    pub version: String,
    /// `app.getLocale()`.
    pub locale: String,
    /// `process.execPath`.
    pub exe: PathBuf,
    pub steam: Arc<dyn SteamLibraries>,
    pub telemetry: Option<Arc<dyn UninstallTelemetry>>,
    /// Есть ли запуск через оболочку Windows.
    pub shell: bool,
}

impl SetupServices {
    fn launch_deps(&self) -> LaunchDeps<'_> {
        LaunchDeps {
            lock: self.lock.as_ref(),
            registry: self.registry.as_ref(),
            processes: self.processes.as_ref(),
            env: &self.env,
            sleep: self.sleep.as_ref(),
            log: self.log.as_ref(),
            shell: self.shell,
        }
    }

    fn shortcuts(&self, id: &Identity) -> std::io::Result<Shortcuts> {
        Shortcuts::new(self.links.clone(), self.folders.as_ref(), id)
    }

    fn local_appdata(&self) -> Option<String> {
        self.env
            .get("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
    }
}

/// Код выхода тихой установки по ошибке: `IN_USE` → 3, `BAD_DIR` → 4, прочее → 1.
pub fn exit_code_of(error: &InstallError) -> i32 {
    if error.is(install::code::IN_USE) {
        exit::IN_USE
    } else if error.is(install::code::BAD_DIR) {
        exit::BAD_DIR
    } else {
        exit::FAILED
    }
}

/// Тихая установка (`/S`): как её запускают обновляторы прежних версий (`--updated`, `/D=`,
/// `--force-run`). Ждёт выхода прежнего HLSR до 60 с, ставит поверх с сохранением `HLSRInstallId`,
/// с `--force-run` запускает установленный `HLSR.exe`. Код выхода — `0/1/3/4`.
pub fn run_setup_silent(
    services: &SetupServices,
    args: &SetupArgs,
    source: Result<Source, String>,
    setup_file: Option<&str>,
) -> i32 {
    let log = services.log.as_ref();
    let build = build_of(&source, log);
    let id = &build.identity;
    let values = services.registry.read(&id.install_key).unwrap_or_default();
    let lookup = services.env.lookup();
    let install_dir = resolve_install_dir(args, &values, id, &lookup);
    if let Some(broken) = broken_setup(&build) {
        log.error(&format!("[setup] {broken}"));
        return exit::FAILED;
    }
    let Ok(source) = source else {
        return exit::FAILED;
    };
    log.info(&format!(
        "[setup] тихая установка {} в {install_dir}{}",
        build.version_or(&services.version),
        if args.updated {
            " (обновление)"
        } else {
            ""
        }
    ));
    if !acquire_lock(services.lock.as_ref(), LOCK_WAIT, services.sleep.as_ref()) {
        log.warn("[setup] HLSR не завершился — обновление отменено");
        return exit::IN_USE;
    }
    let shortcuts = match services.shortcuts(id) {
        Ok(shortcuts) => shortcuts,
        Err(error) => {
            log.error(&format!("[setup]  {error}"));
            return exit::FAILED;
        }
    };
    let library = library_root(&services.user_data);
    let mut options = InstallOptions::new(
        &source,
        &install_dir,
        services.registry.as_ref(),
        &shortcuts,
        log,
    );
    options.env = &lookup;
    options.update = true;
    options.setup_file = setup_file;
    options.library_root = &library;
    options.sleep = services.sleep.as_ref();
    options.processes = Some(services.processes.as_ref());
    let electron = legacy_traces::electron_install(Path::new(&install_dir));
    match install::install(options) {
        Ok(_) => {
            if electron {
                legacy_traces::mark_setup(&services.user_data, log);
            }
        }
        Err(error) => {
            log.error(&format!(
                "[setup] {} {}",
                error.code.as_deref().unwrap_or_default(),
                error.message
            ));
            return exit_code_of(&error);
        }
    }
    if args.force_run
        && let Err(error) = launch_installed(&services.launch_deps(), &install_dir, &[])
    {
        log.error(&format!(
            "[setup] {} {}",
            error.code.as_deref().unwrap_or_default(),
            error.message
        ));
        return exit::FAILED;
    }
    exit::OK
}

/// Очистка библиотеки удаления: `cleanupLibrary(root, { purge: true, installDir, ownMarker })`.
fn cleanup_library_for(
    services: &SetupServices,
) -> impl Fn(&str, &str, Option<&str>) -> library_cleanup::CleanupLibraryResult + '_ {
    move |root, install_dir, own_marker| {
        let lookup = services.env.lookup();
        library_cleanup::cleanup_library(
            root,
            &CleanupOptions {
                env: Some(&lookup),
                steam: Some(services.steam.as_ref()),
                install_dir: Some(install_dir.to_owned()),
                purge: true,
                own_marker: own_marker.map(str::to_owned),
                ..CleanupOptions::default()
            },
        )
    }
}

/// Удаление одной копии: каталог — каталог своего exe, сборка — по списку файлов, подтверждённому
/// маркером владения (иначе по GUID маркера и не боевая, `identityOf`).
struct UninstallRun {
    install_dir: String,
    identity: Identity,
    shortcuts: Shortcuts,
    library_path: String,
    /// Данные HLSR: настройки и вход (`userData`), журналы (`%LOCALAPPDATA%\<имя>`).
    data_dirs: Vec<String>,
}

impl UninstallRun {
    fn new(services: &SetupServices) -> std::io::Result<UninstallRun> {
        let path = win_path();
        let install_dir = path.dirname(&services.exe.to_string_lossy());
        let identity = uninstall::identity_of(&install_dir);
        let shortcuts = services.shortcuts(&identity)?;
        let mut data_dirs = vec![services.user_data.to_string_lossy().into_owned()];
        data_dirs.extend(
            services
                .local_appdata()
                .map(|local| path.join(&[&local, &services.app_name])),
        );
        Ok(UninstallRun {
            install_dir,
            identity,
            shortcuts,
            library_path: library_root(&services.user_data),
            data_dirs,
        })
    }

    fn plan(&self, services: &SetupServices) -> UninstallPlan {
        uninstall::plan(
            &self.install_dir,
            Some(&self.library_path),
            &self.identity,
            Some(services.registry.as_ref()),
            &services.env.lookup(),
        )
    }

    /// `data` — данные, которые уносит полное удаление (в тихом режиме — никакие).
    fn run(
        &self,
        services: &SetupServices,
        remove_games: bool,
        data: &[String],
    ) -> uninstall::UninstallResult {
        let lookup = services.env.lookup();
        let cleanup = cleanup_library_for(services);
        let updater_cache = format!("{}-updater", services.app_name);
        let exe = services.exe.to_string_lossy();
        let registry = services.registry.as_ref();
        let remove_protocol = |name: &str| uninstall::remove_protocol_client(registry, &exe, name);
        uninstall::uninstall(UninstallOptions {
            install_dir: &self.install_dir,
            library_root: Some(&self.library_path),
            remove_games,
            registry,
            shortcuts: &self.shortcuts,
            cleanup_library: &cleanup,
            identity: &self.identity,
            env: &lookup,
            log: services.log.as_ref(),
            data_dirs: data,
            updater_cache_name: Some(&updater_cache),
            temp_root: Some(&services.folders.temp()),
            remove_protocol_client: Some(&remove_protocol),
            is_alive: None,
        })
    }
}

/// `scheduleRemoval` для итога удаления: каталог программы, данные, опустевшая папка HLSR; скрипт
/// ждёт выхода этого процесса сам.
fn schedule(
    services: &SetupServices,
    pending: &uninstall::UninstallResult,
    relaunch: Option<&str>,
) -> Result<(), uninstall::ScheduleError> {
    uninstall::schedule_removal(
        services.processes.as_ref(),
        pending.remove_program.as_deref(),
        &services.env,
        relaunch,
        &uninstall::RemovalTargets {
            data: &pending.remove_data,
            parent: pending.remove_parent.as_deref(),
            pid: Some(std::process::id()),
            own_marker: pending.own_marker.as_deref(),
        },
        &services.folders.temp(),
    )
}

/// `scheduleRemoval` и повторный запуск установщика при выходе (`will-quit`).
pub fn on_quit(
    services: &SetupServices,
    pending: Option<&uninstall::UninstallResult>,
    relaunch: Option<&str>,
) {
    let log = services.log.as_ref();
    if let Some(pending) = pending {
        match schedule(services, pending, relaunch) {
            Ok(()) => return,
            Err(error) => log.error(&format!("[uninstall] каталог программы не удалён: {error}")),
        }
    }
    let Some(relaunch) = relaunch else {
        return;
    };
    let command = Command {
        file: relaunch.to_owned(),
        args: Vec::new(),
        cwd: Some(win_path().dirname(relaunch)),
        env: launch_environment(&services.env),
        hidden: false,
    };
    if let Err(error) = services.processes.spawn(&command) {
        log.error(&format!("[uninstall] установщик не открыт заново: {error}"));
    }
}

/// Тихое удаление (`HLSR.exe --hlsr-uninstall /S [/HLSRDELETE…]`): ждёт выхода HLSR до 60 с,
/// флаги прежнего деинсталлятора чистят данные до удаления программы, затем удаление и отложенное
/// удаление каталога программы (`app.exit()` 0.4.9 не шлёт `will-quit` — планируется здесь, без
/// повторного запуска установщика). Код выхода: 0 — всё удалено, 1 — с предупреждениями, 3 — занято.
pub fn run_uninstall_silent(services: &SetupServices, args: &SetupArgs) -> i32 {
    let log = services.log.as_ref();
    let run = match UninstallRun::new(services) {
        Ok(run) => run,
        Err(error) => {
            log.error(&format!("[uninstall] {error}"));
            return exit::FAILED;
        }
    };
    if !acquire_lock(services.lock.as_ref(), LOCK_WAIT, services.sleep.as_ref()) {
        return exit::IN_USE;
    }
    // Настройки и статистика чистятся до удаления программы, как и прежде.
    let extra = args.data_cleanup_args();
    if !extra.is_empty() {
        let mut argv = vec![
            services.exe.to_string_lossy().into_owned(),
            cleanup_cli::FLAG.to_owned(),
        ];
        argv.extend(extra.iter().map(|arg| (*arg).to_owned()));
        argv.extend(["--install-dir".to_owned(), run.install_dir.clone()]);
        let path = win_path();
        let local = services.local_appdata();
        let lookup = services.env.lookup();
        let cleaned = cleanup_cli::run(
            &RunOptions {
                argv,
                silent: true,
                user_data: Some(services.user_data.to_string_lossy().into_owned()),
                app_name: Some(services.app_name.clone()),
                launcher_version: Some(services.version.clone()),
                logs_dir: local
                    .as_ref()
                    .map(|local| path.join(&[local, &services.app_name, "logs"])),
                updater_cache: local
                    .as_ref()
                    .map(|local| path.join(&[local, &format!("{}-updater", services.app_name)])),
                install_dir: Some(run.install_dir.clone()),
                env: Some(&lookup),
                steam: Some(services.steam.as_ref()),
                telemetry: services.telemetry.as_deref(),
                ..RunOptions::default()
            },
            &mut std::io::sink(),
        );
        log.info(&format!(
            "[uninstall] очистка данных: {}",
            cleaned.lines.join(" | ")
        ));
    }
    // Тихий режим: данные — по флагам прежнего деинсталлятора (выше).
    let result = run.run(services, args.delete_games, &[]);
    if result.pending()
        && let Err(error) = schedule(services, &result, None)
    {
        log.error(&format!("[uninstall] каталог программы не удалён: {error}"));
    }
    if result.status == UninstallStatus::Ok {
        exit::OK
    } else {
        exit::FAILED
    }
}

// ---------------------------------------------------------------------------------- окно установки

/// Backend окна установки (`runSetup` без `/S`). «Удалить» запускает установленную копию с
/// `--hlsr-uninstall` (она знает свой каталог и проверяет владение), установщик закрывается.
pub struct SetupBackend {
    services: SetupServices,
    source: Arc<Result<Source, String>>,
    identity: Identity,
    broken: Option<String>,
    setup_file: Option<String>,
    library: String,
    shortcuts: Shortcuts,
    install_dir: Mutex<String>,
    quit: Arc<dyn Fn() + Send + Sync>,
}

impl SetupBackend {
    /// Backend и начальное состояние окна. Блокировка берётся сразу, без ожидания; не взялась —
    /// окно с ошибкой «HLSR сейчас запущен». Установленная версия не старше этой — сразу «готово».
    pub fn open(
        mut services: SetupServices,
        args: &SetupArgs,
        source: Result<Source, String>,
        setup_file: Option<String>,
        quit: Arc<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<(Arc<SetupBackend>, InstallerState)> {
        let build = build_of(&source, services.log.as_ref());
        // Версия окна и сравнений с установленной — та, что ставится.
        services.version = build.version_or(&services.version).to_owned();
        let id = build.identity.clone();
        let values = services.registry.read(&id.install_key).unwrap_or_default();
        let install_dir = resolve_install_dir(args, &values, &id, &services.env.lookup());
        let broken = broken_setup(&build);
        let locked = acquire_lock(
            services.lock.as_ref(),
            Duration::ZERO,
            services.sleep.as_ref(),
        );
        let uninstall_values = services
            .registry
            .read(&id.uninstall_key)
            .unwrap_or_default();
        let installed = sz_value(&uninstall_values, "DisplayVersion").filter(|version| {
            layout::is_version(version)
                && Path::new(&win_path().join(&[&install_dir, layout::EXE])).exists()
        });
        let update = installed.as_deref().is_some_and(|installed| {
            compare_semver(&services.version, installed).is_ok_and(|order| order.is_gt())
        });
        let phase = if broken.is_some() || !locked {
            Phase::Error
        } else if installed.is_some() && !update {
            Phase::Done
        } else {
            Phase::Idle
        };
        let state = InstallerState {
            phase,
            directory: install_dir.clone(),
            language: language(&services.locale, setup_file.as_deref()).to_owned(),
            can_uninstall: installed.is_some(),
            version: Some(services.version.clone()),
            installed_version: installed.clone(),
            update,
            error: broken
                .clone()
                .or_else(|| (!locked).then(|| LOCK_ERROR.to_owned())),
            ..InstallerState::default()
        };
        let shortcuts = services.shortcuts(&id)?;
        let library = library_root(&services.user_data);
        let backend = SetupBackend {
            services,
            source: Arc::new(source),
            identity: id,
            broken,
            setup_file,
            library,
            shortcuts,
            install_dir: Mutex::new(install_dir),
            quit,
        };
        Ok((Arc::new(backend), state))
    }

    fn install_dir(&self) -> String {
        self.install_dir
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }
}

impl InstallerBackend for SetupBackend {
    fn can_select_directory(&self) -> bool {
        true
    }

    fn select_directory(&self, directory: &str, ctx: &Ctx<'_>) -> Result<(), String> {
        let dir = win_path().resolve(&[directory]);
        *self
            .install_dir
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = dir.clone();
        let existing = build_of(
            &Ok(Source::Dir(PathBuf::from(&dir))),
            self.services.log.as_ref(),
        );
        let current = existing
            .list
            .as_ref()
            .filter(|_| Path::new(&win_path().join(&[&dir, layout::EXE])).exists())
            .map(|list| list.version.clone());
        let is_update = current.as_deref().is_some_and(|current| {
            compare_semver(&self.services.version, current).is_ok_and(|order| order.is_gt())
        });
        let phase = if self.broken.is_some() {
            Phase::Error
        } else if current.is_some() && !is_update {
            Phase::Done
        } else {
            Phase::Idle
        };
        ctx.send(StatePatch {
            directory: Some(dir),
            phase: Some(phase),
            installed_version: Some(current.clone()),
            update: Some(is_update),
            can_uninstall: Some(current.is_some()),
            error: Some(self.broken.clone()),
            warning: Some(None),
            ..StatePatch::default()
        });
        Ok(())
    }

    fn install(&self, desktop_shortcut: bool, ctx: &Ctx<'_>) -> Result<bool, BackendError> {
        let services = &self.services;
        if !services.lock.is_held()
            && !acquire_lock(
                services.lock.as_ref(),
                Duration::ZERO,
                services.sleep.as_ref(),
            )
        {
            ctx.send(StatePatch::phase(Phase::Error).with_error(Some(LOCK_ERROR.to_owned())));
            return Ok(false);
        }
        let source = match (&self.broken, self.source.as_ref()) {
            (None, Ok(source)) => source,
            (broken, _) => {
                ctx.send(StatePatch::phase(Phase::Error).with_error(broken.clone()));
                return Ok(false);
            }
        };
        ctx.send(
            StatePatch::phase(Phase::Installing)
                .with_error(None)
                .with_warning(None)
                .with_progress(Some(0)),
        );
        let install_dir = self.install_dir();
        let lookup = services.env.lookup();
        let mut shown = 0u32;
        let mut on_progress = |done: u64, total: u64| {
            let progress = if total == 0 {
                0
            } else {
                (done as f64 / total as f64 * 100.0).floor() as u32
            };
            if progress != shown {
                shown = progress;
                ctx.send(StatePatch::default().with_progress(Some(progress)));
            }
        };
        let mut on_commit = || ctx.commit();
        let mut options = InstallOptions::new(
            source,
            &install_dir,
            services.registry.as_ref(),
            &self.shortcuts,
            services.log.as_ref(),
        );
        options.cancel = ctx.cancel_token();
        options.env = &lookup;
        options.desktop_shortcut = Some(desktop_shortcut);
        options.setup_file = self.setup_file.as_deref();
        options.library_root = &self.library;
        options.sleep = services.sleep.as_ref();
        options.on_progress = Some(&mut on_progress);
        options.on_commit = Some(&mut on_commit);
        options.processes = Some(services.processes.as_ref());
        let electron = legacy_traces::electron_install(Path::new(&install_dir));
        let result = install::install(options)?;
        if electron {
            legacy_traces::mark_setup(&services.user_data, services.log.as_ref());
        }
        ctx.send(StatePatch {
            phase: Some(Phase::Done),
            update: Some(false),
            installed_version: Some(Some(result.version)),
            can_uninstall: Some(false),
            progress: Some(None),
            warning: Some((!result.warnings.is_empty()).then(|| result.warnings.join(". "))),
            ..StatePatch::default()
        });
        Ok(true)
    }

    fn launch(&self, _ctx: &Ctx<'_>) -> Result<bool, String> {
        launch_installed(&self.services.launch_deps(), &self.install_dir(), &[])
            .map_err(|error| error.message)?;
        (self.quit)();
        Ok(true)
    }

    /// Удаляет установленная копия: установщик запускает её (с «Назад» к себе) и закрывается.
    fn uninstall_plan(&self, ctx: &Ctx<'_>) -> Option<UninstallPlan> {
        let services = &self.services;
        let mut args = vec![layout::UNINSTALL_FLAG.to_owned()];
        if let Some(setup) = return_setup_of(self.setup_file.as_deref()) {
            args.extend([layout::RETURN_SETUP_FLAG.to_owned(), setup]);
        }
        if let Err(error) = launch_installed(&services.launch_deps(), &self.install_dir(), &args) {
            services.log.error(&format!(
                "[setup] запуск удаления не удался: {}",
                error.message
            ));
            if !services.lock.is_held() {
                acquire_lock(
                    services.lock.as_ref(),
                    Duration::ZERO,
                    services.sleep.as_ref(),
                );
            }
            ctx.send(StatePatch::default().with_error(Some(format!(
                "Не удалось запустить удаление: {}",
                hlsr_core::js::slice_utf16(&error.message, 300)
            ))));
            return None;
        }
        (self.quit)();
        None
    }

    fn uninstall(&self, _remove_games: bool, _ctx: &Ctx<'_>) -> Result<bool, String> {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------------- окно удаления

/// Backend окна удаления (`runUninstall` без `/S`): эта копия HLSR удаляется и поставить себя заново
/// не может — «Назад» закрывает окно, а установщик откроется, когда каталог будет удалён.
pub struct UninstallBackend {
    services: SetupServices,
    run: UninstallRun,
    return_setup: Option<String>,
    /// Что удалить после выхода: каталог программы, данные, опустевшую папку HLSR.
    pending: Mutex<Option<uninstall::UninstallResult>>,
    relaunch: Mutex<Option<String>>,
    quit: Arc<dyn Fn() + Send + Sync>,
}

/// Блокировка удаления не взялась: окно не открывается, код выхода 3 (сначала — сообщение).
pub const UNINSTALL_RUNNING_MESSAGE: &str = "HLSR сейчас запущен. Закройте лаунчер и запустите удаление ещё раз.\n\nHLSR is running. Close it and start the uninstall again.";

impl UninstallBackend {
    /// Backend и начальное состояние; `Ok(None)` — HLSR запущен (показать
    /// [`UNINSTALL_RUNNING_MESSAGE`] и выйти с кодом 3).
    pub fn open(
        services: SetupServices,
        args: &SetupArgs,
        quit: Arc<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<Option<(Arc<UninstallBackend>, InstallerState, String)>> {
        let run = UninstallRun::new(&services)?;
        let return_setup = return_setup_of(args.return_setup.as_deref());
        if !acquire_lock(
            services.lock.as_ref(),
            Duration::ZERO,
            services.sleep.as_ref(),
        ) {
            return Ok(None);
        }
        let plan = run.plan(&services);
        let title = format!("{} — удаление", run.identity.product_name);
        let state = InstallerState {
            phase: Phase::UninstallConfirm,
            directory: run.install_dir.clone(),
            language: if services.locale.starts_with("ru") {
                "ru"
            } else {
                "en"
            }
            .to_owned(),
            can_uninstall: true,
            version: Some(services.version.clone()),
            can_go_back: Some(return_setup.is_some()),
            uninstall_plan: Some(plan),
            ..InstallerState::default()
        };
        let backend = UninstallBackend {
            services,
            run,
            return_setup,
            pending: Mutex::new(None),
            relaunch: Mutex::new(None),
            quit,
        };
        Ok(Some((Arc::new(backend), state, title)))
    }

    /// `will-quit`: отложенное удаление и/или повторный запуск установщика.
    pub fn on_quit(&self) {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let relaunch = self
            .relaunch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        on_quit(&self.services, pending.as_ref(), relaunch.as_deref());
    }
}

impl InstallerBackend for UninstallBackend {
    fn install(&self, _desktop_shortcut: bool, _ctx: &Ctx<'_>) -> Result<bool, BackendError> {
        Ok(false)
    }

    fn launch(&self, _ctx: &Ctx<'_>) -> Result<bool, String> {
        Ok(false)
    }

    fn uninstall_plan(&self, _ctx: &Ctx<'_>) -> Option<UninstallPlan> {
        Some(self.run.plan(&self.services))
    }

    fn uninstall(&self, remove_games: bool, ctx: &Ctx<'_>) -> Result<bool, String> {
        ctx.send(StatePatch::phase(Phase::Uninstalling).with_error(None));
        // Полное удаление («Также удалить игры») уносит и данные HLSR.
        let result = self
            .run
            .run(&self.services, remove_games, &self.run.data_dirs);
        *self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            result.pending().then(|| result.clone());
        ctx.send(StatePatch {
            phase: Some(Phase::Uninstalled),
            can_uninstall: Some(false),
            warning: Some((!result.warnings.is_empty()).then(|| result.warnings.join(" "))),
            ..StatePatch::default()
        });
        Ok(true)
    }

    fn has_back(&self) -> bool {
        self.return_setup.is_some()
    }

    fn back(&self, _ctx: &Ctx<'_>) -> bool {
        let Some(setup) = &self.return_setup else {
            return false;
        };
        *self
            .relaunch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(setup.clone());
        (self.quit)();
        true
    }
}
