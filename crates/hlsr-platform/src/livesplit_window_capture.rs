//! Windows: снимок окна LiveSplit для предпросмотра оверлея (`livesplit-window-capture.js` 0.4.9).
//!
//! 0.4.9 снимала окно обёрткой над WinAPI в PowerShell (до неё — своим `hlsr-capture.exe`, который
//! антивирусы блокировали). Здесь те же вызовы делаются прямо в процессе лаунчера: ни своего exe,
//! ни PowerShell. Снимок и коды те же: `HLSRLS1\0`, ширина, высота (`uint32` LE), затем строки
//! BGRX сверху вниз; 0 — готово, 2 — окна нет (LiveSplit закрыт или свёрнут), 3 — снимок не
//! получился.
//!
//! `PrintWindow` шлёт окну сообщения: зависший LiveSplit задержит вызов, поэтому его зовут из
//! `spawn_blocking` с тайм-аутом (`api/launch.rs`).

#![cfg(windows)]

use crate::livesplit_overlay::{MAX_SIDE, SNAPSHOT_MAGIC};
use std::path::Path;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HDC, ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::Storage::Xps::{PW_CLIENTONLY, PrintWindow};
use windows_sys::Win32::UI::HiDpi::{GetWindowDpiAwarenessContext, SetThreadDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetClientRect, GetWindow, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible,
};
use windows_sys::core::BOOL;

pub const CODE_OK: u32 = 0;
pub const CODE_NO_WINDOW: u32 = 2;
pub const CODE_FAILED: u32 = 3;

/// `PW_RENDERFULLCONTENT`: окно, нарисованное через DirectComposition, тоже попадает в снимок.
const PW_RENDERFULLCONTENT: u32 = 2;
/// `SENTINEL` в `scripts/livesplit-overlay/common.h`: что осталось этим цветом, окно не нарисовало.
const SENTINEL: u32 = 0x5A00_FF5A;

/// Итог снимка: код и причина сбоя (у кода 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub code: u32,
    pub error: Option<String>,
}

impl Outcome {
    fn done(code: u32) -> Outcome {
        Outcome { code, error: None }
    }

    fn failed(error: &str) -> Outcome {
        Outcome {
            code: CODE_FAILED,
            error: Some(error.to_owned()),
        }
    }
}

/// `Capture(file)`: снимок главного окна любого `LiveSplit.exe` в `file`.
pub fn capture(file: &Path) -> Result<Outcome, String> {
    let pids = crate::rinput_menu_fix_win::process_ids("LiveSplit")?;
    capture_of(&pids, file)
}

/// Снимок главного окна процессов `pids` (у `capture` — процессов LiveSplit).
pub fn capture_of(pids: &[u32], file: &Path) -> Result<Outcome, String> {
    let Some(window) = find_main_window(pids) else {
        return Ok(Outcome::done(CODE_NO_WINDOW));
    };
    // Размер окна снимается в DPI-контексте самого LiveSplit, иначе при масштабе Windows больше
    // 100% картинка занимает только часть холста.
    // SAFETY: окно — дескриптор из EnumWindows; контекст потока возвращается ниже.
    let previous = unsafe {
        let context = GetWindowDpiAwarenessContext(window);
        if context.is_null() {
            context
        } else {
            SetThreadDpiAwarenessContext(context)
        }
    };
    let result = capture_client(window, file);
    if !previous.is_null() {
        // SAFETY: прежний контекст этого же потока.
        unsafe { SetThreadDpiAwarenessContext(previous) };
    }
    result
}

