//! Доставка и платформа в приложении — то, что `AppManager.js` 0.4.7 делал с `GameDelivery`:
//! фасад доставки ([`DeliveryApi`]) и фасад платформы ([`PlatformApi`]) с портами онлайна
//! (уведомления, события версий, телеметрия, достижения, Discord), Steam для онлайн-профиля,
//! мост присутствия игры (`GamePresenceBridge` → Discord, канал прошлого запуска в
//! `presence-endpoint.json`), перезапуск от администратора (замок экземпляра), Proton на Linux,
//! трей во время игры (порт `TrayWindow` на окне GPUI); `initialize()` — библиотека, фокус окна
//! ([`window_focused`]), игра из аргументов запуска ([`window_ready`]); `dispose()` — в обратном
//! порядке. Каналы окна идут в [`Platform::delivery`] и [`Platform::api`] (`services.rs`).
//! Видимость окна (опрос Steam) — общая сверка `visibility.rs`.

use crate::online::Online;
use crate::visibility::WindowVisibility;
use hlsr_core::instance::PrimaryInstance;
use hlsr_core::paths::{Paths, Resources};
use hlsr_core::store::Store;
use hlsr_delivery::api::{ApiOptions, ApiPorts, DeliveryApi, LibraryStore, ProtonPrefix};
use hlsr_delivery::download_settings::DownloadSettings;
use hlsr_delivery::environment::Environment;
use hlsr_delivery::feature_flag;
use hlsr_delivery::game_actions::LauncherIdentity;
use hlsr_delivery::library_path::default_library_path;
use hlsr_delivery::ports::{
    Dialogs, FileFilter, Logger, OpenDialog, OverlayConfig, Registry, SaveDialog, Shell,
    ShortcutLink, SystemFreeSpace, SystemRegistry, TracingLogger,
};
use hlsr_delivery::providers::BoxFuture;
use hlsr_online::api::OnlineApi;
use hlsr_online::chat_http::ChatHttp;
use hlsr_online::discord_rpc::discord_rich_presence_bridge::GamePresenceLink;
use hlsr_online::online_profile::profile_state::LocalAccount;
use hlsr_online::seventv::SevenTvBridge;
use hlsr_online::twitch_badges::TwitchBadgeBridge;
use hlsr_online::twitch_emotes::TwitchEmoteBridge;
use hlsr_online::youtube::YouTubeChatBridge;
use hlsr_platform::api::system::{SystemFactory, SystemOptions};
use hlsr_platform::api::{ElevationPorts, OnlineHooks, PlatformApi, PlatformOptions, ProtonPorts};
use hlsr_platform::elevation::{InstanceLock, SystemElevator};
use hlsr_platform::game_presence::endpoint_store;
use hlsr_platform::game_presence::{BridgeOptions, GamePresenceBridge};
use hlsr_platform::livesplit_overlay::{LiveSplitOverlay, OverlayOptions};
use hlsr_platform::tray_controller::TrayWindow;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Доставка и платформа запущенного лаунчера.
pub struct Platform {
    delivery: Arc<DeliveryApi>,
    api: Arc<PlatformApi>,
    presence: Option<Arc<GamePresenceBridge>>,
    presence_file: PathBuf,
    env: Environment,
    proton: Option<ProtonPorts>,
    logger: Arc<dyn Logger>,
}

/// Онлайн для платформы: игры — в телеметрию и онлайн-профиль.
struct OnlineLink(Arc<OnlineApi>);

impl OnlineHooks for OnlineLink {
    fn game_starting(&self, game_id: &str, installed: Option<&Value>) {
        self.0.game_starting(game_id, installed);
    }

    fn game_launch_failed(&self, game_id: &str, error: &Value) {
        self.0.game_launch_failed(game_id, error);
    }

    fn game_running(&self, game_id: &str) {
        self.0.game_running(game_id);
    }

    fn game_finished(&self, game_id: &str, last_exit: Option<&Value>, crashed: bool) {
        self.0.game_finished(game_id, last_exit, crashed);
    }

    fn game_stats_from_server(&self, game_id: &str) -> Option<Value> {
        self.0.game_stats_from_server(game_id)
    }
}

