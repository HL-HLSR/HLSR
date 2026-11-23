//! Настройки игры на странице «Персонализация» (`GameSettingsModal.vue` embedded 0.4.9): слева
//! список разделов (с подразделами внешнего вида), справа раздел и под ним общий подвал. Разделы
//! стоят в одной ячейке и сменяются свайпом в сторону нажатого (`l-slide-next/prev`), у каждого
//! своя прокрутка. Логика — [`Session`] (`hlsr-ui`); здесь вид и связь со страницей.
//!
//! Точки монтирования разделов других задач: `personal` — [`GamePersonalization`] (G8),
//! `configs` — [`GameConfigEditor`] (G5), `keyboard` и `scripts` — [`GameKeyboard`] и
//! [`GameScripts`] (G7). Их конструктор — `X::new(&SectionHost, cx)`.

use super::cfg_editor::GameConfigEditor;
use super::demos::{GameDemosView, engine_of};
use super::footer::footer;
use super::keyboard::GameKeyboard;
use super::launch::{args_style, launch};
use super::nav::nav;
use super::page::GameConfigsPanel;
use super::records::{GameGhostsView, version};
use super::scripts::GameScripts;
use crate::kit::{FieldEvent, TextInput};
use crate::motion::{SLIDE_NEXT, SLIDE_PREV, swap};
use crate::personal::editor::GamePersonalization;
use crate::scale::{u, viewport};
use crate::scroll::{Scroll, ScrollArea};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyView, App, AppContext, Context, Entity, EventEmitter, IntoElement,
    ParentElement, Render, StyleRefinement, Styled, Subscription, WeakEntity, Window, div,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::config_chips::ConfigGame;
use hlsr_ui::controllers::game_settings::sections::leave_part;
use hlsr_ui::controllers::game_settings::{Env, Event, Session, View, Zone};
use hlsr_ui::services::Services;
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Что получают разделы других задач: сервисы, сессия настроек игры, игра, язык и страница
/// (её вопрос о несохранённом — уход из «Файлов cfg»).
#[derive(Clone)]
pub struct SectionHost {
    pub services: Services,
    pub session: Rc<Session>,
    pub game_id: String,
    pub language: Language,
    pub page: WeakEntity<GameConfigsPanel>,
}

/// Что окно сообщает странице.
pub enum SettingsEvent {
    /// `$emit("saved", settings)`.
    Saved(Value),
}

impl EventEmitter<SettingsEvent> for GameSettings {}

/// Настройки одной игры (`:key="gameId"`: другая игра — новый вид и новая сессия).
pub struct GameSettings {
    host: SectionHost,
    page: WeakEntity<GameConfigsPanel>,
    args: Entity<TextInput>,
    proton: Entity<TextInput>,
    personal: Entity<GamePersonalization>,
    configs: Entity<GameConfigEditor>,
    keyboard: Entity<GameKeyboard>,
    scripts: Entity<GameScripts>,
    demos: Entity<GameDemosView>,
    ghosts: Entity<GameGhostsView>,
    scrolls: HashMap<&'static str, Scroll>,
    nav_scroll: Scroll,
    events: Rc<RefCell<Vec<Event>>>,
    /// Раздел прошлого кадра (`watch activeSection`).
    seen: Option<&'static str>,
    /// Последняя версия данных сессии, о которой уведомлены закэшированные разделы.
    seen_data_rev: u64,
    /// Рамка ошибки полей на прошлом кадре: `set_invalid` перерисовывает поле.
    invalid: [bool; 2],
    _subs: Vec<Subscription>,
}

/// Перемены сессии → вид: задачей потока окна, не внутри текущего обновления (сессия зовёт
/// слушателей и из обработчиков самого вида).
fn poker(cx: &mut Context<GameSettings>) -> Rc<dyn Fn()> {
    let weak = cx.weak_entity();
    let app = cx.to_async();
    let executor = cx.foreground_executor().clone();
    let scheduled = Rc::new(Cell::new(false));
    Rc::new(move || {
        if scheduled.replace(true) {
            return;
        }
        let (weak, mut app, scheduled) = (weak.clone(), app.clone(), scheduled.clone());
        executor
            .spawn(async move {
                scheduled.set(false);
                weak.update(&mut app, |this, cx| this.sync(cx)).ok();
            })
            .detach();
    })
}

