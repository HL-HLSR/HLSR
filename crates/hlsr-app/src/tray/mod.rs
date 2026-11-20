//! Трей — `TrayController` 0.4.7: значок есть, только пока окно спрятано.
//! Подсказка `HLSR`, меню «Показать HLSR» / разделитель / «Выход» на языке `settings.json` в
//! момент создания значка, щелчок и двойной щелчок возвращают окно. Когда прятать окно во время
//! игры, решает трей платформы (`hlsr_platform::tray_controller`): он зовёт [`hide_for_game`] и
//! [`show_after_game`] через порт `TrayWindow` (`platform.rs`).
//!
//! Значок создаётся в потоке окна GPUI, когда цикл событий уже идёт (`tray-icon` получает свои
//! сообщения из того же цикла `GetMessage`); события значка приходят из любого потока и идут в
//! поток окна через [`hlsr_gpui::bridge::invoke`].

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

use crate::window_ops;
use gpui::{App, Global};
use hlsr_core::language::Language;
use hlsr_core::paths::Paths;
use hlsr_core::settings::Settings;
use hlsr_platform::tray_controller::TrayController as GameTray;
use hlsr_ui::tr;
use std::sync::Arc;

/// `icons/hlsrc.png` — значок трея 0.4.7 (`app.getAppPath()/icons/hlsrc.png`).
const ICON_PNG: &[u8] = include_bytes!("../../../../icons/hlsrc.png");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayLabels {
    pub tooltip: String,
    pub show: String,
    pub quit: String,
}