/// Мост присутствия игры для Discord (`discordRpc.attachGamePresence(gamePresence)`).
struct PresenceLink(Arc<GamePresenceBridge>);

impl GamePresenceLink for PresenceLink {
    fn set_discord_connected(&self, connected: bool) {
        self.0.set_discord_connected(connected);
    }

    fn diagnostics(&self) -> Value {
        self.0.diagnostics()
    }
}

/// Окно для трея платформы во время игры: команды окну-хосту (`host:hide-for-game`,
/// `host:show-after-game`); значок трея рисует хост.
struct GameTrayWindow;

impl TrayWindow for GameTrayWindow {
    fn available(&self) -> bool {
        crate::host::window_shown()
    }

    fn hide(&self) -> bool {
        crate::host::command("hide-for-game", Value::Null)
    }

    fn show(&self) {
        crate::host::command("show-after-game", Value::Null);
    }

    fn destroy_tray(&self) {
        crate::host::command("destroy-tray", Value::Null);
    }
}

/// Замок единственного экземпляра для перезапуска от администратора.
struct InstanceLink(Arc<Mutex<PrimaryInstance>>);

impl InstanceLock for InstanceLink {
    fn release(&self) -> Result<(), String> {
        self.0.lock().map_err(|error| error.to_string())?.release();
        Ok(())
    }

    fn reacquire(&self) {
        if let Ok(mut instance) = self.0.lock() {
            instance.reacquire();
        }
    }
}

/// `shell` Electron для каналов окна (`open-logs-folder`).
pub fn app_shell() -> Arc<dyn Shell> {
    Arc::new(AppShell)
}

/// Диалоги лаунчера для «Демо» (`demo.rs`).
pub fn app_dialogs() -> Arc<dyn Dialogs> {
    Arc::new(AppDialogs)
}

/// `dialog.showOpenDialog`/`showSaveDialog` 0.4.7 — системные диалоги (rfd) в фоновом потоке:
/// папка библиотеки, Proton, картинки и cfg персонализации, импорт биндов, перенос сборки.
/// Владелец — окно лаунчера ([`crate::host::dialog_owner`]).
struct AppDialogs;

/// Диалог с владельцем, заголовком, фильтрами и начальным местом (`defaultPath`: папка — открыть
/// в ней, путь файла — его папка и имя). Владелец (Windows) делает диалог модальным окну и ставит
/// его на монитор окна.
pub fn file_dialog(
    owner: Option<isize>,
    title: Option<&str>,
    default_path: Option<&Path>,
    filters: &[FileFilter],
) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new();
    #[cfg(windows)]
    if let Some(owner) = owner {
        dialog = dialog.set_parent(&crate::host::Owner(owner));
    }
    #[cfg(not(windows))]
    let _ = owner;
    if let Some(title) = title {
        dialog = dialog.set_title(title);
    }
    for filter in filters {
        dialog = dialog.add_filter(filter.name.as_str(), &filter.extensions);
    }
    if let Some(path) = default_path.filter(|path| !path.as_os_str().is_empty()) {
        if path.is_dir() {
            dialog = dialog.set_directory(path);
        } else {
            if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                dialog = dialog.set_directory(parent);
            }
            if let Some(name) = path.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy());
            }
        }
    }
    dialog
}

impl Dialogs for AppDialogs {
    fn show_open_dialog(&self, options: OpenDialog) -> BoxFuture<'_, Vec<PathBuf>> {
        Box::pin(async move {
            let owner = crate::host::dialog_owner().await;
            tokio::task::spawn_blocking(move || {
                let dialog = file_dialog(
                    owner,
                    options.title.as_deref(),
                    options.default_path.as_deref().map(Path::new),
                    &options.filters,
                )
                .set_can_create_directories(options.properties.contains(&"createDirectory"));
                let folders = options.properties.contains(&"openDirectory");
                let many = options.properties.contains(&"multiSelections");
                match (folders, many) {
                    (true, true) => dialog.pick_folders().unwrap_or_default(),
                    (true, false) => dialog.pick_folder().into_iter().collect(),
                    (false, true) => dialog.pick_files().unwrap_or_default(),
                    (false, false) => dialog.pick_file().into_iter().collect(),
                }
            })
            .await
            .unwrap_or_default()
        })
    }

    fn show_save_dialog(&self, options: SaveDialog) -> BoxFuture<'_, Option<PathBuf>> {
        Box::pin(async move {
            let owner = crate::host::dialog_owner().await;
            tokio::task::spawn_blocking(move || {
                file_dialog(
                    owner,
                    None,
                    options.default_path.as_deref().map(Path::new),
                    &options.filters,
                )
                .save_file()
            })
            .await
            .ok()
            .flatten()
        })
    }
}