impl GameSettings {
    pub fn new(
        services: &Services,
        game: &ConfigGame,
        initial_section: &str,
        language: Language,
        page: WeakEntity<GameConfigsPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let env = crate::env::GpuiEnv::new(cx);
        let session = Session::new(
            services.clone(),
            Env {
                game_id: game.id.clone(),
                game_title: game.title.clone(),
                initial_section: initial_section.to_owned(),
                linux: cfg!(target_os = "linux"),
                language,
                translator: Rc::new(hlsr_ui::tr),
                zone: Zone::Local,
            },
            env,
        );
        let poke = poker(cx);
        session.set_render({
            let poke = poke.clone();
            move || poke()
        });
        let events: Rc<RefCell<Vec<Event>>> = Rc::default();
        session.set_sink({
            let events = events.clone();
            move |event| {
                events.borrow_mut().push(event);
                poke();
            }
        });
        let host = SectionHost {
            services: services.clone(),
            session: session.clone(),
            game_id: game.id.clone(),
            language,
            page: page.clone(),
        };
        let goldsrc = engine_of(&game.id) == hlsr_ui::controllers::game_demos::Engine::GoldSrc;
        let args = cx.new(|cx| {
            TextInput::new(window, cx)
                .style(args_style)
                .max_length(512)
                .placeholder(if goldsrc {
                    "-heapsize 524288 +fps_max 100"
                } else {
                    "+fps_max 300 -high"
                })
        });
        let proton = cx.new(|cx| {
            TextInput::new(window, cx)
                .style(args_style)
                .max_length(512)
                .placeholder("PROTON_USE_WINED3D=1 mangohud %command%")
        });
        let personal = cx.new(|cx| GamePersonalization::new(&host, cx));
        let subs = vec![
            // Подразделы внешнего вида в списке слева и «Сохранить» вопроса страницы следят за ним.
            cx.observe(&personal, |_, _, cx| cx.notify()),
            cx.subscribe(&args, |this, input, event: &FieldEvent, cx| {
                if matches!(event, FieldEvent::Input) {
                    let text = input.read(cx).text().to_owned();
                    this.host.session.set_args(&text);
                }
            }),
            cx.subscribe(&proton, |this, input, event: &FieldEvent, cx| {
                if matches!(event, FieldEvent::Input) {
                    let text = input.read(cx).text().to_owned();
                    this.host.session.set_proton(&text);
                }
            }),
        ];
        let demos =
            cx.new(|cx| GameDemosView::new(services, &game.id, engine_of(&game.id), language, cx));
        let active = session.active_section();
        if active == "demos" {
            demos.update(cx, |demos, cx| demos.load(cx));
        }
        let ghosts = {
            let settings = cx.weak_entity();
            cx.new(|_| GameGhostsView::new(session.clone(), settings, language))
        };
        let this = GameSettings {
            personal,
            configs: cx.new(|cx| GameConfigEditor::new(&host, cx)),
            keyboard: cx.new(|cx| GameKeyboard::new(&host, cx)),
            scripts: cx.new(|cx| GameScripts::new(&host, cx)),
            demos,
            ghosts,
            host,
            page,
            args,
            proton,
            scrolls: HashMap::new(),
            nav_scroll: Scroll::new(),
            events,
            seen: Some(active),
            seen_data_rev: session.data_revision(),
            invalid: [false; 2],
            _subs: subs,
        };
        session.mounted();
        this
    }

    pub fn session(&self) -> &Rc<Session> {
        &self.host.session
    }

    pub fn personal(&self) -> &Entity<GamePersonalization> {
        &self.personal
    }

    pub fn configs(&self) -> &Entity<GameConfigEditor> {
        &self.configs
    }

    /// Сбросить буферы превью, когда страница «Персонализация» скрыта.
    pub fn release_idle_caches(&mut self, cx: &mut Context<Self>) {
        self.personal
            .update(cx, |personal, cx| personal.release_idle_caches(cx));
        cx.notify();
    }

