//! Окно лаунчера на GPUI: размер и место как у 0.4.9 (`CreateWindow.js`, `window-size.js`:
//! рабочая область основного монитора → окно и зум страницы), масштаб интерфейса из
//! `settings.json` и `ui-scale-apply`, AppUserModelID (показ окна — `window_ops.rs`). Окна режимов —
//! размеры 0.4.7 ([`FixedWindow`]).

use gpui::{
    App, AppContext, Bounds, DisplayId, Entity, Global, Pixels, WindowHandle, point, px, size,
};
use hlsr_core::window_size::{Layout, Size};
use hlsr_gpui::app::LauncherRoot;
use hlsr_gpui::scale;
use hlsr_ui::services::Services;
use std::sync::atomic::{AtomicBool, Ordering};

/// Окно уже нарисовало первый кадр. Паника до этого — ошибка запуска.
static WINDOW_SHOWN: AtomicBool = AtomicBool::new(false);

pub fn window_shown() -> bool {
    WINDOW_SHOWN.load(Ordering::SeqCst)
}

/// Окно лаунчера (поток окна).
struct Launcher(WindowHandle<LauncherRoot>);

impl Global for Launcher {}

/// Рабочая область основного монитора в логических px (`screen.getPrimaryDisplay().workArea`).
pub fn work_area(cx: &App) -> Bounds<Pixels> {
    let Some(display) = cx.primary_display() else {
        return Bounds::new(point(px(0.), px(0.)), size(px(1600.), px(900.)));
    };
    #[cfg(windows)]
    {
        // `rcWork` монитора.
        display.visible_bounds()
    }
    #[cfg(not(windows))]
    {
        use crate::work_area::{WorkArea, of_monitor};
        let bounds = display.bounds();
        // shortcut: `_NET_WORKAREA` — в px X11, `bounds` GPUI — логические: при масштабе X11 ≠ 1
        // область неточна; проверить на Linux (V4).
        let area = of_monitor(WorkArea {
            x: f32::from(bounds.origin.x) as i32,
            y: f32::from(bounds.origin.y) as i32,
            width: f32::from(bounds.size.width) as u32,
            height: f32::from(bounds.size.height) as u32,
        });
        Bounds::new(
            point(px(area.x as f32), px(area.y as f32)),
            size(px(area.width as f32), px(area.height as f32)),
        )
    }
}

/// `getTargetSize` 0.4.9: окно и зум страницы для области `area` и масштаба `ui_scale`.
fn layout_for(area: Bounds<Pixels>, ui_scale: f64) -> Layout {
    let area = Size::new(
        f32::from(area.size.width) as u32,
        f32::from(area.size.height) as u32,
    );
    scale::window_layout(area, ui_scale)
}

fn window_size(layout: &Layout) -> gpui::Size<Pixels> {
    size(px(layout.width as f32), px(layout.height as f32))
}

/// Открыть окно лаунчера: без рамки, прозрачное; закрытие окна — выход (`window-all-closed`
/// 0.4.9). Всегда по центру рабочей области основного монитора (`center: true` 0.4.9); полный
/// экран прошлого запуска возвращает `window_ops::restore_fullscreen`.
pub fn open_launcher(cx: &mut App, ui_scale: f64, services: Services) -> Result<(), String> {
    let display = live_display(cx);
    let area = display.as_ref().map_or_else(|| work_area(cx), |d| d.1);
    let layout = layout_for(area, ui_scale);
    let bounds = hlsr_gpui::window::place(area, window_size(&layout), None);
    let display = display.map(|d| d.0);
    open_with(
        cx,
        bounds,
        display,
        scale::rem_size(layout.zoom_factor),
        |cx| cx.new(|cx| LauncherRoot::new(services, cx)),
    )?;
    tracing::info!(
        "[window] окно {}x{}, масштаб {} %, зум {}",
        layout.width,
        layout.height,
        (layout.ui_scale * 100.0).round(),
        layout.zoom_factor
    );
    Ok(())
}

/// Живая проверка (dev-hooks): `HLSR_LIVE_DISPLAY=x,y` — монитор с этой точкой (логические px,
/// например `-2560,0` у монитора слева) и его рабочая область. Окно сразу открывается там, не
/// мелькая на основном мониторе.
pub(crate) fn live_display(cx: &App) -> Option<(DisplayId, Bounds<Pixels>)> {
    let value = crate::modes::dev_env("HLSR_LIVE_DISPLAY")?;
    let at = value.split_once(',').and_then(|(x, y)| {
        Some(point(
            px(x.trim().parse::<f32>().ok()?),
            px(y.trim().parse::<f32>().ok()?),
        ))
    });
    let found = at.and_then(|at| {
        cx.displays()
            .into_iter()
            .find(|display| display.bounds().contains(&at))
    });
    match found {
        Some(display) => Some((display.id(), display.visible_bounds())),
        None => {
            tracing::warn!("[window] HLSR_LIVE_DISPLAY={value}: монитора с этой точкой нет");
            None
        }
    }
}

