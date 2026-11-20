//! Поиск Steam, его процесса, библиотек и приложений на Windows (`SteamLocator.js`): Steam из
//! реестра (`HKCU\Software\Valve\Steam`: `SteamPath`, `SteamExe`; `…\ActiveProcess`: `pid`),
//! запущенный процесс Steam, `libraryfolders.vdf`, `appmanifest_<id>.acf`, полнота Source SDK Base.
//!
//! Реестр читается через порт [`Registry`] (WinAPI вместо `reg.exe`, решение 2 спецификации);
//! значения приводятся к тому, что видел 0.4.7 после разбора вывода `reg query`: `REG_DWORD` —
//! число, остальные — строка без пробелов по краям.

use crate::process_inspect::{
    ProcessInfo, ProcessInspector, SOURCE_TASKLIST, node_path, system_inspector,
};
use crate::steam_fs::{RealFs, SteamFs, read_to_string};
use crate::vdf::{self, VdfValue, get_insensitive, get_insensitive_in};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::json_text::js_number;
use hlsr_delivery::errors::{Code, DeliveryError};
use hlsr_delivery::ports::{
    Logger, Registry, RegistryHive, RegistryValue, SystemRegistry, TracingLogger,
};
use hlsr_delivery::timing::{Clock, system_clock};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// `STEAM_KEY` без куста (`HKCU`).
pub const STEAM_KEY: &str = "Software\\Valve\\Steam";
/// `ACTIVE_PROCESS_KEY` без куста (`HKCU`).
pub const ACTIVE_PROCESS_KEY: &str = "Software\\Valve\\Steam\\ActiveProcess";
/// `SOURCE_SDK_APP_ID`.
pub const SOURCE_SDK_APP_ID: u32 = 243730;

const SDK_REQUIRED_DIRS: [&str; 3] = ["hl2", "bin", "platform"];
const SDK_REQUIRED_FILES: [&str; 1] = ["hl2.exe"];

/// Значение реестра, как его возвращает `readRegistryValue`: нет (`null`), число (`REG_DWORD`)
/// или строка.
#[derive(Debug, Clone, PartialEq)]
pub enum RegData {
    Missing,
    Number(f64),
    Text(String),
}

impl RegData {
    /// Истинность в JS: непустая строка, число не 0 и не `NaN`.
    pub fn truthy(&self) -> bool {
        match self {
            RegData::Missing => false,
            RegData::Number(number) => *number != 0.0 && !number.is_nan(),
            RegData::Text(text) => !text.is_empty(),
        }
    }

    /// `String(value)` для значения, которое есть.
    pub fn js_string(&self) -> String {
        match self {
            RegData::Missing => "null".to_owned(),
            RegData::Number(number) => hlsr_core::js::number::number_to_string(*number),
            RegData::Text(text) => text.clone(),
        }
    }

    /// `Number(value)`: число — оно же, строка — `Number("…")`, `null` — 0.
    pub fn to_number(&self) -> f64 {
        match self {
            RegData::Missing => 0.0,
            RegData::Number(number) => *number,
            RegData::Text(text) => js::to_number(text),
        }
    }
}

/// `readRegistryValue(key, name)` в HKCU: `REG_DWORD` — число, `REG_SZ` и `REG_EXPAND_SZ` — строка
/// (`reg query` печатает её как есть, без раскрытия переменных; пробелы по краям срезает разбор).
pub fn read_registry_value(registry: &dyn Registry, key: &str, name: &str) -> RegData {
    match registry.read_value(RegistryHive::CurrentUser, key, name) {
        None => RegData::Missing,
        Some(RegistryValue::Dword(number)) => RegData::Number(f64::from(number)),
        Some(RegistryValue::Sz(text) | RegistryValue::ExpandSz(text)) => {
            RegData::Text(js::trim(&text).to_owned())
        }
    }
}

/// Установка Steam: `{installPath, steamExe}`, на Linux ещё `steamArgs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamInstall {
    pub install_path: String,
    pub steam_exe: String,
    /// Аргументы запуска (только Linux: Flatpak — `run com.valvesoftware.Steam`).
    pub steam_args: Option<Vec<String>>,
}