    /// Сессия сообщила о переменах: события — странице, вид — заново; закэшированные разделы
    /// перерисовываются только при смене данных сессии (чистое переключение вкладки и свайп их
    /// не трогают; «Персонализация» и «Демо» сессию не читают).
    fn sync(&mut self, cx: &mut Context<Self>) {
        let data_rev = self.host.session.data_revision();
        if data_rev != self.seen_data_rev {
            self.seen_data_rev = data_rev;
            for section in [
                self.configs.entity_id(),
                self.keyboard.entity_id(),
                self.scripts.entity_id(),
            ] {
                App::notify(cx, section);
            }
            self.ghosts.update(cx, |ghosts, cx| ghosts.stale(cx));
        }
        let events = std::mem::take(&mut *self.events.borrow_mut());
        for event in events {
            match event {
                Event::Saved(settings) => cx.emit(SettingsEvent::Saved(settings)),
                // Ожидание клавиши «Скриптов» кончилось (другой скрипт, уход с раздела); событие
                // приходит и от нового «Назначить» — его захват не трогаем.
                Event::StopCapture if self.host.session.scripts_capturing().is_none() => {
                    crate::keys::stop();
                }
                _ => {}
            }
        }
        cx.notify();
    }

    /// Щелчок по разделу (`selectSection`): переход через «Персонализацию» — сначала вопрос
    /// страницы о несохранённом там, откуда уходят.
    pub fn select_section(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from = self.host.session.active_section();
        if from == id {
            return;
        }
        let Some(part) = leave_part(from, id) else {
            self.host.session.select_section(id);
            cx.notify();
            return;
        };
        let weak = cx.weak_entity();
        let page = self.page.clone();
        // Страница читает эти настройки (несохранённое) — вопрос после текущего обновления.
        window.defer(cx, move |window, cx| {
            page.update(cx, |page, cx| {
                page.settle_unsaved(
                    Some(part),
                    Box::new(move |_, window, cx| {
                        weak.update(cx, |this, cx| {
                            this.host.session.select_section(id);
                            this.watch(window, cx);
                            cx.notify();
                        })
                        .ok();
                    }),
                    window,
                    cx,
                )
            })
            .ok();
        });
    }

    /// Подраздел внешнего вида в списке слева.
    pub fn select_personal(&mut self, id: &str, cx: &mut Context<Self>) {
        if self
            .personal
            .update(cx, |personal, cx| personal.select(id, cx))
        {
            cx.notify();
        }
    }

    pub fn reset(&mut self) {
        self.host.session.reset();
    }

    /// «Отмена» на странице: правки отброшены (`onCancel` embedded).
    pub fn discard(&mut self) {
        self.host.session.discard_changes();
    }

    pub fn submit(&mut self) {
        self.host.session.submit();
    }

    pub fn reload_after_change(&mut self) {
        self.host.session.reload_after_change();
    }

    pub fn delete_edited(&mut self) {
        self.host.session.delete_edited();
    }

    pub fn dismiss_edited(&mut self) {
        self.host.session.dismiss_edited();
    }

    pub fn remove_imported(&mut self) {
        self.host.session.remove_imported_cfg();
    }

