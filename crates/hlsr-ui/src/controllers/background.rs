//! Фон лаунчера (`LauncherBackground.vue` 0.4.7, в 0.4.9 тот же; кадр эпизодов —
//! [`Selection::of_episodes`], «повтори цвета» — `memory_game`): какие кадры и передний план
//! показаны, когда прежние гаснут, кому принадлежат картинки, и параллакс по движению мыши.
//!
//! Кадр ведёт себя как `preload` компонента: пока новая картинка грузится, прежняя остаётся;
//! когда готова — слой нового кадра входит (opacity 0 → 1 за 500 мс), прежний уходит;
//! картинка не загрузилась или кадра нет — прежний уходит, остаётся запасной градиент. Передний
//! план — так же, с раскладкой, которая приходит вместе с ним. Картинки держит область
//! `games/<id>`; погасший слой её не освобождает: как кеш Chromium, повторный показ игры берёт
//! картинки сразу, лишнее вытесняет предел загрузчика. Вид рисует слои из
//! [`BackgroundController::frames`]/[`BackgroundController::fronts`]; адрес картинки в слое —
//! `bundle:<имя>` (`game_art`), её декодирует и уменьшает `ImageLoader` до окна.
//!
//! Параллакс — модель [`Parallax`](crate::parallax::Parallax) на кадрах окна вместо
//! `requestAnimationFrame`: пока модель просит кадр ([`BackgroundController::frame_pending`]), вид
//! зовёт [`BackgroundController::frame`] на каждом своём кадре (частоту кадров решает вид), и шаг
//! идёт по времени кадра; кадр не нужен — окно не перерисовывается. Движение мыши, фокус окна и
//! закрытие окон поверх приходят снаружи.

use crate::env::Env;
use crate::game_art::{self, FrontLayout, GameArt};
use crate::images::{Fit, ImageLoader, ImageRequest, ImageSource};
use crate::parallax::{self, Parallax, Vars};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// Слой кадра игры: `source` — адрес картинки (`bundle:…`), `scope` — кто её держит.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundLayer {
    pub key: i32,
    pub source: String,
    pub scope: String,
    /// `false` — слой уходит (гаснет), потом контроллер его убирает.
    pub alive: bool,
}

/// Слой переднего плана и его раскладка.
#[derive(Debug, Clone, PartialEq)]
pub struct FrontLayer {
    pub key: i32,
    pub source: String,
    pub scope: String,
    pub alive: bool,
    pub layout: FrontLayout,
}

/// Всё, что контроллеру нужно знать об окне и режиме.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackgroundInputs {
    /// Поверх открыто окно (paused): фон замирает.
    pub paused: bool,
    /// Окно в фокусе (`document.hasFocus()`).
    pub focused: bool,
    /// Окно показано (window-visibility).
    pub visible: bool,
    /// Параллакс разрешён: prefs «анимации» и не reduced-motion.
    pub moving: bool,
    /// Логические px.
    pub width: f64,
    pub height: f64,
}

/// Область (`scope`) картинок игры.
fn scope_of(game: &str) -> String {
    format!("games/{game}")
}

/// Адрес картинки арта в разметке.
fn source_of(name: &str) -> String {
    format!("bundle:{name}")
}

/// Что показывать: кадр и передний план игры; `None` — ничего (разделы без фона игры).
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// Область картинок.
    pub scope: String,
    pub hero: &'static str,
    pub front: Option<(&'static str, FrontLayout)>,
}

impl Selection {
    /// Арт игры по идентификатору лаунчера.
    pub fn of_game(game: &str) -> Option<Selection> {
        let art = GameArt::of(game)?;
        Some(Selection {
            scope: scope_of(game),
            hero: art.hero,
            front: art.front.map(|front| (front, art.layout)),
        })
    }

    /// Выбран эпизод (`currentEpisodeId` 0.4.9): свой кадр без переднего плана; у игры без кадра
    /// эпизодов — обычный арт.
    pub fn of_episodes(game: &str) -> Option<Selection> {
        let art = GameArt::of(game)?;
        match art.episodes_hero {
            Some(hero) => Some(Selection {
                scope: scope_of(game),
                hero,
                front: None,
            }),
            None => Selection::of_game(game),
        }
    }
}

