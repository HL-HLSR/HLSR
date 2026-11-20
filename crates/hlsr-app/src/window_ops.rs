//! Окно лаунчера на GPUI: команды окна (`window-command` 0.4.9: полный экран, свернуть, закрыть,
//! вернуть), полный экран кнопки шапки (`createPlayerFullscreen` с `bounds` = рабочая область и
//! `fillZoom`, `AppManager.js`), спрятать и показать окно для трея, вывести окно вперёд (второй
//! запуск, `game-crash-report`), события окна интерфейсу (`launcher-fullscreen-changed`,
//! `window-visibility`).
//!
//! Размер и место окна Win32 меняются после текущей работы GPUI (как его `resize`): сообщения
//! `WM_SIZE`/`WM_MOVE` GPUI принимает только вне своего обновления.

use crate::window::launcher;
use gpui::{App, Bounds, Global, Pixels, Window, WindowHandle};
use hlsr_core::window_size::{Size, fill_zoom};
use hlsr_gpui::app::LauncherRoot;
use hlsr_gpui::scale;
use hlsr_ui::services::ServiceEvent;
use hlsr_ui::services::router::ChannelRouter;
use serde_json::{Value, json};
use std::sync::Arc;

/// Расхождение места и размера окна с целью полного экрана, которое ещё не выход: Windows
/// округляет до px устройства при масштабе экрана.
const SAME_BOUNDS_PX: f32 = 2.;

/// Кто держит полный экран окна (`win.__hlsrFullscreen`): кнопка шапки или плеер «Демо».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// Кнопка «Полный экран» (F11): окно на рабочую область, раскладка — `fillZoom`.
    Launcher,
    /// Плеер «Демо» (`demo-player-fullscreen`, DM6): окно на весь монитор, зум прежний.
    #[cfg_attr(not(test), allow(dead_code))]
    Player,
}

impl Owner {
    /// Событие интерфейсу, когда полный экран снят не по его команде (`changed(false)`).
    fn changed_event(self) -> &'static str {
        match self {
            Owner::Launcher => "launcher-fullscreen-changed",
            Owner::Player => hlsr_demo::api::PLAYER_FULLSCREEN_CHANGED,
        }
    }
}

/// Окно для полного экрана; место и размер — логические px.
pub trait FullscreenWindow {
    fn bounds(&self) -> Bounds<Pixels>;
    /// Рабочая область монитора окна (`getDisplayMatching(...).workArea`).
    fn work_area(&self) -> Bounds<Pixels>;
    /// Весь монитор окна.
    fn screen(&self) -> Bounds<Pixels>;
    /// Зум страницы (rem / 16).
    fn zoom(&self) -> f64;
    /// Видно и не свёрнуто.
    fn shown(&self) -> bool;
    fn set_bounds(&mut self, bounds: Bounds<Pixels>);
    fn set_zoom(&mut self, zoom: f64);
}

#[derive(Debug, Clone, Copy)]
struct Saved {
    owner: Owner,
    bounds: Bounds<Pixels>,
    zoom: f64,
    target: Bounds<Pixels>,
    /// Окно уже встало на цель: дальше любое расхождение — выход силами системы.
    reached: bool,
}

/// Полный экран окна (`createPlayerFullscreen` 0.4.9, один владелец на окно). Состояние своё:
/// окно прозрачное и без рамки, «настоящего» полного экрана у него нет.
#[derive(Debug, Default)]
pub struct Fullscreen {
    active: Option<Saved>,
    /// Выход в свёрнутом или спрятанном окне ждёт показа: Windows иначе не вернёт размер.
    pending: Option<Saved>,
}

impl Fullscreen {
    pub fn owner(&self) -> Option<Owner> {
        self.active.map(|saved| saved.owner)
    }

    pub fn active(&self, owner: Owner) -> bool {
        self.owner() == Some(owner)
    }

    /// `set(win, on)`: ответ — в полном экране ли окно у `owner` после команды. Полный экран у
    /// другого владельца не трогается, ответ — `on` (окно и так на весь экран).
    pub fn set(&mut self, owner: Owner, on: bool, window: &mut impl FullscreenWindow) -> bool {
        if self.owner().is_some_and(|current| current != owner) {
            return on;
        }
        if on {
            self.enter(owner, window);
        } else {
            self.leave(window);
        }
        self.active(owner)
    }