    /// `watch activeSection`: «Демо» читает список при каждом открытии; «Персонализация» и
    /// остальные разделы пишут файл игрока каждая сама — сторона, куда перешли, перечитывает
    /// его, если в ней нет правок.
    fn watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.host.session.active_section();
        let previous = self.seen.replace(active);
        if previous == Some(active) {
            return;
        }
        cx.defer_in(window, move |this, _, cx| {
            // Все фоновые перечитывания после смены вкладки ждём до конца 350-мс свайпа, чтобы
            // ответы IPC не перерисовывали кэшированные разделы посреди анимации перехода.
            let after_swipe = std::time::Duration::from_millis(380);
            if active == "demos" {
                if previous.is_none() || !this.demos.read(cx).has_loaded() {
                    this.demos.update(cx, |demos, cx| demos.load(cx));
                } else {
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(after_swipe).await;
                        this.update(cx, |this, cx| {
                            if this.host.session.active_section() == "demos" {
                                this.demos.update(cx, |demos, cx| demos.load(cx));
                            }
                        })
                        .ok();
                    })
                    .detach();
                }
            }
            if previous.is_some() {
                if active == "personal" {
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(after_swipe).await;
                        this.update(cx, |this, cx| {
                            if this.host.session.active_section() == "personal" {
                                this.personal
                                    .update(cx, |personal, cx| personal.on_window_focus(cx));
                            }
                        })
                        .ok();
                    })
                    .detach();
                } else if previous == Some("personal") {
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(after_swipe).await;
                        this.update(cx, |this, _| {
                            if this.host.session.active_section() != "personal" {
                                this.host.session.on_window_focus();
                            }
                        })
                        .ok();
                        // После конца свайпа (350 мс) отпустить тяжёлые буферы скрытого превью.
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(70))
                            .await;
                        this.update(cx, |this, cx| {
                            if this.host.session.active_section() != "personal" {
                                this.personal
                                    .update(cx, |p, cx| p.release_idle_caches(cx));
                            }
                        })
                        .ok();
                    })
                    .detach();
                }
            }
        });
    }

    /// Поля ввода следуют за черновиком (загрузка, сброс, отмена), ошибка — рамкой.
    fn sync_inputs(&mut self, view: &View, cx: &mut Context<Self>) {
        for (index, (input, text, error)) in [
            (&self.args, &view.args, &view.args_error),
            (&self.proton, &view.proton, &view.proton_error),
        ]
        .into_iter()
        .enumerate()
        {
            let invalid = !error.is_empty();
            let flip = self.invalid[index] != invalid;
            self.invalid[index] = invalid;
            input.update(cx, |input, cx| {
                if input.text() != text {
                    input.set_text(text.clone(), cx);
                }
                if flip {
                    input.set_invalid(invalid, cx);
                }
            });
        }
    }

    fn body(&mut self, id: &'static str) -> Scroll {
        self.scrolls.entry(id).or_default().clone()
    }
}

/// Тело раздела: своя прокрутка, поля 12/4/16 (4 px сбоку — чтобы кольцо фокуса и тени крайних
/// элементов не обрезались прокруткой).
pub(super) fn body(id: &'static str, scroll: &Scroll, child: AnyElement) -> AnyElement {
    ScrollArea::new(
        gpui::ElementId::Name(format!("gs-body-{id}").into()),
        scroll,
    )
    .size_full()
    .child(div().px(u(4.)).pt(u(12.)).pb(u(16.)).child(child))
    .into_any_element()
}

/// Всё, из чего строится любой раздел (содержимое ячейки при свайпе рисуется и для
/// уходящего, и для входящего раздела).
#[derive(Clone)]
struct Bodies {
    view: Rc<View>,
    weak: WeakEntity<GameSettings>,
    args: Entity<TextInput>,
    proton: Entity<TextInput>,
    personal: AnyView,
    configs: AnyView,
    keyboard: AnyView,
    scripts: AnyView,
    demos: AnyView,
    ghosts: AnyView,
    scrolls: HashMap<&'static str, Scroll>,
    language: Language,
    theme: Theme,
    /// Кэш видов ([`crate::cache::on`]).
    cache: bool,
}

impl Bodies {
    /// Раздел-вид во всю ячейку: закэшированный (свайп и наведение списка его не рисуют).
    fn section(&self, view: &AnyView) -> AnyElement {
        let style = StyleRefinement::default().size_full();
        crate::cache::cached(view.clone(), style, self.cache)
    }