impl TrayLabels {
    pub fn for_language(lang: Language) -> TrayLabels {
        TrayLabels {
            tooltip: "HLSR".to_owned(),
            show: tr(lang, "#L_TRAY_SHOW", &[]),
            quit: tr(lang, "#L_TRAY_QUIT", &[]),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct TrayError(pub String);

/// Что пришло от значка. Обработчик вызывается в любом потоке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    Show,
    Quit,
}

pub type TrayEvents = Arc<dyn Fn(TrayEvent) + Send + Sync>;

/// Язык подписей меню: спрашивается при каждом создании значка.
pub type TrayLanguage = Box<dyn Fn() -> Language>;

/// `translate` трея 0.4.7 (`game-delivery/index.js:186-192`): `trayLabel(key, currentLocale())`,
/// где `currentLocale()` (`AppManager.js:424-435`) читает `settings.json` заново;
/// `config.language`, равный 1, — русский, иначе английский.
pub fn settings_language(paths: &Paths) -> Language {
    Settings::open(paths).language().unwrap_or(Language::En)
}

/// Значок в трее системы.
pub trait TrayBackend {
    fn create(&mut self, labels: TrayLabels) -> Result<(), TrayError>;
    fn destroy(&mut self);
}

/// Значок `hlsrc.png` в RGBA.
pub fn icon_rgba() -> Result<(Vec<u8>, u32, u32), TrayError> {
    let image = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
        .map_err(|e| TrayError(format!("иконка не прочитана: {e}")))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Ok((image.into_raw(), width, height))
}

/// Значок и признак «окно спрятано»; само окно прячут и показывают [`hide_for_game`] и
/// [`show_after_game`].
pub struct TrayController {
    language: TrayLanguage,
    backend: Box<dyn TrayBackend>,
    icon: bool,
    hidden: bool,
}

impl TrayController {
    pub fn new(language: TrayLanguage, backend: Box<dyn TrayBackend>) -> TrayController {
        TrayController {
            language,
            backend,
            icon: false,
            hidden: false,
        }
    }

    /// Значок для «спрятать»: `false` — значок не создан, окно остаётся на экране.
    pub fn hide_to_tray(&mut self) -> bool {
        if self.hidden {
            return true;
        }
        if !self.ensure_icon() {
            tracing::warn!("[tray] иконка не создана, окно остаётся на экране");
            return false;
        }
        self.hidden = true;
        true
    }

    /// Окно вернулось: значок снять.
    pub fn show(&mut self) {
        self.hidden = false;
        self.destroy_icon();
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    fn ensure_icon(&mut self) -> bool {
        if self.icon {
            return true;
        }
        // Меню строится заново с каждым значком (`ensureTray`): язык — на сейчас.
        match self
            .backend
            .create(TrayLabels::for_language((self.language)()))
        {
            Ok(()) => {
                self.icon = true;
                true
            }
            Err(e) => {
                tracing::warn!("[tray] иконка не создана: {e}");
                false
            }
        }
    }

    fn destroy_icon(&mut self) {
        if self.icon {
            self.backend.destroy();
            self.icon = false;
        }
    }
}

impl Drop for TrayController {
    fn drop(&mut self) {
        self.destroy_icon();
    }
}

/// Трей окна лаунчера (поток окна). «Показать HLSR» и щелчок по значку идут через трей платформы
/// (`TrayController.show()` 0.4.7 сбрасывает и его таймеры), если он есть.
struct Tray {
    controller: TrayController,
    game: Option<Arc<GameTray>>,
}

impl Global for Tray {}

/// Спрятать окно в трей; `false` — окна нет или значок не создан, окно осталось.
pub fn hide_for_game(cx: &mut App) -> bool {
    if !cx.has_global::<Tray>() {
        return false;
    }
    if is_hidden(cx) {
        return true;
    }
    if crate::window::launcher(cx).is_none() || !cx.global_mut::<Tray>().controller.hide_to_tray() {
        return false;
    }
    window_ops::hide(cx);
    true
}

/// Вернуть окно (развернуть, показать, фокус) и снять значок. Окно не открылось (Linux) — значок
/// остаётся: без него лаунчер не вернуть до перезапуска.
pub fn show_after_game(cx: &mut App) {
    window_ops::reveal(cx);
    if crate::window::launcher(cx).is_some() && cx.has_global::<Tray>() {
        cx.global_mut::<Tray>().controller.show();
    }
}

/// Снять значок, не показывая окна (`destroyTray`).
pub fn destroy_icon(cx: &mut App) {
    if cx.has_global::<Tray>() {
        cx.global_mut::<Tray>().controller.destroy_icon();
    }
}

/// Спрятано ли окно в трей.
pub fn is_hidden(cx: &App) -> bool {
    cx.try_global::<Tray>()
        .is_some_and(|tray| tray.controller.is_hidden())
}

gpui::actions!(hlsr_debug, [HideToTray]);

/// Поставить трей в потоке окна: язык меню — из `settings.json` в `paths`, «Показать HLSR» — через
/// трей платформы `game`. Значок снимается при выходе. F9 (отладка и dev-hooks) прячет окно.
pub fn install(cx: &mut App, paths: &Paths, game: Option<Arc<GameTray>>) {
    let paths = paths.clone();
    let language: TrayLanguage = Box::new(move || settings_language(&paths));
    let controller = TrayController::new(language, platform_backend(dispatch_events()));
    cx.set_global(Tray { controller, game });
    cx.on_app_quit(|cx| {
        destroy_icon(cx);
        async {}
    })
    .detach();
    if cfg!(any(debug_assertions, feature = "dev-hooks")) {
        cx.bind_keys([gpui::KeyBinding::new("f9", HideToTray, None)]);
        // Действие приходит из обработки клавиши, окно ещё занято — спрятать после неё.
        cx.on_action(|_: &HideToTray, cx| cx.defer(debug_hide_to_tray));
    }
}

/// F9 (`debug-hide-to-tray`, отладка и dev-hooks): спрятать окно в трей.
fn debug_hide_to_tray(cx: &mut App) {
    if is_hidden(cx) {
        return;
    }
    if hide_for_game(cx) {
        tracing::info!("[tray] F9: окно спрятано в трей");
    }
}

fn dispatch_events() -> TrayEvents {
    Arc::new(|event| {
        hlsr_gpui::bridge::invoke(move |cx| handle_event(event, cx));
    })
}

fn handle_event(event: TrayEvent, cx: &mut App) {
    match event {
        TrayEvent::Show => {
            let game = cx.try_global::<Tray>().and_then(|tray| tray.game.clone());
            match game {
                Some(game) => game.show(),
                None => show_after_game(cx),
            }
        }
        // `app.quit()` 0.4.7.
        TrayEvent::Quit => cx.quit(),
    }
}

#[cfg(target_os = "linux")]
fn platform_backend(events: TrayEvents) -> Box<dyn TrayBackend> {
    Box::new(linux::Ksni::new(events))
}

#[cfg(windows)]
fn platform_backend(events: TrayEvents) -> Box<dyn TrayBackend> {
    Box::new(windows::TrayIcon::new(events))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn platform_backend(_events: TrayEvents) -> Box<dyn TrayBackend> {
    struct NoTray;
    impl TrayBackend for NoTray {
        fn create(&mut self, _labels: TrayLabels) -> Result<(), TrayError> {
            Err(TrayError("трей не поддерживается".to_owned()))
        }
        fn destroy(&mut self) {}
    }
    Box::new(NoTray)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[derive(Default)]
    struct Counts {
        created: Cell<u32>,
        destroyed: Cell<u32>,
        labels: RefCell<Option<TrayLabels>>,
    }

    struct Stub {
        fail: bool,
        counts: Rc<Counts>,
    }

    impl TrayBackend for Stub {
        fn create(&mut self, labels: TrayLabels) -> Result<(), TrayError> {
            if self.fail {
                return Err(TrayError("нет StatusNotifierWatcher".to_owned()));
            }
            self.counts.created.set(self.counts.created.get() + 1);
            *self.counts.labels.borrow_mut() = Some(labels);
            Ok(())
        }
        fn destroy(&mut self) {
            self.counts.destroyed.set(self.counts.destroyed.get() + 1);
        }
    }

    fn controller_with(fail: bool, language: TrayLanguage) -> (TrayController, Rc<Counts>) {
        let counts = Rc::new(Counts::default());
        let stub = Stub {
            fail,
            counts: counts.clone(),
        };
        (TrayController::new(language, Box::new(stub)), counts)
    }

    fn controller(fail: bool) -> (TrayController, Rc<Counts>) {
        controller_with(fail, Box::new(|| Language::Ru))
    }

    fn english() -> TrayLabels {
        TrayLabels {
            tooltip: "HLSR".to_owned(),
            show: "Show HLSR".to_owned(),
            quit: "Quit".to_owned(),
        }
    }

    fn temp_paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        (dir, paths)
    }

    #[test]
    fn hide_without_icon_keeps_window() {
        let (mut tray, counts) = controller(true);
        assert!(!tray.hide_to_tray());
        assert!(!tray.is_hidden());
        assert_eq!(counts.created.get(), 0);
    }

    #[test]
    fn show_after_hide_removes_icon() {
        let (mut tray, counts) = controller(false);
        assert!(tray.hide_to_tray());
        assert!(tray.is_hidden());
        assert_eq!(counts.created.get(), 1);
        // Повторное «спрятать» не создаёт второй значок.
        assert!(tray.hide_to_tray());
        assert_eq!(counts.created.get(), 1);

        tray.show();
        assert!(!tray.is_hidden());
        assert_eq!(counts.destroyed.get(), 1);
        // Значка нет — снимать нечего.
        tray.show();
        assert_eq!(counts.destroyed.get(), 1);
    }

    #[test]
    fn icon_goes_away_with_the_controller() {
        let (mut tray, counts) = controller(false);
        tray.hide_to_tray();
        drop(tray);
        assert_eq!(counts.destroyed.get(), 1);
    }

    #[test]
    fn labels_follow_language() {
        let language = Rc::new(Cell::new(Language::En));
        let current = language.clone();
        let (mut tray, counts) = controller_with(false, Box::new(move || current.get()));
        tray.hide_to_tray();
        assert_eq!(counts.labels.borrow().clone(), Some(english()));
        // Язык сменили, пока значка нет: следующий значок — на новом языке.
        tray.show();
        language.set(Language::Ru);
        tray.hide_to_tray();
        assert_eq!(counts.created.get(), 2);
        assert_eq!(
            counts.labels.borrow().clone(),
            Some(TrayLabels::for_language(Language::Ru))
        );
        assert_eq!(TrayLabels::for_language(Language::Ru).show, "Показать HLSR");
        assert_eq!(TrayLabels::for_language(Language::Ru).quit, "Выход");
    }

    #[test]
    fn labels_read_settings_json_at_each_icon() {
        let (_dir, paths) = temp_paths();
        Settings::open(&paths).set_language(Language::En);
        let source = paths.clone();
        let (mut tray, counts) =
            controller_with(false, Box::new(move || settings_language(&source)));
        tray.hide_to_tray();
        assert_eq!(counts.labels.borrow().clone(), Some(english()));
        tray.show();
        // Окно настроек пишет язык в `settings.json`, трей читает его при следующем значке.
        Settings::open(&paths).set_language(Language::Ru);
        tray.hide_to_tray();
        assert_eq!(
            counts.labels.borrow().clone(),
            Some(TrayLabels::for_language(Language::Ru))
        );
    }

    #[test]
    fn settings_language_is_english_unless_russian() {
        let (_dir, paths) = temp_paths();
        let settings = paths.file("settings.json");
        std::fs::write(&settings, r#"{"config":{"language":1}}"#).unwrap();
        assert_eq!(settings_language(&paths), Language::Ru);
        std::fs::write(&settings, r#"{"config":{"language":"ru"}}"#).unwrap();
        assert_eq!(settings_language(&paths), Language::En);
        std::fs::write(&settings, r#"{"config":{"language":0}}"#).unwrap();
        assert_eq!(settings_language(&paths), Language::En);
    }

    #[test]
    fn tray_icon_decodes() {
        let (rgba, width, height) = icon_rgba().unwrap();
        assert!(width > 0 && height > 0);
        assert_eq!(rgba.len(), (width * height * 4) as usize);
    }
}
