//! «Демо» в приложении — то, что `AppManager.js` 0.4.9 делал с `DemoTool`: фасад [`DemoApi`]
//! (Store `demo-tool.json`, демо установленных игр, FFmpeg и bxt-rs из `ToolService` библиотеки,
//! «Монтажёр» профиля, диалоги, Корзина, папка «Видео», корень заданий в данных пользователя),
//! адаптеры рендера HL1 и Ghosting на Win32 (реестр, процессы, Steam, монитор), хуки запуска
//! игры (`RENDER_ACTIVE`, снять mute перед «Играть»), полный экран плеера (общий с кнопкой шапки,
//! `window_ops`), Discord «Рендерит демку», через 15 с — уборка и фоновый сброс mute, при выходе —
//! `dispose()`.

use crate::online::Online;
use crate::platform::{self, Platform};
use hlsr_core::logger::{Logger, TracingLogger};
use hlsr_core::paths::Paths;
use hlsr_core::store::Store;
use hlsr_delivery::api::DeliveryApi;
use hlsr_delivery::demo_archive::demo_sources;
use hlsr_delivery::manifests::{game_ids, local_config_of};
use hlsr_delivery::ports::{FreeSpace, SystemFreeSpace};
use hlsr_delivery::providers::BoxFuture;
use hlsr_demo::api::{self, DemoApi, DemoOptions, RenderGame};
use hlsr_demo::game_render::{Inspector, Ports, SteamStatus, WinRegistry};
use hlsr_demo::ghosting::GhostingAdapter;
use hlsr_demo::goldsrc::Hl1Adapter;
use hlsr_demo::library::GameContext;
use hlsr_demo::render_host::RenderHost;
use hlsr_platform::api::PlatformApi;
use hlsr_platform::steam_locator::{LocatorOptions, SteamLocating, SteamLocator};
use hlsr_ui::services::{ChannelService, Reply};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use tokio::runtime::Handle;
use tokio::sync::broadcast;

/// Каналы «Демо» для окна (`ChannelRouter`).
pub struct DemoChannels(pub Arc<DemoApi>);

impl ChannelService for DemoChannels {
    fn call<'a>(&'a self, channel: &'a str, args: &'a [Value]) -> BoxFuture<'a, Option<Reply>> {
        Box::pin(self.0.call(channel, args))
    }
}

type Games = Arc<dyn Fn() -> Vec<GameContext<'static>> + Send + Sync>;

