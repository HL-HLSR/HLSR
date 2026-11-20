//! Запуск в префиксе Proton (`linux/ProtonRunner.js`): префикс игры
//! `<данные>/compatdata/<id>/pfx`, `umu-run` (из ресурсов или `PATH`) и его окружение (без
//! переменных AppImage и лаунчера), wineserver префикса на хосте, подготовка префикса после смены
//! Proton (`wineboot -u`), шрифты (corefonts через winetricks, Carlito для LiveSplit), мост к
//! клиенту Steam через точку входа Steam Runtime, обёртки из параметров Proton игрока
//! (`mangohud %command%`), ограничение CPU 0 (`taskset`), журнал запуска.
//!
//! Логика собирается на всех ОС (пути — по правилам `path` этой ОС, как у Node) и проверяется
//! тестами с поддельным запуском процессов; настоящие `umu-run`, `wineserver` и `taskset` — на
//! Linux. [`ProtonRunner`] реализует [`PrefixLauncher`] для запуска инструментов в префиксе игры.

use crate::linux::abort::{Abort, Shared};
use crate::linux::prefix_command::{
    CommandOutput, ProtonError, WaitOptions, wait_for_prefix_command,
};
use crate::linux::proton_manager::{ProtonEntry, ProtonManager, real_path};
use crate::linux::python_runtime::{PythonEnvironment, PythonOptions, PythonRuntime, io_error};
use crate::linux::user_namespaces::{
    UserNamespaces, UserNamespacesCheck, UserNamespacesOptions, umu_root,
};
use crate::spawn::{
    Child, Env, PATH_DELIMITER, SpawnOptions, Spawner, Stdio, SystemSpawner, executable_in_path,
    is_executable, process_env,
};
use crate::tool_runner::{PrefixLaunchOptions, PrefixLaunched, PrefixLauncher};
use hlsr_core::js;
use hlsr_core::js::path::NodePath;
use hlsr_delivery::errors::{Code, DeliveryError, Failure, RawError};
use hlsr_delivery::ports::{Logger, TracingLogger};
use hlsr_delivery::providers::BoxFuture;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

/// `CORE_FONT_FILES`: шрифты corefonts, без которых игра рисует меню квадратами.
pub const CORE_FONT_FILES: [&str; 10] = [
    "andalemo.ttf",
    "arial.ttf",
    "comic.ttf",
    "cour.ttf",
    "georgia.ttf",
    "impact.ttf",
    "times.ttf",
    "trebuc.ttf",
    "verdana.ttf",
    "webdings.ttf",
];
/// `LIVESPLIT_FONT_PROFILE`.
pub const LIVESPLIT_FONT_PROFILE: &str = "livesplit";
/// `LIVESPLIT_FONT_MARKER`.
pub const LIVESPLIT_FONT_MARKER: &str = ".hlsr-livesplit-fonts-v1";
/// `PREFIX_PROTON_MARKER`: какой Proton последним готовил префикс.
pub const PREFIX_PROTON_MARKER: &str = ".hlsr-prefix-proton";
/// Отметка готовых corefonts.
pub const CORE_FONTS_MARKER: &str = ".hlsr-corefonts-ready";
/// `HOST_WINESERVER_TIMEOUT_MS`.
pub const HOST_WINESERVER_TIMEOUT: Duration = Duration::from_millis(10_000);
/// `UMU_ONLY_ENV`: umu всегда ставит `UMU_ID`, а Proton при нём отключает lsteamclient — игре,
/// которой нужен живой Steam, их не передают.
pub const UMU_ONLY_ENV: [&str; 6] = [
    "GAMEID",
    "STORE",
    "PROTON_VERB",
    "WINEPREFIX",
    "UMU_ID",
    "UMU_USE_STEAM",
];
/// Переменные AppImage и чужого Proton, которые не передаются дальше.
pub const STRIPPED_ENV: [&str; 8] = [
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "PYTHONPATH",
    "PYTHONHOME",
    "QT_PLUGIN_PATH",
    "APPDIR",
    "STEAM_COMPAT_DATA_PATH",
    "STEAM_COMPAT_CLIENT_INSTALL_PATH",
];

/// Шрифт LiveSplit: файл в ресурсах, имя в префиксе, SHA-256.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveSplitFont {
    pub source: &'static str,
    pub target: &'static str,
    pub sha256: &'static str,
}

/// `LIVESPLIT_FONTS`: Carlito вместо Calibri и Segoe UI.
pub const LIVESPLIT_FONTS: [LiveSplitFont; 4] = [
    LiveSplitFont {
        source: "Carlito-Regular.ttf",
        target: "carlito.ttf",
        sha256: "f6418f708baede9789daef5d458c0f53d2a888af9820e8062934e504fedc6595",
    },
    LiveSplitFont {
        source: "Carlito-Bold.ttf",
        target: "carlitob.ttf",
        sha256: "bb5d20f79b82599ec72983597437373a80f2d2085fa91fc144fd74e876a594db",
    },
    LiveSplitFont {
        source: "Carlito-Italic.ttf",
        target: "carlitoi.ttf",
        sha256: "0b019225e58d702bfedcbd35c21696769f8ee115cb6343f84c2f240312450d1c",
    },
    LiveSplitFont {
        source: "Carlito-BoldItalic.ttf",
        target: "carlitobi.ttf",
        sha256: "b32928186c119599e03ca6a1ffc680fdcb7fac95772f4b95d989cf6cd3861517",
    },
];

/// `RUNTIME_BY_TOOL_APPID`: Steam Linux Runtime, который требует сборка Proton.
pub const RUNTIME_BY_TOOL_APPID: [(&str, &str); 2] =
    [("1628350", "steamrt3"), ("4183110", "steamrt4")];

// `\s` JS: пробелы Unicode с U+FEFF, без U+0085; `\d` — ASCII.
static REQUIRE_TOOL_APPID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""require_tool_appid"[\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}]+"([0-9]+)""#)
        .expect("шаблон toolmanifest.vdf")
});
static BWRAP_DENIED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)bwrap:.*(?:Permission denied|Operation not permitted)").expect("шаблон bwrap")
});
static ALREADY_INSTALLED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"winetricks verb ['"]corefonts['"] is already installed"#)
        .expect("шаблон winetricks")
});
static WINESERVER_OF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([^/]+)/files/bin/wineserver$").expect("шаблон wineserver"));

/// `requiredRuntimeName(protonPath)` по тексту `toolmanifest.vdf` (`None` — файла нет).
pub fn required_runtime_name_of(text: Option<&str>) -> Option<&'static str> {
    let captures = REQUIRE_TOOL_APPID.captures(text?)?;
    RUNTIME_BY_TOOL_APPID
        .iter()
        .find(|(app_id, _)| *app_id == &captures[1])
        .map(|(_, runtime)| *runtime)
}

/// `requiredRuntimeName(protonPath)`: `steamrt3`/`steamrt4` из `toolmanifest.vdf` сборки.
pub fn required_runtime_name(proton_path: &Path) -> Option<&'static str> {
    let text = js::fs::read_file(&proton_path.join("toolmanifest.vdf"))
        .ok()
        .map(|bytes| js::utf8_string(&bytes));
    required_runtime_name_of(text.as_deref())
}

/// `windowsPathOf(file)`: путь для аргументов программ Windows — `Z:` и `\` (Wine отображает
/// корень Linux на диск `Z:`).
pub fn windows_path_of(file: &str) -> String {
    windows_path_of_with(&NodePath::native(), file)
}

/// [`windows_path_of`] с правилами `path` заданной ОС.
pub fn windows_path_of_with(path: &NodePath, file: &str) -> String {
    format!("Z:{}", path.resolve(&[file]).replace('/', "\\"))
}