/// `shell` Electron: браузер, папка, файл в проводнике, ярлыки `.lnk` игр на рабочем столе
/// (Windows; в Linux ярлык игры — `.desktop`, его пишет `GameActions` без оболочки).
struct AppShell;

impl Shell for AppShell {
    fn open_path<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, String> {
        let path = path.to_path_buf();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || system::open_path(&path))
                .await
                .unwrap_or_else(|error| error.to_string())
        })
    }

    /// Открытие уходит в фоновый поток (`ShellExecuteW` может думать): ответ сразу `Ok`, отказ
    /// оболочки — строка журнала. Каналы платформы, как 0.4.7, на ответ не смотрят.
    fn open_external(&self, url: &str) -> Result<(), String> {
        let url = url.to_owned();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = open_external(&url) {
                tracing::warn!("[shell] ссылка не открыта: {error}");
            }
        });
        Ok(())
    }

    fn show_item_in_folder(&self, path: &Path) {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || system::show_item_in_folder(&path));
    }

    fn write_shortcut_link(&self, path: &Path, link: &ShortcutLink) -> bool {
        system::write_shortcut_link(path, link)
    }

    fn read_shortcut_link(&self, path: &Path) -> Result<ShortcutLink, String> {
        system::read_shortcut_link(path)
    }
}

/// `shell.openExternal(url)` (браузер): `ShellExecuteW("open")` в Windows, `xdg-open` в Linux — для
/// каналов платформы и онлайна (ссылки новостей, профиля). Блокирует: звать в фоновом потоке.
pub fn open_external(url: &str) -> Result<(), String> {
    system::open_external(url)
}

/// Папка «Рабочий стол» (`app.getPath("desktop")`): в Windows — известная папка оболочки
/// (`SHGetKnownFolderPath(FOLDERID_Desktop)`: она бывает перенесена в OneDrive или на другой диск
/// и называется не `Desktop`), иначе `~/Desktop`.
fn desktop_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(dir) = known_desktop() {
        return dir;
    }
    std::env::home_dir().unwrap_or_default().join("Desktop")
}

#[cfg(windows)]
fn known_desktop() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

    let mut raw: *mut u16 = std::ptr::null_mut();
    // SAFETY: `FOLDERID_Desktop` — константа; без токена — текущий пользователь; результат —
    // строка с нулём на конце, выделенная COM, она освобождается ниже ровно один раз.
    let result = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_Desktop,
            KF_FLAG_DEFAULT as u32,
            std::ptr::null_mut(),
            &mut raw,
        )
    };
    let dir = (result >= 0 && !raw.is_null()).then(|| {
        // SAFETY: `raw` указывает на строку UTF-16 с нулём на конце.
        let length = (0..).take_while(|&at| unsafe { *raw.add(at) } != 0).count();
        // SAFETY: `length` единиц UTF-16 прочитаны выше.
        let wide = unsafe { std::slice::from_raw_parts(raw, length) };
        PathBuf::from(std::ffi::OsString::from_wide(wide))
    });
    // SAFETY: память выделена `SHGetKnownFolderPath` (в том числе при ошибке она может быть
    // выделена) и освобождается один раз; `CoTaskMemFree(NULL)` допустим.
    unsafe { CoTaskMemFree(raw.cast()) };
    dir
}