impl SteamInstall {
    /// Объект JS: `{installPath, steamExe}` и `steamArgs`, если он есть.
    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("installPath".into(), self.install_path.clone().into());
        object.insert("steamExe".into(), self.steam_exe.clone().into());
        if let Some(args) = &self.steam_args {
            object.insert("steamArgs".into(), json!(args));
        }
        Value::Object(object)
    }
}

/// Работающий Steam: PID, откуда узнали (`registry`, `registry-name`, `scan`, `proc`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningSteam {
    pub pid: u32,
    pub executable_path: Option<String>,
    pub started_at: Option<String>,
    pub source: Option<String>,
}

/// Приложение Steam, найденное в библиотеке.
#[derive(Debug, Clone, PartialEq)]
pub struct AppInfo {
    pub app_id: u32,
    pub library: String,
    pub manifest_path: String,
    pub installdir: String,
    pub app_path: String,
    /// `StateFlags` (Windows), `None` — нет или 0.
    pub state_flags: Option<f64>,
    /// `name` (Windows).
    pub name: Option<String>,
}

impl AppInfo {
    /// Объект ответа `preflight`: `{appId, library, manifestPath, installdir, appPath}`.
    pub fn to_value(&self) -> Value {
        json!({
            "appId": self.app_id,
            "library": self.library,
            "manifestPath": self.manifest_path,
            "installdir": self.installdir,
            "appPath": self.app_path,
        })
    }
}

/// Итог `preflight`: Steam, его процесс и приложение.
#[derive(Debug, Clone, PartialEq)]
pub struct Preflight {
    pub steam: SteamInstall,
    pub running: RunningSteam,
    pub app: AppInfo,
}

/// `{installPath, steamExe, [steamArgs,] pid, source}` — Steam с процессом.
pub fn steam_summary(steam: &SteamInstall, running: &RunningSteam) -> Value {
    let mut object = match steam.to_value() {
        Value::Object(object) => object,
        _ => serde_json::Map::new(),
    };
    object.insert("pid".into(), running.pid.into());
    object.insert("source".into(), running.source.clone().into());
    Value::Object(object)
}

/// `{ok: false, code, message}` — ответ `describe*` при ошибке.
pub fn failure_value(error: &DeliveryError) -> Value {
    json!({
        "ok": false,
        "code": error.code.as_str(),
        "message": error.message,
    })
}

/// То, что `SteamAccount` берёт у поиска Steam (`locateSteam`, `describeSteamOnly`) — общее для
/// Windows и Linux.
pub trait SteamLocating: Send + Sync {
    fn locate_steam(&self) -> Result<SteamInstall, DeliveryError>;

    /// `describeSteamOnly()`: `{ok, code, steam, app?}` или `{ok: false, code, message}`.
    fn describe_steam_only(&self) -> Value;
}

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct LocatorOptions {
    pub fs: Option<Arc<dyn SteamFs>>,
    pub registry: Option<Arc<dyn Registry>>,
    pub inspector: Option<Arc<dyn ProcessInspector>>,
    /// `platform`; по умолчанию — эта ОС. Не Windows — `locateSteam` отвечает отказом.
    pub platform: Option<Platform>,
    pub logger: Option<Arc<dyn Logger>>,
    /// `missTtlMs`: сколько помнится «Steam не запущен» (по умолчанию 8000).
    pub miss_ttl_ms: Option<i64>,
    /// `now`: часы (`Date.now()`).
    pub now: Option<Clock>,
}

struct Miss {
    pid: RegData,
    steam_exe: String,
    at: i64,
    code: Code,
    message: String,
}

struct Running {
    pid: u32,
    steam_exe: String,
    result: RunningSteam,
}

#[derive(Default)]
struct Caches {
    running: Option<Running>,
    miss: Option<Miss>,
    steam: Option<SteamInstall>,
}

/// `SteamLocator` (Windows).
pub struct SteamLocator {
    fs: Arc<dyn SteamFs>,
    registry: Arc<dyn Registry>,
    inspector: Arc<dyn ProcessInspector>,
    platform: Platform,
    path: NodePath,
    logger: Arc<dyn Logger>,
    miss_ttl_ms: i64,
    now: Clock,
    caches: Mutex<Caches>,
}