fn sha256_file(file: &Path) -> std::io::Result<String> {
    let bytes = js::fs::read_file(file)?;
    Ok(hlsr_delivery::file_ops::hex(&Sha256::digest(bytes)))
}

/// `livesplitFontRegistry()`: `.reg` с Carlito и заменами Calibri и Segoe UI.
pub fn livesplit_font_registry() -> String {
    [
        "REGEDIT4",
        "",
        "[HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows NT\\CurrentVersion\\Fonts]",
        "\"Carlito (TrueType)\"=\"carlito.ttf\"",
        "\"Carlito Bold (TrueType)\"=\"carlitob.ttf\"",
        "\"Carlito Italic (TrueType)\"=\"carlitoi.ttf\"",
        "\"Carlito Bold Italic (TrueType)\"=\"carlitobi.ttf\"",
        "",
        "[HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows NT\\CurrentVersion\\FontSubstitutes]",
        "\"Calibri\"=\"Carlito\"",
        "\"Segoe UI\"=\"Carlito\"",
        "",
    ]
    .join("\r\n")
}

/// `coreFontsStatus(prefix)`: шрифты на месте (заголовок TrueType/OpenType), запись в
/// `winetricks.log`, отметка HLSR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreFontsStatus {
    pub ready: bool,
    pub logged: bool,
    pub missing: Vec<String>,
}

impl CoreFontsStatus {
    pub fn to_json(&self) -> Value {
        json!({"ready": self.ready, "logged": self.logged, "missing": self.missing})
    }
}

/// Шрифт — не меньше 12 байт и с заголовком `00010000`, `OTTO` или `true`.
fn font_ok(file: &Path) -> bool {
    use std::io::Read as _;
    let Ok(mut handle) = std::fs::File::open(file) else {
        return false;
    };
    if handle.metadata().map_or(true, |meta| meta.len() < 12) {
        return false;
    }
    let mut header = [0u8; 4];
    if handle.read_exact(&mut header).is_err() {
        return false;
    }
    header == [0, 1, 0, 0] || &header == b"OTTO" || &header == b"true"
}

/// `coreFontsStatus(prefix)`.
pub fn core_fonts_status(prefix: &Path) -> CoreFontsStatus {
    let logged = js::fs::read_file(&prefix.join("winetricks.log"))
        .map(|bytes| {
            crate::linux::proton_manager::split_lines(&js::utf8_string(&bytes))
                .iter()
                .any(|line| js::trim(line) == "corefonts")
        })
        .unwrap_or(false);
    let directory = prefix.join("drive_c").join("windows").join("Fonts");
    let names = js::fs::read_dir_names(&directory).unwrap_or_default();
    let missing: Vec<String> = CORE_FONT_FILES
        .iter()
        .filter(|name| {
            match names
                .iter()
                .find(|candidate| candidate.to_lowercase() == **name)
            {
                Some(actual) => !font_ok(&directory.join(actual)),
                None => true,
            }
        })
        .map(|name| (*name).to_owned())
        .collect();
    let marked = prefix.join(CORE_FONTS_MARKER).exists();
    CoreFontsStatus {
        ready: (logged || marked) && missing.is_empty(),
        logged,
        missing,
    }
}

/// `allowedCpuListWithoutZero()` по тексту `/proc/self/status`: допустимые CPU без нулевого
/// (`1-7`, `2-4,7`); `None` — нечего оставить или не прочиталось.
pub fn allowed_cpu_list_without_zero_in(status: Option<&str>) -> Option<String> {
    static LINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^Cpus_allowed_list:[\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}]*([0-9,-]+)")
            .expect("шаблон Cpus_allowed_list")
    });
    let captures = LINE.captures(status?)?;
    let mut allowed = Vec::new();
    for part in captures[1].split(',') {
        let bounds: Vec<f64> = part.split('-').map(js::to_number).collect();
        if bounds.iter().any(|value| {
            !(value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0) || *value < 0.0
        }) {
            continue;
        }
        let start = bounds[0].max(1.0);
        let end = if bounds.len() == 2 {
            bounds[1]
        } else {
            bounds[0]
        };
        if end < start {
            continue;
        }
        let number = |x: f64| js::to_string(&hlsr_core::json_text::js_number(x));
        allowed.push(if start == end {
            number(start)
        } else {
            format!("{}-{}", number(start), number(end))
        });
    }
    (!allowed.is_empty()).then(|| allowed.join(","))
}

/// `allowedCpuListWithoutZero()` этого процесса.
pub fn allowed_cpu_list_without_zero() -> Option<String> {
    let bytes = std::fs::read("/proc/self/status").ok()?;
    allowed_cpu_list_without_zero_in(Some(&js::utf8_string(&bytes)))
}

