//! «Скрипты» HL1 на настоящем окне GPUI при зуме 1; 0,75; 0,72; 1,25: щелчок по «Назначить»
//! (`PostMessageW` в его середину), затем правый Shift (скан-код 0x36; GoldSrc сторон Shift не
//! различает — `SHIFT`), снова «Назначить» и XBUTTON1 над кнопкой — `MOUSE4`. Оба бинда — в
//! черновике сессии: кнопка нашлась по рамке своего кадра, область захвата совпала с ней.
//!
//! Окно открывается только на невидимом столе, поэтому тест `#[ignore]`:
//! `python scripts/slint/headless-win.py cargo test -p hlsr-gpui --features dev-hooks --test
//! scripts_capture -- --ignored`.
#![cfg(all(windows, feature = "dev-hooks"))]

use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div, point, px, rgb, size,
};
use hlsr_core::language::Language;
use hlsr_gpui::configs::scripts::GameScripts;
use hlsr_gpui::configs::sections::SectionHost;
use hlsr_ui::controllers::game_settings::{Env, Session, Zone};
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant, screens_049_dir};
use hlsr_ui::services::Services;
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_XBUTTONDOWN, XBUTTON1,
};

const ZOOMS: [f64; 4] = [1.0, 0.75, 0.72, 1.25];

struct Root {
    scripts: Option<Entity<GameScripts>>,
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        hlsr_gpui::theme::frame(window, cx);
        div()
            .size_full()
            .p(hlsr_gpui::scale::u(16.))
            .bg(rgb(0x0b0e13))
            .children(self.scripts.clone())
    }
}

fn post(hwnd: HWND, message: u32, wparam: usize, lparam: isize) {
    // SAFETY: HWND живого окна этого потока.
    assert_ne!(unsafe { PostMessageW(hwnd, message, wparam, lparam) }, 0);
}

fn point_lparam(x: i32, y: i32) -> isize {
    (x as u16 as isize) | ((y as u16 as isize) << 16)
}

/// Середина рамки (логические px окна) в px клиента.
fn center(bounds: Bounds<gpui::Pixels>, scale: f32) -> isize {
    let c = bounds.center();
    point_lparam(
        (f32::from(c.x) * scale).round() as i32,
        (f32::from(c.y) * scale).round() as i32,
    )
}

async fn wait(cx: &AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Ключи черновика биндов сессии.
fn bound_keys(session: &Session) -> Vec<String> {
    match session.draft().get("binds") {
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

struct Opened {
    window: WindowHandle<Root>,
    hwnd: HWND,
    scale: f32,
}

fn open_window(cx: &mut App) -> Opened {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(1400.), px(900.)),
        ))),
        ..Default::default()
    };
    let window = cx
        .open_window(options, |_, cx| cx.new(|_| Root { scripts: None }))
        .expect("окно");
    let (hwnd, scale) = window
        .update(cx, |_, window, _| {
            (
                hlsr_gpui::win32::hwnd(window).expect("HWND") as HWND,
                window.scale_factor(),
            )
        })
        .expect("окно");
    Opened {
        window,
        hwnd,
        scale,
    }
}

/// Раздел «Скрипты» с новой сессией настроек HL1 в окне при зуме `zoom`.
fn mount(zoom: f64, window: WindowHandle<Root>, cx: &mut App) -> Rc<Session> {
    let scene = Scene::load(&screens_049_dir(), "configs/launch").expect("сцена");
    let variant = Variant {
        theme: "hlsr".into(),
        lang: "ru".into(),
        scale: 1.0,
        zoom: None,
        rail: None,
    };
    let services: Services = Arc::new(FixtureServices::new(&scene, &variant).expect("фикстура"));
    let env = hlsr_gpui::env::GpuiEnv::new(cx);
    let session = Session::new(
        services.clone(),
        Env {
            game_id: "70".into(),
            game_title: "Half-Life".into(),
            initial_section: "scripts".into(),
            linux: false,
            language: Language::Ru,
            translator: Rc::new(hlsr_ui::tr),
            zone: Zone::Local,
        },
        env,
    );
    let host = SectionHost {
        services,
        session: session.clone(),
        game_id: "70".into(),
        language: Language::Ru,
        page: WeakEntity::new_invalid(),
    };
    window
        .update(cx, |root, window, cx| {
            window.set_rem_size(hlsr_gpui::scale::rem_size(zoom));
            root.scripts = Some(cx.new(|cx| GameScripts::new(&host, cx)));
            cx.notify();
        })
        .expect("окно");
    // Перемены сессии → перерисовка окна.
    let app = cx.to_async();
    let executor = cx.foreground_executor().clone();
    session.set_render(move || {
        let app = app.clone();
        executor
            .spawn(async move {
                app.update(|cx| cx.refresh_windows());
            })
            .detach();
    });
    session.mounted();
    session
}
/// Рамка «Назначить» первой строки, как её нарисовал прошлый кадр.
async fn capture_button(cx: &AsyncApp) -> Bounds<gpui::Pixels> {
    for _ in 0..100 {
        wait(cx, 50).await;
        cx.update(|cx| cx.refresh_windows());
        if let Some(bounds) = hlsr_gpui::snap::bounds_of("gsc-capture-0")
            && bounds.size.width > px(0.)
        {
            return bounds;
        }
    }
    panic!("кнопки «Назначить» нет на экране");
}