    fn render(&self, id: &'static str) -> AnyElement {
        let theme = &self.theme;
        let language = self.language;
        let t = move |key: &str| hlsr_ui::tr(language, key, &[]);
        let scroll = self.scrolls.get(id).cloned().unwrap_or_default();
        match id {
            // Свою прокрутку и нижнюю строку ведут сами.
            "personal" => div()
                .size_full()
                .px(u(4.))
                .pt(u(12.))
                .overflow_hidden()
                .child(self.section(&self.personal))
                .into_any_element(),
            "keyboard" => div()
                .size_full()
                .px(u(4.))
                .pt(u(12.))
                .pb(u(16.))
                .overflow_hidden()
                .child(self.section(&self.keyboard))
                .into_any_element(),
            // `.game-settings__body` встроенного окна (12 4 16) сильнее `__body--configs` (низ 20).
            "configs" => div()
                .size_full()
                .px(u(4.))
                .pt(u(12.))
                .pb(u(16.))
                .overflow_hidden()
                .child(self.section(&self.configs))
                .into_any_element(),
            // Как «Клавиатура»: список и скрипт — от тех же краёв, что схема клавиш.
            "scripts" => div()
                .size_full()
                .px(u(4.))
                .pt(u(12.))
                .pb(u(16.))
                .overflow_hidden()
                .child(self.section(&self.scripts))
                .into_any_element(),
            "launch" => body(
                id,
                &scroll,
                launch(
                    &self.view,
                    &self.args,
                    &self.proton,
                    self.weak.clone(),
                    &t,
                    theme,
                ),
            ),
            // Свою прокрутку ведёт сам: место — вся ячейка.
            "demos" => self.section(&self.demos),
            // Свою прокрутку ведёт сам: место — вся ячейка.
            "ghosts" => self.section(&self.ghosts),
            "version" => body(id, &scroll, version(&self.view, &t, theme)),
            _ => div().into_any_element(),
        }
    }
}