struct Shown {
    /// Адрес кадра, который виден (или уходит последним показанным).
    hero: Option<String>,
    front: Option<String>,
    /// Что нужно (`src`, `foreground`).
    wanted_hero: Option<String>,
    wanted_front: Option<String>,
    /// Поколение: загрузка прежнего желания результата не даёт (`release()` 0.4.7).
    hero_generation: u64,
    front_generation: u64,
    next_key: i32,
}

struct Inner {
    env: Rc<dyn Env>,
    loader: ImageLoader,
    frames: RefCell<Vec<BackgroundLayer>>,
    fronts: RefCell<Vec<FrontLayer>>,
    tool: Cell<bool>,
    /// Размер окна в физических px (область картинок); до первого — 1600 × 900.
    window_px: Cell<Option<(f64, f64)>>,
    /// Длительность угасания слоя (`--l-fade`), мс; без анимаций — 0.
    fade_ms: Cell<u64>,
    shown: RefCell<Shown>,
    parallax: RefCell<Option<Parallax>>,
    /// Шагов параллакса с запуска (тесты, замер плавности).
    steps: Cell<u64>,
    /// Поколение уборки: новая отодвигает прежнюю.
    cleanup_generation: Cell<u64>,
    on_change: RefCell<Option<Rc<dyn Fn()>>>,
}

/// Контроллер фона; клон — тот же контроллер.
#[derive(Clone)]
pub struct BackgroundController {
    inner: Rc<Inner>,
}

impl BackgroundController {
    /// Фон; картинки арта — `game_art::register_bundles()`, сроки угасания — по часам `env`.
    pub fn new(env: Rc<dyn Env>, loader: ImageLoader) -> BackgroundController {
        BackgroundController {
            inner: Rc::new(Inner {
                env,
                loader,
                frames: RefCell::default(),
                fronts: RefCell::default(),
                tool: Cell::new(false),
                window_px: Cell::new(None),
                fade_ms: Cell::new(0),
                shown: RefCell::new(Shown {
                    hero: None,
                    front: None,
                    wanted_hero: None,
                    wanted_front: None,
                    hero_generation: 0,
                    front_generation: 0,
                    next_key: 1,
                }),
                parallax: RefCell::new(None),
                steps: Cell::new(0),
                cleanup_generation: Cell::new(0),
                on_change: RefCell::default(),
            }),
        }
    }

    /// Кому сообщать о переменах (слои, параллакс, запрос кадра): вид перерисовывается.
    pub fn on_change(&self, listener: impl Fn() + 'static) {
        *self.inner.on_change.borrow_mut() = Some(Rc::new(listener));
    }

    fn changed(&self) {
        let listener = self.inner.on_change.borrow().clone();
        if let Some(listener) = listener {
            listener();
        }
    }

    /// Размер окна в физических px: под него уменьшаются картинки.
    pub fn set_window_px(&self, width: f64, height: f64) {
        self.inner
            .window_px
            .set((width > 0.0 && height > 0.0).then_some((width, height)));
    }

    /// Длительность угасания слоя (`--l-fade`, мс; без анимаций — 0).
    pub fn set_fade_ms(&self, ms: u64) {
        self.inner.fade_ms.set(ms);
    }

    /// Показать фон игры `game` (`backgroundSrc`, `foregroundSrc`); `None` или игра без арта —
    /// фона игры нет.
    pub fn show_game(&self, game: Option<&str>) {
        self.show(game.and_then(Selection::of_game));
    }

    /// То же по готовому выбору.
    pub fn show(&self, selection: Option<Selection>) {
        let hero = selection
            .as_ref()
            .map(|s| (source_of(s.hero), s.scope.clone()));
        let front = selection.as_ref().and_then(|s| {
            s.front
                .map(|(name, layout)| (source_of(name), s.scope.clone(), layout))
        });
        self.set_hero(hero);
        self.set_front(front);
    }

