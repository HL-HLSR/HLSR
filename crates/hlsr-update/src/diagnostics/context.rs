//! Контекст диагностики: всё, что `DiagnosticsRunner` берёт у других подпроектов, за одним
//! трейтом (`ctx` в `AppManager.js:848-875`). Реализации — в приложении (задача 9 и ПП1–ПП4); в
//! тестах — подделка. Необязательное в 0.4.7 (поле `ctx` без функции) здесь — метод с
//! умолчанием «нет данных»: строка проверки получается та же.

use crate::ports::BoxFuture;
use hlsr_core::js::Platform;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// Значения, которым не нужно ожидание (поля `ctx`, не функции). Контекст отдаёт их заново
/// на каждый прогон.
#[derive(Debug, Clone)]
pub struct ContextData {
    /// `ctx.appVersion`; пусто — `?`.
    pub app_version: String,
    /// `ctx.packaged`: установленная сборка (`packaged`) или запуск из каталога сборки (`dev`).
    pub packaged: bool,
    /// ОС лаунчера (`ctx.platform || process.platform`): Linux-ветка проверок или Windows-ветка.
    pub platform: Platform,
    /// `process.arch` и `os.arch()`: `x64`, `arm64`… (не `x64` — предупреждение).
    pub arch: String,
    /// `os.release()`: версия ядра Linux или Windows.
    pub os_release: String,
    /// Windows: интеграция с ОС (`ctx.nativeAvailable`) загружена. В Rust нет нативного модуля
    /// Electron — значение отдаёт приложение (например, готовность `hlsr-platform`).
    pub native_available: bool,
    /// Почему не загружена (`ctx.nativeLoadError`), до 500 знаков.
    pub native_load_error: String,
    /// Linux: `umu-run` найден (`ctx.umuAvailable`).
    pub umu_available: bool,
    /// Linux: выбранный Proton (`ctx.protonSelected`).
    pub proton_selected: Option<String>,
    /// Каталог данных (`ctx.userDataPath`); нет — «папка настроек недоступна».
    pub user_data_path: Option<PathBuf>,
    /// `ctx.updateState`; пусто — `idle`.
    pub update_state: String,
    /// `ctx.updateEnabled`; `None` — не задано (считается включённым).
    pub update_enabled: Option<bool>,
    /// Библиотека игр (`ctx.libraryPath`).
    pub library_path: Option<String>,
    /// Свободное место в МБ (`ctx.freeSpaceMb`); `None` — не определено.
    pub free_space_mb: Option<f64>,
    /// `ctx.steamHealth`; `None` — `{status: "unknown", code: "STEAM_CHECK_FAILED"}`.
    pub steam_health: Option<SteamHealth>,
}

impl Default for ContextData {
    fn default() -> ContextData {
        ContextData {
            app_version: String::new(),
            packaged: false,
            platform: Platform::CURRENT,
            arch: node_arch(std::env::consts::ARCH).to_owned(),
            os_release: String::new(),
            native_available: true,
            native_load_error: String::new(),
            umu_available: false,
            proton_selected: None,
            user_data_path: None,
            update_state: String::new(),
            update_enabled: None,
            library_path: None,
            free_space_mb: None,
            steam_health: None,
        }
    }
}

/// Архитектура Rust → имя `process.arch` Node (`x86_64` → `x64`).
pub fn node_arch(rust_arch: &str) -> &str {
    match rust_arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    }
}

/// `ctx.steamHealth`: Steam запущен и доступен для проверки владения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamHealth {
    /// `available`, `unknown` или другое (проблема).
    pub status: String,
    pub code: Option<String>,
    pub detail: Option<String>,
}

/// Результат `ctx.launchAvailability()` (`describeAvailability`): среда запуска игры.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchAvailability {
    pub ok: bool,
    pub code: Option<String>,
}

/// Состояние рендерера для строк «Отрисовка интерфейса» (Р10 спецификации: данные ПП1 вместо
/// сведений Chromium `gpuPreference.snapshot`). Смысл полей — как у 0.4.7: «включено» — режим с
/// видеокартой (OpenGL, Vulkan, DX12), «выключено» — программный.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuStatus {
    /// Сохранённая настройка просит видеокарту (`configuredEnabled`).
    pub configured_enabled: bool,
    /// С чем запущен этот процесс (`appliedEnabled`); `None` — неизвестно.
    pub applied_enabled: Option<bool>,
    /// Сведения об отрисовке получены (`available`).
    pub available: bool,
    /// Видеокарта действительно работает (`hardwareEnabled`).
    pub hardware_enabled: bool,
    /// Идёт программная отрисовка (`softwareRenderer`).
    pub software_renderer: bool,
    /// Название видеокарты (`device`); пусто — строки нет.
    pub device: String,
    /// Версия драйвера (`driver`); пусто — строки нет.
    pub driver: String,
}