/// Создать доставку и платформу (конструктор `AppManager`) и выставить путь библиотеки (начало
/// `initialize()`, до службы обновления); остальное — [`Platform::initialize`]. Вызывать в среде
/// tokio, в потоке окна, до показа окна.
pub fn start_platform(
    paths: &Paths,
    online: &Online,
    instance: Arc<Mutex<PrimaryInstance>>,
    window_visible: WindowVisibility,
) -> Platform {
    let logger: Arc<dyn Logger> = Arc::new(TracingLogger);
    let online_api = online.api().clone();
    let ports = online_api.ports();
    let data_dir = paths.data_dir.clone();
    let env = Environment::process();
    let registry: Arc<dyn Registry> = Arc::new(SystemRegistry);
    let launcher_version = env!("CARGO_PKG_VERSION").to_owned();

    let download_settings = DownloadSettings::new(data_dir.join("download-settings.json"));
    let rate_limiter = download_settings.rate_limiter().clone();

    // Proton нужен только в Linux: игры и LiveSplit запускаются через umu.
    let proton = cfg!(target_os = "linux").then(|| {
        let manager = hlsr_platform::linux::proton_manager::ProtonManager::new(
            hlsr_platform::linux::proton_manager::ProtonManagerOptions {
                data_dir: data_dir.clone(),
                logger: Some(logger.clone()),
                ..Default::default()
            },
        );
        let runner = hlsr_platform::linux::proton_runner::ProtonRunner::new(
            hlsr_platform::linux::proton_runner::ProtonRunnerOptions {
                manager: Some(Arc::new(manager.clone())),
                data_dir: data_dir.clone(),
                resources_path: Resources::detect().dir().to_path_buf(),
                logger: Some(logger.clone()),
                ..Default::default()
            },
        );
        ProtonPorts { manager, runner }
    });

    let overlay = Arc::new(LiveSplitOverlay::new(OverlayOptions {
        base_dir: Some(data_dir.clone()),
        logger: Some(logger.clone()),
        ..OverlayOptions::default()
    }));
    overlay.remove_legacy_capture();

    let factory = Arc::new(SystemFactory::new(SystemOptions {
        logger: logger.clone(),
        launcher_version: launcher_version.clone(),
        env: env.clone(),
        registry: registry.clone(),
        notifier: Some(ports.notifier.clone()),
        rate_limiter: Some(rate_limiter),
        logs_dir: Some(paths.logs_dir.clone()),
        user_data_path: Some(data_dir.clone()),
        proton: proton.clone(),
    }));
    let mut options = PlatformOptions::new(factory);
    options.logger = logger.clone();
    options.user_data_path = Some(data_dir.clone());
    options.env = env.clone();
    options.notifications = Some(online_api.notifications().clone());
    options.online = Arc::new(OnlineLink(online_api.clone()));
    options.achievements = Some(ports.achievements.clone());
    options.presence = Some(ports.presence.clone());
    options.shell = Arc::new(AppShell);
    options.dialogs = Arc::new(AppDialogs);
    options.tray_window = Some(Arc::new(GameTrayWindow));
    options.window_visible = Arc::new(move || window_visible.visible());
    options.elevation = Some(ElevationPorts {
        elevator: Arc::new(SystemElevator),
        lock: Arc::new(InstanceLink(instance)),
        quit: Arc::new(|| {
            crate::host::quit();
        }),
    });
    options.overlay = Some(overlay.clone());
    options.proton = proton.clone();
    let api = PlatformApi::new(options);

    let proton_prefix: Option<ProtonPrefix> = proton.as_ref().map(|proton| {
        let runner = proton.runner.clone();
        Arc::new(move |game_id: &str| runner.prefix_for(game_id).ok().map(PathBuf::from))
            as ProtonPrefix
    });
    let library: Arc<dyn LibraryStore> = Arc::new(Mutex::new(Store::open(
        paths.file("library.json"),
        hlsr_core::defaults::defaults("library"),
    )));
    let delivery = DeliveryApi::new(ApiOptions {
        library_path: default_library_path().to_string_lossy().into_owned(),
        launcher_version,
        user_data_path: Some(data_dir.clone()),
        env: env.clone(),
        desktop: desktop_dir(),
        launcher: LauncherIdentity {
            exec_path: std::env::current_exe().unwrap_or_default(),
            app_image: std::env::var("APPIMAGE").ok(),
            packaged: !cfg!(debug_assertions),
            app_path: None,
        },
        ports: ApiPorts {
            logger: logger.clone(),
            notifier: ports.notifier.clone(),
            telemetry: Some(ports.telemetry.clone()),
            achievements: Some(ports.achievements.clone()),
            presence: Some(ports.presence.clone()),
            overlay: Some(overlay as Arc<dyn OverlayConfig>),
            dialogs: Arc::new(AppDialogs),
            shell: Arc::new(AppShell),
            registry,
            free_space: Arc::new(SystemFreeSpace),
            proton_prefix,
        },
        clock: None,
        factory: Some(api.service_factory()),
        download_settings: Some(Arc::new(Mutex::new(download_settings))),
        library_store: Some(library),
        schedule_updates: true,
    });
    api.bind_delivery(&delivery);

    // Steam для онлайн-профиля (`accountProvider`/`friendProvider`).
    let account = {
        let api = Arc::downgrade(&api);
        Arc::new(move || {
            let state = api
                .upgrade()
                .map_or(Value::Null, |api| api.steam_account_state());
            let text = |key: &str| state[key].as_str().map(str::to_owned);
            LocalAccount {
                steam_id64: text("steamId64"),
                persona_name: text("personaName"),
                avatar_data_uri: text("avatarDataUri"),
            }
        })
    };
    let friends = {
        let api = Arc::downgrade(&api);
        Arc::new(move || {
            api.upgrade()
                .map_or_else(Vec::new, |api| api.steam_friends())
        })
    };
    // В QA профиль берёт аккаунт и друзей из файлов подмены (`online.rs`).
    if !online.profile_qa() {
        online_api.set_steam_providers(Some(account), Some(friends));
    }

    // Мост присутствия игры: канал прошлого запуска, если игра пережила перезапуск лаунчера.
    let presence_file = data_dir.join("presence-endpoint.json");
    let presence = start_presence(&presence_file, &online_api, &api, &logger);

    // Начало `initialize()` (`AppManager.js:568-569`): путь библиотеки.
    delivery.ensure_default_install_path();
    delivery.set_library_path(&delivery.current_library_path());

    Platform {
        delivery,
        api,
        presence,
        presence_file,
        env,
        proton,
        logger,
    }
}