    fn enter(&mut self, owner: Owner, window: &mut impl FullscreenWindow) {
        if self.active.is_some() {
            return;
        }
        // Отложенный выход не дождался показа: прежние размер и зум — его.
        let (bounds, zoom) = match self.pending.take() {
            Some(saved) => (saved.bounds, saved.zoom),
            None => (window.bounds(), window.zoom()),
        };
        let target = match owner {
            Owner::Launcher => {
                let area = window.work_area();
                window.set_zoom(fill_zoom(Size::new(
                    f32::from(area.size.width) as u32,
                    f32::from(area.size.height) as u32,
                )));
                area
            }
            Owner::Player => window.screen(),
        };
        window.set_bounds(target);
        self.active = Some(Saved {
            owner,
            bounds,
            zoom,
            target,
            reached: false,
        });
    }

    fn leave(&mut self, window: &mut impl FullscreenWindow) {
        let Some(saved) = self.active.take() else {
            return;
        };
        if window.shown() {
            restore(&saved, window);
        } else {
            self.pending = Some(saved);
        }
    }

    /// Окно сменило место или размер, свернулось, спряталось или вернулось. Ответ — чей полный
    /// экран сняла система (свернули, спрятали в трей, Win+стрелки, перетащили): интерфейсу —
    /// `changed(false)`.
    pub fn window_changed(&mut self, window: &mut impl FullscreenWindow) -> Option<Owner> {
        if window.shown()
            && let Some(saved) = self.pending.take()
        {
            restore(&saved, window);
        }
        let saved = self.active.as_mut()?;
        if window.shown() {
            if same(window.bounds(), saved.target) {
                saved.reached = true;
                return None;
            }
            // Окно ещё идёт к цели (место и размер приходят разными сообщениями).
            if !saved.reached {
                return None;
            }
        }
        let owner = saved.owner;
        self.leave(window);
        Some(owner)
    }
}

fn restore(saved: &Saved, window: &mut impl FullscreenWindow) {
    window.set_bounds(saved.bounds);
    if saved.owner == Owner::Launcher {
        window.set_zoom(saved.zoom);
    }
}

fn same(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    let close = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() <= SAME_BOUNDS_PX;
    close(a.origin.x, b.origin.x)
        && close(a.origin.y, b.origin.y)
        && close(a.size.width, b.size.width)
        && close(a.size.height, b.size.height)
}

/// Полный экран и канал событий окна (поток окна).
struct Ops {
    fullscreen: Fullscreen,
    events: Option<Arc<ChannelRouter>>,
}

impl Global for Ops {}

fn ops(cx: &mut App) -> &mut Ops {
    if !cx.has_global::<Ops>() {
        cx.set_global(Ops {
            fullscreen: Fullscreen::default(),
            events: None,
        });
    }
    cx.global_mut::<Ops>()
}

/// Куда слать события окна интерфейсу.
pub fn set_events(cx: &mut App, router: Arc<ChannelRouter>) {
    ops(cx).events = Some(router);
}

/// `webContents.send(channel, payload)`.
pub fn emit(cx: &mut App, channel: &str, payload: Value) {
    if let Some(events) = &ops(cx).events {
        events.emit(ServiceEvent {
            channel: channel.to_owned(),
            payload,
        });
    }
}

/// Окно GPUI для полного экрана.
struct GpuiWindow<'a> {
    window: &'a mut Window,
    cx: &'a App,
}

impl FullscreenWindow for GpuiWindow<'_> {
    fn bounds(&self) -> Bounds<Pixels> {
        self.window.bounds()
    }

    fn work_area(&self) -> Bounds<Pixels> {
        #[cfg(windows)]
        if let Some(display) = self.window.display(self.cx) {
            return display.visible_bounds();
        }
        // shortcut: Linux — рабочая область основного монитора, а не монитора окна; V4.
        crate::window::work_area(self.cx)
    }

    fn screen(&self) -> Bounds<Pixels> {
        self.window
            .display(self.cx)
            .map_or_else(|| self.window.bounds(), |display| display.bounds())
    }

    fn zoom(&self) -> f64 {
        f64::from(scale::zoom(self.window))
    }

    fn shown(&self) -> bool {
        shown(self.window)
    }

    fn set_bounds(&mut self, bounds: Bounds<Pixels>) {
        set_bounds(self.window, self.cx, bounds);
    }

    fn set_zoom(&mut self, zoom: f64) {
        self.window.set_rem_size(scale::rem_size(zoom));
        self.window.refresh();
    }
}