/// Один зум: RSHIFT → SHIFT, затем XBUTTON1 → MOUSE4.
async fn check(zoom: f64, opened: &Opened, cx: &AsyncApp) -> Result<(), String> {
    let session = cx.update(|cx| mount(zoom, opened.window, cx));
    // Рамки целей — уже нового раздела.
    wait(cx, 200).await;
    for _ in 0..100 {
        wait(cx, 50).await;
        if session.binds_game_id().is_some() && session.scripts().installed {
            break;
        }
    }
    let button = capture_button(cx).await;
    let at = center(button, opened.scale);
    post(opened.hwnd, WM_LBUTTONDOWN, 1, at);
    post(opened.hwnd, WM_LBUTTONUP, 0, at);
    wait(cx, 300).await;
    if session.scripts_capturing().is_none() {
        return Err(format!(
            "×{zoom}: щелчок по «Назначить» не начал захват ({button:?})"
        ));
    }
    // Правый Shift: скан-код 0x36.
    post(opened.hwnd, WM_KEYDOWN, 0x10, 1 | (0x36 << 16));
    wait(cx, 300).await;
    let keys = bound_keys(&session);
    if !keys.iter().any(|key| key == "SHIFT") || session.scripts_capturing().is_some() {
        return Err(format!("×{zoom}: правый Shift не назначен: {keys:?}"));
    }
    let button = capture_button(cx).await;
    let at = center(button, opened.scale);
    post(opened.hwnd, WM_LBUTTONDOWN, 1, at);
    post(opened.hwnd, WM_LBUTTONUP, 0, at);
    wait(cx, 300).await;
    post(opened.hwnd, WM_XBUTTONDOWN, usize::from(XBUTTON1) << 16, at);
    wait(cx, 300).await;
    let keys = bound_keys(&session);
    if !keys.iter().any(|key| key == "MOUSE4") {
        return Err(format!(
            "×{zoom}: MOUSE4 над кнопкой не назначена: {keys:?}"
        ));
    }
    Ok(())
}

#[test]
#[ignore = "окно GPUI: только под scripts/slint/headless-win.py, с -- --ignored"]
fn capture_assigns_right_shift_and_mouse4_at_every_zoom() {
    assert!(
        std::env::var_os("HLSR_HEADLESS_DESKTOP").is_some(),
        "scripts_capture: только под scripts/slint/headless-win.py"
    );
    let results: Rc<RefCell<Vec<Result<(), String>>>> = Rc::default();
    let platform = gpui_windows::WindowsPlatform::new(false).expect("платформа GPUI");
    let out = results.clone();
    gpui::Application::with_platform(Rc::new(platform)).run(move |cx: &mut App| {
        hlsr_gpui::init(cx);
        let out = out.clone();
        let opened = open_window(cx);
        cx.spawn(async move |cx| {
            for zoom in ZOOMS {
                let result = check(zoom, &opened, cx).await;
                out.borrow_mut().push(result);
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
        // Сторож: зависшее окно не держит тест вечно.
        cx.spawn(async move |cx| {
            wait(cx, 60_000).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    let results = results.borrow();
    assert_eq!(results.len(), ZOOMS.len(), "не все зумы: {results:?}");
    let failed: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    assert!(failed.is_empty(), "{failed:#?}");
}