    /// `toolBackground`: фон инструмента вместо кадра игры.
    pub fn set_tool(&self, on: bool) {
        if self.inner.tool.replace(on) != on {
            self.changed();
        }
    }

    pub fn tool(&self) -> bool {
        self.inner.tool.get()
    }

    /// Слои кадра: живой последним, уходящие гаснут.
    pub fn frames(&self) -> Vec<BackgroundLayer> {
        self.inner.frames.borrow().clone()
    }

    /// Слои переднего плана.
    pub fn fronts(&self) -> Vec<FrontLayer> {
        self.inner.fronts.borrow().clone()
    }

    /// Переменные параллакса (`--l-bg-par-*`, `--l-bg-front-*`).
    pub fn vars(&self) -> Option<Vars> {
        self.inner.parallax.borrow().as_ref().map(Parallax::vars)
    }

    /// Параллакс просит кадр: вид зовёт [`BackgroundController::frame`] на следующем кадре окна.
    pub fn frame_pending(&self) -> bool {
        self.inner
            .parallax
            .borrow()
            .as_ref()
            .is_some_and(Parallax::frame_pending)
    }

    /// `mousemove` окна, логические px.
    pub fn pointer_moved(&self, x: f64, y: f64) {
        self.pointer(|model| model.pointer_moved(x, y));
    }

    /// Курсор ушёл из окна.
    pub fn pointer_left(&self) {
        self.pointer(Parallax::pointer_left);
    }

    /// Курсор меняет только цель параллакса: перемена объявляется, лишь когда он стоял и пошёл.
    /// Идущие кадры ведёт вид ([`BackgroundController::frame`]) со своим пределом частоты —
    /// перерисовка на каждый `mousemove` шла бы в такт монитора.
    fn pointer(&self, change: impl FnOnce(&mut Parallax)) {
        let started = (self.inner.parallax.borrow_mut().as_mut()).is_some_and(|model| {
            let idle = !model.frame_pending();
            change(model);
            idle && model.frame_pending()
        });
        if started {
            self.changed();
        }
    }

    /// Сколько шагов параллакса сделано с запуска (тесты, замер плавности).
    pub fn parallax_steps(&self) -> u64 {
        self.inner.steps.get()
    }

    /// Слои кадра и переднего плана, живые и гаснущие (тесты).
    pub fn layer_counts(&self) -> (usize, usize) {
        (
            self.inner.frames.borrow().len(),
            self.inner.fronts.borrow().len(),
        )
    }

    // ---- кадр ----

    /// `preload(value)`.
    fn set_hero(&self, wanted: Option<(String, String)>) {
        let inner = &self.inner;
        let source = wanted.as_ref().map(|(source, _)| source.clone());
        {
            // `release()`: прежняя загрузка результата не даст.
            let mut shown = inner.shown.borrow_mut();
            shown.hero_generation += 1;
            shown.wanted_hero = source.clone();
        }
        let Some((source, scope)) = wanted else {
            self.drop_hero();
            return;
        };
        if inner.shown.borrow().hero.as_deref() == Some(source.as_str()) {
            return;
        }
        let generation = inner.shown.borrow().hero_generation;
        let weak = Rc::downgrade(inner);
        let request = self.request_for(&source, &scope, None);
        let (shown_source, shown_scope) = (source.clone(), scope.clone());
        inner.loader.request(request, move |result| {
            let Some(inner) = weak.upgrade() else { return };
            let this = BackgroundController { inner };
            if this.inner.shown.borrow().hero_generation != generation {
                return;
            }
            match result {
                Ok(_) => this.show_hero(shown_source, shown_scope),
                Err(error) => {
                    tracing::warn!("[background] кадр {shown_source} не загружен: {error}");
                    this.drop_hero();
                }
            }
        });
    }

    /// Новый кадр готов: прежний уходит, новый входит.
    fn show_hero(&self, source: String, scope: String) {
        let key = self.next_key();
        {
            let mut frames = self.inner.frames.borrow_mut();
            frames.iter_mut().for_each(|layer| layer.alive = false);
            frames.push(BackgroundLayer {
                key,
                source: source.clone(),
                scope,
                alive: true,
            });
        }
        self.inner.shown.borrow_mut().hero = Some(source);
        self.schedule_cleanup();
    }

