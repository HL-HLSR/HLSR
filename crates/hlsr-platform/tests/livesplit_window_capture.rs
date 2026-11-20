//! Снимок окна LiveSplit без своего exe (`livesplit-window-capture.js` 0.4.9, здесь — Win32 прямо в
//! процессе): окно-заглушка на своём невидимом рабочем столе снимается в формате `HLSRLS1\0`, а
//! без окна LiveSplit — код 2. На экран владельца ничего не выводится: поток теста работает на
//! отдельном рабочем столе, `EnumWindows` видит только его окна.

#![cfg(windows)]

use hlsr_core::win32::wide;
use hlsr_platform::livesplit_overlay::parse_snapshot;
use hlsr_platform::livesplit_window_capture::{
    CODE_NO_WINDOW, CODE_OK, capture, capture_of, find_main_window,
};
use windows_sys::Win32::Foundation::{GENERIC_ALL, RECT};
use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, UpdateWindow};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, CreateDesktopW, SetThreadDesktop,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, MSG,
    PM_REMOVE, PeekMessageW, RegisterClassW, TranslateMessage, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    WS_VISIBLE,
};

/// Цвет заливки окна-заглушки: R=0x12, G=0x34, B=0x56 (`COLORREF` — 0x00BBGGRR).
const FILL: u32 = 0x0056_3412;

/// Запускает `body` в потоке на своём рабочем столе.
fn on_hidden_desktop(body: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        let name = wide(&format!("hlsr-capture-test-{}", std::process::id()));
        // SAFETY: имя — строка UTF-16 с нулём; дескриптор закрывается ниже.
        let desktop = unsafe {
            CreateDesktopW(
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                GENERIC_ALL,
                std::ptr::null(),
            )
        };
        assert!(
            !desktop.is_null(),
            "рабочий стол не создан: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: у нового потока ещё нет окон и хуков.
        assert_ne!(unsafe { SetThreadDesktop(desktop) }, 0, "SetThreadDesktop");
        body();
        // SAFETY: окна потока уничтожены в `body`.
        unsafe { CloseDesktop(desktop) };
    })
    .join()
    .expect("поток теста");
}

/// Сообщения окон этого потока (перерисовка заглушки).
fn pump_messages() {
    // SAFETY: MSG — локальная структура; окна принадлежат этому потоку.
    unsafe {
        let mut message: MSG = std::mem::zeroed();
        while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[test]
fn stub_window_is_captured_in_the_snapshot_format() {
    on_hidden_desktop(|| {
        let class = wide("HlsrCaptureStub");
        // SAFETY: класс с процедурой по умолчанию и сплошной кистью; строки живут до конца теста.
        let window = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.hInstance = instance;
            wc.hbrBackground = CreateSolidBrush(FILL);
            wc.lpszClassName = class.as_ptr();
            assert_ne!(RegisterClassW(&wc), 0, "RegisterClassW");
            CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                0,
                0,
                320,
                240,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        assert!(!window.is_null(), "окно не создано");
        // Окно рисует себя (WM_PAINT с заливкой кистью класса), как живой LiveSplit.
        // SAFETY: окно этого потока.
        unsafe { UpdateWindow(window) };

        let pid = std::process::id();
        assert_eq!(find_main_window(&[pid]), Some(window));
        assert_eq!(find_main_window(&[pid.wrapping_add(1)]), None);

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("снимок окна.bin");
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: окно живо.
        unsafe { GetClientRect(window, &mut rect) };
        // DWM обновляет снимок окна асинхронно (PW_RENDERFULLCONTENT): первые снимки могут быть
        // ещё пустыми, как у только что открытого LiveSplit. Редактор снимает раз в две секунды.
        let mut center = Vec::new();
        for _ in 0..40 {
            let outcome = capture_of(&[pid], &file).unwrap();
            assert_eq!(outcome.code, CODE_OK, "{outcome:?}");
            let bytes = std::fs::read(&file).unwrap();
            assert_eq!(&bytes[..8], b"HLSRLS1\0");
            let snapshot = parse_snapshot(&bytes).expect("снимок разбирается");
            assert_eq!(
                (snapshot.width, snapshot.height),
                (rect.right as u32, rect.bottom as u32)
            );
            // Середина клиентской области — заливка окна в BGRX.
            let at = ((snapshot.height / 2 * snapshot.width + snapshot.width / 2) * 4) as usize;
            center = snapshot.pixels[at..at + 3].to_vec();
            if center == [0x56, 0x34, 0x12] {
                break;
            }
            pump_messages();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(center, [0x56, 0x34, 0x12]);

        // Окна LiveSplit на этом столе нет: код 2, файла нет.
        let none = dir.path().join("none.bin");
        assert_eq!(capture(&none).unwrap().code, CODE_NO_WINDOW);
        assert!(!none.exists());

        // SAFETY: окно этого потока.
        unsafe { DestroyWindow(window) };
    });
}