/// Результат `findSteamProcess`: совпавший по пути процесс и число процессов образа без пути
/// (не от `tasklist`: те — «путь узнать нечем», а не «другие права»).
struct Scan {
    found: Option<ProcessInfo>,
    blind: usize,
}

impl SteamLocator {
    pub fn new(options: LocatorOptions) -> SteamLocator {
        let platform = options.platform.unwrap_or(Platform::CURRENT);
        SteamLocator {
            fs: options.fs.unwrap_or_else(|| Arc::new(RealFs)),
            registry: options.registry.unwrap_or_else(|| Arc::new(SystemRegistry)),
            inspector: options.inspector.unwrap_or_else(system_inspector),
            platform,
            path: node_path(platform),
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            miss_ttl_ms: options.miss_ttl_ms.unwrap_or(8000),
            now: options.now.unwrap_or_else(system_clock),
            caches: Mutex::new(Caches::default()),
        }
    }

    fn caches(&self) -> std::sync::MutexGuard<'_, Caches> {
        self.caches
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn read(&self, key: &str, name: &str) -> RegData {
        read_registry_value(self.registry.as_ref(), key, name)
    }

    /// `locateSteam()`.
    pub fn locate_steam(&self) -> Result<SteamInstall, DeliveryError> {
        if self.platform != Platform::Win32 {
            return Err(DeliveryError::new(
                Code::SteamNotInstalled,
                "Запуск через Steam поддерживается только в Windows",
            ));
        }
        if let Some(steam) = self.caches().steam.clone() {
            return Ok(steam);
        }

        let steam_path = self.read(STEAM_KEY, "SteamPath");
        let steam_exe_value = self.read(STEAM_KEY, "SteamExe");

        if !steam_path.truthy() && !steam_exe_value.truthy() {
            return Err(DeliveryError::new(
                Code::SteamNotInstalled,
                "Steam не найден в реестре текущего пользователя",
            ));
        }

        let root = if steam_path.truthy() {
            self.path.normalize(&steam_path.js_string())
        } else {
            self.path
                .dirname(&self.path.normalize(&steam_exe_value.js_string()))
        };
        let steam_exe = if steam_exe_value.truthy() {
            self.path.normalize(&steam_exe_value.js_string())
        } else {
            self.path.join(&[&root, "steam.exe"])
        };

        if !self.fs.exists(&steam_exe) {
            return Err(DeliveryError::new(
                Code::SteamNotInstalled,
                format!("Файл Steam не найден: {steam_exe}"),
            ));
        }

        let steam = SteamInstall {
            install_path: root,
            steam_exe,
            steam_args: None,
        };
        self.caches().steam = Some(steam.clone());
        Ok(steam)
    }

