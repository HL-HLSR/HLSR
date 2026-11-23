//! Панель друзей Steam (`FriendsPanel.vue`, в 0.4.9 без изменений с 0.4.7): опускается из
//! правого верхнего угла под меню аккаунта (`l-pop`), без подложки — лаунчер за ней работает.
//! Список из памяти лаунчера сразу и свежий следом, поиск при списке длиннее восьми, группы
//! «В HLSR» и «Steam», буква вместо аватара, которого нет. Esc закрывает. Логика —
//! [`FriendsState`].

use crate::icons::{fa_glyph, spinner};
use crate::image::{Pictures, device_size};
use crate::kit::button::{focus_handle, focus_visible, outline, white};
use crate::kit::{FieldEvent, FieldStyle, TextInput};
use crate::layer::{TipExt, Z_TOAST};
use crate::motion::{AnimGate, Origin, POP, Timing, hover, mix, presence, registry};
use crate::paint::{ring, shadow};
use crate::scale::{u, viewport, zoom};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::shell::root::ShellContext;
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, IntoElement, ObjectFit, Render, Role,
    Subscription, Window, anchored, deferred, div, img, point, prelude::*, px,
};
use hlsr_ui::controllers::friends::{Effect, FriendsState};
use hlsr_ui::images::{Fit, ImageRequest, ImageSource};
use hlsr_ui::services::Services;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// Область загрузчика картинок панели.
const SCOPE: &str = "friends";

/// `.friends__input`: 40 px, 13 px, без рамки и фона.
fn field_style(theme: &Theme) -> FieldStyle {
    let clear = gpui::transparent_black();
    FieldStyle {
        height: 40.,
        pad_x: 0.,
        size: 13.,
        line: 0.,
        bg: clear,
        hover_bg: clear,
        border: clear,
        hover_border: clear,
        focus_border: clear,
        invalid_border: clear,
        ring: true,
        glow: None,
        outline: None,
        ..FieldStyle::gx(theme)
    }
}

pub struct FriendsPanel {
    services: Services,
    state: FriendsState,
    open: bool,
    /// Растёт при каждом открытии: ответы прежней панели не применяются.
    generation: u64,
    search: Option<(Entity<TextInput>, Subscription)>,
    scroll: Scroll,
    /// Окно видимости списка (`crate::rows`).
    rows: crate::rows::Rows,
    pictures: Pictures,
    /// Центр панели в слое на всё окно — точка опоры `l-pop` (у 0.4.9 переход у самой `.friends`).
    origin: Origin,
    /// Высота панели прошлого кадра, CSS px (высота по содержимому, не больше `max-height`).
    height: Rc<Cell<f32>>,
    /// Закрытая панель доигрывает уход (`l-pop-leave`): до конца она строится прежней.
    leaving: bool,
    /// Esc закрывает панель и без фокуса в ней (`document` keydown 0.4.9).
    _escape: Subscription,
}

impl FriendsPanel {
    pub fn new(services: &Services, cx: &mut Context<FriendsPanel>) -> FriendsPanel {
        let pictures = cx.try_global::<Pictures>().cloned().unwrap_or_else(|| {
            Pictures::new(hlsr_ui::images::ImageLoader::new(
                Rc::new(hlsr_ui::images::NoRemote),
                &*crate::env::GpuiEnv::new(cx),
            ))
        });
        cx.on_release({
            let pictures = pictures.clone();
            move |_, cx| pictures.release_scope(SCOPE, cx)
        })
        .detach();
        let escape = cx.observe_keystrokes(|this, event, _, cx| {
            if this.open && event.keystroke.key == "escape" {
                this.close(cx);
            }
        });
        FriendsPanel {
            services: services.clone(),
            state: FriendsState::new(),
            open: false,
            generation: 0,
            search: None,
            scroll: Scroll::default(),
            rows: Default::default(),
            pictures,
            origin: Origin::CENTER,
            height: Rc::default(),
            leaving: false,
            _escape: escape,
        }
    }

    fn wanted(cx: &App) -> bool {
        ShellContext::get(cx).is_some_and(|shell| shell.flows.with_state(|s| s.friends_open))
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(shell) = ShellContext::get(cx) {
            shell.flows.close_friends();
        }
        cx.notify();
    }