/// `protonFlag(value)`: как Proton читает `PROTON_NO_*` — пусто и `0` значат «не задано».
pub fn proton_flag(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

/// `protonHasWinetricks(protonPath)`: umu ставит verb'ы из `protonfixes/winetricks` сборки.
pub fn proton_has_winetricks(proton_path: &Path) -> bool {
    std::fs::metadata(proton_path.join("protonfixes").join("winetricks"))
        .is_ok_and(|meta| meta.is_file())
}

/// Тот же файл: устройство и номер (`dev`/`ino` из `stat`; в Windows — серийный номер тома и
/// индекс файла, как у Node).
pub fn file_identity(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, GetFileInformationByHandle,
        };
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .ok()?;
        // SAFETY: структура заполняется системой; дескриптор открыт и живёт до конца вызова.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
        (ok != 0).then(|| {
            (
                u64::from(info.dwVolumeSerialNumber),
                (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            )
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

/// wineserver префикса (`prefixWineservers`): PID, файл программы и путь к нему.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wineserver {
    pub pid: u32,
    pub dev: u64,
    pub ino: u64,
    pub exe_path: Option<String>,
}

/// `prefixWineservers(prefix, procRoot, tmpRoot)`: сокет wineserver лежит в
/// `/tmp/.wine-<uid>/server-<dev>-<ino>` по `stat` префикса, и сервер работает из этой папки.
/// По ней находится сервер префикса, в том числе поднятый в контейнере Steam Runtime; файлы
/// сверяются по `dev`/`ino` через `/proc/<pid>/{cwd,exe}`.
pub fn prefix_wineservers_at(
    prefix: &Path,
    proc_root: &Path,
    tmp_root: &Path,
    uid: u32,
) -> Vec<Wineserver> {
    let Some((dev, ino)) = file_identity(prefix) else {
        return Vec::new();
    };
    let server_dir = tmp_root
        .join(format!(".wine-{uid}"))
        .join(format!("server-{dev:x}-{ino:x}"));
    let Some(server_dir) = file_identity(&server_dir) else {
        return Vec::new();
    };
    let Ok(entries) = js::fs::read_dir_names(proc_root) else {
        return Vec::new();
    };
    let mut servers = Vec::new();
    for pid in entries {
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        // Процесс уже завершился или недоступен — дальше.
        let dir = proc_root.join(&pid);
        let Ok(comm) = std::fs::read(dir.join("comm")) else {
            continue;
        };
        if js::trim(&js::utf8_string(&comm)) != "wineserver" {
            continue;
        }
        if file_identity(&dir.join("cwd")) != Some(server_dir) {
            continue;
        }
        let Some((dev, ino)) = file_identity(&dir.join("exe")) else {
            continue;
        };
        let exe_path = std::fs::read_link(dir.join("exe")).ok().map(|target| {
            let text = target.to_string_lossy().into_owned();
            text.strip_suffix(" (deleted)")
                .map(str::to_owned)
                .unwrap_or(text)
        });
        let Ok(pid) = pid.parse() else { continue };
        servers.push(Wineserver {
            pid,
            dev,
            ino,
            exe_path,
        });
    }
    servers
}

/// `prefixWineservers(prefix)` этой машины.
pub fn prefix_wineservers(prefix: &Path) -> Vec<Wineserver> {
    let uid = crate::linux::steam_locator::current_uid().unwrap_or(0);
    prefix_wineservers_at(prefix, Path::new("/proc"), Path::new("/tmp"), uid)
}

/// Ошибка запуска в префиксе: `DeliveryError` 0.4.7 (код, который видит интерфейс) или обычная
/// ошибка команды (`code`, `exitCode`, вывод).
#[derive(Debug, Clone, PartialEq)]
pub enum RunnerError {
    Delivery(DeliveryError),
    Proton(ProtonError),
}

impl RunnerError {
    pub fn message(&self) -> &str {
        match self {
            RunnerError::Delivery(error) => &error.message,
            RunnerError::Proton(error) => &error.message,
        }
    }

    /// `error.code`.
    pub fn code(&self) -> Option<&str> {
        match self {
            RunnerError::Delivery(error) => Some(error.code.as_str()),
            RunnerError::Proton(error) => error.code.as_deref(),
        }
    }

    fn delivery(code: Code, message: impl Into<String>, details: Value) -> RunnerError {
        RunnerError::Delivery(DeliveryError::new(code, message).with_details(details))
    }
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for RunnerError {}

impl From<ProtonError> for RunnerError {
    fn from(error: ProtonError) -> RunnerError {
        RunnerError::Proton(error)
    }
}

impl From<DeliveryError> for RunnerError {
    fn from(error: DeliveryError) -> RunnerError {
        RunnerError::Delivery(error)
    }
}

impl From<RawError> for RunnerError {
    fn from(error: RawError) -> RunnerError {
        RunnerError::Proton(ProtonError::from(error))
    }
}

impl From<RunnerError> for Failure {
    fn from(error: RunnerError) -> Failure {
        match error {
            RunnerError::Delivery(error) => Failure::Delivery(error),
            RunnerError::Proton(error) => Failure::Raw(RawError {
                code: error.code,
                message: error.message,
                stderr: error.stderr,
                ..RawError::default()
            }),
        }
    }
}

/// Сборка Proton, в которой идёт запуск (`launch.proton`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtonBuild {
    pub path: String,
    pub name: String,
}

impl ProtonBuild {
    pub fn to_json(&self) -> Value {
        json!({"path": self.path, "name": self.name})
    }
}

impl From<ProtonEntry> for ProtonBuild {
    fn from(entry: ProtonEntry) -> ProtonBuild {
        ProtonBuild {
            path: entry.path,
            name: entry.name,
        }
    }
}

/// Выбранная сборка (`manager.selected()`).
pub trait ProtonSelection: Send + Sync {
    fn selected(&self) -> Option<ProtonBuild>;
}

impl ProtonSelection for ProtonManager {
    fn selected(&self) -> Option<ProtonBuild> {
        ProtonManager::selected(self).map(ProtonBuild::from)
    }
}

/// Мост к клиенту Steam (`options.steamBridge`): игра видна в Steam под своим AppID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamBridge {
    pub app_id: i64,
    pub steam_path: String,
    pub install_path: Option<String>,
}

/// Параметры `launch(gameId, executable, args, options)`.
#[derive(Debug, Clone, Default)]
pub struct LaunchOptions {
    /// Рабочая папка; по умолчанию — папка exe.
    pub cwd: Option<PathBuf>,
    /// Переменные поверх окружения запуска.
    pub env: Env,
    /// Обёртка из параметров Proton (`mangohud --dlsym`).
    pub wrapper: Vec<String>,
    pub exclude_cpu0: bool,
    /// Вывод umu, Proton и Wine — в этот файл (каждый запуск заново).
    pub log_file: Option<PathBuf>,
    pub steam_bridge: Option<SteamBridge>,
    pub skip_fonts: bool,
    /// `livesplit` — ещё и шрифты LiveSplit.
    pub font_profile: Option<String>,
    /// `signal`: отмена до запуска.
    pub abort: Option<Abort>,
}

/// Итог `launch`: процесс сессии, сборка, префикс и программа, которая запущена.
#[derive(Debug, Clone)]
pub struct ProtonLaunch {
    pub child: Child,
    pub proton: ProtonBuild,
    pub prefix: String,
    pub command: String,
}

/// Команда и окружение запуска (`environment(gameId)`).
#[derive(Debug, Clone)]
pub struct LaunchEnvironment {
    pub command: String,
    pub prefix: String,
    pub proton: ProtonBuild,
    pub env: Env,
}

/// Что от запуска в префиксе нужно `LaunchService` (тесты подставляют подделку).
pub trait ProtonLauncher: Send + Sync {
    fn launch<'a>(
        &'a self,
        game_id: &'a str,
        executable: &'a Path,
        args: &'a [String],
        options: LaunchOptions,
    ) -> BoxFuture<'a, Result<ProtonLaunch, RunnerError>>;
    /// `ensureLiveSplitFonts(gameId)`.
    fn ensure_live_split_fonts<'a>(
        &'a self,
        game_id: &'a str,
    ) -> BoxFuture<'a, Result<(), RunnerError>>;
    /// `manager.selected()`.
    fn selected(&self) -> Option<ProtonBuild>;
    /// `umuPath()`.
    fn umu_path(&self) -> Option<String>;
}

/// Найти wineserver'ы префикса (`prefixWineservers`).
pub type PrefixWineservers = Arc<dyn Fn(&Path) -> Vec<Wineserver> + Send + Sync>;

/// Режим дисплея игры (`displayModeOf(gameId)`): `PROTON_ENABLE_WAYLAND` или `None`.
pub type DisplayModeOf = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct ProtonRunnerOptions {
    /// `manager`: выбранная сборка.
    pub manager: Option<Arc<dyn ProtonSelection>>,
    /// `dataDir`.
    pub data_dir: PathBuf,
    /// `resourcesPath`.
    pub resources_path: PathBuf,
    pub logger: Option<Arc<dyn Logger>>,
    /// `spawnFn`.
    pub spawner: Option<Arc<dyn Spawner>>,
    pub prefix_command_timeout_ms: Option<u64>,
    pub prefix_command_kill_grace_ms: Option<u64>,
    pub python_runtime: Option<Arc<dyn PythonEnvironment>>,
    pub user_namespaces: Option<Arc<dyn UserNamespacesCheck>>,
    /// `displayModeOf(gameId)`; по умолчанию — `PROTON_ENABLE_WAYLAND` из настроек запуска игры.
    pub display_mode_of: Option<DisplayModeOf>,
    pub prefix_wineservers: Option<PrefixWineservers>,
    /// `umuPath()`; по умолчанию — ресурсы, затем `PATH`.
    pub umu_path: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
    /// `process.env`; по умолчанию — окружение лаунчера без переменных Slint и winit.
    pub env: Option<Env>,
    /// Часы для отметок и журнала.
    pub now: Option<Arc<dyn Fn() -> SystemTime + Send + Sync>>,
}

type Pending = Shared<Result<(), RunnerError>>;

struct Inner {
    manager: Arc<dyn ProtonSelection>,
    data_dir: String,
    resources_path: String,
    logger: Arc<dyn Logger>,
    spawner: Arc<dyn Spawner>,
    timeout_ms: Option<u64>,
    kill_grace_ms: Option<u64>,
    python: Arc<dyn PythonEnvironment>,
    user_namespaces: Arc<dyn UserNamespacesCheck>,
    display_mode_of: DisplayModeOf,
    prefix_wineservers: PrefixWineservers,
    umu_path: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
    env: Option<Env>,
    now: Arc<dyn Fn() -> SystemTime + Send + Sync>,
    path: NodePath,
    fonts_pending: Mutex<HashMap<String, Pending>>,
    prefix_pending: Mutex<HashMap<String, Pending>>,
}