/// Фасад «Демо» запущенного лаунчера. Вызывать в среде tokio, до открытия окна.
pub fn start(
    runtime: &Handle,
    paths: &Paths,
    platform: &Platform,
    online: &Online,
) -> Arc<DemoApi> {
    let logger: Arc<dyn Logger> = Arc::new(TracingLogger);
    let store = Arc::new(Mutex::new(Store::open(
        paths.file(api::STORE_FILE),
        api::store_defaults(),
    )));
    let jobs_root = jobs_root(paths);
    let delivery = Arc::downgrade(platform.delivery());
    let games: Games = {
        let delivery = delivery.clone();
        Arc::new(move || {
            delivery
                .upgrade()
                .map_or_else(Vec::new, |d| demo_contexts(&d))
        })
    };
    let tools = {
        let delivery = delivery.clone();
        move || {
            let delivery = delivery.upgrade()?;
            delivery.ensure_service()?;
            delivery.tools()
        }
    };
    let free_space: Arc<dyn FreeSpace> = Arc::new(SystemFreeSpace);
    let base = RenderBase {
        store: store.clone(),
        jobs_root: jobs_root.clone(),
        logger: logger.clone(),
        games: games.clone(),
        delivery: delivery.clone(),
        platform: Arc::downgrade(platform.api()),
        steam: Arc::new(SteamLocator::new(LocatorOptions::default())),
    };
    let bxtrs_override = dev_path("HLSR_DEMO_BXTRS");
    let hl1 = {
        let tools = tools.clone();
        let given = bxtrs_override.clone();
        Hl1Adapter::new(
            base.ports(hlsr_demo::goldsrc::GAME_ID),
            free_space.clone(),
            Box::new(move || {
                let tools = tools();
                api::bxtrs_located(
                    hlsr_core::js::Platform::CURRENT,
                    given.as_deref(),
                    tools.as_deref(),
                )
                .map(|(found, _)| found)
            }),
        )
    };
    let ghosting = GhostingAdapter::new(base.ports(hlsr_demo::ghosting::GAME_ID), free_space);
    let renderers: Vec<Arc<dyn RenderGame>> = vec![Arc::new(hl1), Arc::new(ghosting)];
    let achievements = online.api().ports().achievements.clone();
    let demo = DemoApi::new(DemoOptions {
        store,
        games: Box::new(move || games()),
        tools: Box::new(tools),
        achieve: Box::new(move |id: &str| achievements.achieve(id)),
        dialogs: platform::app_dialogs(),
        shell: platform::app_shell(),
        trash: Box::new(trash),
        fullscreen: Box::new(|on| {
            Box::pin(async move {
                crate::host::command("player-fullscreen", serde_json::json!(on))
            })
        }),
        videos_dir: videos_dir(),
        jobs_root: Some(jobs_root),
        platform: hlsr_core::js::Platform::CURRENT,
        ffmpeg_override: hlsr_demo::ffmpeg::env_override(),
        bxtrs_override,
        fake_render: dev_path(api::FAKE_FLAG).is_some_and(|flag| flag == Path::new("1")),
        renderers,
        logger,
        limit_bytes: None,
    });
    hook_launch(platform.api(), &demo);
    hook_discord(runtime, &demo, online);
    // Размер монитора — в кэш, как только пойдёт цикл окна: первое окно экспорта не видит 720p.
    screen_size();
    demo.start_background(runtime);
    demo
}

/// Пока идёт рендер игры, её запуск из лаунчера отказывает (`RENDER_ACTIVE`, и у доставки);
/// mute после рендера снимается перед её запуском.
fn hook_launch(platform: &Arc<PlatformApi>, demo: &Arc<DemoApi>) {
    let weak = Arc::downgrade(demo);
    platform.set_render_active(Some(Arc::new(move |game_id: &str| {
        weak.upgrade()
            .is_some_and(|demo| demo.render_active(game_id))
    })));
    let weak = Arc::downgrade(demo);
    platform.set_before_launch(Some(Arc::new(move |game_id: &str| {
        let (weak, game_id) = (weak.clone(), game_id.to_owned());
        Box::pin(async move {
            let Some(demo) = weak.upgrade() else {
                return Ok(());
            };
            // Mute не снят — игра всё равно стартует, как в 0.4.9; причину пишет reset_mute.
            tokio::task::spawn_blocking(move || demo.reset_mute_before_launch(&game_id))
                .await
                .map(drop)
                .map_err(|error| format!("сброс mute перед запуском: {error}"))
        })
    })));
}

/// Discord: «Рендерит демку», пока идёт предпросмотр или экспорт (`demo-job-progress`), до
/// `demo-job-done` или `demo-job-error` (`AppManager.js` 0.4.9).
fn hook_discord(runtime: &Handle, demo: &DemoApi, online: &Online) {
    let mut events = demo.subscribe();
    let online = Arc::downgrade(online.api());
    runtime.spawn(async move {
        loop {
            let rendering = match events.recv().await {
                Ok(event) => match event.channel {
                    "demo-job-progress" => true,
                    "demo-job-done" | "demo-job-error" => false,
                    _ => continue,
                },
                // Потерянные события могли унести конец задания; идущее вернёт «рендерит»
                // следующим `demo-job-progress`.
                Err(broadcast::error::RecvError::Lagged(_)) => false,
                Err(broadcast::error::RecvError::Closed) => break,
            };
            let Some(online) = online.upgrade() else {
                break;
            };
            online.discord().set_demo_rendering(rendering);
        }
    });
}