/// `isVisible() && !isMinimized()` окна.
pub fn shown(window: &Window) -> bool {
    #[cfg(windows)]
    {
        hlsr_gpui::win32::shown(window)
    }
    #[cfg(not(windows))]
    {
        window.is_visible()
    }
}

/// Полный экран окна у `owner`: `on` — войти, иначе выйти. Ответ — как у `set`.
pub fn set_fullscreen(cx: &mut App, owner: Owner, on: bool) -> bool {
    let Some(handle) = launcher(cx) else {
        return false;
    };
    let mut fullscreen = std::mem::take(&mut ops(cx).fullscreen);
    let applied = handle
        .update(cx, |_, window, cx| {
            fullscreen.set(owner, on, &mut GpuiWindow { window, cx })
        })
        .unwrap_or(false);
    ops(cx).fullscreen = fullscreen;
    applied
}

fn fullscreen_owner(cx: &mut App) -> Option<Owner> {
    ops(cx).fullscreen.owner()
}

/// Окно сменило место, размер или видимость: полный экран, снятый системой, — интерфейсу.
fn window_changed(window: &mut Window, cx: &mut App) {
    let mut fullscreen = std::mem::take(&mut ops(cx).fullscreen);
    let gone = fullscreen.window_changed(&mut GpuiWindow { window, cx });
    ops(cx).fullscreen = fullscreen;
    if let Some(owner) = gone {
        tracing::info!("[window] полный экран снят окном");
        emit(cx, owner.changed_event(), json!(false));
    }
}

/// Следить за местом, размером и видимостью окна (выход из полного экрана силами системы).
pub fn watch(handle: WindowHandle<LauncherRoot>, cx: &mut App) {
    let _ = handle.update(cx, |_, window, cx| {
        cx.observe_window_bounds(window, |_, window, cx| window_changed(window, cx))
            .detach();
        window
            .observe_window_visibility(|_, window, cx| window_changed(window, cx))
            .detach();
    });
}

/// `ui-scale-apply` 0.4.9: новый размер — из обычного окна. Полный экран кнопки снимается
/// (`launcher-fullscreen-changed` false); у плеера «Демо» — `false`: масштаб применится при
/// следующем запуске (`FULLSCREEN`).
pub fn leave_for_ui_scale(cx: &mut App) -> bool {
    match fullscreen_owner(cx) {
        Some(Owner::Player) => false,
        Some(Owner::Launcher) => {
            set_fullscreen(cx, Owner::Launcher, false);
            emit(cx, Owner::Launcher.changed_event(), json!(false));
            true
        }
        None => true,
    }
}

/// `window-command` 0.4.9 (`AppManager.js`). Звать в потоке окна.
pub fn window_command(cx: &mut App, command: Option<&Value>) -> Value {
    if launcher(cx).is_none() {
        return json!({"ok": false, "code": "NO_WINDOW"});
    }
    match command.and_then(Value::as_str) {
        // Ответ — состояние после команды (true — окно на весь экран).
        Some("fullscreen") => {
            // Окно на весь экран у плеера «Демо»: кнопка шапки им не владеет и ничего не меняет.
            if fullscreen_owner(cx) == Some(Owner::Player) {
                return json!({"ok": true, "fullscreen": false});
            }
            let on = fullscreen_owner(cx).is_none();
            json!({"ok": true, "fullscreen": set_fullscreen(cx, Owner::Launcher, on)})
        }
        Some("fullscreen-exit") => {
            json!({"ok": true, "fullscreen": set_fullscreen(cx, Owner::Launcher, false)})
        }
        Some("fullscreen-state") => {
            json!({"ok": true, "fullscreen": fullscreen_owner(cx) == Some(Owner::Launcher)})
        }
        Some("minimize") => {
            crate::visibility::Visibility::set_ui_visible(cx, false);
            if let Some(handle) = launcher(cx) {
                let _ = handle.update(cx, |_, window, _| window.minimize_window());
            }
            json!({"ok": true})
        }
        // `window.close()` — окно одно, его закрытие — выход (`window-all-closed`).
        Some("close") => {
            cx.quit();
            json!({"ok": true})
        }
        Some("restore") => {
            reveal(cx);
            json!({"ok": true})
        }
        _ => {
            let shown = command.map_or_else(|| "undefined".to_owned(), hlsr_core::js::to_string);
            tracing::warn!("[window] отклонена неизвестная команда окна: {shown}");
            json!({"ok": false, "code": "UNKNOWN_COMMAND"})
        }
    }
}