    /// Кадра нет (или не загрузился): все слои уходят.
    fn drop_hero(&self) {
        self.inner
            .frames
            .borrow_mut()
            .iter_mut()
            .for_each(|layer| layer.alive = false);
        self.inner.shown.borrow_mut().hero = None;
        self.schedule_cleanup();
    }

    // ---- передний план ----

    /// `preloadFront(value)` вместе с `frontLayout`.
    fn set_front(&self, wanted: Option<(String, String, FrontLayout)>) {
        let inner = &self.inner;
        let source = wanted.as_ref().map(|(source, _, _)| source.clone());
        inner.shown.borrow_mut().front_generation += 1;
        inner.shown.borrow_mut().wanted_front = source.clone();
        let Some((source, scope, layout)) = wanted else {
            self.drop_front();
            return;
        };
        // Тот же кадр уже виден: меняется только раскладка (`shownLayout = frontLayout`).
        if inner.shown.borrow().front.as_deref() == Some(source.as_str()) {
            inner
                .fronts
                .borrow_mut()
                .iter_mut()
                .filter(|layer| layer.alive)
                .for_each(|layer| layer.layout = layout);
            self.changed();
            return;
        }
        let generation = inner.shown.borrow().front_generation;
        let weak = Rc::downgrade(inner);
        let request = self.request_for(&source, &scope, Some(layout.height_cqh));
        let (shown_source, shown_scope) = (source.clone(), scope.clone());
        inner.loader.request(request, move |result| {
            let Some(inner) = weak.upgrade() else { return };
            let this = BackgroundController { inner };
            if this.inner.shown.borrow().front_generation != generation {
                return;
            }
            match result {
                Ok(_) => this.show_front(shown_source, shown_scope, layout),
                Err(error) => {
                    tracing::warn!(
                        "[background] передний план {shown_source} не загружен: {error}"
                    );
                    this.drop_front();
                }
            }
        });
    }

    fn show_front(&self, source: String, scope: String, layout: FrontLayout) {
        let key = self.next_key();
        {
            let mut fronts = self.inner.fronts.borrow_mut();
            fronts.iter_mut().for_each(|layer| layer.alive = false);
            fronts.push(FrontLayer {
                key,
                source: source.clone(),
                scope,
                alive: true,
                layout,
            });
        }
        self.inner.shown.borrow_mut().front = Some(source);
        self.schedule_cleanup();
    }

    fn drop_front(&self) {
        self.inner
            .fronts
            .borrow_mut()
            .iter_mut()
            .for_each(|layer| layer.alive = false);
        self.inner.shown.borrow_mut().front = None;
        self.schedule_cleanup();
    }

    fn next_key(&self) -> i32 {
        let mut shown = self.inner.shown.borrow_mut();
        let key = shown.next_key;
        shown.next_key += 1;
        key
    }

    /// Запрос картинки слоя кадра под нынешнее окно: вид берёт картинку из кеша загрузчика (после
    /// смены размера окна — грузит заново).
    pub fn frame_request(&self, layer: &BackgroundLayer) -> ImageRequest {
        self.request_for(&layer.source, &layer.scope, None)
    }

    /// Запрос картинки слоя переднего плана под нынешнее окно.
    pub fn front_request(&self, layer: &FrontLayer) -> ImageRequest {
        self.request_for(&layer.source, &layer.scope, Some(layer.layout.height_cqh))
    }

    /// Картинки выбора под нынешнее окно (кадр и передний план): вид готовит их заранее, чтобы
    /// кадр и фигуры вошли одним шагом.
    pub fn requests(&self, selection: &Selection) -> Vec<ImageRequest> {
        let hero = self.request_for(&source_of(selection.hero), &selection.scope, None);
        let front = selection.front.map(|(name, layout)| {
            self.request_for(&source_of(name), &selection.scope, Some(layout.height_cqh))
        });
        std::iter::once(hero).chain(front).collect()
    }

