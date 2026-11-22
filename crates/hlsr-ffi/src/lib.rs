//! HLSR как библиотека для окна на WinUI (C#): службы лаунчера (доставка, платформа, онлайн,
//! обновление) без GPUI, каналы окна — по C ABI.
//!
//! Порядок запуска и выхода — как `hlsr-app/src/startup.rs`. Каналы — те же, что у окна GPUI и
//! у `ipcRenderer` 0.4.x: `hlsr_invoke(id, channel, args)` → событие `reply`, события служб — тем
//! же обратным вызовом, команды окну — `host:<имя>`.
//!
//! Все строки — UTF-8 с нулём на конце. Строки, которые возвращает библиотека, освобождаются
//! [`hlsr_free_string`].

#![allow(clippy::missing_safety_doc)]

mod demo;
mod host;
mod modes;
mod online;
mod platform;
mod services;
mod update;
mod visibility;
mod winui;

use hlsr_core::instance::{self, Acquired, PrimaryInstance};
use hlsr_core::language::ensure_language;
use hlsr_core::logging;
use hlsr_core::migration::local_storage::migrate_local_storage;
use hlsr_core::paths::Paths;
use hlsr_core::settings::Settings;
use hlsr_ui::services::router::ChannelRouter;
use hlsr_ui::services::{MainProcess, StoreRead};
use serde_json::{Value, json};
use std::ffi::{CStr, CString, c_char};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};
use visibility::{Visibility, WindowEvents};

pub use host::EventCallback;

const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(1);

/// Код [`hlsr_start`]: запущено.
pub const START_OK: i32 = 0;
/// Уже запущен другой HLSR: аргументы переданы ему.
pub const START_FORWARDED: i32 = 1;
/// Ошибка запуска (подробности — в журнале и событии `startup-error`).
pub const START_FAILED: i32 = 2;
/// Запущено ранее.
pub const START_ALREADY: i32 = 3;

struct Launcher {
    runtime: tokio::runtime::Runtime,
    router: Arc<ChannelRouter>,
    instance: Arc<Mutex<PrimaryInstance>>,
    online: online::Online,
    update: update::Update,
    platform: Option<platform::Platform>,
    visibility: Arc<Visibility>,
    _log: Option<Box<dyn std::any::Any + Send>>,
}

static LAUNCHER: Mutex<Option<Launcher>> = Mutex::new(None);

fn text(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: строка хоста с нулём на конце, живёт на время вызова.
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
}

fn args(ptr: *const c_char) -> Vec<Value> {
    match serde_json::from_str::<Value>(&text(ptr)) {
        Ok(Value::Array(items)) => items,
        Ok(Value::Null) | Err(_) => Vec::new(),
        Ok(other) => vec![other],
    }
}

fn out(value: String) -> *mut c_char {
    CString::new(value.replace('\0', ""))
        .unwrap_or_default()
        .into_raw()
}

fn with_router<T>(f: impl FnOnce(&Launcher) -> T) -> Option<T> {
    let guard = LAUNCHER.lock().ok()?;
    guard.as_ref().map(f)
}

/// Запустить службы. `callback` получает события и ответы из потоков tokio.
#[unsafe(no_mangle)]
pub extern "C" fn hlsr_start(callback: Option<EventCallback>) -> i32 {
    host::set_callback(callback);
    let Ok(mut slot) = LAUNCHER.lock() else {
        return START_FAILED;
    };
    if slot.is_some() {
        return START_ALREADY;
    }
    match std::panic::catch_unwind(start) {
        Ok(Ok(Some(launcher))) => {
            *slot = Some(launcher);
            START_OK
        }
        Ok(Ok(None)) => START_FORWARDED,
        Ok(Err(message)) => {
            tracing::error!("Startup error: {message}");
            host::emit("startup-error", &json!(message));
            START_FAILED
        }
        Err(_) => {
            host::emit("startup-error", &json!("panic"));
            START_FAILED
        }
    }
}

