//! Окна режимов на GPUI: установщик и удаление (`installer-window.js` 0.4.9: 640 × 560 без рамки,
//! прозрачное, углы рисует страница) и внешний апдейтер (`updater/main.js`: 460 × 280 с рамкой
//! ОС). Логика режимов — `InstallerController` и `UpdaterMain`; виды — `hlsr_gpui::installer` и
//! `hlsr_gpui::updater`. С `dev-hooks` `HLSR_SMOKE_SNAPSHOT=<png>` — снимок окна и выход, а
//! `HLSR_SNAP_WINDOW=<сцена.json>` — окно сцены сверки (`hlsr-snap windows`) без работы режима.

use crate::window::FixedWindow;
use gpui::{
    App, AppContext, Bounds, Pixels, QuitMode, Render, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, WindowOptions, px, size,
};
use hlsr_gpui::installer::InstallerRoot;
use hlsr_gpui::motion::AnimGate;
use hlsr_gpui::updater::{Edit, UpdaterRoot};
use hlsr_ui::controllers::installer::{InstallerController, PickDirectory, Spawn};
use hlsr_ui::controllers::updater::UpdaterCommand;
use hlsr_update::installer_window::InstallerWindow;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};

/// `app.quit()` из любого потока: цикл событий окна режима кончается.
pub fn quit_hook() -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(|| {
        hlsr_gpui::bridge::quit();
    })
}

/// Окно установщика: без рамки, прозрачное, без скругления DWM (`roundedCorners: false`).
fn installer_options(title: &str, bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(title.to_owned().into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        is_resizable: false,
        window_decorations: Some(WindowDecorations::Client),
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("hlsr".into()),
        ..hlsr_gpui::window::full_rate()
    }
}

/// Окно апдейтера с рамкой ОС: клиентская область по центру рабочей области.
#[cfg_attr(not(windows), allow(dead_code))]
fn updater_options(bounds: Bounds<Pixels>) -> WindowOptions {
    hlsr_gpui::window::mode_options("HLSR Updater", bounds)
}

/// Клиентская область апдейтера: 460 × 280 вместе с рамкой Windows 11.
#[cfg_attr(not(windows), allow(dead_code))]
fn updater_bounds(cx: &App) -> Bounds<Pixels> {
    let (width, height) = hlsr_gpui::updater::CLIENT;
    let outer = FixedWindow::Updater.bounds(cx);
    hlsr_gpui::window::place(outer, size(px(width), px(height)), None)
}

/// Приложение GPUI окна режима: `open` открывает окно (`Err` — не открылось). `Err` — окна нет
/// (нет графики или окно не открылось).
fn run_app(open: impl FnOnce(&mut App) -> Result<(), String> + 'static) -> Result<(), String> {
    let platform = hlsr_gpui::app::platform()?;
    let opened: Rc<RefCell<Result<(), String>>> = Rc::new(RefCell::new(Ok(())));
    let result = opened.clone();
    gpui::Application::with_platform(platform)
        .with_assets(hlsr_gpui::assets::Assets)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            hlsr_gpui::init(cx);
            hlsr_gpui::bridge::run_queue(cx);
            AnimGate::set_reduce_motion(cx, crate::reduced_motion::os_prefers_reduced_motion());
            if let Err(error) = open(cx) {
                *opened.borrow_mut() = Err(format!("окно не открылось: {error}"));
                cx.quit();
            }
        });
    result.replace(Ok(()))
}

/// Снимок окна (`HLSR_SMOKE_SNAPSHOT`) под `dev-hooks`.
fn schedule_snapshot<V: Render>(window: gpui::WindowHandle<V>, cx: &mut App) {
    #[cfg(feature = "dev-hooks")]
    let _ = window.update(cx, |_, window, cx| hlsr_gpui::smoke::schedule(window, cx));
    #[cfg(not(feature = "dev-hooks"))]
    let _ = (window, cx);
}

/// Долгие методы окна установщика — в своём потоке.
fn spawn_thread() -> Spawn {
    Arc::new(|work| {
        if let Err(error) = std::thread::Builder::new()
            .name("hlsr-installer".into())
            .spawn(work)
        {
            tracing::error!("[installer] поток не запущен: {error}");
        }
    })
}

