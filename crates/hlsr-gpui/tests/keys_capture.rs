//! Захват клавиши на настоящем окне GPUI: сообщения `PostMessageW` в очередь окна → имя движка;
//! поглощённые нажатия до GPUI не доходят, после захвата — доходят.
//!
//! Окно открывается только на невидимом столе, поэтому тест `#[ignore]`:
//! `python scripts/slint/headless-win.py cargo test -p hlsr-gpui --test keys_capture -- --ignored`.
//! Без `HLSR_HEADLESS_DESKTOP` тест падает, а не открывает окно на экране.
#![cfg(windows)]

use gpui::{App, AppContext, Bounds, Empty, Window, WindowBounds, WindowOptions, point, px, size};
use hlsr_gpui::keys;
use hlsr_ui::keyboard_layout::Engine;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WM_KEYDOWN, WM_MOUSEWHEEL, WM_SYSKEYDOWN, WM_XBUTTONDOWN, XBUTTON1,
};

/// `lParam` нажатия: скан-код, расширенная клавиша, контекст Alt.
fn key_lparam(scancode: u8, extended: bool, alt: bool) -> isize {
    1 | isize::from(scancode) << 16 | isize::from(extended) << 24 | isize::from(alt) << 29
}

fn point_lparam(x: i32, y: i32) -> isize {
    (x as u16 as isize) | (y as u16 as isize) << 16
}

/// Сообщение окну: (сообщение, wParam, lParam).
type Message = (u32, usize, isize);

/// Шаг: движок и сообщения, которые шлются окну.
fn steps(hwnd: HWND) -> Vec<(Engine, Vec<Message>)> {
    let mut screen = POINT { x: 10, y: 10 };
    // SAFETY: HWND живого окна этого потока, POINT на стеке.
    unsafe { ClientToScreen(hwnd, &mut screen) };
    vec![
        // F13 движок не знает — захват ждёт дальше; правый Shift (0x36) в Source — RSHIFT.
        (
            Engine::Source,
            vec![
                (WM_KEYDOWN, 0x7C, key_lparam(0x64, false, false)),
                (WM_KEYDOWN, 0x10, key_lparam(0x36, false, false)),
            ],
        ),
        // Enter цифрового блока с Alt — WM_SYSKEYDOWN, расширенный 0x1C.
        (
            Engine::GoldSrc,
            vec![(WM_SYSKEYDOWN, 0x0D, key_lparam(0x1C, true, true))],
        ),
        // Кнопка «назад» над кнопкой захвата.
        (
            Engine::GoldSrc,
            vec![(
                WM_XBUTTONDOWN,
                usize::from(XBUTTON1) << 16,
                point_lparam(10, 10),
            )],
        ),
        // Колесо от себя (координаты экрана).
        (
            Engine::GoldSrc,
            vec![(WM_MOUSEWHEEL, 120 << 16, point_lparam(screen.x, screen.y))],
        ),
        // Esc — отмена без клавиши.
        (
            Engine::GoldSrc,
            vec![(WM_KEYDOWN, 0x1B, key_lparam(0x01, false, false))],
        ),
    ]
}

#[derive(Default)]
struct Log {
    captured: Vec<Option<&'static str>>,
    seen_by_gpui: Vec<String>,
    done: bool,
}

fn post(hwnd: HWND, messages: &[Message]) {
    for &(message, wparam, lparam) in messages {
        // SAFETY: HWND живого окна этого потока.
        assert_ne!(unsafe { PostMessageW(hwnd, message, wparam, lparam) }, 0);
    }
}

fn step(index: usize, log: Rc<RefCell<Log>>, window: &mut Window, cx: &mut App) {
    let hwnd = hlsr_gpui::win32::hwnd(window).expect("HWND") as HWND;
    let Some((engine, messages)) = steps(hwnd).into_iter().nth(index) else {
        assert_eq!(keys::capturing(), None);
        // Без захвата клавиша идёт в GPUI как обычно.
        post(hwnd, &[(WM_KEYDOWN, 0x41, key_lparam(0x1E, false, false))]);
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            log.borrow_mut().done = true;
            cx.update(|cx| cx.quit());
        })
        .detach();
        return;
    };
    let area = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(100.)));
    let next = log.clone();
    keys::start(
        window,
        cx,
        "+attack",
        engine,
        area,
        move |end, window, cx| {
            assert_eq!(end.owner, "+attack");
            next.borrow_mut().captured.push(end.key);
            step(index + 1, next, window, cx);
        },
    );
    assert_eq!(keys::capturing().as_deref(), Some("+attack"));
    post(hwnd, &messages);
}

#[test]
#[ignore = "окно GPUI: только под scripts/slint/headless-win.py, с -- --ignored"]
fn posted_messages_are_captured_before_gpui() {
    assert!(
        std::env::var_os("HLSR_HEADLESS_DESKTOP").is_some(),
        "keys_capture: только под scripts/slint/headless-win.py"
    );
    let log = Rc::new(RefCell::new(Log::default()));
    let platform = gpui_windows::WindowsPlatform::new(false).expect("платформа GPUI");
    let run_log = log.clone();
    gpui::Application::with_platform(Rc::new(platform)).run(move |cx: &mut App| {
        let seen = run_log.clone();
        cx.intercept_keystrokes(move |event, _, _| {
            seen.borrow_mut()
                .seen_by_gpui
                .push(event.keystroke.key.clone());
        })
        .detach();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(200.), px(100.)),
            ))),
            ..Default::default()
        };
        let window = cx
            .open_window(options, |_, cx| cx.new(|_| Empty))
            .expect("окно");
        let log = run_log.clone();
        window
            .update(cx, |_, window, cx| step(0, log, window, cx))
            .expect("окно");
        // Сторож: зависший захват не держит тест вечно.
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs(20))
                .await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    let log = log.borrow();
    assert!(log.done, "не дошли до конца: {:?}", log.captured);
    assert_eq!(
        log.captured,
        [
            Some("RSHIFT"),
            Some("KP_ENTER"),
            Some("MOUSE4"),
            Some("MWHEELUP"),
            None
        ]
    );
    assert_eq!(log.seen_by_gpui, ["a"]);
}