    /// `assertSteamRunning(steam)`.
    pub fn assert_steam_running(
        &self,
        steam: &SteamInstall,
    ) -> Result<RunningSteam, DeliveryError> {
        let pid = self.read(ACTIVE_PROCESS_KEY, "pid");
        let registry_problem;

        {
            let caches = self.caches();
            if let Some(miss) = &caches.miss
                && miss.pid == pid
                && miss.steam_exe == steam.steam_exe
                && (self.now)() - miss.at < self.miss_ttl_ms
            {
                return Err(DeliveryError::new(miss.code, miss.message.clone()));
            }
        }

        let valid_pid = match &pid {
            RegData::Number(number) if number.fract() == 0.0 && *number > 0.0 => {
                Some(*number as u32)
            }
            _ => None,
        };

        match valid_pid {
            None => registry_problem = "в реестре нет активного процесса".to_owned(),
            Some(pid) if !self.inspector.is_alive(pid) => {
                registry_problem = format!("процесс {pid} из реестра мёртв");
            }
            Some(pid) => {
                let cached = {
                    let caches = self.caches();
                    caches
                        .running
                        .as_ref()
                        .filter(|cache| cache.pid == pid && cache.steam_exe == steam.steam_exe)
                        .map(|cache| cache.result.clone())
                };
                if let Some(result) = cached {
                    return Ok(result);
                }

                let actual = self.inspector.inspect(pid);

                if self.same_image_name(actual.as_ref(), steam) {
                    // Путь узнать нечем (система не отвечает, ответил только tasklist), но
                    // живой процесс из реестра Steam называется steam.exe.
                    self.logger.warn(&format!(
                        "[steam] путь процесса {pid} недоступен ({}); принимаю Steam по имени образа",
                        self.inspector_failure()
                            .unwrap_or_else(|| "нет данных".to_owned())
                    ));
                    let result = RunningSteam {
                        pid,
                        executable_path: Some(steam.steam_exe.clone()),
                        started_at: None,
                        source: Some("registry-name".to_owned()),
                    };
                    self.remember_running(pid, steam, &result);
                    return Ok(result);
                }

                match actual
                    .as_ref()
                    .and_then(|a| a.executable_path.as_deref())
                    .filter(|path| !path.is_empty())
                {
                    None => {
                        let failure = self.inspector_failure();
                        registry_problem = format!(
                            "процесс {pid} из реестра не удалось опознать{}",
                            failure.map(|f| format!(": {f}")).unwrap_or_default()
                        );
                    }
                    Some(path) if !self.inspector.same_path(Some(path), Some(&steam.steam_exe)) => {
                        registry_problem = format!("процесс {pid} из реестра — не Steam: {path}");
                    }
                    Some(path) => {
                        let result = RunningSteam {
                            pid,
                            executable_path: Some(path.to_owned()),
                            started_at: actual.as_ref().and_then(|a| a.started_at.clone()),
                            source: Some("registry".to_owned()),
                        };
                        self.remember_running(pid, steam, &result);
                        return Ok(result);
                    }
                }
            }
        }

        let scan = self.find_steam_process(steam);
        if let Some(found) = scan.found {
            self.logger.warn(&format!(
                "[steam] {registry_problem}; нашёл работающий Steam по пути: PID {}",
                found.pid
            ));
            return Ok(RunningSteam {
                pid: found.pid,
                executable_path: found.executable_path,
                started_at: found.started_at,
                source: Some("scan".to_owned()),
            });
        }

        let (code, message) = if scan.blind > 0 {
            (
                Code::SteamElevated,
                format!(
                    "Steam запущен с другими правами: {registry_problem}, \
                     процессов steam.exe без доступа к пути: {}",
                    scan.blind
                ),
            )
        } else {
            (
                Code::SteamNotRunning,
                format!("Steam не запущен ({registry_problem})"),
            )
        };

        self.caches().miss = Some(Miss {
            pid,
            steam_exe: steam.steam_exe.clone(),
            at: (self.now)(),
            code,
            message: message.clone(),
        });

        Err(DeliveryError::new(code, message))
    }

    fn remember_running(&self, pid: u32, steam: &SteamInstall, result: &RunningSteam) {
        self.caches().running = Some(Running {
            pid,
            steam_exe: steam.steam_exe.clone(),
            result: result.clone(),
        });
    }

    /// `sameImageName`: ответил только `tasklist` (имя образа без пути), и образ называется так
    /// же, как `steam.exe` из реестра.
    fn same_image_name(&self, actual: Option<&ProcessInfo>, steam: &SteamInstall) -> bool {
        let Some(actual) = actual else {
            return false;
        };
        if actual
            .executable_path
            .as_deref()
            .is_some_and(|p| !p.is_empty())
            || actual.source.as_deref() != Some(SOURCE_TASKLIST)
        {
            return false;
        }
        let Some(name) = actual.name.as_deref().filter(|name| !name.is_empty()) else {
            return false;
        };
        let steam_name = NodePath::new(Platform::Win32, "C:\\").basename(&steam.steam_exe);
        name.to_lowercase() == steam_name.to_lowercase()
    }

    fn inspector_failure(&self) -> Option<String> {
        self.inspector.last_failure()
    }

    fn find_steam_process(&self, steam: &SteamInstall) -> Scan {
        let candidates = self
            .inspector
            .find_by_image_name(&self.path.basename(&steam.steam_exe));
        let found = candidates
            .iter()
            .find(|item| {
                item.executable_path.as_deref().is_some_and(|path| {
                    !path.is_empty() && self.inspector.same_path(Some(path), Some(&steam.steam_exe))
                })
            })
            .cloned();
        // Без пути от tasklist — это не «другие права», а просто нечем узнать путь.
        let blind = candidates
            .iter()
            .filter(|item| {
                item.executable_path.as_deref().is_none_or(str::is_empty)
                    && item.source.as_deref() != Some(SOURCE_TASKLIST)
            })
            .count();
        Scan { found, blind }
    }