/// `gameDelivery.demoContexts()`: установленные игры с папками демо.
fn demo_contexts(delivery: &DeliveryApi) -> Vec<GameContext<'static>> {
    let Some(service) = delivery.ensure_service() else {
        return Vec::new();
    };
    game_ids()
        .into_iter()
        .filter_map(|game_id| {
            let config = local_config_of(game_id)?;
            let installed = service.install_service().read_installed(game_id).ok()??;
            let install_path = installed
                .get("installPath")
                .filter(|path| hlsr_core::js::truthy(path))
                .map(|path| hlsr_core::js::to_template_string(Some(path)))?;
            let has_demos = !demo_sources(config, Path::new(&install_path)).0.is_empty();
            has_demos.then(|| GameContext {
                game_id: game_id.to_owned(),
                install_path,
                config,
            })
        })
        .collect()
}

/// Корень заданий: Windows — `%LOCALAPPDATA%\<имя>\demo-jobs` (не перемещаемый профиль), Linux —
/// данные лаунчера (0700). Временные данные отладки (`HLSR_DATA_DIR`) — туда же, не в настоящий
/// `%LOCALAPPDATA%`.
fn jobs_root(paths: &Paths) -> PathBuf {
    let local = std::env::var_os("LOCALAPPDATA")
        .filter(|dir| cfg!(windows) && !dir.is_empty() && dev_path("HLSR_DATA_DIR").is_none());
    local
        .map_or_else(
            || paths.data_dir.clone(),
            |dir| PathBuf::from(dir).join(paths.app_name),
        )
        .join(hlsr_demo::jobs::JOBS_DIR)
}

/// Подмена из окружения — только отладка и `dev-hooks`, как `HLSR_DEMO_FFMPEG`.
fn dev_path(name: &str) -> Option<PathBuf> {
    if !cfg!(any(debug_assertions, feature = "dev-hooks")) {
        return None;
    }
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Связи рендера игр с лаунчером: Store, корень заданий, игры, Steam, монитор.
struct RenderBase {
    store: Arc<Mutex<Store>>,
    jobs_root: PathBuf,
    logger: Arc<dyn Logger>,
    games: Games,
    delivery: Weak<DeliveryApi>,
    platform: Weak<PlatformApi>,
    steam: Arc<SteamLocator>,
}

impl RenderBase {
    fn ports(&self, game_id: &'static str) -> Ports {
        let games = self.games.clone();
        let (delivery, platform) = (self.delivery.clone(), self.platform.clone());
        let busy = self.delivery.clone();
        let steam = self.steam.clone();
        let logger = self.logger.clone();
        Ports {
            windows: cfg!(windows),
            jobs_root: Some(self.jobs_root.clone()),
            store: Some(self.store.clone()),
            logger: self.logger.clone(),
            registry: Box::new(WinRegistry::current_user()),
            inspector: Box::new(SystemProcesses),
            install_path: Box::new(move || {
                games()
                    .into_iter()
                    .find(|context| context.game_id == game_id)
                    .map(|context| PathBuf::from(context.install_path))
            }),
            // Игра запущена или как раз запускается из лаунчера.
            game_running: Box::new(move |id: &str| {
                let running = delivery
                    .upgrade()
                    .and_then(|delivery| delivery.service())
                    .is_some_and(|service| service.is_game_running(id));
                running
                    || platform
                        .upgrade()
                        .and_then(|platform| platform.processes())
                        .is_some_and(|processes| processes.is_running(id))
            }),
            // Идёт установка, обновление или ремонт: файлы игры меняются.
            game_busy: Box::new(move || {
                busy.upgrade()
                    .and_then(|delivery| delivery.service())
                    .is_some_and(|service| service.is_busy())
            }),
            steam_status: Box::new(move || {
                let status = steam.describe_steam_only();
                SteamStatus {
                    ok: status["ok"] == true,
                    code: status["code"].as_str().map(str::to_owned),
                }
            }),
            screen_size: Box::new(screen_size),
            host_factory: Box::new(move || Box::new(RenderHost::new(logger.clone()))),
        }
    }
}

/// Процессы системы для рендера: сбой списка — ошибка (рендер тогда не начинается).
struct SystemProcesses;

impl Inspector for SystemProcesses {
    fn find_by_image_name(&self, image: &str) -> Result<Vec<Option<String>>, String> {
        #[cfg(windows)]
        {
            use hlsr_platform::process_inspect::SystemProcesses as _;
            hlsr_platform::process_inspect::winapi::WinApiProcesses
                .find_by_image_name(image)
                .map(|list| {
                    list.into_iter()
                        .filter(|process| process.pid > 0)
                        .map(|process| process.executable_path)
                        .collect()
                })
        }
        #[cfg(not(windows))]
        {
            Ok(hlsr_platform::process_inspect::system_inspector()
                .find_by_image_name(image)
                .into_iter()
                .map(|process| process.executable_path)
                .collect())
        }
    }

    fn same_path(&self, left: &str, right: &str) -> bool {
        hlsr_platform::process_inspect::same_path(Some(left), Some(right))
    }
}

/// Основной монитор в DIP (игры не DPI-aware): размер из GPUI, обновляется в потоке окна при
/// каждом запросе — рендер его не ждёт.
fn screen_size() -> Option<(u32, u32)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    // SAFETY: без аргументов-указателей; процесс DPI-aware (манифест окна), значения в пикселях.
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    (w > 0 && h > 0).then_some((w as u32, h as u32))
}