/// Выбор каталога (`dialog.showOpenDialog(window, …)`, `openDirectory`, `createDirectory`):
/// владелец — окно установщика (`owner`, HWND; 0 — окна ещё нет), диалог модален ему и открыт
/// над ним.
fn pick_directory(owner: Arc<AtomicIsize>) -> PickDirectory {
    Arc::new(move |start: &str| {
        let owner = Some(owner.load(Ordering::Acquire)).filter(|hwnd| *hwnd != 0);
        let start = Some(std::path::Path::new(start));
        crate::platform::file_dialog(owner, None, start, &[])
            .set_can_create_directories(true)
            .pick_folder()
            .map(|path| path.to_string_lossy().into_owned())
    })
}

/// `installer-open-discord`: Discord HLSR в браузере (адрес знает только main).
fn open_discord() {
    let Some(url) = hlsr_delivery::links::url_for(&serde_json::json!("discord")) else {
        return;
    };
    if let Err(error) = crate::platform::open_external(url) {
        tracing::warn!("[installer] Discord не открыт: {error}");
    }
}

/// Окно установщика (`openInstallerWindow`): страница — контроллер установщика; закрытие системой
/// и кнопкой «Закрыть» ждёт работу (`close` 0.4.9), окно закрывает `destroy` установщика.
pub fn run_installer(title: &str, window: Arc<InstallerWindow>) -> Result<(), String> {
    window.set_on_destroy({
        let quit = quit_hook();
        move || quit()
    });
    let (views, received) = tokio::sync::mpsc::unbounded_channel();
    let owner = Arc::new(AtomicIsize::new(0));
    let page = InstallerController::new(
        window,
        Arc::new(move |view| {
            let _ = views.send(view);
        }),
        spawn_thread(),
        pick_directory(owner.clone()),
        Arc::new(open_discord),
    );
    let title = title.to_owned();
    run_app(move |cx| {
        let bounds = FixedWindow::Installer.bounds(cx);
        let closing = page.clone();
        let handle = cx
            .open_window(installer_options(&title, bounds), move |window, cx| {
                window.on_window_should_close(cx, move |_, _| {
                    closing.close();
                    false
                });
                #[cfg(windows)]
                {
                    hlsr_gpui::win32::system_corners_off(window);
                    if let Some(hwnd) = hlsr_gpui::win32::hwnd(window) {
                        owner.store(hwnd, Ordering::Release);
                    }
                }
                cx.new(|cx| InstallerRoot::new(page, received, cx))
            })
            .map_err(|error| format!("{error:#}"))?;
        schedule_snapshot(handle, cx);
        #[cfg(feature = "dev-hooks")]
        autoclick(handle, cx);
        Ok(())
    })
}

/// `dev-hooks`: `HLSR_INSTALLER_CLICKS=<n>` — n нажатий основной кнопки раз в 0,8 с событиями окна
/// (сквозная установка на невидимом столе, где нет ввода ОС).
#[cfg(feature = "dev-hooks")]
fn autoclick(handle: gpui::WindowHandle<InstallerRoot>, cx: &mut App) {
    let Some(clicks) = super::dev_env("HLSR_INSTALLER_CLICKS").and_then(|n| n.parse::<u32>().ok())
    else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        window
            .spawn(cx, async move |cx| {
                for _ in 0..clicks {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(800))
                        .await;
                    let _ = cx.update(|window, cx| {
                        let Some(at) = hlsr_gpui::snap::bounds_of("installer-action") else {
                            return;
                        };
                        let at = at.center();
                        let left = gpui::MouseButton::Left;
                        hlsr_gpui::snap::pointer_move(window, cx, at);
                        hlsr_gpui::snap::press(window, cx, at, left, 1, true);
                        hlsr_gpui::snap::press(window, cx, at, left, 1, false);
                    });
                }
            })
            .detach();
    });
}