    /// `v-if="friendsOpen"`: `mounted()` (`load`) и `beforeUnmount()`.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let open = Self::wanted(cx);
        if open == self.open {
            return;
        }
        self.open = open;
        self.generation += 1;
        if open {
            self.leaving = false;
            self.state = FriendsState::new();
            self.search = None;
            self.scroll = Scroll::default();
            let effects = self.state.mounted();
            self.run(effects, cx);
        } else {
            // `beforeUnmount` сразу, а уходящая панель видна прежней до конца ухода: аватары
            // отдаются после него.
            self.state.unmounted();
            let pop = crate::liquid::motion(cx.global::<Theme>(), POP);
            let leave = registry().transition_length(pop, false, AnimGate::still(cx));
            let leave = Duration::from_secs_f64(leave.max(0.) / 1000.);
            self.leaving = true;
            let generation = self.generation;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(leave).await;
                this.update(cx, |this, cx| {
                    if this.generation == generation {
                        this.leaving = false;
                        this.pictures.release_scope(SCOPE, cx);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn run(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for Effect { tag, channel, args } in effects {
            let reply = self.services.invoke(channel, args);
            let generation = self.generation;
            cx.spawn(async move |this, cx| {
                let reply = reply.await;
                this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    let effects = this.state.answer(tag, reply);
                    this.run(effects, cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    fn open_profile(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(shell) = ShellContext::get(cx) {
            shell.flows.open_runner_profile(id);
        }
    }

    fn panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (vw, vh) = viewport(window);
        let shell_tr: hlsr_ui::controllers::bridge::Tr = {
            let language = ShellContext::get(cx)
                .map_or(hlsr_core::language::Language::En, |s| s.shell.language());
            Rc::new(move |key, args| hlsr_ui::tr(language, key, args))
        };
        let tr = {
            let shell_tr = shell_tr.clone();
            move |key: &str| shell_tr(key, &[])
        };
        let view = self.state.view(&shell_tr);
        let (text, dim, faint, border) = (
            theme.color("l-text"),
            theme.color("l-text-dim"),
            theme.color("l-text-faint"),
            theme.color("l-border"),
        );
        let accent = theme.color("l-accent");
        let radius_sm = theme.len("l-radius-sm");
        let title = tr("#L_FRIENDS_TITLE");
        let close_label = tr("#L_CLOSE");
        let close_id = gpui::ElementId::from("friends-close");
        let close_focus = focus_handle(&close_id, window, cx);
        let close_ring = focus_visible(&close_focus, window);
        let on_close = cx.listener(|this, _, _, cx| this.close(cx));
        let close = hover("friends-close-hover", Timing::fast(), move |t| {
            div()
                .id("friends-close")
                .role(Role::Button)
                .button_text()
                .aria_label(close_label.clone())
                .tip(close_label)
                .relative()
                .flex_none()
                .size(u(40.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(u(radius_sm))
                .bg(white(0.06 * t))
                .text_color(mix(faint, text, t))
                .track_focus(&close_focus)
                .cursor_pointer()
                .on_click(on_close)
                .child(fa_glyph("fas fa-times", 13.33))
                .when(close_ring, |el| {
                    el.child(outline(zoom, radius_sm, -2., 2., accent))
                })
        });
        // `box-shadow: inset 0 -1px 0 --l-border` — линия 1 px по низу.
        let rule = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(1.))
            .bg(border);
        let head = div()
            .relative()
            .flex()
            .items_center()
            .gap(u(8.))
            .py(u(12.))
            .px(u(16.))
            .child(rule)
            .child(spaced(
                title.to_uppercase(),
                12.,
                16.,
                FontWeight::BOLD,
                0.12,
                text,
            ))
            .child(
                div()
                    .flex_1()
                    .text_px(11.5, 16.)
                    .text_color(faint)
                    .child(view.counter.clone()),
            )
            .child(close);
        let search = view.search.then(|| {
            let input = match &self.search {
                Some((input, _)) => input.clone(),
                None => {
                    let placeholder = tr("#L_FRIENDS_SEARCH");
                    let input = cx.new(|cx| {
                        TextInput::new(window, cx)
                            .style(field_style)
                            .placeholder(placeholder)
                    });
                    let sub = cx.subscribe(&input, |this, input, event: &FieldEvent, cx| {
                        if *event == FieldEvent::Input {
                            let text = input.read(cx).value();
                            this.state.set_query(&text);
                            cx.notify();
                        }
                    });
                    self.search = Some((input.clone(), sub));
                    input
                }
            };
            div()
                .flex()
                .items_center()
                .gap(u(8.))
                .px(u(16.))
                .border_b_1()
                .border_color(border)
                .text_color(faint)
                .child(fa_glyph("fas fa-search", 11.))
                .child(div().flex_1().min_w_0().child(input))
        });
        let loading = view.loading.then(|| {
            div()
                .my(u(12.))
                .mx(u(8.))
                .child(spinner(12.5, faint))
                .into_any_element()
        });
        let hint_label = tr("#L_RUNNER_FRIENDS_HLSR");
        // Окно видимости (`crate::rows`): строятся записи у видимой области; запись — в обёртке,
        // чтобы её поля (`my`, `mt`) входили в замеренную рамку.
        let keys = view
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| format!("{index}:{}:{}", entry.kind, entry.id))
            .collect();
        let slots = self.rows.plan(&self.scroll, keys, 0., window, cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        for slot in slots {
            let index = match slot {
                crate::rows::Slot::Row(index) => index,
                crate::rows::Slot::Space(height) => {
                    rows.push(div().flex_none().h(height).into_any_element());
                    continue;
                }
            };
            let entry = &view.entries[index];
            let mut cell: Vec<AnyElement> = Vec::new();
            {
                let rows = &mut cell;
                match entry.kind {
                    "title" => rows.push(
                        div()
                            .flex()
                            .gap(u(7.))
                            .py(u(8.))
                            .px(u(12.))
                            .my(u(5.))
                            .text_px(11., 18.)
                            .font_weight(FontWeight::BOLD)
                            .text_color(dim)
                            .child(entry.text.clone())
                            .child(div().text_color(faint).child(entry.count.to_string()))
                            .into_any_element(),
                    ),
                    "note" => rows.push(
                        div()
                            .my(u(12.))
                            .mx(u(8.))
                            .text_px(12.5, 18.75)
                            .text_color(faint)
                            .child(entry.text.clone())
                            .into_any_element(),
                    ),
                    _ => {
                        let photo = ImageSource::parse(&entry.avatar).and_then(|source| {
                            let (w, h) = device_size(window, 32., 32.);
                            let request =
                                ImageRequest::new(source, SCOPE).with_target(w, h, Fit::Cover);
                            self.pictures.get(request, window, cx).and_then(Result::ok)
                        });
                        let avatar = div()
                            .flex_none()
                            .size(u(32.))
                            .rounded(u(8.))
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(white(0.06))
                            .map(|el| match photo {
                                Some(photo) => el.child(
                                    img(photo)
                                        .size_full()
                                        .rounded(u(8.))
                                        .object_fit(ObjectFit::Cover),
                                ),
                                None => el.child(
                                    div()
                                        .text_px(14., 16.)
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(faint)
                                        .child(entry.letter.clone()),
                                ),
                            });
                        let id = entry.id.clone();
                        let in_hlsr = entry.in_hlsr.then(|| {
                            div()
                                .id(("friend-hlsr", index))
                                .ml_auto()
                                .text_color(theme.color("accent"))
                                .tip(hint_label.clone())
                                .child(fa_glyph("fas fa-check-circle", 12.))
                        });
                        rows.push(
                            div()
                                .id(("friend", index))
                                .role(Role::Button)
                                .button_text()
                                .when(entry.gap, |el| el.mt(u(2.)))
                                .flex()
                                .items_center()
                                .gap(u(12.))
                                .p(u(8.))
                                // `border: 1px solid transparent`.
                                .border_1()
                                .border_color(gpui::transparent_black())
                                .rounded(u(radius_sm))
                                .cursor_pointer()
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.open_profile(&id, cx)),
                                )
                                .child(avatar)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_px(13., 16.)
                                        .text_color(dim)
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(entry.text.clone()),
                                )
                                .children(in_hlsr)
                                .into_any_element(),
                        );
                    }
                }
            }
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .children(cell)
                    .into_any_element(),
            );
        }
        let (edge, control, topbar) = (
            theme.len("l-edge-pad"),
            theme.len("l-header-control"),
            theme.len("l-topbar-h"),
        );
        let width = (vw - 32.).min(340.);
        let max_h = (vh - topbar - 40.).min(560.);
        let top = (topbar + control) / 2. + 8.;
        let right = edge + control * 2. + 24.;
        let height = match self.height.get() {
            h if h > 0. => h,
            _ => max_h,
        };
        self.origin = Origin::new((0., vw - right - width / 2.), (0., top + height / 2.));
        let measured = self.height.clone();
        // Внутри масштаба rem окна × k: высота без масштаба — по rem этого кадра.
        let probe = gpui::canvas(
            move |bounds, window, _| {
                measured.set(f32::from(bounds.size.height) / crate::scale::zoom(window));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let panel = div()
            .id("friends")
            .role(Role::Dialog)
            .aria_label(title)
            .occlude()
            .absolute()
            .top(u(top))
            .left(u(vw - right - width))
            .w(u(width))
            .max_h(u(max_h))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.color("l-modal-bg"))
            .rounded(theme.rem("l-radius"))
            .shadow(vec![
                ring(zoom, 1., theme.color("l-modal-border")),
                shadow(zoom, 0., 20., 60., 0., gpui::black().opacity(0.55)),
            ]);
        let radius = theme.rem("l-radius");
        crate::liquid::glass(
            panel,
            crate::liquid::Kind::Window,
            radius,
            gpui::Pixels::ZERO,
            &theme,
            zoom,
        )
        .child(probe)
        .child(head)
        .children(search)
        .child(
            ScrollArea::new("friends-scroll", &self.scroll)
                .bar(Bar::Thin)
                .flex_1()
                .min_h_0()
                .child(
                    div().p(u(8.)).flex().flex_col().children(loading).child(
                        self.rows
                            .measure(&self.scroll, div().flex().flex_col().children(rows)),
                    ),
                ),
        )
        .into_any_element()
    }
}

impl Render for FriendsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        if Self::wanted(cx) != self.open {
            self.sync(cx);
        }
        let open = self.open;
        // Закрытая — пока доигрывает уход: `presence` рисует то, что ей дали.
        let shown = open || self.leaving;
        let panel = shown.then(|| self.panel(window, cx));
        let pop = crate::liquid::motion(cx.global::<Theme>(), POP);
        let layer = presence("friends-pop", open, pop, move || {
            div().size_full().children(panel)
        })
        .origin(self.origin);
        let view = window.viewport_size();
        deferred(
            anchored()
                .position(point(px(0.), px(0.)))
                .child(div().w(view.width).h(view.height).child(layer)),
        )
        .with_priority(Z_TOAST)
    }
}