    /// `add(root)` из `steamLibraries`: папка после `resolve`, без повторов (регистр не важен).
    fn add_library(&self, libraries: &mut Vec<String>, seen: &mut HashSet<String>, root: &str) {
        if root.is_empty() {
            return;
        }
        let resolved = self.path.resolve(&[root]);
        if seen.insert(resolved.to_lowercase()) {
            libraries.push(resolved);
        }
    }

    /// `steamLibraries(steam)`: корень Steam и папки из `libraryfolders.vdf`, без повторов
    /// (регистр не важен).
    pub fn steam_libraries(&self, steam: &SteamInstall) -> Vec<String> {
        let mut libraries = Vec::new();
        let mut seen = HashSet::new();

        self.add_library(&mut libraries, &mut seen, &steam.install_path);

        let file = self
            .path
            .join(&[&steam.install_path, "steamapps", "libraryfolders.vdf"]);
        if !self.fs.exists(&file) {
            return libraries;
        }

        let data = match read_to_string(self.fs.as_ref(), &file)
            .map_err(|error| error.to_string())
            .and_then(|text| vdf::parse(&text).map_err(|error| error.message))
        {
            Ok(data) => data,
            Err(message) => {
                self.logger.warn(&format!(
                    "[steam-locator] libraryfolders.vdf не разобран: {message}"
                ));
                return libraries;
            }
        };

        let Some(folders) = get_insensitive(&data, "libraryfolders").and_then(VdfValue::as_object)
        else {
            return libraries;
        };
        for (key, entry) in folders.iter() {
            if key.is_empty() || !key.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            match entry {
                VdfValue::String(root) => self.add_library(&mut libraries, &mut seen, root),
                VdfValue::Object(object) => {
                    if let Some(root) = get_insensitive(object, "path") {
                        self.add_library(&mut libraries, &mut seen, &root.js_string());
                    }
                }
            }
        }
        libraries
    }

    /// `findApp(steam, appId)`.
    pub fn find_app(&self, steam: &SteamInstall, app_id: u32) -> Result<AppInfo, DeliveryError> {
        let libraries = self.steam_libraries(steam);
        let mut corrupted: Option<(String, String)> = None;

        for library in &libraries {
            let manifest_path =
                self.path
                    .join(&[library, "steamapps", &format!("appmanifest_{app_id}.acf")]);
            if !self.fs.exists(&manifest_path) {
                continue;
            }

            let data = match read_to_string(self.fs.as_ref(), &manifest_path)
                .map_err(|error| error.to_string())
                .and_then(|text| vdf::parse(&text).map_err(|error| error.message))
            {
                Ok(data) => data,
                Err(message) => {
                    corrupted = Some((manifest_path, message));
                    continue;
                }
            };

            let Some(app_state) = get_insensitive(&data, "AppState") else {
                corrupted = Some((manifest_path, "нет секции AppState".to_owned()));
                continue;
            };

            let declared = get_insensitive_in(Some(app_state), "appid")
                .map_or(f64::NAN, |value| js::to_number(&value.js_string()));
            if declared != f64::from(app_id) {
                return Err(DeliveryError::new(
                    Code::SteamAppIdMismatch,
                    format!(
                        "appmanifest_{app_id}.acf объявляет AppID {}",
                        js::number::number_to_string(declared)
                    ),
                )
                .with_details(json!({
                    "manifestPath": manifest_path,
                    "expected": app_id,
                    "actual": js_number(declared),
                })));
            }

            let installdir =
                get_insensitive_in(Some(app_state), "installdir").map(VdfValue::js_string);
            let installdir = match installdir {
                Some(dir) if !dir.is_empty() && !is_unsafe_installdir(&dir) => dir,
                other => {
                    return Err(DeliveryError::new(
                        Code::SourceSdkIncomplete,
                        format!(
                            "Недопустимый installdir в appmanifest: {}",
                            other.unwrap_or_else(|| "undefined".to_owned())
                        ),
                    )
                    .with_details(json!({ "manifestPath": manifest_path })));
                }
            };

            let app_path = self
                .path
                .join(&[library, "steamapps", "common", &installdir]);
            if !self.fs.exists(&app_path) {
                return Err(DeliveryError::new(
                    Code::SourceSdkIncomplete,
                    format!("Каталог приложения из appmanifest не найден: {app_path}"),
                )
                .with_details(json!({ "manifestPath": manifest_path, "appPath": app_path })));
            }

            let state_flags = get_insensitive_in(Some(app_state), "StateFlags")
                .map(|value| js::to_number(&value.js_string()))
                .filter(|flags| *flags != 0.0 && !flags.is_nan());
            let name = get_insensitive_in(Some(app_state), "name")
                .map(VdfValue::js_string)
                .filter(|name| !name.is_empty());

            return Ok(AppInfo {
                app_id,
                library: library.clone(),
                manifest_path,
                installdir,
                app_path,
                state_flags,
                name,
            });
        }

        if let Some((manifest_path, message)) = corrupted {
            return Err(DeliveryError::new(
                Code::SourceSdkIncomplete,
                format!("appmanifest повреждён: {message}"),
            )
            .with_details(json!({ "manifestPath": manifest_path })));
        }

        Err(DeliveryError::new(
            Code::SourceSdkNotInstalled,
            format!("Приложение Steam {app_id} не установлено ни в одной библиотеке"),
        )
        .with_details(json!({ "appId": app_id, "libraries": libraries })))
    }