/// Папка «Видео» (`app.getPath("videos")`); не узнать — `~/Videos`.
fn videos_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(dir) = known_videos() {
        return dir;
    }
    // shortcut: Linux — без XDG_VIDEOS_DIR из user-dirs.dirs; читать его, если попросят.
    std::env::home_dir().unwrap_or_default().join("Videos")
}

#[cfg(windows)]
fn known_videos() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Videos, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

    let mut raw: *mut u16 = std::ptr::null_mut();
    // SAFETY: `FOLDERID_Videos` — константа; без токена — текущий пользователь; результат —
    // строка с нулём на конце, выделенная COM, она освобождается ниже ровно один раз.
    let result = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_Videos,
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
    // SAFETY: память выделена `SHGetKnownFolderPath` и освобождается один раз; NULL допустим.
    unsafe { CoTaskMemFree(raw.cast()) };
    dir
}

/// `shell.trashItem(path)`: Windows — Корзина (`SHFileOperationW` с отменой), Linux — `gio trash`.
fn trash(file: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        use windows_sys::Win32::UI::Shell::{
            FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
            SHFILEOPSTRUCTW, SHFileOperationW,
        };
        // Список путей: каждый с нулём, в конце ещё один ноль.
        let from: Vec<u16> = file.as_os_str().encode_wide().chain([0, 0]).collect();
        let mut operation = SHFILEOPSTRUCTW {
            hwnd: std::ptr::null_mut(),
            wFunc: FO_DELETE,
            pFrom: from.as_ptr(),
            pTo: std::ptr::null(),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
            fAnyOperationsAborted: 0,
            hNameMappings: std::ptr::null_mut(),
            lpszProgressTitle: std::ptr::null(),
        };
        // SAFETY: `from` — список с двумя нулями на конце, живёт до конца вызова; окна нет.
        let code = unsafe { SHFileOperationW(&mut operation) };
        if code != 0 || operation.fAnyOperationsAborted != 0 {
            return Err(format!("SHFileOperationW: {code:#x}"));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let status = std::process::Command::new("gio")
            .arg("trash")
            .arg("--")
            .arg(file)
            .status()
            .map_err(|error| format!("gio trash: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("gio trash: {status}"))
        }
    }
}
