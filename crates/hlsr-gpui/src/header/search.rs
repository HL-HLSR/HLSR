//! Поиск бегунов в шапке (`RunnerSearch.vue` 0.4.9): поле поиска F6 (лупа, крестик, до 64
//! знаков, у гостя недоступно — подсказка «Войдите через Steam…»), запрос `runner-search` через
//! 350 мс после ввода (от двух знаков), список под полем (`l-pop`, до 352 px с прокруткой):
//! кольцо загрузки, ошибка сети, «Профили не найдены», строки — аватар 40 в рамке оформления,
//! ник со значком подписчика, SteamID (или «скрыт»). ↑/↓ — по кругу, Enter и щелчок — профиль
//! (`openRunnerProfile`), Esc очищает поле, щелчок мимо поиска закрывает список. Логика —
//! `hlsr_ui::controllers::runner_search`.

use super::{SCOPE, Z_TOPBAR};
use crate::common::{avatar_frame, nick_name};
use crate::icons::spinner;
use crate::image::{Pictures, device_size};
use crate::kit::button::white;
use crate::kit::input::{FieldEvent, Kind, TextInput};
use crate::layer::{TipExt, anchor_probe};
use crate::liquid;
use crate::motion::{Origin, POP, presence};
use crate::paint::{ring, shadow};
use crate::scale::{u, zoom};
use crate::shell::root::ShellContext;
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{
    AnyElement, Bounds, Context, ElementId, Entity, Focusable, FontWeight, ObjectFit, Pixels,
    Render, SharedString, Subscription, Task, Window, deferred, div, img, prelude::*, px,
};
use hlsr_ui::controllers::runner_search::{
    Effect, SEARCH_DELAY, SearchMachine, SearchRow, SearchView, Tag,
};
use hlsr_ui::images::{Fit, ImageRequest, ImageSource};
use hlsr_ui::services::{Reply, Services};
use std::cell::Cell;
use std::rc::Rc;

/// `maxlength="64"`.
const MAX_LENGTH: usize = 64;

pub struct RunnerSearch {
    services: Services,
    machine: SearchMachine,
    /// Поле — с первого кадра (ему нужно окно).
    input: Option<Entity<TextInput>>,
    /// `setTimeout(search, 350)`.
    timer: Option<Task<()>>,
    /// Рамка поля в прошлом кадре (px окна): нажатие в нём список не закрывает.
    field: Rc<Cell<Bounds<Pixels>>>,
    pictures: Option<Pictures>,
    subscriptions: Vec<Subscription>,
    _events: Task<()>,
}

impl RunnerSearch {
    pub fn new(services: &Services, cx: &mut Context<RunnerSearch>) -> RunnerSearch {
        let events =
            crate::bridge::listen(cx, services.subscribe(), |this: &mut Self, event, cx| {
                if event.channel == "runner-profile-state" {
                    let effects = this.machine.profile_state(&event.payload);
                    this.run(effects, cx);
                }
            });
        let mut search = RunnerSearch {
            services: services.clone(),
            machine: SearchMachine::new(),
            input: None,
            timer: None,
            field: Rc::default(),
            pictures: cx.try_global::<Pictures>().cloned(),
            subscriptions: Vec::new(),
            _events: events,
        };
        let effects = search.machine.mounted();
        search.run(effects, cx);
        search
    }