/// `ProtonRunner`. Копии — тот же запускальщик.
#[derive(Clone)]
pub struct ProtonRunner {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ProtonRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtonRunner")
            .field("data_dir", &self.inner.data_dir)
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `PROTON_ENABLE_WAYLAND` из настроек запуска игры (`displayModeOf`).
pub fn display_mode_of(game_id: &str) -> Option<String> {
    hlsr_delivery::manifests::launch_config_of(game_id)?
        .environment
        .iter()
        .find(|(key, _)| *key == "PROTON_ENABLE_WAYLAND")
        .map(|(_, value)| (*value).to_owned())
}

/// `readText(file)`: текст без пробелов по краям или `None`.
fn read_trimmed(file: &Path) -> Option<String> {
    js::fs::read_file(file)
        .ok()
        .map(|bytes| js::trim(&js::utf8_string(&bytes)).to_owned())
}

/// Файл с правами 0600.
fn write_private(file: &Path, bytes: &[u8]) -> Result<(), ProtonError> {
    crate::linux::proton_manager::write_private(file, bytes)
}

/// `/^[a-z0-9]+(?:-[a-z0-9]+)*$/`.
fn valid_game_id(game_id: &str) -> bool {
    !game_id.is_empty()
        && game_id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

impl ProtonRunner {
    pub fn new(options: ProtonRunnerOptions) -> ProtonRunner {
        let logger = options.logger.unwrap_or_else(|| Arc::new(TracingLogger));
        let python: Arc<dyn PythonEnvironment> = match options.python_runtime {
            Some(python) => python,
            None => Arc::new(PythonRuntime::new(PythonOptions {
                data_dir: options.data_dir.clone(),
                logger: Some(logger.clone()),
                ..PythonOptions::default()
            })),
        };
        let user_namespaces: Arc<dyn UserNamespacesCheck> = match options.user_namespaces {
            Some(check) => check,
            None => Arc::new(UserNamespaces::new(UserNamespacesOptions {
                resources_path: options.resources_path.clone(),
                logger: Some(logger.clone()),
                ..UserNamespacesOptions::default()
            })),
        };
        ProtonRunner {
            inner: Arc::new(Inner {
                manager: options
                    .manager
                    .unwrap_or_else(|| Arc::new(NoProton) as Arc<dyn ProtonSelection>),
                data_dir: options.data_dir.to_string_lossy().into_owned(),
                resources_path: options.resources_path.to_string_lossy().into_owned(),
                logger,
                spawner: options.spawner.unwrap_or_else(|| Arc::new(SystemSpawner)),
                timeout_ms: options.prefix_command_timeout_ms,
                kill_grace_ms: options.prefix_command_kill_grace_ms,
                python,
                user_namespaces,
                display_mode_of: options
                    .display_mode_of
                    .unwrap_or_else(|| Arc::new(display_mode_of)),
                prefix_wineservers: options
                    .prefix_wineservers
                    .unwrap_or_else(|| Arc::new(prefix_wineservers)),
                umu_path: options.umu_path,
                env: options.env,
                now: options.now.unwrap_or_else(|| Arc::new(SystemTime::now)),
                path: NodePath::native(),
                fonts_pending: Mutex::new(HashMap::new()),
                prefix_pending: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn join(&self, parts: &[&str]) -> String {
        self.inner.path.join(parts)
    }

    /// `process.env` для этого запускальщика.
    fn process_env(&self) -> Env {
        self.inner.env.clone().unwrap_or_else(process_env)
    }

    fn iso_now(&self) -> String {
        js::iso_string((self.inner.now)())
    }

    /// `manager.selected()`.
    pub fn selected(&self) -> Option<ProtonBuild> {
        self.inner.manager.selected()
    }

    /// Сколько подготовок шрифтов идёт (`fontsPending.size`).
    pub fn fonts_pending(&self) -> usize {
        lock(&self.inner.fonts_pending).len()
    }

    /// Сколько подготовок префикса идёт (`prefixPending.size`).
    pub fn prefix_pending(&self) -> usize {
        lock(&self.inner.prefix_pending).len()
    }

    /// `umuPath()`: `resources/umu/umu-run`, иначе `umu-run` из `PATH`.
    pub fn umu_path(&self) -> Option<String> {
        if let Some(umu_path) = &self.inner.umu_path {
            return umu_path();
        }
        let bundled = self.join(&[&self.inner.resources_path, "umu", "umu-run"]);
        if is_executable(Path::new(&bundled)) {
            return Some(bundled);
        }
        executable_in_path("umu-run", &self.process_env()).map(|p| p.to_string_lossy().into_owned())
    }

    /// `prefixFor(gameId)`: `<данные>/compatdata/<id>/pfx`.
    pub fn prefix_for(&self, game_id: &str) -> Result<String, ProtonError> {
        if !valid_game_id(game_id) {
            return Err(ProtonError::new(
                "Недопустимый идентификатор игры для Proton-префикса",
            ));
        }
        Ok(self.join(&[&self.inner.data_dir, "compatdata", game_id, "pfx"]))
    }

    /// `environment(gameId)`: команда `umu-run`, префикс, сборка и окружение. Из окружения лаунчера
    /// убираются переменные AppImage (`LD_*`, `PYTHON*`, `QT_PLUGIN_PATH`, `APPDIR` и его пути в
    /// `PATH`) и чужого Proton; Wayland — по сессии, но всё в префиксе игры идёт в её режиме
    /// дисплея.
    pub fn environment(&self, game_id: &str) -> Result<LaunchEnvironment, ProtonError> {
        let Some(proton) = self.selected() else {
            return Err(ProtonError::new(
                "Proton не установлен. Откройте настройки и скачайте Proton Wineland.",
            ));
        };
        let Some(umu) = self.umu_path() else {
            return Err(ProtonError::new(
                "umu-run не найден. Пересоберите AppImage с umu или установите umu-launcher.",
            ));
        };
        let prefix = self.prefix_for(game_id)?;
        std::fs::create_dir_all(self.inner.path.dirname(&prefix)).map_err(io_error)?;
        let process = self.process_env();
        let mut env = process.clone();
        for key in STRIPPED_ENV {
            env.remove(key);
        }
        if let Some(appdir) = process.get("APPDIR").filter(|appdir| !appdir.is_empty()) {
            let path = env.get("PATH").cloned().unwrap_or_default();
            let kept: Vec<&str> = path
                .split(PATH_DELIMITER)
                .filter(|part| !part.starts_with(appdir.as_str()))
                .collect();
            env.insert("PATH".to_owned(), kept.join(&PATH_DELIMITER.to_string()));
        }
        // Wineland сам включает Wayland; явное значение делает умолчание HLSR предсказуемым, а
        // PROTON_ENABLE_WAYLAND=0 остаётся ручной поправкой.
        if !env.contains_key("PROTON_ENABLE_WAYLAND") {
            let wayland = env.get("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
            env.insert(
                "PROTON_ENABLE_WAYLAND".to_owned(),
                if wayland { "1" } else { "0" }.to_owned(),
            );
        }
        // Драйвер дисплея выбирает explorer.exe первой программы префикса: всё в префиксе игры —
        // в её режиме (HL1 — X11).
        if let Some(display) = (self.inner.display_mode_of)(game_id) {
            env.insert("PROTON_ENABLE_WAYLAND".to_owned(), display);
        }
        // Кеш шейдеров рядом с префиксом игры.
        env.entry("PROTON_LOCAL_SHADER_CACHE".to_owned())
            .or_insert_with(|| "1".to_owned());
        env.insert("WINEPREFIX".to_owned(), prefix.clone());
        env.insert("PROTONPATH".to_owned(), proton.path.clone());
        // umu по умолчанию ждёт выхода игры (waitforexitandrun) и держит LiveSplit.
        env.insert("PROTON_VERB".to_owned(), "run".to_owned());
        env.insert("GAMEID".to_owned(), "0".to_owned());
        env.insert("STORE".to_owned(), "none".to_owned());
        Ok(LaunchEnvironment {
            command: umu,
            prefix,
            proton,
            env,
        })
    }

    /// `ensureHostWineserver(launch, environment)`: wineserver префикса поднимается на хосте, вне
    /// контейнеров Steam Runtime: только он читает память процессов всех контейнеров префикса
    /// (SourceSplit в LiveSplit находит игру). Работающий сервер не трогается; новый префикс
    /// создаёт сам Proton. Итог: `unavailable`, `no-prefix`, `started`, `running`, `failed`.
    pub async fn ensure_host_wineserver(
        &self,
        launch: &LaunchEnvironment,
        environment: &Env,
    ) -> Result<&'static str, RunnerError> {
        let server = self.join(&[&launch.proton.path, "files", "bin", "wineserver"]);
        if !is_executable(Path::new(&server)) {
            return Ok("unavailable");
        }
        if !Path::new(&self.join(&[&launch.prefix, "system.reg"])).exists() {
            return Ok("no-prefix");
        }
        self.assert_prefix_server_matches(launch, &server)?;
        let mut env = Env::new();
        env.insert("PATH".to_owned(), "/usr/bin:/bin".to_owned());
        let home = environment
            .get("HOME")
            .filter(|home| !home.is_empty())
            .cloned()
            .unwrap_or_else(|| {
                std::env::home_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        env.insert("HOME".to_owned(), home);
        env.insert("WINEPREFIX".to_owned(), launch.prefix.clone());
        // Синхронизация сервера совпадает с клиентами: WINEFSYNC Proton ставит так же, а
        // PROTON_NO_NTSYNC сервер читает сам.
        let fsync = proton_flag(environment.get("PROTON_NO_FSYNC").map(String::as_str));
        env.insert(
            "WINEFSYNC".to_owned(),
            if fsync { "0" } else { "1" }.to_owned(),
        );
        if let Some(ntsync) = environment.get("PROTON_NO_NTSYNC") {
            env.insert("PROTON_NO_NTSYNC".to_owned(), ntsync.clone());
        }
        let child = self.inner.spawner.spawn(
            &server,
            &[],
            SpawnOptions {
                env,
                cwd: None,
                stdio: Stdio::Ignore,
                detached: true,
                windows_hide: None,
            },
        );
        // Без -f wineserver уходит в фон, когда сокет готов: 0 — запущен, 2 — уже работает.
        let code = match tokio::time::timeout(HOST_WINESERVER_TIMEOUT, child.ended()).await {
            Ok(crate::spawn::ChildExit::Exit { code, .. }) => code,
            _ => None,
        };
        match code {
            Some(0) => {
                self.inner.logger.info(&format!(
                    "[proton] wineserver префикса запущен на хосте: {}",
                    launch.prefix
                ));
                Ok("started")
            }
            Some(2) => Ok("running"),
            other => {
                let code = other.map_or_else(|| "null".to_owned(), |c| c.to_string());
                self.inner.logger.warn(&format!(
                    "[proton] wineserver на хосте не запустился (код {code}); его поднимет контейнер"
                ));
                Ok("failed")
            }
        }
    }

    /// `assertPrefixServerMatches(launch, server)`: клиенты Wine работают только с сервером своей
    /// сборки. Чужой сервер не гасится (в нём могут быть программы игрока) — игрока просят
    /// закрыть их.
    fn assert_prefix_server_matches(
        &self,
        launch: &LaunchEnvironment,
        server: &str,
    ) -> Result<(), RunnerError> {
        let Some(own) = file_identity(Path::new(server)) else {
            return Ok(());
        };
        let servers = (self.inner.prefix_wineservers)(Path::new(&launch.prefix));
        let Some(foreign) = servers.iter().find(|item| (item.dev, item.ino) != own) else {
            return Ok(());
        };
        let other = foreign
            .exe_path
            .as_deref()
            .and_then(|path| WINESERVER_OF.captures(path))
            .map_or_else(|| "другой сборки".to_owned(), |c| c[1].to_owned());
        Err(RunnerError::delivery(
            Code::LaunchFailed,
            format!(
                "В префиксе ещё работает Wine из Proton {other} (wineserver, PID {}), а выбран {}. Закройте LiveSplit и другие программы этой игры и запустите снова.",
                foreign.pid, launch.proton.name
            ),
            json!({"pid": foreign.pid, "proton": other}),
        ))
    }

    /// `runPrefixCommand(gameId, args, environment)`: команда `umu-run` в префиксе с выводом и
    /// тайм-аутом; отказ `bwrap` при первом запуске umu (он только скачал bwrap) — повтор после
    /// проверки пространств имён.
    pub async fn run_prefix_command(
        &self,
        game_id: &str,
        args: &[String],
        environment: &Env,
    ) -> Result<CommandOutput, RunnerError> {
        let mut launch = self.environment(game_id)?;
        launch.env = self.inner.python.prepare_environment(launch.env).await?;
        self.inner.user_namespaces.ensure(&launch.env).await?;
        let mut merged = launch.env.clone();
        merged.extend(environment.clone());
        self.ensure_host_wineserver(&launch, &merged).await?;
        launch.env.extend(environment.clone());
        let options = WaitOptions {
            label: Some(format!("umu-run {}", args.join(" "))),
            timeout_ms: self.inner.timeout_ms,
            kill_grace_ms: self.inner.kill_grace_ms,
        };
        let run = || {
            let options = options.clone();
            let child = self.inner.spawner.spawn(
                &launch.command,
                args,
                SpawnOptions {
                    env: launch.env.clone(),
                    cwd: None,
                    stdio: Stdio::Pipe,
                    detached: true,
                    windows_hide: None,
                },
            );
            async move { wait_for_prefix_command(&child, &options).await }
        };
        match run().await {
            Ok(output) => Ok(output),
            Err(error) => {
                if !BWRAP_DENIED.is_match(error.stderr.as_deref().unwrap_or("")) {
                    return Err(error.into());
                }
                self.inner.user_namespaces.ensure(&launch.env).await?;
                Ok(run().await?)
            }
        }
    }

    /// `prefixNeedsProtonUpdate(prefix, protonPath)`: в `drive_c` есть ссылка на файл другой
    /// сборки из `protons/`, который есть и в выбранной (ссылка на файл, которого новая сборка
    /// не поставляет, миграции не требует).
    pub fn prefix_needs_proton_update(
        &self,
        prefix: &str,
        proton_path: &str,
    ) -> Result<bool, ProtonError> {
        let path = &self.inner.path;
        let managed_root = format!(
            "{}{}",
            self.join(&[&self.inner.data_dir, "protons"]),
            path.sep()
        );
        let selected_root = format!("{}{}", path.resolve(&[proton_path]), path.sep());
        let mut directories = vec![self.join(&[prefix, "drive_c"])];
        while let Some(directory) = directories.pop() {
            let entries = match js::fs::read_dir(Path::new(&directory)) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(io_error(error)),
            };
            for entry in entries {
                let file = self.join(&[&directory, &entry.name]);
                if entry.file_type.is_dir() {
                    directories.push(file);
                } else if entry.file_type.is_symlink() {
                    let link = std::fs::read_link(&file).map_err(io_error)?;
                    let target = path.resolve(&[&directory, &link.to_string_lossy()]);
                    if !target.starts_with(&managed_root) || target.starts_with(&selected_root) {
                        continue;
                    }
                    let relative: Vec<&str> = target[managed_root.len()..]
                        .split(path.sep())
                        .skip(1)
                        .collect();
                    let relative = relative.join(path.sep());
                    if !relative.is_empty()
                        && Path::new(&self.join(&[proton_path, &relative])).exists()
                    {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// Ждать общую подготовку или начать её (`this.fontsPending`, `this.prefixPending`).
    fn shared(
        &self,
        map: fn(&Inner) -> &Mutex<HashMap<String, Pending>>,
        key: &str,
        start: impl FnOnce() -> Option<Pending>,
    ) -> Option<Pending> {
        let mut pending = lock(map(&self.inner));
        if let Some(shared) = pending.get(key) {
            return Some(shared.clone());
        }
        let shared = start()?;
        pending.insert(key.to_owned(), shared.clone());
        Some(shared)
    }

    /// `ensurePrefixRuntime(gameId, environment)`: после смены Proton префикс обновляется
    /// (`wineboot -u`) до шрифтов и запуска игры. Отметка `.hlsr-prefix-proton` помнит, что этот
    /// Proton уже готовил префикс (чужой Proton может оставить часть ссылок на прежнюю сборку).
    pub async fn ensure_prefix_runtime(
        &self,
        game_id: &str,
        environment: &Env,
    ) -> Result<(), RunnerError> {
        let pending = lock(&self.inner.prefix_pending).get(game_id).cloned();
        if let Some(pending) = pending {
            return pending.wait().await;
        }
        let proton = self.selected();
        let prefix = self.prefix_for(game_id)?;
        let Some(proton) = proton else {
            return Ok(());
        };
        if !self.prefix_needs_proton_update(&prefix, &proton.path)? {
            return Ok(());
        }
        let marker = self.join(&[&prefix, PREFIX_PROTON_MARKER]);
        let current = {
            let proton = proton.clone();
            let prefix = prefix.clone();
            let path = self.inner.path.clone();
            move || {
                let real = real_path(Path::new(&proton.path))
                    .map(|real| real.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| proton.path.clone());
                hlsr_core::json_text::stringify(
                    &json!({
                        "proton": real,
                        "protonVersion": read_trimmed(Path::new(&path.join(&[&proton.path, "version"]))),
                        "prefixVersion": read_trimmed(Path::new(&path.join(&[&prefix, "version"]))),
                    }),
                    "",
                )
            }
        };
        if read_trimmed(Path::new(&marker)).as_deref() == Some(current().as_str()) {
            return Ok(());
        }
        let shared = self.shared(
            |inner| &inner.prefix_pending,
            game_id,
            || {
                let this = self.clone();
                let cleaner = self.clone();
                let key = game_id.to_owned();
                let game = game_id.to_owned();
                let environment = environment.clone();
                Some(Shared::spawn(
                    async move {
                        let args = ["wineboot.exe".to_owned(), "-u".to_owned()];
                        this.run_prefix_command(&game, &args, &environment).await?;
                        if let Err(error) =
                            write_private(Path::new(&marker), format!("{}\n", current()).as_bytes())
                        {
                            this.inner.logger.warn(&format!(
                                "[proton] отметка подготовки префикса не записана: {}",
                                error.message
                            ));
                        }
                        this.inner.logger.info(&format!(
                            "[proton] префикс {game} подготовлен для {}",
                            proton.name
                        ));
                        Ok(())
                    },
                    move || {
                        lock(&cleaner.inner.prefix_pending).remove(&key);
                    },
                ))
            },
        );
        match shared {
            Some(shared) => shared.wait().await,
            None => Ok(()),
        }
    }

    /// `ensureCoreFonts(gameId, environment)`: corefonts через winetricks (один раз на префикс;
    /// холодный префикс может выйти с 0 без шрифтов — одна повторная попытка), отметка
    /// `.hlsr-corefonts-ready`. Повреждённые шрифты при записи в `winetricks.log` — ошибка с их
    /// списком.
    pub async fn ensure_core_fonts(
        &self,
        game_id: &str,
        environment: &Env,
    ) -> Result<(), RunnerError> {
        let prefix = self.prefix_for(game_id)?;
        let key = format!("core:{game_id}");
        let shared = self.shared(
            |inner| &inner.fonts_pending,
            &key,
            || {
                let this = self.clone();
                let cleaner = self.clone();
                let game = game_id.to_owned();
                let environment = environment.clone();
                let remove = key.clone();
                Some(Shared::spawn(
                    async move { this.install_core_fonts(&game, &prefix, &environment).await },
                    move || {
                        lock(&cleaner.inner.fonts_pending).remove(&remove);
                    },
                ))
            },
        );
        shared.expect("подготовка шрифтов начата").wait().await
    }

    async fn install_core_fonts(
        &self,
        game_id: &str,
        prefix: &str,
        environment: &Env,
    ) -> Result<(), RunnerError> {
        let prefix_path = Path::new(prefix);
        let marker = prefix_path.join(CORE_FONTS_MARKER);
        let status = core_fonts_status(prefix_path);
        if status.ready {
            // umu 1.4.4 отвечает 1 на уже поставленный verb: отметки нет после переезда — не беда.
            if !marker.exists() {
                write_private(&marker, format!("{}\n", self.iso_now()).as_bytes())?;
            }
            return Ok(());
        }
        if status.logged {
            return Err(ProtonError::new(format!(
                "В префиксе corefonts отмечены установленными, но повреждены или отсутствуют: {}. Префикс: {prefix}",
                status.missing.join(", ")
            ))
            .into());
        }
        if let Some(proton) = self.selected()
            && !proton_has_winetricks(Path::new(&proton.path))
        {
            return Err(RunnerError::delivery(
                Code::LaunchFailed,
                format!(
                    "В Proton {} нет protonfixes/winetricks, поэтому он не может установить шрифты в префикс игры. Для первого запуска выберите в настройках Proton Wineland (или GE-Proton); потом можно вернуться к прежнему.",
                    proton.name
                ),
                json!({"proton": proton.name}),
            ));
        }
        self.inner
            .logger
            .info(&format!("[proton] устанавливаю corefonts в {prefix}"));
        let args = ["winetricks".to_owned(), "corefonts".to_owned()];
        for attempt in 0..2 {
            let output = match self.run_prefix_command(game_id, &args, environment).await {
                Ok(output) => Some(output),
                Err(error) => {
                    // Только проверенный случай: verb поставлен параллельно.
                    let already = match &error {
                        RunnerError::Proton(error) => {
                            error.exit_code == Some(1)
                                && ALREADY_INSTALLED.is_match(&format!(
                                    "{}\n{}",
                                    error.stderr.as_deref().unwrap_or(""),
                                    error.stdout.as_deref().unwrap_or("")
                                ))
                        }
                        RunnerError::Delivery(_) => false,
                    };
                    if !already || !core_fonts_status(prefix_path).ready {
                        return Err(error);
                    }
                    None
                }
            };
            let installed = core_fonts_status(prefix_path);
            if installed.missing.is_empty() {
                break;
            }
            if attempt == 0 && !installed.logged {
                self.inner.logger.warn(
                    "[proton] corefonts не завершены после инициализации префикса; повторяю установку",
                );
                continue;
            }
            let details = output
                .map(|output| {
                    [output.stderr, output.stdout]
                        .into_iter()
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let details = if details.is_empty() {
                String::new()
            } else {
                format!("\n{details}")
            };
            return Err(ProtonError::new(format!(
                "Установка corefonts завершилась без необходимых шрифтов: {}. Префикс: {prefix}{details}",
                installed.missing.join(", ")
            ))
            .into());
        }
        std::fs::create_dir_all(prefix_path).map_err(io_error)?;
        write_private(&marker, format!("{}\n", self.iso_now()).as_bytes())?;
        Ok(())
    }

    /// `liveSplitFontsReady(prefix)`: отметка есть и все Carlito на месте.
    pub fn live_split_fonts_ready(&self, prefix: &Path) -> bool {
        if !prefix.join(LIVESPLIT_FONT_MARKER).exists() {
            return false;
        }
        let fonts = prefix.join("drive_c").join("windows").join("Fonts");
        LIVESPLIT_FONTS
            .iter()
            .all(|font| sha256_file(&fonts.join(font.target)).is_ok_and(|hash| hash == font.sha256))
    }

    /// `ensureLiveSplitFonts(gameId)`: Carlito в `windows/Fonts` префикса и замены Calibri и
    /// Segoe UI в реестре (`regedit /S`).
    pub async fn ensure_live_split_fonts(&self, game_id: &str) -> Result<(), RunnerError> {
        let prefix = self.prefix_for(game_id)?;
        if self.live_split_fonts_ready(Path::new(&prefix)) {
            return Ok(());
        }
        let key = format!("livesplit:{game_id}");
        let shared = self.shared(
            |inner| &inner.fonts_pending,
            &key,
            || {
                let this = self.clone();
                let cleaner = self.clone();
                let game = game_id.to_owned();
                let remove = key.clone();
                Some(Shared::spawn(
                    async move { this.install_live_split_fonts(&game, &prefix).await },
                    move || {
                        lock(&cleaner.inner.fonts_pending).remove(&remove);
                    },
                ))
            },
        );
        shared.expect("подготовка шрифтов начата").wait().await
    }

    async fn install_live_split_fonts(
        &self,
        game_id: &str,
        prefix: &str,
    ) -> Result<(), RunnerError> {
        let source_dir = PathBuf::from(self.join(&[&self.inner.resources_path, "livesplit-fonts"]));
        let font_dir = PathBuf::from(self.join(&[prefix, "drive_c", "windows", "Fonts"]));
        let windows_temp = PathBuf::from(self.join(&[prefix, "drive_c", "windows", "temp"]));
        std::fs::create_dir_all(&font_dir).map_err(io_error)?;
        std::fs::create_dir_all(&windows_temp).map_err(io_error)?;
        for font in LIVESPLIT_FONTS {
            let source = source_dir.join(font.source);
            if sha256_file(&source).map_err(io_error)? != font.sha256 {
                return Err(ProtonError::new(format!(
                    "Шрифт LiveSplit повреждён: {}",
                    font.source
                ))
                .into());
            }
            let target = font_dir.join(font.target);
            let temporary = PathBuf::from(format!(
                "{}.{}.tmp",
                target.to_string_lossy(),
                std::process::id()
            ));
            std::fs::copy(&source, &temporary).map_err(io_error)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644))
                    .map_err(io_error)?;
            }
            std::fs::rename(&temporary, &target).map_err(io_error)?;
        }
        let registry_file = windows_temp.join("hlsr-livesplit-fonts.reg");
        write_private(&registry_file, livesplit_font_registry().as_bytes())?;
        let result = async {
            let wayland = self
                .environment(game_id)?
                .env
                .get("PROTON_ENABLE_WAYLAND")
                .cloned()
                .unwrap_or_default();
            let mut env = Env::new();
            env.insert("PROTON_ENABLE_WAYLAND".to_owned(), wayland);
            env.insert(
                "PROTON_USE_X11_EXCLUSIVE".to_owned(),
                "LiveSplit.exe".to_owned(),
            );
            let args = [
                "regedit.exe".to_owned(),
                "/S".to_owned(),
                "C:\\windows\\temp\\hlsr-livesplit-fonts.reg".to_owned(),
            ];
            self.run_prefix_command(game_id, &args, &env).await
        }
        .await;
        // Файл уже мог исчезнуть.
        let _ = std::fs::remove_file(&registry_file);
        result?;
        write_private(
            &PathBuf::from(self.join(&[prefix, LIVESPLIT_FONT_MARKER])),
            format!("{}\n", self.iso_now()).as_bytes(),
        )?;
        self.inner
            .logger
            .info(&format!("[proton] установлены шрифты LiveSplit в {prefix}"));
        Ok(())
    }

    /// `ensureFonts(gameId, profile, environment)`.
    pub async fn ensure_fonts(
        &self,
        game_id: &str,
        profile: Option<&str>,
        environment: &Env,
    ) -> Result<(), RunnerError> {
        self.ensure_core_fonts(game_id, environment).await?;
        if profile == Some(LIVESPLIT_FONT_PROFILE) {
            self.ensure_live_split_fonts(game_id).await?;
        }
        Ok(())
    }

    /// `steamBridgeCommand(launch, executable, args, bridge)`: запуск напрямую через точку входа
    /// Steam Runtime, как это делает Steam: без переменных umu, с префиксом той же папки.
    pub fn steam_bridge_command(
        &self,
        launch: &LaunchEnvironment,
        executable: &str,
        args: &[String],
        bridge: Option<&SteamBridge>,
    ) -> Result<(String, Vec<String>, Env), ProtonError> {
        let Some(bridge) = bridge.filter(|bridge| bridge.app_id > 0) else {
            return Err(ProtonError::new("Для моста к Steam нужен AppID игры"));
        };
        if bridge.steam_path.is_empty() || !self.inner.path.is_absolute(&bridge.steam_path) {
            return Err(ProtonError::new(
                "Для моста к Steam нужен путь к клиенту Steam",
            ));
        }
        let Some(runtime) = required_runtime_name(Path::new(&launch.proton.path)) else {
            let name = if launch.proton.name.is_empty() {
                &launch.proton.path
            } else {
                &launch.proton.name
            };
            return Err(ProtonError::new(format!(
                "Не удалось определить Steam Runtime для Proton {name}"
            )));
        };
        let runtime_dir = self.join(&[&umu_root(&launch.env).to_string_lossy(), runtime]);
        let entry_point = self.join(&[&runtime_dir, "_v2-entry-point"]);
        if !is_executable(Path::new(&entry_point)) {
            return Err(ProtonError::new(format!(
                "Steam Runtime {runtime} не найден: {entry_point}"
            )));
        }
        let mut env = launch.env.clone();
        for key in UMU_ONLY_ENV {
            env.remove(key);
        }
        let app_id = bridge.app_id.to_string();
        let tools = format!("{}:{runtime_dir}", launch.proton.path);
        let install = bridge
            .install_path
            .clone()
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| self.inner.path.dirname(executable));
        for (key, value) in [
            ("STEAM_COMPAT_DATA_PATH", launch.prefix.clone()),
            (
                "STEAM_COMPAT_SHADER_PATH",
                self.join(&[&launch.prefix, "shadercache"]),
            ),
            (
                "STEAM_COMPAT_CLIENT_INSTALL_PATH",
                bridge.steam_path.clone(),
            ),
            ("STEAM_COMPAT_INSTALL_PATH", install),
            ("STEAM_COMPAT_TOOL_PATHS", tools.clone()),
            ("STEAM_COMPAT_MOUNTS", tools),
            ("STEAM_COMPAT_APP_ID", app_id.clone()),
            ("SteamAppId", app_id.clone()),
            ("SteamGameId", app_id),
        ] {
            env.insert(key.to_owned(), value);
        }
        let mut argv = vec![
            "--verb=run".to_owned(),
            "--".to_owned(),
            self.join(&[&launch.proton.path, "proton"]),
            "run".to_owned(),
            executable.to_owned(),
        ];
        argv.extend(args.iter().cloned());
        Ok((entry_point, argv, env))
    }

    /// `wrapperPath(name, env)`: абсолютный путь — как есть, имя — из `PATH`, относительный путь
    /// не ищется.
    pub fn wrapper_path(&self, name: &str, env: &Env) -> Result<String, RunnerError> {
        let found = if self.inner.path.is_absolute(name) {
            is_executable(Path::new(name)).then(|| name.to_owned())
        } else if name.contains('/') {
            None
        } else {
            executable_in_path(name, env).map(|path| path.to_string_lossy().into_owned())
        };
        found.ok_or_else(|| {
            RunnerError::delivery(
                Code::ProtonWrapperMissing,
                format!("Программа из параметров Proton не найдена: {name}"),
                Value::Null,
            )
        })
    }

    /// `launch(gameId, executable, args, options)`: подготовка префикса и шрифтов, окружение,
    /// wineserver на хосте, мост к Steam, обёртка, `taskset`, журнал; процесс сессии.
    pub async fn launch(
        &self,
        game_id: &str,
        executable: &Path,
        args: &[String],
        options: LaunchOptions,
    ) -> Result<ProtonLaunch, RunnerError> {
        let executable_text = executable.to_string_lossy().into_owned();
        if !self.inner.path.is_absolute(&executable_text) || !executable.exists() {
            return Err(
                ProtonError::new(format!("Исполняемый файл не найден: {executable_text}")).into(),
            );
        }
        let throw_if_aborted = || match &options.abort {
            Some(abort) if abort.is_aborted() => {
                Err(RunnerError::from(ProtonError::abort("Запуск отменён")))
            }
            _ => Ok(()),
        };
        throw_if_aborted()?;
        self.ensure_prefix_runtime(game_id, &options.env).await?;
        throw_if_aborted()?;
        if !options.skip_fonts {
            self.ensure_fonts(game_id, options.font_profile.as_deref(), &options.env)
                .await?;
        }
        // Шрифты общие с обычными запусками: отмена предпросмотра не останавливает их, но и не
        // запускает игру после них.
        throw_if_aborted()?;
        let mut launch = self.environment(game_id)?;
        launch.env = self.inner.python.prepare_environment(launch.env).await?;
        self.inner.user_namespaces.ensure(&launch.env).await?;
        throw_if_aborted()?;
        let mut merged = launch.env.clone();
        merged.extend(options.env.clone());
        self.ensure_host_wineserver(&launch, &merged).await?;
        let mut command = launch.command.clone();
        let mut argv: Vec<String> = std::iter::once(executable_text.clone())
            .chain(args.iter().cloned())
            .collect();
        if options.steam_bridge.is_some() {
            let (bridged, bridged_argv, env) = self.steam_bridge_command(
                &launch,
                &executable_text,
                args,
                options.steam_bridge.as_ref(),
            )?;
            command = bridged;
            argv = bridged_argv;
            launch.env = env;
        }
        // Обёртка игрока (mangohud, gamemoderun) запускает umu-run целиком, как «mangohud %command%»
        // в Steam.
        if let Some((first, rest)) = options.wrapper.split_first() {
            argv = rest
                .iter()
                .cloned()
                .chain(std::iter::once(command.clone()))
                .chain(argv)
                .collect();
            command = self.wrapper_path(first, &launch.env)?;
        }
        if options.exclude_cpu0 {
            let taskset = executable_in_path("taskset", &self.process_env());
            let allowed = allowed_cpu_list_without_zero();
            match (taskset, allowed) {
                (Some(taskset), Some(allowed)) => {
                    argv = ["-c".to_owned(), allowed, command.clone()]
                        .into_iter()
                        .chain(argv)
                        .collect();
                    command = taskset.to_string_lossy().into_owned();
                }
                _ => self.inner.logger.warn(
                    "[proton] доступный набор CPU или taskset не найден; ограничение CPU 0 пропущено",
                ),
            }
        }
        // Вывод umu, Proton и Wine — в файл: без него падение игры видно только по коду выхода.
        // Без PROTON_LOG Proton ставит WINEDEBUG=-all, и файл остаётся маленьким.
        let mut stdio = Stdio::Ignore;
        if let Some(log_file) = &options.log_file {
            let opened = (|| -> std::io::Result<std::fs::File> {
                if let Some(parent) = log_file.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut file_options = std::fs::OpenOptions::new();
                file_options.write(true).create(true).truncate(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt as _;
                    file_options.mode(0o600);
                }
                let mut file = file_options.open(log_file)?;
                std::io::Write::write_all(
                    &mut file,
                    format!(
                        "# {} {}: {} {}\n",
                        self.iso_now(),
                        launch.proton.name,
                        executable_text,
                        args.join(" ")
                    )
                    .as_bytes(),
                )?;
                Ok(file)
            })();
            match opened {
                Ok(file) => stdio = Stdio::Log(file),
                Err(error) => self.inner.logger.warn(&format!(
                    "[proton] журнал запуска не открыт: {}",
                    io_error(error).message
                )),
            }
        }
        launch.env.extend(options.env.clone());
        let cwd = options
            .cwd
            .clone()
            .unwrap_or_else(|| PathBuf::from(self.inner.path.dirname(&executable_text)));
        let child = self.inner.spawner.spawn(
            &command,
            &argv,
            SpawnOptions {
                env: launch.env,
                cwd: Some(cwd),
                stdio,
                detached: true,
                windows_hide: None,
            },
        );
        child.spawned().await?;
        Ok(ProtonLaunch {
            child,
            proton: launch.proton,
            prefix: launch.prefix,
            command,
        })
    }
}

/// Выбора нет (менеджер не передан).
struct NoProton;

impl ProtonSelection for NoProton {
    fn selected(&self) -> Option<ProtonBuild> {
        None
    }
}

impl ProtonLauncher for ProtonRunner {
    fn launch<'a>(
        &'a self,
        game_id: &'a str,
        executable: &'a Path,
        args: &'a [String],
        options: LaunchOptions,
    ) -> BoxFuture<'a, Result<ProtonLaunch, RunnerError>> {
        Box::pin(ProtonRunner::launch(
            self, game_id, executable, args, options,
        ))
    }

    fn ensure_live_split_fonts<'a>(
        &'a self,
        game_id: &'a str,
    ) -> BoxFuture<'a, Result<(), RunnerError>> {
        Box::pin(ProtonRunner::ensure_live_split_fonts(self, game_id))
    }

    fn selected(&self) -> Option<ProtonBuild> {
        ProtonRunner::selected(self)
    }

    fn umu_path(&self) -> Option<String> {
        ProtonRunner::umu_path(self)
    }
}

/// Инструмент (LiveSplit) в префиксе игры: `protonRunner.launch(gameId, exe, args,
/// {cwd, fontProfile, env})` из `ToolService.js`.
impl PrefixLauncher for ProtonRunner {
    fn launch<'a>(
        &'a self,
        game_id: Option<&'a str>,
        exe: &'a Path,
        args: &'a [String],
        options: &'a PrefixLaunchOptions,
    ) -> BoxFuture<'a, Result<PrefixLaunched, Failure>> {
        Box::pin(async move {
            // На Linux ToolService всегда передаёт игру (по умолчанию ghosting-renewed).
            let Some(game_id) = game_id else {
                return Err(Failure::from(RunnerError::from(ProtonError::new(
                    "Недопустимый идентификатор игры для Proton-префикса",
                ))));
            };
            let launch_options = LaunchOptions {
                cwd: Some(options.cwd.clone()),
                env: options
                    .env
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
                font_profile: options.font_profile.clone(),
                ..LaunchOptions::default()
            };
            let launched = ProtonRunner::launch(self, game_id, exe, args, launch_options)
                .await
                .map_err(Failure::from)?;
            Ok(PrefixLaunched {
                child: launched.child.handle(),
                prefix: PathBuf::from(launched.prefix),
            })
        })
    }
}