/// `revealWindow` 0.4.7 (второй запуск, «Показать HLSR», вход в профиль): развернуть, показать,
/// отдать фокус.
pub fn reveal(cx: &mut App) {
    #[cfg(not(windows))]
    {
        if linux::reopen(cx) {
            return;
        }
    }
    let Some(handle) = launcher(cx) else { return };
    let _ = handle.update(cx, |_, window, cx| {
        #[cfg(windows)]
        win32::after(window, cx, win32::reveal);
        #[cfg(not(windows))]
        {
            let _ = cx;
            window.activate_window();
        }
    });
}

/// Отчёт о сбое игры (`game-crash-report` 0.4.9): свёрнутое окно развернуть, видимое — вперёд;
/// спрятанное в трей покажет трей после выхода игры.
pub fn bring_forward(cx: &mut App) {
    let Some(handle) = launcher(cx) else { return };
    let _ = handle.update(cx, |_, window, cx| {
        #[cfg(windows)]
        win32::after(window, cx, win32::bring_forward);
        #[cfg(not(windows))]
        {
            let _ = cx;
            window.activate_window();
        }
    });
}

/// Спрятать окно в трей (`window.hide()`).
pub fn hide(cx: &mut App) {
    #[cfg(windows)]
    {
        let Some(handle) = launcher(cx) else { return };
        let _ = handle.update(cx, |_, window, cx| win32::after(window, cx, win32::hide));
    }
    #[cfg(not(windows))]
    linux::close(cx);
}

/// Место и размер окна (логические px, клиентская область): после текущей работы GPUI.
pub fn set_bounds(window: &mut Window, cx: &App, bounds: Bounds<Pixels>) {
    #[cfg(windows)]
    win32::set_bounds(window, cx, bounds);
    // shortcut: у GPUI на Linux нет переноса окна — только размер (центр при смене масштаба не
    // сохраняется, полный экран не встаёт в угол рабочей области); проверить на Linux (V4).
    #[cfg(not(windows))]
    {
        let _ = cx;
        window.resize(bounds.size);
    }
}

#[cfg(windows)]
mod win32 {
    use gpui::{App, Bounds, Pixels, Window};
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetWindowRect, IsIconic, IsWindowVisible, SW_HIDE, SW_RESTORE, SW_SHOW,
        SWP_NOACTIVATE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos, ShowWindow,
    };

    /// Win32-работа с окном после текущей работы GPUI.
    pub fn after(window: &Window, cx: &App, work: fn(HWND)) {
        let Some(hwnd) = hlsr_gpui::win32::hwnd(window) else {
            return;
        };
        cx.foreground_executor()
            .spawn(async move { work(hwnd as HWND) })
            .detach();
    }

    pub fn hide(hwnd: HWND) {
        // SAFETY: HWND окна этого потока; GPUI уничтожает окно тоже в этом потоке.
        unsafe { ShowWindow(hwnd, SW_HIDE) };
    }

    pub fn reveal(hwnd: HWND) {
        // SAFETY: HWND окна этого потока.
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
    }

    pub fn bring_forward(hwnd: HWND) {
        // SAFETY: HWND окна этого потока.
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            if IsWindowVisible(hwnd) != 0 {
                SetForegroundWindow(hwnd);
            }
        }
    }

    /// `bounds` — клиентская область (как `window.bounds()` GPUI); рамка окна (невидимые края
    /// DWM слева, справа и снизу) добавляется по текущему окну.
    pub fn set_bounds(window: &Window, cx: &App, bounds: Bounds<Pixels>) {
        let Some(hwnd) = hlsr_gpui::win32::hwnd(window) else {
            return;
        };
        let scale = window.scale_factor();
        let device = |v: Pixels| (f32::from(v) * scale).round() as i32;
        let (x, y) = (device(bounds.origin.x), device(bounds.origin.y));
        let (width, height) = (device(bounds.size.width), device(bounds.size.height));
        cx.foreground_executor()
            .spawn(async move {
                let hwnd = hwnd as HWND;
                let (mut outer, mut inner) = (RECT::default(), RECT::default());
                let mut origin = POINT::default();
                // SAFETY: HWND окна этого потока; структуры на стеке на время вызовов; порядок окон
                // не меняется.
                let ok = unsafe {
                    GetWindowRect(hwnd, &mut outer);
                    GetClientRect(hwnd, &mut inner);
                    ClientToScreen(hwnd, &mut origin);
                    let (left, top) = (origin.x - outer.left, origin.y - outer.top);
                    let right = outer.right - (origin.x + inner.right);
                    let bottom = outer.bottom - (origin.y + inner.bottom);
                    SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        x - left,
                        y - top,
                        width + left + right,
                        height + top + bottom,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
                if ok == 0 {
                    tracing::warn!(
                        "[window] место окна не задано: {}",
                        std::io::Error::last_os_error()
                    );
                }
            })
            .detach();
    }
}