/// Окно «HLSR Updater» (`updater/main.js`): `edits` — события ядра для страницы, `on_command` —
/// «Отменить»/«Закрыть»; `on_open` — окно открыто (сессия начинается); закрыть окно системой можно,
/// если `on_close` разрешает (тогда — выход). Апдейтер — только Windows (`modes::updater`).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn run_updater(
    edits: tokio::sync::mpsc::UnboundedReceiver<Edit>,
    on_command: impl Fn(UpdaterCommand) + 'static,
    on_open: impl FnOnce() + 'static,
    on_close: impl Fn() -> bool + 'static,
) -> Result<(), String> {
    run_app(move |cx| {
        let bounds = updater_bounds(cx);
        let handle = cx
            .open_window(updater_options(bounds), move |window, cx| {
                // Окно закрывает `quit`: разрешённое закрытие GPUI уничтожал окно сам, и выход
                // потом обращался к уже уничтоженному.
                window.on_window_should_close(cx, move |_, cx| {
                    if on_close() {
                        cx.quit();
                    }
                    false
                });
                cx.new(|cx| UpdaterRoot::new(edits, Rc::new(on_command), cx))
            })
            .map_err(|error| format!("{error:#}"))?;
        schedule_snapshot(handle, cx);
        on_open();
        Ok(())
    })
}

/// Окно сцены сверки (`hlsr-snap windows`): сцена `tools/snap/windows.json` и `{zoom, width,
/// height, language}` варианта; снимок — `HLSR_SMOKE_SNAPSHOT`.
#[cfg(feature = "dev-hooks")]
pub fn snap(file: &str) -> i32 {
    use hlsr_ui::controllers::installer::{Extra, PageInputs, state_from_value, view};
    use hlsr_ui::controllers::updater::UpdaterPage;
    use serde_json::Value;
    let scene: Value = match std::fs::read_to_string(file)
        .map_err(|error| error.to_string())
        .and_then(|text| serde_json::from_str(&text).map_err(|error| error.to_string()))
    {
        Ok(scene) => scene,
        Err(error) => {
            eprintln!("сцена {file}: {error}");
            return 1;
        }
    };
    let number =
        |key: &str, default: f64| scene.get(key).and_then(Value::as_f64).unwrap_or(default);
    let zoom = number("zoom", 1.) as f32;
    let (width, height) = (number("width", 640.) as f32, number("height", 560.) as f32);
    let language = scene
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("ru")
        .to_owned();
    let installer = scene.get("window").and_then(Value::as_str) == Some("installer");
    let result = run_app(move |cx| {
        let bounds = hlsr_gpui::window::place(
            crate::window::work_area(cx),
            size(px(width * zoom), px(height * zoom)),
            None,
        );
        let rem = px(16. * zoom);
        if installer {
            let mut state = scene.get("state").cloned().unwrap_or(Value::Null);
            state["language"] = Value::from(language);
            let shown = state_from_value(&state);
            let extra = Extra::from_value(&state);
            let mut inputs = PageInputs::default();
            inputs.first_render(&shown, &extra);
            // Флажки и шаги страницы — ключи `$…` состояния сцены.
            inputs.intro_step = state.get("$intro").and_then(Value::as_u64).unwrap_or(0) as u8;
            inputs.games = state.get("$games").and_then(Value::as_bool).unwrap_or(true);
            inputs.desktop = state
                .get("$desktop")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let shown = view(&shown, &extra, &inputs);
            let handle = cx
                .open_window(
                    installer_options("HLSR — установка", bounds),
                    move |window, cx| {
                        #[cfg(windows)]
                        hlsr_gpui::win32::system_corners_off(window);
                        window.set_rem_size(rem);
                        cx.new(|_| InstallerRoot::fixture(shown))
                    },
                )
                .map_err(|error| format!("{error:#}"))?;
            schedule_snapshot(handle, cx);
        } else {
            let mut page = UpdaterPage::new();
            for event in scene
                .get("events")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(init) = event.get("init") {
                    let target = init
                        .get("targetVersion")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    page.init(&language, target);
                } else if let Some(stage) = event.get("stage") {
                    page.stage(stage);
                } else if let Some(progress) = event.get("progress") {
                    page.progress(progress);
                }
            }
            let dark = scene.get("dark").and_then(Value::as_bool).unwrap_or(false);
            let handle = cx
                .open_window(updater_options(bounds), move |window, cx| {
                    window.set_rem_size(rem);
                    cx.new(|_| UpdaterRoot::fixture(page, dark))
                })
                .map_err(|error| format!("{error:#}"))?;
            schedule_snapshot(handle, cx);
        }
        Ok(())
    });
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}