/// `FindLiveSplit()`: самое большое видимое несвёрнутое окно верхнего уровня без владельца у
/// процессов `pids` (окна настроек принадлежат главному).
pub fn find_main_window(pids: &[u32]) -> Option<HWND> {
    struct Search<'a> {
        pids: &'a [u32],
        best: HWND,
        area: i64,
    }
    unsafe extern "system" fn visit(window: HWND, param: LPARAM) -> BOOL {
        // SAFETY: param — указатель на Search, живой на время EnumWindows.
        let search = unsafe { &mut *(param as *mut Search) };
        // SAFETY: окно из EnumWindows; функции только читают его свойства.
        unsafe {
            if IsWindowVisible(window) == 0
                || !GetWindow(window, GW_OWNER).is_null()
                || IsIconic(window) != 0
            {
                return 1;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(window, &mut pid);
            if !search.pids.contains(&pid) {
                return 1;
            }
            let mut rect: RECT = std::mem::zeroed();
            if GetClientRect(window, &mut rect) == 0 {
                return 1;
            }
            let area = i64::from(rect.right) * i64::from(rect.bottom);
            if area > search.area {
                search.best = window;
                search.area = area;
            }
        }
        1
    }
    if pids.is_empty() {
        return None;
    }
    let mut search = Search {
        pids,
        best: std::ptr::null_mut(),
        area: 0,
    };
    // SAFETY: обратный вызов получает указатель на локальную `search` только внутри вызова.
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
    (!search.best.is_null()).then_some(search.best)
}

/// Контекст памяти и DIB-холст; освобождаются при выходе.
struct Canvas {
    dc: HDC,
    bitmap: *mut core::ffi::c_void,
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: объекты созданы CreateCompatibleDC/CreateDIBSection и освобождаются один раз.
        unsafe {
            if !self.dc.is_null() {
                DeleteDC(self.dc);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
        }
    }
}

/// `CaptureClient(window, file)`: `PrintWindow` видит окно даже под полноэкранной игрой; если он
/// ничего не нарисовал, берётся `BitBlt` из DC окна.
fn capture_client(window: HWND, file: &Path) -> Result<Outcome, String> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: окно из EnumWindows, rect — локальная структура.
    if unsafe { GetClientRect(window, &mut rect) } == 0 || rect.right < 16 || rect.bottom < 16 {
        return Ok(Outcome::failed("окно LiveSplit слишком маленькое"));
    }
    let side = MAX_SIDE as i32;
    let (width, height) = (rect.right.min(side), rect.bottom.min(side));
    // SAFETY: структура из нулей допустима, поля задаются ниже.
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = -height;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `info` описывает 32-битный DIB сверху вниз; дескрипторы освобождает Canvas.
    let canvas = unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        Canvas {
            dc,
            bitmap: CreateDIBSection(
                dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            ),
        }
    };
    if canvas.dc.is_null() || canvas.bitmap.is_null() || bits.is_null() {
        return Ok(Outcome::failed("холст не создан"));
    }
    let total = (width * height) as usize;
    // SAFETY: CreateDIBSection отдал буфер width*height пикселей по 4 байта; живёт, пока жив Canvas.
    let pixels = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u32>(), total) };
    pixels.fill(SENTINEL);
    // SAFETY: DC и холст живы; окно из EnumWindows.
    let printed = unsafe {
        SelectObject(canvas.dc, canvas.bitmap);
        PrintWindow(window, canvas.dc, PW_CLIENTONLY | PW_RENDERFULLCONTENT) != 0
    };
    // PrintWindow не тронул картинку: 64 пробы равны SENTINEL.
    let untouched = (0..64).all(|i| pixels[total / 64 * i] == SENTINEL);
    if !printed || untouched {
        // SAFETY: DC окна берётся и отдаётся здесь же.
        let copied = unsafe {
            let source = GetDC(window);
            if source.is_null() {
                return Ok(Outcome::failed("нет DC окна"));
            }
            let copied = BitBlt(canvas.dc, 0, 0, width, height, source, 0, 0, SRCCOPY) != 0;
            ReleaseDC(window, source);
            copied
        };
        if !copied {
            return Ok(Outcome::failed("BitBlt не удался"));
        }
    }
    let mut header = [0u8; 16];
    header[..8].copy_from_slice(SNAPSHOT_MAGIC);
    header[8..12].copy_from_slice(&(width as u32).to_le_bytes());
    header[12..].copy_from_slice(&(height as u32).to_le_bytes());
    // Пиксели пишутся прямо из DIB, без копии холста: Windows всегда little-endian.
    // SAFETY: `pixels` как байты: тот же буфер DIB, `total * 4` байт, живёт, пока жив Canvas.
    let bytes = unsafe { std::slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), total * 4) };
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut out = std::fs::File::create(file)?;
        out.write_all(&header)?;
        out.write_all(bytes)
    };
    write().map_err(|error| error.to_string())?;
    Ok(Outcome::done(CODE_OK))
}