/// Linux: поверхность Wayland не спрятать — окно закрывается, корень (`Entity`) живёт и
/// открывается в новом окне.
#[cfg(not(windows))]
mod linux {
    use crate::visibility::Visibility;
    use gpui::{App, Bounds, Entity, Global, Pixels};
    use hlsr_gpui::app::LauncherRoot;

    /// Корень, место и rem спрятанного окна.
    struct Closed(Entity<LauncherRoot>, Bounds<Pixels>, Pixels);

    impl Global for Closed {}

    // shortcut: закрыть и открыть окно — только проверка типов, живьём не проверено; V4.
    pub fn close(cx: &mut App) {
        let Some(handle) = crate::window::launcher(cx) else {
            return;
        };
        let Ok(root) = handle.entity(cx) else { return };
        let Ok((bounds, rem)) = handle.update(cx, |_, window, _| {
            let placed = (window.bounds(), window.rem_size());
            window.remove_window();
            placed
        }) else {
            return;
        };
        cx.set_global(Closed(root, bounds, rem));
        crate::window::forget_launcher(cx);
        Visibility::closed_window(cx);
    }

    /// Открыть окно спрятанного корня; `false` — окно не пряталось или не открылось (корень
    /// остаётся спрятанным, значок трея попробует ещё раз).
    pub fn reopen(cx: &mut App) -> bool {
        if !cx.has_global::<Closed>() {
            return false;
        }
        let Closed(root, bounds, rem) = cx.remove_global::<Closed>();
        match crate::window::reopen_launcher(cx, root.clone(), bounds, rem) {
            Ok(handle) => {
                if let Some(visibility) = cx.try_global::<Visibility>().cloned() {
                    visibility.watch(handle, cx);
                }
                super::watch(handle, cx);
                true
            }
            Err(error) => {
                tracing::warn!("[window] окно не открыто: {error}");
                cx.set_global(Closed(root, bounds, rem));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    /// Окно-подделка: место и размер ставятся сразу, `sets` — журнал `set_bounds`.
    #[derive(Debug)]
    struct Fake {
        bounds: Bounds<Pixels>,
        zoom: f64,
        shown: bool,
        sets: Vec<Bounds<Pixels>>,
    }

    fn lp(v: f32) -> Pixels {
        Pixels::from(v)
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(lp(x), lp(y)), size(lp(w), lp(h)))
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                bounds: rect(360., 202., 1200., 676.),
                zoom: 0.75,
                shown: true,
                sets: Vec::new(),
            }
        }
    }

    impl FullscreenWindow for Fake {
        fn bounds(&self) -> Bounds<Pixels> {
            self.bounds
        }
        fn work_area(&self) -> Bounds<Pixels> {
            rect(0., 0., 1920., 1040.)
        }
        fn screen(&self) -> Bounds<Pixels> {
            rect(0., 0., 1920., 1080.)
        }
        fn zoom(&self) -> f64 {
            self.zoom
        }
        fn shown(&self) -> bool {
            self.shown
        }
        fn set_bounds(&mut self, bounds: Bounds<Pixels>) {
            self.sets.push(bounds);
            self.bounds = bounds;
        }
        fn set_zoom(&mut self, zoom: f64) {
            self.zoom = zoom;
        }
    }