fn start() -> Result<Option<Launcher>, String> {
    let paths = Paths::detect(hlsr_core::paths::APP_NAME).map_err(|e| e.to_string())?;
    let log = logging::init(&paths)
        .ok()
        .map(|guard| Box::new(guard) as Box<dyn std::any::Any + Send>);
    tracing::info!("[ffi] HLSR {} (WinUI)", env!("CARGO_PKG_VERSION"));
    let forwarded: Vec<String> = std::env::args().skip(1).collect();
    let mut instance = match instance::acquire(&paths, forwarded).map_err(|e| e.to_string())? {
        Acquired::Forwarded => return Ok(None),
        Acquired::Primary(instance) => instance,
    };
    migrate_local_storage(&paths, SystemTime::now());
    let mut settings = Settings::open(&paths);
    let language = ensure_language(&mut settings);
    tracing::info!("[startup] язык {}", language.code());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("hlsr-worker")
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let _enter = runtime.enter();

    let second_instances = instance.listen();
    let instance = Arc::new(Mutex::new(instance));
    let visibility = Arc::new(Visibility::new());
    let profile_qa = online::ProfileQa::from_process();
    let online = online::start_online(&paths, visibility.state(), &profile_qa);
    let configured = settings.renderer().as_str().to_owned();
    let update = update::start_update(
        &paths,
        &online,
        update::RendererInfo {
            configured,
            started_with: "winui".to_owned(),
            active: "winui".to_owned(),
        },
    );
    let platform = platform::start_platform(&paths, &online, instance.clone(), visibility.state());
    update.bind_platform(platform.delivery(), platform.api());
    update.api().initialize();
    platform.initialize();
    online.initialize();
    visibility.connect(Box::new(LauncherEvents {
        online: Arc::downgrade(online.api()),
        update: update.clone(),
        delivery: Arc::downgrade(platform.delivery()),
        platform: Arc::downgrade(platform.api()),
    }));
    listen_second_instances(second_instances, platform.delivery());
    let router = services::start(runtime.handle(), &paths, &platform, &online, update.api());

    // События служб — хосту.
    let mut events = router.subscribe();
    runtime.spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    host::emit(&event.channel, &event.payload);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!("[ffi] окно отстало от событий: {skipped}");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    drop(_enter);
    Ok(Some(Launcher {
        runtime,
        router,
        instance,
        online,
        update,
        platform: Some(platform),
        visibility,
        _log: log,
    }))
}

/// `ipcRenderer.invoke(channel, ...args)`: ответ — событие `reply` с
/// `{id, ok: true, value}` или `{id, ok: false, error}`.
#[unsafe(no_mangle)]
pub extern "C" fn hlsr_invoke(id: u64, channel: *const c_char, args_json: *const c_char) {
    let channel = text(channel);
    let args = args(args_json);
    let started = with_router(|launcher| {
        let reply = launcher.router.invoke(&channel, args);
        launcher.runtime.spawn(async move {
            let payload = match reply.await {
                Ok(value) => json!({"id": id, "ok": true, "value": value}),
                Err(error) => json!({"id": id, "ok": false, "error": error}),
            };
            host::emit("reply", &payload);
        });
    });
    if started.is_none() {
        host::emit(
            "reply",
            &json!({"id": id, "ok": false, "error": "NOT_STARTED"}),
        );
    }
}

/// `ipcRenderer.send(channel, ...args)`.
#[unsafe(no_mangle)]
pub extern "C" fn hlsr_send(channel: *const c_char, args_json: *const c_char) {
    let channel = text(channel);
    let args = args(args_json);
    with_router(|launcher| {
        let _enter = launcher.runtime.enter();
        launcher.router.send(&channel, args);
    });
}

/// `ipcRenderer.sendSync(channel, ...args)`: JSON ответа.
#[unsafe(no_mangle)]
pub extern "C" fn hlsr_send_sync(channel: *const c_char, args_json: *const c_char) -> *mut c_char {
    let channel = text(channel);
    let args = args(args_json);
    let value = with_router(|launcher| {
        let _enter = launcher.runtime.enter();
        launcher.router.send_sync(&channel, args)
    })
    .unwrap_or(Value::Null);
    out(value.to_string())
}

/// `store.read(name)`: `{found, valid, data}`.
#[unsafe(no_mangle)]
pub extern "C" fn hlsr_store_read(name: *const c_char) -> *mut c_char {
    let name = text(name);
    let value = with_router(|launcher| match launcher.router.store_read(&name) {
        StoreRead::Missing => json!({"found": false, "valid": false, "data": null}),
        StoreRead::Invalid => json!({"found": true, "valid": false, "data": null}),
        StoreRead::Found(data) => json!({"found": true, "valid": true, "data": data}),
    })
    .unwrap_or(json!({"found": false, "valid": false, "data": null}));
    out(value.to_string())
}