/// `new GamePresenceBridge` + `start()`, окружение игры (канал сохраняется в
/// `presence-endpoint.json`), связь с Discord и потребители чата в игре (`AppManager.js:405-425`:
/// 7TV, YouTube, значки и эмоуты Twitch).
fn start_presence(
    file: &Path,
    online: &Arc<OnlineApi>,
    api: &Arc<PlatformApi>,
    logger: &Arc<dyn Logger>,
) -> Option<Arc<GamePresenceBridge>> {
    let saved = endpoint_store::load(file);
    let discord = online.clone();
    let bridge = GamePresenceBridge::new(BridgeOptions {
        log: Some(logger.clone()),
        token: saved.as_ref().map(|endpoint| endpoint.token.clone()),
        pipe_name: saved.as_ref().map(|endpoint| endpoint.pipe_name.clone()),
        port: saved.as_ref().and_then(|endpoint| endpoint.port),
        on_presence: Some(Arc::new(move |state: &Value| {
            discord.discord().set_game_state(state)
        })),
        ..BridgeOptions::default()
    });
    // До `start()`: в 0.4.9 потребители подключаются после него, но в том же такте Node; здесь
    // канал принимает игру сразу, и её `hello` прошёл бы мимо потребителей.
    let handle = tokio::runtime::Handle::current();
    bridge.attach_chat_consumer(Arc::new(SevenTvBridge::new(
        ChatHttp::new(),
        logger.clone(),
        handle.clone(),
    )));
    bridge.attach_chat_consumer(Arc::new(YouTubeChatBridge::new(
        ChatHttp::new(),
        logger.clone(),
        handle.clone(),
        1,
    )));
    bridge.attach_chat_consumer(Arc::new(TwitchBadgeBridge::new(logger.clone())));
    bridge.attach_chat_consumer(Arc::new(TwitchEmoteBridge::new(
        ChatHttp::new(),
        logger.clone(),
        handle,
    )));
    bridge.start();
    let environment = {
        let bridge = Arc::downgrade(&bridge);
        let file = file.to_path_buf();
        let logger = logger.clone();
        Arc::new(move || {
            let bridge = bridge.upgrade()?;
            let environment = bridge.environment();
            if let Err(error) = endpoint_store::save_endpoint(&file, &bridge.endpoint()) {
                logger.warn(&format!(
                    "[game-presence] канал не сохранён: {}",
                    error.message
                ));
            }
            Some(environment.into_iter().collect())
        })
    };
    api.set_game_environment_provider(Some(environment));
    online
        .discord()
        .attach_game_presence(Arc::new(PresenceLink(bridge.clone())));
    Some(bridge)
}