    #[test]
    fn the_header_button_fills_the_work_area_and_comes_back() {
        let mut window = Fake::new();
        let mut fullscreen = Fullscreen::default();
        assert!(fullscreen.set(Owner::Launcher, true, &mut window));
        // Рабочая область (панель задач видна), раскладка 1600×900 растянута: min(1,2; 1,1556).
        assert_eq!(window.bounds, rect(0., 0., 1920., 1040.));
        assert!((window.zoom - 1040. / 900.).abs() < 1e-9);
        assert!(fullscreen.active(Owner::Launcher));
        // Второй вход ничего не меняет.
        assert!(fullscreen.set(Owner::Launcher, true, &mut window));
        assert_eq!(window.sets.len(), 1);

        assert!(!fullscreen.set(Owner::Launcher, false, &mut window));
        assert_eq!(window.bounds, rect(360., 202., 1200., 676.));
        assert_eq!(window.zoom, 0.75);
        assert_eq!(fullscreen.owner(), None);
        // Выход без полного экрана — ничего.
        assert!(!fullscreen.set(Owner::Launcher, false, &mut window));
        assert_eq!(window.sets.len(), 2);
    }

    #[test]
    fn one_owner_at_a_time() {
        let mut window = Fake::new();
        let mut fullscreen = Fullscreen::default();
        assert!(fullscreen.set(Owner::Player, true, &mut window));
        // Плеер — на весь монитор, зум прежний.
        assert_eq!(window.bounds, rect(0., 0., 1920., 1080.));
        assert_eq!(window.zoom, 0.75);
        // Кнопка не трогает чужой полный экран: «применено» = то, о чём просили.
        assert!(fullscreen.set(Owner::Launcher, true, &mut window));
        assert!(!fullscreen.set(Owner::Launcher, false, &mut window));
        assert!(fullscreen.active(Owner::Player));
        assert_eq!(window.sets.len(), 1);
        assert!(!fullscreen.set(Owner::Player, false, &mut window));
        assert_eq!(window.bounds, rect(360., 202., 1200., 676.));
    }

    #[test]
    fn the_system_takes_fullscreen_away_by_moving_the_window() {
        let mut window = Fake::new();
        let mut fullscreen = Fullscreen::default();
        fullscreen.set(Owner::Launcher, true, &mut window);
        // До цели окно идёт по шагам: размер уже новый, место — старое. Это не выход.
        window.bounds = rect(360., 202., 1920., 1040.);
        assert_eq!(fullscreen.window_changed(&mut window), None);
        // Встало на цель (округление на 1 px — то же место).
        window.bounds = rect(0., 1., 1920., 1040.);
        assert_eq!(fullscreen.window_changed(&mut window), None);
        assert!(fullscreen.active(Owner::Launcher));
        // Win+Shift+стрелка или перетаскивание: окно ушло — полного экрана нет, интерфейсу false.
        window.bounds = rect(1920., 0., 1920., 1040.);
        assert_eq!(
            fullscreen.window_changed(&mut window),
            Some(Owner::Launcher)
        );
        assert_eq!(window.bounds, rect(360., 202., 1200., 676.));
        assert_eq!(window.zoom, 0.75);
        assert_eq!(fullscreen.window_changed(&mut window), None);
    }

    #[test]
    fn minimized_window_leaves_fullscreen_when_shown_again() {
        let mut window = Fake::new();
        let mut fullscreen = Fullscreen::default();
        fullscreen.set(Owner::Launcher, true, &mut window);
        // Свернули (или спрятали в трей): интерфейс узнаёт сразу, размер — потом.
        window.shown = false;
        assert_eq!(
            fullscreen.window_changed(&mut window),
            Some(Owner::Launcher)
        );
        assert_eq!(fullscreen.owner(), None);
        assert_eq!(window.bounds, rect(0., 0., 1920., 1040.));
        assert_eq!(fullscreen.window_changed(&mut window), None);
        assert_eq!(window.sets.len(), 1);
        // Развернули — прежние размер и зум.
        window.shown = true;
        assert_eq!(fullscreen.window_changed(&mut window), None);
        assert_eq!(window.bounds, rect(360., 202., 1200., 676.));
        assert_eq!(window.zoom, 0.75);
    }

    #[test]
    fn command_exit_while_hidden_waits_too() {
        let mut window = Fake::new();
        let mut fullscreen = Fullscreen::default();
        fullscreen.set(Owner::Launcher, true, &mut window);
        window.shown = false;
        assert!(!fullscreen.set(Owner::Launcher, false, &mut window));
        assert_eq!(window.sets.len(), 1);
        // Вход до показа: прежний размер — тот, что был до первого входа.
        window.shown = true;
        fullscreen.set(Owner::Launcher, true, &mut window);
        fullscreen.set(Owner::Launcher, false, &mut window);
        assert_eq!(window.bounds, rect(360., 202., 1200., 676.));
        assert_eq!(window.zoom, 0.75);
    }
}