    /// Запрос картинки слоя: кадр — область окна с запасом параллакса в cover, передний план — по
    /// высоте `height_cqh` окна.
    fn request_for(
        &self,
        source: &str,
        scope: &str,
        front_height_cqh: Option<f64>,
    ) -> ImageRequest {
        // Арт — файлы сборки: точный размер области (одно уменьшение, как у Chromium).
        let request = ImageRequest::new(
            ImageSource::parse(source).unwrap_or_else(|| ImageSource::Bundle(source.to_owned())),
            scope,
        )
        .exact();
        let (width, height) = self.inner.window_px.get().unwrap_or((1600.0, 900.0));
        match front_height_cqh {
            None => request.with_target(
                (width * parallax::ZOOM).ceil() as u32,
                (height * parallax::ZOOM).ceil() as u32,
                Fit::Cover,
            ),
            Some(cqh) => {
                // Ширина области не ограничивает картинку: масштаб по высоте (Fit::Contain).
                request.with_target(
                    u32::MAX / 4,
                    (height * cqh / 100.0).ceil() as u32,
                    Fit::Contain,
                )
            }
        }
    }

    /// Уходящие слои убираются, когда погасли (без анимаций — сразу, следующим вызовом).
    fn schedule_cleanup(&self) {
        self.changed();
        let generation = self.inner.cleanup_generation.get() + 1;
        self.inner.cleanup_generation.set(generation);
        let after = Duration::from_millis(self.inner.fade_ms.get().max(1));
        let weak = Rc::downgrade(&self.inner);
        self.inner.env.after(
            after,
            Box::new(move || {
                if let Some(inner) = weak.upgrade()
                    && inner.cleanup_generation.get() == generation
                {
                    BackgroundController { inner }.cleanup();
                }
            }),
        );
    }

    fn cleanup(&self) {
        let inner = &self.inner;
        inner.frames.borrow_mut().retain(|layer| layer.alive);
        inner.fronts.borrow_mut().retain(|layer| layer.alive);
        self.changed();
    }

    // ---- параллакс ----

    /// Окно, фокус, видимость, пауза или разрешение движения изменились.
    pub fn inputs_changed(&self, inputs: &BackgroundInputs) {
        let window = (inputs.width, inputs.height);
        let mut guard = self.inner.parallax.borrow_mut();
        match guard.as_mut() {
            None => {
                // created() и mounted().
                let mut model = Parallax::new(window, inputs.focused, false, inputs.moving);
                model.mount();
                if !inputs.visible {
                    model.set_visible(false);
                }
                if inputs.paused {
                    model.set_paused(true);
                }
                *guard = Some(model);
            }
            Some(model) => {
                if (model.window_size()) != (window.0.round(), window.1.round()) {
                    model.resized(window.0, window.1);
                }
                if model.parallax_property() != inputs.moving {
                    model.set_parallax(inputs.moving);
                }
                if model.paused() != inputs.paused {
                    model.set_paused(inputs.paused);
                }
                if model.focused() != inputs.focused {
                    model.focus(inputs.focused);
                }
                if model.visible() != inputs.visible {
                    model.set_visible(inputs.visible);
                }
            }
        }
        drop(guard);
        self.changed();
    }

    /// Кадр окна: шаг, если кадр запрошен, по времени кадра (`step(now)`). Вид зовёт его, пока
    /// рисует кадр, и следующий кадр просит сам: перемена не объявляется (иначе вид перерисовывался
    /// бы на каждом кадре монитора, мимо своего предела частоты).
    pub fn frame(&self) {
        if let Some(model) = self.inner.parallax.borrow_mut().as_mut()
            && model.frame_pending()
        {
            model.frame(self.inner.env.now());
            self.inner.steps.set(self.inner.steps.get() + 1);
        }
    }
}

/// Игра, с которой лаунчер стартует (`resolveStartupGame`): арт для фона.
pub fn startup_selection(last_launched: Option<&serde_json::Value>) -> Option<Selection> {
    Selection::of_game(&crate::controllers::game_availability::startup_game(
        last_launched,
    ))
}