/// `ready` окна (`AppManager.js:1018-1021`): игра из аргументов запуска.
pub fn window_ready(delivery: &DeliveryApi) {
    // Как `LaunchArgs.forwarded`: аргумент не в Unicode не роняет окно.
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    delivery.select_game_from_args(&argv);
}

/// `gameDelivery.checkOnFocus()` на `focus` окна (`AppManager.js:575-579`, после
/// `updateManager.checkOnFocus()`): проверки обновлений игр и частый опрос Steam.
pub fn window_focused(delivery: &DeliveryApi, api: &PlatformApi) {
    delivery.check_on_focus();
    api.check_on_focus();
}

impl Platform {
    /// `gameDelivery.initialize()` и Proton (`AppManager.js:1048-1052`), после
    /// `updateManager.initialize()`: флаг доставки в журнал, первая загрузка Proton (Linux).
    pub fn initialize(&self) {
        let flag = feature_flag::describe(&self.env);
        let providers = flag.allowed_provider_types.join(", ");
        self.logger.info(&format!(
            "[game-delivery] feature flag {}={}; разрешённые провайдеры: {}",
            feature_flag::FLAG,
            flag.remote_enabled,
            if providers.is_empty() {
                "нет"
            } else {
                &providers
            }
        ));
        if let Some(proton) = &self.proton {
            let manager = proton.manager.clone();
            let logger = self.logger.clone();
            tokio::spawn(async move {
                if let Err(error) = manager.ensure_reference().await {
                    logger.warn(&format!(
                        "[proton] первая загрузка отложена: {}",
                        error.message
                    ));
                }
                manager.check_for_update().await;
            });
        }
    }

    /// Фасад доставки (каналы окна подпроекта 2).
    pub fn delivery(&self) -> &Arc<DeliveryApi> {
        &self.delivery
    }

    /// Фасад платформы (каналы окна подпроекта 3).
    pub fn api(&self) -> &Arc<PlatformApi> {
        &self.api
    }

    /// Часть `dispose()` 0.4.7: канал присутствия (если игр нет — файл канала больше не нужен,
    /// следующий запуск сменит токен), мост присутствия, доставка и платформа.
    pub fn stop(self) {
        if !self.api.is_any_game_running() {
            endpoint_store::clear(&self.presence_file);
        }
        if let Some(presence) = &self.presence {
            presence.shutdown();
        }
        self.api.dispose();
        self.delivery.dispose();
    }
}

/// Перед всем остальным в выпуске на Windows (`src/main/index.js:213-240`): рабочий каталог —
/// каталог exe, `steam_appid.txt` SteamVR рядом с exe удаляется.
pub fn prepare_exe_dir() {
    if !cfg!(windows) || !hlsr_delivery::channel_config::is_production_build() {
        return;
    }
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return;
    };
    if let Err(error) = std::env::set_current_dir(&dir) {
        tracing::warn!("рабочий каталог не сменён: {error}");
    }
    hlsr_platform::legacy_app_id::remove_legacy_steam_app_id(&dir, &TracingLogger);
}

#[cfg(windows)]
mod system {
    use hlsr_core::win32::wide;
    use hlsr_delivery::ports::ShortcutLink;
    use hlsr_update::windows::shortcuts::{ShellLinks as _, ShortcutSpec, SystemShellLinks};
    use std::ffi::OsStr;
    use std::path::Path;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    /// Текст исключения `shell.readShortcutLink` Electron.
    pub const READ_SHORTCUT_FAILED: &str = "Failed to read shortcut link";