    /// `assertSourceSdkComplete(app)`: есть `hl2.exe` и папки `hl2`, `bin`, `platform`.
    pub fn assert_source_sdk_complete(&self, app: &AppInfo) -> Result<(), DeliveryError> {
        let mut missing = Vec::new();
        for name in SDK_REQUIRED_FILES {
            if !self.fs.exists(&self.path.join(&[&app.app_path, name])) {
                missing.push(name.to_owned());
            }
        }
        for name in SDK_REQUIRED_DIRS {
            let is_dir = self
                .fs
                .stat(&self.path.join(&[&app.app_path, name]))
                .is_ok_and(|stat| stat.is_dir);
            if !is_dir {
                missing.push(format!("{name}{}", self.path.sep()));
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        Err(DeliveryError::new(
            Code::SourceSdkIncomplete,
            format!(
                "Установка Source SDK Base неполная: нет {}",
                missing.join(", ")
            ),
        )
        .with_details(json!({ "appPath": app.app_path, "missing": missing })))
    }

    /// `preflight(appId)`; `None` — Source SDK Base.
    pub fn preflight(&self, app_id: Option<u32>) -> Result<Preflight, DeliveryError> {
        let id = app_id.unwrap_or(SOURCE_SDK_APP_ID);
        let steam = self.locate_steam()?;
        let running = self.assert_steam_running(&steam)?;
        let app = self.find_app(&steam, id)?;
        self.assert_source_sdk_complete(&app)?;
        Ok(Preflight {
            steam,
            running,
            app,
        })
    }

    /// `describeAvailability(appId)`: `{ok: true, code: null, steam, app}` или
    /// `{ok: false, code, message}`.
    pub fn describe_availability(&self, app_id: Option<u32>) -> Value {
        match self.preflight(app_id) {
            Ok(result) => json!({
                "ok": true,
                "code": null,
                "steam": steam_summary(&result.steam, &result.running),
                "app": result.app.to_value(),
            }),
            Err(error) => failure_value(&error),
        }
    }
}

impl SteamLocating for SteamLocator {
    fn locate_steam(&self) -> Result<SteamInstall, DeliveryError> {
        SteamLocator::locate_steam(self)
    }

    /// `describeSteamOnly()`: `{ok: true, code: null, steam, app: null}`.
    fn describe_steam_only(&self) -> Value {
        let result = self
            .locate_steam()
            .and_then(|steam| self.assert_steam_running(&steam).map(|r| (steam, r)));
        match result {
            Ok((steam, running)) => json!({
                "ok": true,
                "code": null,
                "steam": steam_summary(&steam, &running),
                "app": null,
            }),
            Err(error) => failure_value(&error),
        }
    }
}

/// `/[\\/]|\.\./.test(installdir)`: разделитель пути или `..`.
pub(crate) fn is_unsafe_installdir(installdir: &str) -> bool {
    installdir.contains(['\\', '/']) || installdir.contains("..")
}