/// Окно лаунчера с корнем `root`: место и размер `bounds` (на мониторе `display`, без него — на
/// основном), rem `rem`.
fn open_with(
    cx: &mut App,
    bounds: Bounds<Pixels>,
    display: Option<DisplayId>,
    rem: Pixels,
    root: impl FnOnce(&mut App) -> Entity<LauncherRoot>,
) -> Result<WindowHandle<LauncherRoot>, String> {
    let options = gpui::WindowOptions {
        display_id: display,
        ..hlsr_gpui::window::launcher_options(bounds)
    };
    let handle = cx
        .open_window(options, |window, cx| {
            window.set_rem_size(rem);
            #[cfg(windows)]
            hlsr_gpui::win32::system_corners_off(window);
            // WM_CLOSE и Alt+F4 — тот же выход, что кнопка «Закрыть»: окно закрывает `quit`.
            // Разрешённое закрытие GPUI уничтожал окно сам, и выход потом обращался к уже
            // уничтоженному («Недопустимый дескриптор окна» в журнале).
            window.on_window_should_close(cx, |_, cx| {
                cx.quit();
                false
            });
            window.on_next_frame(|_, _| WINDOW_SHOWN.store(true, Ordering::SeqCst));
            root(cx)
        })
        .map_err(|error| format!("{error:#}"))?;
    cx.set_global(Launcher(handle));
    Ok(handle)
}

/// Linux, трей: окно закрыто, корень живёт — окна у лаунчера нет.
#[cfg(not(windows))]
pub fn forget_launcher(cx: &mut App) {
    if cx.has_global::<Launcher>() {
        cx.remove_global::<Launcher>();
    }
}

/// Linux, трей: новое окно с прежними корнем, местом и rem.
#[cfg(not(windows))]
pub fn reopen_launcher(
    cx: &mut App,
    root: Entity<LauncherRoot>,
    bounds: Bounds<Pixels>,
    rem: Pixels,
) -> Result<WindowHandle<LauncherRoot>, String> {
    open_with(cx, bounds, None, rem, move |_| root)
}

/// Окно лаунчера, если оно открыто.
pub fn launcher(cx: &App) -> Option<WindowHandle<LauncherRoot>> {
    cx.try_global::<Launcher>().map(|launcher| launcher.0)
}

/// `applyUiScale` 0.4.9: размер — по основному монитору, место — вокруг прежнего центра в пределах
/// рабочей области монитора окна; ответ — масштаб, который поместился. Окна нет — `None`.
pub fn apply_ui_scale(cx: &mut App, ui_scale: f64) -> Option<f64> {
    let handle = launcher(cx)?;
    let layout = layout_for(work_area(cx), ui_scale);
    handle
        .update(cx, |_, window, cx| {
            let current = window.bounds();
            let area = window
                .display(cx)
                .map_or(current, |display| display.visible_bounds());
            let size = window_size(&layout);
            let placed = hlsr_gpui::window::place(area, size, Some(current.center()));
            window.set_rem_size(scale::rem_size(layout.zoom_factor));
            crate::window_ops::set_bounds(window, cx, placed);
        })
        .ok()?;
    tracing::info!(
        "[ui-scale] масштаб {} %, окно {}x{}",
        (layout.ui_scale * 100.0).round(),
        layout.width,
        layout.height
    );
    Some(layout.ui_scale)
}

/// HWND окна лаунчера (кнопка панели задач).
#[cfg(windows)]
pub fn hwnd(cx: &mut App) -> Option<isize> {
    let handle = launcher(cx)?;
    handle
        .update(cx, |_, window, _| hlsr_gpui::win32::hwnd(window))
        .ok()
        .flatten()
}

/// Владелец системных диалогов (`dialog.show*(this.window, …)` 0.4.9): HWND лаунчера из потока
/// окна. Окна нет или не Windows — `None`, диалог без владельца.
pub async fn dialog_owner() -> Option<isize> {
    #[cfg(windows)]
    return hlsr_gpui::bridge::call(hwnd).await.flatten();
    #[cfg(not(windows))]
    None
}

/// Окно режима установщика или апдейтера (ПП6, задача 10): размер в DIP постоянный, место — по
/// центру рабочей области основного монитора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedWindow {
    /// `installer-window.js`: 640×560.
    Installer,
    /// `updater/main.js`: 460×280 вместе с рамкой ОС. Апдейтер — только Windows.
    #[cfg_attr(not(windows), allow(dead_code))]
    Updater,
}

impl FixedWindow {
    /// Размер окна в DIP.
    pub fn size(self) -> (u32, u32) {
        match self {
            FixedWindow::Installer => (640, 560),
            FixedWindow::Updater => (460, 280),
        }
    }

    /// Место и размер по центру рабочей области основного монитора.
    pub fn bounds(self, cx: &App) -> Bounds<Pixels> {
        let (width, height) = self.size();
        hlsr_gpui::window::place(
            work_area(cx),
            size(px(width as f32), px(height as f32)),
            None,
        )
    }
}

/// Windows: тот же AppUserModelID, что `app.setAppUserModelId` 0.4.7 —
/// закрепление на панели задач и уведомления. До создания окна.
#[cfg(windows)]
pub fn set_app_user_model_id() {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    let id = hlsr_core::win32::wide("com.hlsrteam.hlsr");
    // SAFETY: строка с нулём в конце живёт до конца вызова.
    let hr = unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
    if hr < 0 {
        tracing::warn!("[window] AppUserModelID не задан: 0x{hr:08x}");
    }
}

#[cfg(not(windows))]
pub fn set_app_user_model_id() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_windows_have_the_047_sizes() {
        // installer-window.js 640×560 и updater/main.js 460×280 (внешний размер).
        assert_eq!(FixedWindow::Installer.size(), (640, 560));
        assert_eq!(FixedWindow::Updater.size(), (460, 280));
    }

    #[test]
    fn layout_follows_window_size_js() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1040.)));
        let layout = layout_for(area, 1.0);
        assert_eq!((layout.width, layout.height), (1200, 676));
        // Масштаб, который не помещается, уменьшается.
        let small = Bounds::new(point(px(0.), px(0.)), size(px(1366.), px(728.)));
        let layout = layout_for(small, 1.5);
        assert!(layout.ui_scale < 1.5 && layout.width <= 1366 && layout.height <= 728);
    }
}
