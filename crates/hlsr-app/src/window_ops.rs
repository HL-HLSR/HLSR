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
    /// Место и размер ставит система (Linux): окно GPUI там не переносится (Wayland место окна
    /// клиенту не отдаёт), и `set_bounds` растил окно от его места, а не ставил в угол экрана.
    /// Кнопка шапки — развернуть окно (`xdg_toplevel.set_maximized`,
    /// `_NET_WM_STATE_MAXIMIZED_*`): рабочая область, панель задач видна, как на Windows; плеер
    /// «Демо» — полный экран монитора (`set_fullscreen`, `_NET_WM_STATE_FULLSCREEN`). На
    /// Windows — `false`: окно ставится на цель само.
    fn native(&self) -> bool {
        false
    }
    /// Попросить систему поставить окно `owner` на его место или вернуть.
    fn set_native(&mut self, _owner: Owner, _on: bool) {}
    /// Система держит окно на месте `owner`.
    fn native_on(&self, _owner: Owner) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy)]
struct Saved {
    owner: Owner,
    bounds: Bounds<Pixels>,
    zoom: f64,
    target: Bounds<Pixels>,
    /// Окно уже встало на цель: дальше любое расхождение — выход силами системы.
    reached: bool,
    /// Полный экран системы ([`FullscreenWindow::native`]).
    native: bool,
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
        let native = window.native();
        let target = match owner {
            Owner::Launcher => {
                // Рабочая область; у системы (Linux) — оценка, зум уточнится по ответу
                // композитора (`window_changed`).
                let area = window.work_area();
                window.set_zoom(fill(area.size));
                area
            }
            Owner::Player => window.screen(),
        };
        if native {
            window.set_native(owner, true);
        } else {
            window.set_bounds(target);
        }
        self.active = Some(Saved {
            owner,
            bounds,
            zoom,
            target,
            reached: false,
            native,
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
            let placed = if saved.native {
                window.native_on(saved.owner)
            } else {
                same(window.bounds(), saved.target)
            };
            if placed {
                // Развёрнутое системой окно — какого размера дал композитор: раскладка по нему.
                if saved.native && saved.owner == Owner::Launcher {
                    window.set_zoom(fill(window.bounds().size));
                }
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
    if saved.native {
        // Прежние место и размер вернёт композитор.
        window.set_native(saved.owner, false);
    } else {
        window.set_bounds(saved.bounds);
    }
    if saved.owner == Owner::Launcher {
        window.set_zoom(saved.zoom);
    }
}

/// `fillZoom` для окна размера `size` (логические px).
fn fill(size: gpui::Size<Pixels>) -> f64 {
    fill_zoom(Size::new(
        f32::from(size.width) as u32,
        f32::from(size.height) as u32,
    ))
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

    fn native(&self) -> bool {
        cfg!(not(windows))
    }

    fn set_native(&mut self, owner: Owner, on: bool) {
        // У GPUI только переключатели: по состоянию из последнего ответа композитора.
        if self.native_on(owner) == on {
            return;
        }
        match owner {
            Owner::Launcher => self.window.zoom_window(),
            Owner::Player => self.window.toggle_fullscreen(),
        }
    }

    fn native_on(&self, owner: Owner) -> bool {
        match owner {
            Owner::Launcher => self.window.is_maximized(),
            Owner::Player => self.window.is_fullscreen(),
        }
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
    let visible = shown(window);
    ops(cx).fullscreen = fullscreen;
    if let Some(owner) = gone {
        tracing::info!("[window] полный экран снят окном");
        emit(cx, owner.changed_event(), json!(false));
        // Сняли у видимого окна (стащили с развёрнутого, Win+↓) — выбор пользователя; свернуть и
        // спрятать в трей выбор не меняют.
        if owner == Owner::Launcher && visible {
            remember_fullscreen(cx, false);
        }
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

/// Файл в папке данных: полный экран кнопки шапки (F11) — следующий запуск откроется так же.
/// Окно открывается всегда по центру основного монитора (`center: true` 0.4.9), место не
/// помнится.
const STATE_FILE: &str = "window-state.json";

fn state_file(cx: &App) -> Option<std::path::PathBuf> {
    cx.try_global::<hlsr_gpui::shell::root::AppPaths>()
        .map(|paths| paths.0.file(STATE_FILE))
}

/// Был ли полный экран кнопки шапки включён при прошлом запуске.
fn remembered_fullscreen(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|state| state["fullscreen"] == json!(true))
}

/// Запомнить выбор пользователя: полный экран кнопки шапки включён или нет.
fn remember_fullscreen(cx: &mut App, on: bool) {
    let Some(path) = state_file(cx) else { return };
    let old = std::fs::read_to_string(&path).ok();
    if old.is_some_and(|text| remembered_fullscreen(&text) == on) {
        return;
    }
    let bytes = json!({"fullscreen": on}).to_string();
    if let Err(error) = hlsr_core::json_store::write_file_atomic(&path, bytes.as_bytes()) {
        tracing::warn!("[window] полный экран не запомнен: {error}");
    }
}

/// Запуск: окно было в полном экране кнопки шапки — вернуть, интерфейсу — новое состояние.
/// Со второго кадра окна: сначала окно встаёт по центру (KWin ставит окно, когда оно показано),
/// иначе развёрнутое до показа окно не знает своего обычного места и выход из полного экрана
/// оставлял его в углу рабочей области.
pub fn restore_fullscreen(cx: &mut App) {
    let on = state_file(cx)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|text| remembered_fullscreen(&text));
    let Some(handle) = launcher(cx).filter(|_| on) else {
        return;
    };
    let _ = handle.update(cx, |_, window, _| {
        window.on_next_frame(|window, _| {
            window.on_next_frame(|_, cx| {
                // Вне обновления окна: `set_fullscreen` само обновляет окно.
                cx.defer(|cx| {
                    if set_fullscreen(cx, Owner::Launcher, true) {
                        tracing::info!("[window] полный экран с прошлого запуска");
                        emit(cx, Owner::Launcher.changed_event(), json!(true));
                    }
                });
            });
        });
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
            let now = set_fullscreen(cx, Owner::Launcher, on);
            remember_fullscreen(cx, now);
            json!({"ok": true, "fullscreen": now})
        }
        Some("fullscreen-exit") => {
            let now = set_fullscreen(cx, Owner::Launcher, false);
            remember_fullscreen(cx, now);
            json!({"ok": true, "fullscreen": now})
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
    // сохраняется); полный экран на Linux — силами системы (`FullscreenWindow::native`).
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
    #[test]
    fn remembered_fullscreen_is_only_an_explicit_true() {
        use super::remembered_fullscreen;
        assert!(remembered_fullscreen(r#"{"fullscreen":true}"#));
        assert!(!remembered_fullscreen(r#"{"fullscreen":false}"#));
        assert!(!remembered_fullscreen(r#"{"fullscreen":"true"}"#));
        assert!(!remembered_fullscreen("{}"));
        assert!(!remembered_fullscreen("не json"));
    }

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

    /// Окно, чей полный экран ставит система (Linux): просьба — в журнал, состояние — когда
    /// «композитор ответил» ([`Native::configure`]).
    struct Native {
        inner: Fake,
        asked: Vec<bool>,
        on: bool,
    }

    impl Native {
        fn configure(&mut self) {
            if let Some(&on) = self.asked.last() {
                self.on = on;
                // Развёрнуто: рабочая область, которую знает композитор (панель — 40 px снизу).
                self.inner.bounds = if on {
                    rect(0., 0., 1920., 1040.)
                } else {
                    rect(360., 202., 1200., 676.)
                };
            }
        }
    }

    impl FullscreenWindow for Native {
        fn bounds(&self) -> Bounds<Pixels> {
            self.inner.bounds
        }
        fn work_area(&self) -> Bounds<Pixels> {
            self.inner.work_area()
        }
        fn screen(&self) -> Bounds<Pixels> {
            self.inner.screen()
        }
        fn zoom(&self) -> f64 {
            self.inner.zoom
        }
        fn shown(&self) -> bool {
            self.inner.shown
        }
        fn set_bounds(&mut self, bounds: Bounds<Pixels>) {
            self.inner.set_bounds(bounds);
        }
        fn set_zoom(&mut self, zoom: f64) {
            self.inner.zoom = zoom;
        }
        fn native(&self) -> bool {
            true
        }
        fn set_native(&mut self, _owner: Owner, on: bool) {
            if self.on != on {
                self.asked.push(on);
            }
        }
        fn native_on(&self, _owner: Owner) -> bool {
            self.on
        }
    }

    #[test]
    fn on_linux_the_compositor_maximizes_the_window_with_the_taskbar_visible() {
        let mut window = Native {
            inner: Fake::new(),
            asked: Vec::new(),
            on: false,
        };
        let mut fullscreen = Fullscreen::default();
        assert!(fullscreen.set(Owner::Launcher, true, &mut window));
        // Окно само не двигается и не растёт: просьба системе, раскладка — по рабочей области.
        assert!(window.inner.sets.is_empty());
        assert_eq!(window.asked, vec![true]);
        assert!((window.inner.zoom - 1040. / 900.).abs() < 1e-9);
        // Ответ композитора ещё не пришёл — это не выход.
        assert_eq!(fullscreen.window_changed(&mut window), None);
        // Пришёл: раскладка — по размеру, который дал композитор.
        window.configure();
        window.inner.bounds = rect(0., 0., 1920., 1032.);
        assert_eq!(fullscreen.window_changed(&mut window), None);
        assert!(fullscreen.active(Owner::Launcher));
        assert!((window.inner.zoom - 1032. / 900.).abs() < 1e-9);

        // Выход кнопкой: просьба системе, зум прежний; место вернёт композитор.
        assert!(!fullscreen.set(Owner::Launcher, false, &mut window));
        assert_eq!(window.asked, vec![true, false]);
        assert_eq!(window.inner.zoom, 0.75);
        assert!(window.inner.sets.is_empty());

        // Вход и выход силами системы (клавиша композитора): интерфейсу — `changed(false)`.
        window.configure();
        assert!(fullscreen.set(Owner::Launcher, true, &mut window));
        window.configure();
        assert_eq!(fullscreen.window_changed(&mut window), None);
        window.on = false;
        assert_eq!(
            fullscreen.window_changed(&mut window),
            Some(Owner::Launcher)
        );
        assert_eq!(window.inner.zoom, 0.75);
        // Система уже вышла — второй просьбы нет.
        assert_eq!(window.asked, vec![true, false, true]);
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