impl Render for GameSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.watch(window, cx);
        let view = self.host.session.panel_view();
        self.sync_inputs(&view, cx);
        let theme = cx.global::<Theme>().clone();
        let language = self.host.language;
        let t = move |key: &str| hlsr_ui::tr(language, key, &[]);
        let personal_nav = self.personal.read(cx).nav();
        for id in ["launch", "version"] {
            self.body(id);
        }
        let weak = cx.weak_entity();
        let list = nav(
            &view.tabs,
            view.active_id,
            &personal_nav,
            weak.clone(),
            window,
            cx,
        );
        let bodies = Bodies {
            view: view.clone(),
            weak: weak.clone(),
            args: self.args.clone(),
            proton: self.proton.clone(),
            personal: self.personal.clone().into(),
            configs: self.configs.clone().into(),
            keyboard: self.keyboard.clone().into(),
            scripts: self.scripts.clone().into(),
            demos: self.demos.clone().into(),
            scrolls: self.scrolls.clone(),
            ghosts: self.ghosts.clone().into(),
            language,
            theme: theme.clone(),
            cache: crate::cache::on(cx),
        };
        let slide = if view.slide_next {
            SLIDE_NEXT
        } else {
            SLIDE_PREV
        };
        let layout = swap(
            "gs-layout",
            view.active_id,
            slide,
            move |id: &&'static str| bodies.render(id),
        )
        .relative()
        .flex_1()
        .min_h_0()
        .overflow_hidden();
        let start = self
            .page
            .upgrade()
            .map(|page| GameConfigsPanel::footer_start(&page, window, cx))
            .unwrap_or_else(|| div().into_any_element());
        let foot = (!view.footer.hidden).then(|| footer(&view.footer, start, weak, &t, &theme));
        // clamp(170px, 15vw, 216px).
        let nav_w = (viewport(window).0 * 0.15).clamp(170., 216.);
        div()
            .flex()
            .size_full()
            .min_h_0()
            .gap(u(20.))
            // Liquid Glass: окно embedded размывает и преломляет подложку страницы (без заливки).
            .when(crate::liquid::active(&theme), |el| {
                crate::liquid::with_surface(
                    el,
                    crate::liquid::Surface::new(crate::liquid::Kind::Embedded, u(0.))
                        .layer(theme.liquid_share()),
                )
            })
            .child(
                ScrollArea::new("gs-nav", &self.nav_scroll)
                    .flex_none()
                    .w(u(nav_w))
                    .h_full()
                    .child(div().pt(u(12.)).pb(u(16.)).child(list)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(layout)
                    .children(foot),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::renders;
    use gpui::{TestAppContext, VisualTestContext};
    use hlsr_ui::fixtures::{FixtureServices, Scene, Variant, screens_049_dir};
    use std::sync::Arc;
    use std::time::Duration;

    struct Host(Option<Entity<GameSettings>>);

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            crate::theme::frame(window, cx);
            div().size_full().flex().children(self.0.clone())
        }
    }

    fn frame_after(cx: &mut VisualTestContext, ms: u64) {
        cx.executor().advance_clock(Duration::from_millis(ms));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }

    /// Окно настроек Half-Life на фикстурах сцены `configs/kb-main`, открытое на разделе `section`.
    fn open<'a>(
        cx: &'a mut TestAppContext,
        section: &str,
    ) -> (Entity<GameSettings>, &'a mut VisualTestContext) {
        cx.update(crate::init);
        let scene = Scene::load(&screens_049_dir(), "configs/kb-main").expect("сцена");
        let variant = Variant {
            theme: "hlsr".into(),
            lang: "ru".into(),
            scale: 1.0,
            zoom: None,
            rail: None,
        };
        let services: Services =
            Arc::new(FixtureServices::new(&scene, &variant).expect("фикстура"));
        let game = ConfigGame {
            id: "218".into(),
            name: "HL".into(),
            title: "Half-Life".into(),
        };
        let (host, cx) = cx.add_window_view(|_, _| Host(None));
        host.update_in(cx, |host, window, cx| {
            let page = WeakEntity::new_invalid();
            host.0 = Some(cx.new(|cx| {
                GameSettings::new(&services, &game, section, Language::Ru, page, window, cx)
            }));
            cx.notify();
        });
        for _ in 0..10 {
            frame_after(cx, 200);
        }
        let settings = host.read_with(cx, |host, _| host.0.clone().expect("окно"));
        (settings, cx)
    }

    /// Раздел «Клавиатура» закэширован: кадры окна настроек (наведение списка, свайп соседнего
    /// раздела) его не рисуют, а перемены сессии — рисуют.
    #[gpui::test]
    fn sections_skip_frames_of_the_window_and_redraw_on_session_changes(cx: &mut TestAppContext) {
        let (settings, cx) = open(cx, "keyboard");
        let (keyboard, session) = settings.read_with(cx, |settings, _| {
            (settings.keyboard.entity_id(), settings.session().clone())
        });
        assert_eq!(session.active_section(), "keyboard");
        let drawn = renders(keyboard);
        assert!(drawn > 0, "раздел нарисован");

        // Кадр окна настроек (наведение списка разделов, подвал): раздел — повтор.
        settings.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!(renders(keyboard), drawn);

        // Перемена сессии (выбор клавиши): раздел — заново.
        session.kb_select("X");
        cx.run_until_parked();
        assert!(renders(keyboard) > drawn, "сессия дошла до раздела");
    }

    /// Раздел «Демо» закэширован со своей прокруткой: кадр окна настроек его не рисует, его данные
    /// (список заново) — рисуют.
    #[gpui::test]
    fn demos_section_is_cached_and_redraws_on_its_data(cx: &mut TestAppContext) {
        let (settings, cx) = open(cx, "demos");
        let (demos, session) = settings.read_with(cx, |settings, _| {
            (settings.demos.clone(), settings.session().clone())
        });
        assert_eq!(session.active_section(), "demos");
        let id = demos.entity_id();
        let drawn = renders(id);
        assert!(drawn > 0, "раздел нарисован");

        settings.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!(renders(id), drawn, "кадр окна — повтор раздела");

        demos.update(cx, |demos, cx| demos.load(cx));
        cx.run_until_parked();
        assert!(renders(id) > drawn, "свои данные — заново");
    }

    #[gpui::test]
    fn swipe_demos_to_keyboard_keeps_both_sections_cached(cx: &mut TestAppContext) {
        let (settings, cx) = open(cx, "demos");
        let (demos, keyboard, session) = settings.read_with(cx, |s, _| {
            (
                s.demos.entity_id(),
                s.keyboard.entity_id(),
                s.session().clone(),
            )
        });
        session.select_section("keyboard");
        settings.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let mut d_frames = Vec::new();
        let mut k_frames = Vec::new();
        for _ in 0..15 {
            frame_after(cx, 16);
            d_frames.push(renders(demos));
            k_frames.push(renders(keyboard));
        }
        assert_eq!(
            d_frames[2],
            *d_frames.last().unwrap(),
            "demos должен быть из кэша"
        );
        assert_eq!(
            k_frames[2],
            *k_frames.last().unwrap(),
            "keyboard должен быть из кэша"
        );
    }

    struct CachedPanel {
        settings: Option<Entity<GameSettings>>,
    }

    impl Render for CachedPanel {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            crate::live::rendered(cx.entity_id());
            div().size_full().flex().children(self.settings.clone())
        }
    }

    struct CachedPanelHost {
        panel: Entity<CachedPanel>,
        tick: usize,
    }

    impl Render for CachedPanelHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            crate::theme::frame(window, cx);
            div()
                .size_full()
                .flex()
                .child(div().w(gpui::px(1.)).child(self.tick.to_string()))
                .child(
                    self.panel
                        .clone()
                        .cached(StyleRefinement::default().flex_1().size_full()),
                )
        }
    }

    #[gpui::test]
    fn nested_cached_panel_keeps_sections_cached_during_swipes(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let mut scene = Scene::load(&screens_049_dir(), "configs/kb-main").expect("сцена");
        let cfg = Scene::load(&screens_049_dir(), "configs/cfg-files").expect("cfg");
        for (k, v) in cfg.invoke {
            scene.invoke.insert(k, v);
        }
        let variant = Variant {
            theme: "hlsr".into(),
            lang: "ru".into(),
            scale: 1.0,
            zoom: None,
            rail: None,
        };
        let services: Services =
            Arc::new(FixtureServices::new(&scene, &variant).expect("фикстура"));
        let game = ConfigGame {
            id: "70".into(),
            name: "HL1".into(),
            title: "Half-Life".into(),
        };
        let panel = cx.new(|_| CachedPanel { settings: None });
        let (host, cx) = cx.add_window_view({
            let panel = panel.clone();
            move |_, _| CachedPanelHost { panel, tick: 0 }
        });
        cx.simulate_resize(gpui::size(
            crate::scale::zpx(1., 1400.),
            crate::scale::zpx(1., 820.),
        ));
        panel.update_in(cx, |panel, window, cx| {
            let page = WeakEntity::new_invalid();
            panel.settings = Some(cx.new(|cx| {
                GameSettings::new(&services, &game, "demos", Language::Ru, page, window, cx)
            }));
            cx.notify();
        });
        for _ in 0..10 {
            frame_after(cx, 200);
        }
        let settings = panel.read_with(cx, |panel, _| panel.settings.clone().expect("окно"));
        let (kb, cfg_e, scr, demos, session) = settings.read_with(cx, |s, _| {
            (
                s.keyboard.entity_id(),
                s.configs.entity_id(),
                s.scripts.entity_id(),
                s.demos.entity_id(),
                s.session().clone(),
            )
        });
        // Сначала несколько кадров крутится только сосед снаружи (кэшированная панель не
        // перерисовывается, `draw_generation` уходит вперёд), затем свайп будит панель:
        // вложенные разделы должны безопасно синхронизироваться и остаться в кэше на весь свайп.
        for _ in 0..3 {
            host.update(cx, |host, cx| {
                host.tick += 1;
                cx.notify();
            });
            frame_after(cx, 16);
        }
        for (name, target, a, b) in [
            ("demos->keyboard", "keyboard", demos, kb),
            ("keyboard->scripts", "scripts", kb, scr),
            ("scripts->configs", "configs", scr, cfg_e),
            ("configs->demos", "demos", cfg_e, demos),
        ] {
            session.select_section(target);
            settings.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            let mut ra = Vec::new();
            let mut rb = Vec::new();
            for _ in 0..12 {
                frame_after(cx, 16);
                ra.push(renders(a));
                rb.push(renders(b));
            }
            // Первый кадр может разово дорисовать входящий раздел или проснувшийся уходящий,
            // но все последующие кадры свайпа оба вложенных вида обязаны браться из кэша.
            assert_eq!(
                ra[1],
                *ra.last().unwrap(),
                "{name}: уходящий раздел перерисовывался на свайпе ({ra:?})"
            );
            assert_eq!(
                rb[1],
                *rb.last().unwrap(),
                "{name}: входящий раздел перерисовывался на свайпе ({rb:?})"
            );
            for _ in 0..4 {
                frame_after(cx, 200);
            }
        }
    }
}