impl GpuStatus {
    /// Состояние из рендерера ПП1: `configured` — режим в `settings.json` сейчас (`auto`,
    /// `opengl`, `vulkan`, `dx12`, `software`), `started_with` — он же при запуске процесса,
    /// `active` — рендерер, который работает (после отказа GPU это `software`, хотя просили
    /// видеокарту: «программное запасное»). Неизвестный `active` — `None`.
    pub fn from_renderer(
        configured: &str,
        started_with: &str,
        active: Option<&str>,
        device: &str,
        driver: &str,
    ) -> GpuStatus {
        GpuStatus {
            configured_enabled: configured != "software",
            applied_enabled: Some(started_with != "software"),
            available: active.is_some(),
            hardware_enabled: active.is_some_and(|name| name != "software"),
            software_renderer: active == Some("software"),
            device: device.to_owned(),
            driver: driver.to_owned(),
        }
    }
}

/// Игра Steam, владение которой проверяется (`ctx.requiredSteamApps()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamApp {
    pub app_id: u64,
    pub title: String,
}

/// Снимок профиля (`ctx.profile()`, `gameDelivery.diagnosticsProfile`): числа те же, что в окне
/// профиля.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileSnapshot {
    pub signed_in: bool,
    pub play_seconds: f64,
    /// `None` — счётчика нет (в строке будет `null`).
    pub launch_count: Option<f64>,
    pub games_played: Option<f64>,
    pub top_game_id: Option<String>,
    /// (получено, всего).
    pub achievements: Option<(f64, f64)>,
    pub installed_at: Option<String>,
}

/// `ctx.telemetryStatus()` / `telemetryEnabled()` — какая из двух функций есть у контекста.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryInfo {
    /// Ни одной: строки нет.
    Unavailable,
    /// `telemetryStatus()`; `None` — снимок собрать не удалось.
    Status(Option<TelemetryStatus>),
    /// Старый вид: только «включена».
    Enabled(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryStatus {
    pub backend_configured: bool,
    pub enabled: bool,
}

/// Итог `buildMetadataTrust.verifyAgainstSnapshot` (ПП2): `state` — `trusted`,
/// `launcher-update-required`, `repair-required`…; `reason` — код причины.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRuntime {
    pub state: String,
    pub reason: Option<String>,
}

/// Ошибка проверки канала игры (`ChannelService.validate`): код, который видит раннер.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameChannelError {
    pub code: Option<String>,
}

/// Всё, что диагностика берёт у остальных подпроектов.
pub trait DiagnosticsContext: Send + Sync {
    /// Значения без ожидания; зовётся один раз на прогон.
    fn data(&self) -> ContextData;

    /// `ctx.gpuStatus()`: `None` — функции нет, она бросила или вернула пустое.
    fn gpu_status(&self) -> BoxFuture<'_, Option<GpuStatus>> {
        Box::pin(async { None })
    }

    /// `ctx.downloadLimitBps` (число или функция): действующий предел скорости, байт/с.
    fn download_limit_bps(&self) -> Option<f64> {
        None
    }

    /// `ctx.launchAvailability()`: `None` — функции нет, она бросила или вернула пустое.
    fn launch_availability(&self) -> BoxFuture<'_, Option<LaunchAvailability>> {
        Box::pin(async { None })
    }

    /// `ctx.requiredSteamApps()`; пусто, если нет и проверки владения (`ctx.steamOwnership`).
    fn required_steam_apps(&self) -> Vec<SteamApp> {
        Vec::new()
    }

    /// `ctx.steamOwnership(appId, { timeoutMs })`: `Some(true/false)` — куплена или нет; `None` —
    /// не удалось проверить (в 0.4.7 — `owned: null`, исключение тоже).
    fn steam_ownership(&self, _app_id: u64, _timeout: Duration) -> BoxFuture<'_, Option<bool>> {
        Box::pin(async { None })
    }

    /// `ctx.profile()`; `None` — функции нет или снимок не собрался.
    fn profile(&self) -> Option<ProfileSnapshot> {
        None
    }

    /// Телеметрия: функция состояния, старая функция «включена» или ничего.
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::Unavailable
    }

    /// `buildMetadataTrust.verifyAgainstSnapshot({ installRoot, snapshot, launcherVersion })` (ПП2).
    /// `installed` — разобранный `installed.json`.
    fn build_runtime(&self, _installed: &Value, _launcher_version: &str) -> BuildRuntime {
        BuildRuntime {
            state: "repair-required".to_owned(),
            reason: Some("BUILD_METADATA_INVALID".to_owned()),
        }
    }

    /// `buildMetadataReader.read(installPath).sha256` (ПП2): хеш файла сведений о сборке;
    /// `Ok(None)` — файла нет; `Err` — не прочитался.
    fn build_metadata_sha256(&self, _install_path: &str) -> Result<Option<String>, String> {
        Err("не поддерживается".to_owned())
    }

    /// `ChannelService.validate(data, "ghosting-renewed", config)` (ПП2): подпись и форма
    /// канала игры.
    fn validate_game_channel(&self, _data: &Value) -> Result<(), GameChannelError> {
        Err(GameChannelError { code: None })
    }

    /// `ProtonManager.updateCandidate(release) || release.tag_name === RELEASE_TAG` (ПП3): релиз
    /// из списка GitHub подходит лаунчеру.
    fn proton_release_ok(&self, _release: &Value) -> bool {
        false
    }
}