    /// Побочные действия логики.
    fn run(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::StartTimer(request) => {
                    self.timer = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(SEARCH_DELAY).await;
                        this.update(cx, |this, cx| {
                            let effects = this.machine.timer_fired(request);
                            this.run(effects, cx);
                        })
                        .ok();
                    }));
                }
                Effect::StopTimer => self.timer = None,
                Effect::Invoke { tag, channel, args } => {
                    let reply = self.services.invoke(channel, args);
                    cx.spawn(async move |this, cx| {
                        let reply = reply.await;
                        this.update(cx, |this, cx| this.answer(tag, reply, cx)).ok();
                    })
                    .detach();
                }
                Effect::OpenProfile(id) => {
                    if let Some(shell) = ShellContext::get(cx) {
                        shell.flows.clone().open_runner_profile(&id);
                    }
                }
                Effect::ResetText => {
                    if let Some(input) = self.input.clone() {
                        input.update(cx, |input, cx| input.set_text("", cx));
                    }
                }
            }
        }
        self.sync_input(cx);
        cx.notify();
    }

    fn answer(&mut self, tag: Tag, reply: Reply, cx: &mut Context<Self>) {
        let effects = match tag {
            // `runner-state` без `.catch`: отказ ничего не меняет.
            Tag::State => match reply {
                Ok(state) => self.machine.profile_state(&state),
                Err(_) => Vec::new(),
            },
            Tag::Search(request) => self.machine.search_reply(request, reply),
            Tag::Avatars(request, ids) => {
                self.machine.avatars_reply(request, &ids, reply);
                Vec::new()
            }
        };
        self.run(effects, cx);
    }

    /// `:disabled="!signedIn"` и подсказка поля.
    fn sync_input(&self, cx: &mut Context<Self>) {
        let Some(input) = self.input.clone() else {
            return;
        };
        let disabled = !self.machine.view(&super::tr(cx)).signed_in;
        input.update(cx, |input, cx| input.set_disabled(disabled, cx));
    }

    fn field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some(input) = &self.input {
            return input.clone();
        }
        let placeholder = super::tr(cx)("#L_RUNNER_SEARCH", &[]);
        let input = cx.new(|cx| {
            TextInput::new(window, cx)
                .kind(Kind::Search)
                .max_length(MAX_LENGTH)
                .placeholder(placeholder)
        });
        let events = cx.subscribe(&input, |this, input, event: &FieldEvent, cx| {
            let effects = match event {
                FieldEvent::Input => {
                    let text = input.read(cx).text().to_owned();
                    this.machine.input(&text)
                }
                // `@keydown.enter.prevent="choose(results[selected])"`.
                FieldEvent::Enter => this.machine.enter(),
                _ => return,
            };
            this.run(effects, cx);
        });
        let focus = input.read(cx).focus_handle(cx);
        let focused = cx.on_focus(&focus, window, |this, _, cx| {
            this.machine.focus();
            cx.notify();
        });
        self.subscriptions.extend([events, focused]);
        self.input = Some(input.clone());
        self.sync_input(cx);
        input
    }

    /// `.runner-search__results`: подложка окна, рамка `--l-modal-border`, тень 0 20 60 .55.
    fn results(&self, view: SearchView, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let faint = theme.color("l-text-faint");
        let note = |child: AnyElement| {
            div()
                .px(u(8.))
                .py(u(4.))
                .text_px(12.5, 20.)
                .text_color(faint)
                .child(child)
        };
        let tr = super::tr(cx);
        let message = if view.loading {
            Some(note(
                div()
                    .h(u(20.))
                    .flex()
                    .items_center()
                    .child(spinner(12.5, faint))
                    .into_any_element(),
            ))
        } else if view.error {
            Some(note(tr("#L_RUNNER_NETWORK", &[]).into_any_element()))
        } else if view.rows.is_empty() {
            Some(note(tr("#L_RUNNER_SEARCH_EMPTY", &[]).into_any_element()))
        } else {
            None
        };
        let rows: Vec<AnyElement> = view
            .rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| self.row(index, row, view.selected == index as i32, window, cx))
            .collect();
        let field = self.field.get();
        let weak = cx.weak_entity();
        let open = view.open;
        let top = theme.len("l-header-control") + 8.;
        let pop = liquid::motion(&theme, POP);
        let scroll = crate::scroll::Scroll::retained("runner-search-results".into(), window, cx);
        let panel = presence("runner-search-pop", open, pop, move || {
            let results = scroll
                .overflow(div().id("runner-search-results"))
                .occlude()
                .max_h(u(352.))
                .p(u(8.))
                .rounded(theme.rem("l-radius"))
                .bg(theme.color("l-modal-bg"))
                .shadow(vec![
                    ring(zoom, 1., theme.color("l-modal-border")),
                    shadow(zoom, 0., 20., 60., 0., gpui::black().opacity(0.55)),
                ]);
            let radius = theme.rem("l-radius");
            liquid::glass(results, liquid::Kind::Panel, radius, px(0.), &theme, zoom)
                // `outside`: нажатие мимо поиска закрывает список.
                .on_mouse_down_out(move |event, _, cx| {
                    if field.contains(&event.position) {
                        return;
                    }
                    weak.update(cx, |this, cx| {
                        this.machine.close();
                        cx.notify();
                    })
                    .ok();
                })
                .children(message)
                .children(rows)
        })
        .origin(Origin::CENTER)
        .absolute()
        .top(u(top))
        .left_0()
        .right_0();
        deferred(panel).with_priority(Z_TOPBAR).into_any_element()
    }

    /// Строка результата: аватар 40 (`--l-radius-sm`, рамка оформления 3 px), ник и SteamID.
    fn row(
        &self,
        index: usize,
        row: SearchRow,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>();
        let zoom = zoom(window);
        let small = theme.len("l-radius-sm");
        let (text, faint) = (theme.color("l-text"), theme.color("l-text-faint"));
        let (border, hole) = (theme.color("l-border"), theme.color("l-modal-bg"));
        let photo = ImageSource::parse(&row.avatar).and_then(|source| {
            let (w, h) = device_size(window, 40., 40.);
            let request = ImageRequest::new(source, SCOPE).with_target(w, h, Fit::Cover);
            match self.pictures.as_ref()?.get(request, window, cx)? {
                Ok(photo) => Some(photo),
                // `@error="avatarFailed(profile)"`.
                Err(_) => {
                    let id = row.id.clone();
                    cx.defer_in(window, move |this, _, cx| {
                        let effects = this.machine.avatar_failed(&id);
                        // Повторный отказ ничего не меняет: без перерисовки, иначе битая картинка
                        // кеша Steam (загрузчик помнит ошибку) перерисовывала бы шапку каждый кадр.
                        if !effects.is_empty() {
                            this.run(effects, cx);
                        }
                    });
                    None
                }
            }
        });
        let avatar = div()
            .relative()
            .size(u(40.))
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .rounded(u(small))
            .bg(white(0.06))
            .shadow(vec![ring(zoom, 1., border)])
            .map(|el| match photo {
                Some(photo) => el.child(
                    img(photo)
                        .size_full()
                        .rounded(u(small))
                        .object_fit(ObjectFit::Cover),
                ),
                None => el.child(
                    div()
                        .text_px(15., 15.)
                        .font_weight(FontWeight::BOLD)
                        .text_color(faint)
                        .child(row.letter.clone()),
                ),
            });
        let framed = avatar_frame(
            ElementId::NamedInteger("runner-search-frame".into(), index as u64),
            40.,
            avatar,
        )
        .css(row.frame_css.clone())
        .width(3.)
        .radius(small)
        .hole(hole);
        let nick = nick_name(
            ElementId::NamedInteger("runner-search-nick".into(), index as u64),
            row.name.clone(),
        )
        .look(row.nick_style.clone(), row.nick_css.clone())
        .subscriber(row.subscriber, None);
        let names = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(u(2.))
            // Поля под свечение значка и ника съедаются отрицательным отступом.
            .child(
                div()
                    .mx(u(-6.))
                    .my(u(-4.))
                    .px(u(6.))
                    .py(u(4.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(nick),
            )
            .child(
                div()
                    .text_px(11.5, 16.)
                    .text_color(faint)
                    .child(row.id_text.clone()),
            );
        let weak = cx.weak_entity();
        let element = div()
            .id(ElementId::NamedInteger(
                "runner-search-row".into(),
                index as u64,
            ))
            .flex()
            .items_center()
            .gap(u(12.))
            .w_full()
            .min_h(u(48.))
            .pt(u(8.))
            .pb(u(8.))
            .pl(u(8.))
            .pr(u(12.))
            .rounded(u(small))
            .text_px(13., 20.)
            .text_color(text)
            .cursor_pointer()
            .button_text()
            .when(selected, |el| el.bg(white(0.06)))
            .hover(|s| s.bg(white(0.06)))
            .on_click(move |_, _, cx| {
                weak.update(cx, |this, cx| {
                    let effects = this.machine.choose(index);
                    this.run(effects, cx);
                })
                .ok();
            })
            .child(framed)
            .child(names);
        crate::snap::target(format!("runner-search-row-{index}"), element)
    }
}

impl Render for RunnerSearch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.field(window, cx);
        let view = self.machine.view(&super::tr(cx));
        let title: SharedString = super::tr(cx)(
            if view.signed_in {
                "#L_RUNNER_SEARCH"
            } else {
                "#L_RUNNER_SEARCH_GUEST"
            },
            &[],
        )
        .into();
        let results = self.results(view, window, cx);
        let (lg, radius_sm) = {
            let theme = cx.global::<Theme>();
            (theme.liquid_share(), theme.rem("l-radius-sm"))
        };
        let field = div()
            .id("runner-search-field")
            .relative()
            .w_full()
            .tip(title)
            // `@keydown.down/up.prevent="move(±1)"`: у поиска ↑/↓ полю не нужны.
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                let step = match event.keystroke.key.as_str() {
                    "down" => 1,
                    "up" => -1,
                    _ => return,
                };
                if !event.keystroke.modifiers.modified() {
                    this.machine.move_selection(step);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            // Liquid Glass: `.runner-search input` — мягкое стекло под заливкой поля.
            .when(lg > 0., |el| {
                liquid::with_surface(
                    el,
                    liquid::Surface::new(liquid::Kind::Soft, radius_sm).layer(lg),
                )
            })
            .child(crate::snap::target("runner-search-input", input))
            .child(anchor_probe(self.field.clone()));
        div()
            .id("runner-search")
            .relative()
            .w_full()
            .child(field)
            .child(results)
    }
}