    /// `ShellExecuteW(NULL, "open", file)`; не открыто — код оболочки (не больше 32).
    fn shell_open<S: AsRef<OsStr> + ?Sized>(file: &S) -> Result<(), usize> {
        let (verb, file) = (wide("open"), wide(file));
        // SAFETY: строки с нулём в конце живут до конца вызова; окна-владельца нет.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if result as usize > 32 {
            Ok(())
        } else {
            Err(result as usize)
        }
    }

    /// `shell.openExternal(url)`.
    pub fn open_external(url: &str) -> Result<(), String> {
        shell_open(url).map_err(|code| format!("ShellExecuteW: {code}"))
    }

    /// `shell.openPath(path)`: пустая строка — открыто, иначе — текст ошибки.
    pub fn open_path(path: &Path) -> String {
        match shell_open(path) {
            Ok(()) => String::new(),
            Err(code) => format!("Failed to open path (ShellExecuteW: {code})"),
        }
    }

    /// `shell.showItemInFolder(path)`: проводник с выделенным файлом.
    pub fn show_item_in_folder(path: &Path) {
        let mut argument = std::ffi::OsString::from("/select,");
        argument.push(path.as_os_str());
        let spawned = std::process::Command::new("explorer.exe")
            .arg(argument)
            .env_remove("SLINT_SCALE_FACTOR")
            .spawn();
        if let Err(error) = spawned {
            tracing::warn!("[shell] папка не открыта: {error}");
        }
    }

    /// `shell.writeShortcutLink(path, "create", link)`: каталог ярлыка создаётся, файл создаётся
    /// или заменяется, AUMID не задаётся (его нет в `options` `GameActions.js`). `false` — не
    /// создан (Electron причину не отдаёт; здесь она — строка журнала).
    pub fn write_shortcut_link(path: &Path, link: &ShortcutLink) -> bool {
        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            tracing::warn!("[shell] ярлык не создан: {}: {error}", parent.display());
            return false;
        }
        let spec = ShortcutSpec {
            target: link.target.clone(),
            args: link.args.clone(),
            cwd: link.cwd.clone(),
            icon: link.icon.clone(),
            icon_index: link.icon_index,
            app_user_model_id: String::new(),
            description: link.description.clone(),
        };
        match SystemShellLinks.write(path, &spec) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!("[shell] ярлык не создан: {error}");
                false
            }
        }
    }

    /// `shell.readShortcutLink(path)`: поля ярлыка; не прочитан — исключение Electron.
    pub fn read_shortcut_link(path: &Path) -> Result<ShortcutLink, String> {
        let info = SystemShellLinks
            .read(path)
            .map_err(|_| READ_SHORTCUT_FAILED.to_owned())?;
        Ok(ShortcutLink {
            target: info.target,
            args: info.args,
            description: info.description,
            cwd: info.cwd,
            icon: info.icon,
            icon_index: info.icon_index,
        })
    }
}

#[cfg(not(windows))]
mod system {
    use hlsr_delivery::ports::{NoShell, Shell as _, ShortcutLink};
    use std::ffi::OsStr;
    use std::path::Path;
    use std::process::{Command, Stdio};

    /// `xdg-open`, без масштаба окна лаунчера.
    fn xdg_open<S: AsRef<OsStr> + ?Sized>(target: &S) -> Result<(), String> {
        let status = Command::new("xdg-open")
            .arg(target.as_ref())
            .env_remove("SLINT_SCALE_FACTOR")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("xdg-open: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("xdg-open: {status}"))
        }
    }

    /// `shell.openExternal(url)`.
    pub fn open_external(url: &str) -> Result<(), String> {
        xdg_open(url)
    }

    /// `shell.openPath(path)`.
    pub fn open_path(path: &Path) -> String {
        xdg_open(path).err().unwrap_or_default()
    }

    /// `shell.showItemInFolder(path)`: папка файла (без выделения).
    pub fn show_item_in_folder(path: &Path) {
        if let Some(parent) = path.parent()
            && let Err(error) = xdg_open(parent)
        {
            tracing::warn!("[shell] папка не открыта: {error}");
        }
    }

    /// `shell.writeShortcutLink` есть только в Windows.
    pub fn write_shortcut_link(path: &Path, link: &ShortcutLink) -> bool {
        NoShell.write_shortcut_link(path, link)
    }

    /// `shell.readShortcutLink` есть только в Windows.
    pub fn read_shortcut_link(path: &Path) -> Result<ShortcutLink, String> {
        NoShell.read_shortcut_link(path)
    }
}