/// `store.write(name, data)`: записано ли.
#[unsafe(no_mangle)]
pub extern "C" fn hlsr_store_write(name: *const c_char, data_json: *const c_char) -> bool {
    let name = text(name);
    let Ok(data) = serde_json::from_str::<Value>(&text(data_json)) else {
        return false;
    };
    with_router(|launcher| launcher.router.store_write(&name, data)).unwrap_or(false)
}

/// Состояние окна хоста: HWND, видно ли (не свёрнуто), в фокусе ли.
#[unsafe(no_mangle)]
pub extern "C" fn hlsr_window_state(hwnd: isize, visible: bool, focused: bool) {
    host::set_hwnd(hwnd);
    host::set_shown(hwnd != 0);
    let visibility = with_router(|launcher| launcher.visibility.clone());
    if let Some(visibility) = visibility {
        let handle = with_router(|launcher| launcher.runtime.handle().clone());
        if let Some(handle) = handle {
            let _enter = handle.enter();
            visibility.observe(visible, focused);
        }
    }
}

/// Остановить службы по шагам выхода 0.4.7; `exit_code` — код выхода процесса.
#[unsafe(no_mangle)]
pub extern "C" fn hlsr_shutdown(exit_code: i32) {
    let launcher = LAUNCHER.lock().ok().and_then(|mut slot| slot.take());
    let Some(mut launcher) = launcher else {
        return;
    };
    host::set_shown(false);
    launcher.visibility.closed();
    {
        let _enter = launcher.runtime.enter();
        if let Some(demo) = services::DEMO.get() {
            demo.dispose();
        }
        launcher.update.before_quit(&launcher.runtime);
        launcher.online.stop_discord();
        if let Some(platform) = launcher.platform.take() {
            platform.stop();
        }
        launcher.update.dispose();
        launcher.online.stop(&launcher.runtime);
        launcher.update.install_on_quit(exit_code);
        if launcher.update.relaunch_pending() {
            if let Ok(mut instance) = launcher.instance.lock() {
                instance.release();
            }
            launcher.update.spawn_relaunch();
        }
    }
    host::set_callback(None);
    drop(launcher.router);
    launcher.runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
}

/// Попросила ли служба выйти (`app.quit()`).
#[unsafe(no_mangle)]
pub extern "C" fn hlsr_quit_requested() -> bool {
    host::quit_requested()
}

/// Освободить строку, которую вернула библиотека.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlsr_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        // SAFETY: строка выделена `CString::into_raw` этой библиотеки.
        drop(unsafe { CString::from_raw(ptr) });
    }
}

/// События окна для служб в порядке `AppManager.js` (как `LauncherEvents` в `startup.rs`).
struct LauncherEvents {
    online: Weak<hlsr_online::api::OnlineApi>,
    update: update::Update,
    delivery: Weak<hlsr_delivery::api::DeliveryApi>,
    platform: Weak<hlsr_platform::api::PlatformApi>,
}

impl WindowEvents for LauncherEvents {
    fn ready(&self) {
        if let Some(delivery) = self.delivery.upgrade() {
            platform::window_ready(&delivery);
        }
        if let Some(online) = self.online.upgrade() {
            let receipt = online::window_loaded(&online);
            self.update.window_loaded(receipt);
        }
    }

    fn shown(&self) {
        if let Some(online) = self.online.upgrade() {
            online.window_shown();
        }
    }

    fn focused(&self) {
        self.update.api().check_on_focus();
        if let (Some(delivery), Some(platform)) = (self.delivery.upgrade(), self.platform.upgrade())
        {
            platform::window_focused(&delivery, &platform);
        }
        if let Some(online) = self.online.upgrade() {
            online.window_focused();
        }
    }
}

/// Аргументы следующих запусков: показать окно, игра из ярлыка.
fn listen_second_instances(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Vec<String>>,
    delivery: &Arc<hlsr_delivery::api::DeliveryApi>,
) {
    let delivery = Arc::downgrade(delivery);
    tokio::spawn(async move {
        while let Some(argv) = rx.recv().await {
            if !host::command("reveal", Value::Null) {
                break;
            }
            if let Some(delivery) = delivery.upgrade() {
                delivery.select_game_from_args(&argv);
            }
        }
    });
}