#[doc(hidden)]
pub fn register_art() {
    game_art::register_bundles();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::ManualEnv;
    use crate::images::NoRemote;

    /// Ждать, пока поток картинок вернёт ответ (до 5 с).
    fn wait(env: &ManualEnv, done: impl Fn() -> bool) {
        for _ in 0..500 {
            env.run();
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("картинка не пришла");
    }

    #[test]
    fn layers_cross_fade_and_a_shown_game_comes_back_at_once() {
        register_art();
        let env = ManualEnv::new();
        let loader = ImageLoader::new(Rc::new(NoRemote), &*env);
        let background = BackgroundController::new(env.clone(), loader);
        background.set_window_px(320.0, 180.0);
        background.set_fade_ms(500);
        background.show_game(Some("218"));
        wait(&env, || background.layer_counts() == (1, 1));
        let frames = background.frames();
        assert!(frames[0].alive && frames[0].scope == "games/218");

        // Другая игра: прежний слой гаснет, новый входит; после угасания прежний убран.
        background.show_game(Some("70"));
        wait(&env, || background.layer_counts() == (2, 2));
        let frames = background.frames();
        assert_eq!(
            (frames[0].alive, frames[1].alive, frames[1].scope.as_str()),
            (false, true, "games/70")
        );
        env.advance(499.0);
        assert_eq!(background.layer_counts().0, 2, "ещё гаснет");
        env.advance(1.0);
        assert_eq!(background.layer_counts(), (1, 1));

        // Игру уже показывали: кадр и передний план из кеша, смена — сразу, без загрузки.
        assert_eq!(
            background
                .requests(&Selection::of_game("218").unwrap())
                .len(),
            2,
            "кадр и передний план"
        );
        background.show_game(Some("218"));
        assert_eq!(background.layer_counts(), (2, 2));
        assert!(background.frames()[1].alive && background.fronts()[1].alive);

        // Раздел без фона игры: все слои уходят.
        background.show_game(None);
        assert!(background.frames().iter().all(|layer| !layer.alive));
        env.advance(500.0);
        assert_eq!(background.layer_counts(), (0, 0));
    }

    #[test]
    fn episodes_have_their_own_frame_without_front() {
        let episodes = Selection::of_episodes("218").unwrap();
        assert_eq!(
            (episodes.hero, episodes.front, episodes.scope.as_str()),
            ("launcher/hl2-episodes-bg.jpg", None, "games/218")
        );
        assert!(game_art::bytes(episodes.hero).is_some(), "кадр вшит");
        assert_eq!(Selection::of_episodes("70"), Selection::of_game("70"));
    }

    #[test]
    fn parallax_steps_only_when_a_frame_is_requested() {
        let env = ManualEnv::new();
        let background =
            BackgroundController::new(env.clone(), ImageLoader::new(Rc::new(NoRemote), &*env));
        assert!(!background.frame_pending() && background.vars().is_none());
        background.inputs_changed(&BackgroundInputs {
            paused: false,
            focused: true,
            visible: true,
            moving: true,
            width: 1600.0,
            height: 900.0,
        });
        let settle = || {
            let mut guard = 0;
            while background.frame_pending() && guard < 1000 {
                env.advance(16.0);
                background.frame();
                guard += 1;
            }
        };
        settle();
        let changes = Rc::new(Cell::new(0));
        background.on_change({
            let changes = changes.clone();
            move || changes.set(changes.get() + 1)
        });
        background.pointer_moved(100.0, 100.0);
        assert_eq!(changes.get(), 1, "параллакс стоял и пошёл — вид будится");
        background.pointer_moved(120.0, 100.0);
        settle();
        // Кадры идущего параллакса и курсор во время движения вид не будят: кадры он просит сам.
        assert_eq!(changes.get(), 1);
        assert!(background.parallax_steps() > 0);
        assert!(!background.frame_pending(), "модель успокоилась");
        assert!(background.vars().is_some_and(|vars| vars.par.is_some()));
    }
}
