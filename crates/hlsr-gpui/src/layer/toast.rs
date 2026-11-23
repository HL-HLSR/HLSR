//! Тосты (`ToastContainer.vue` + `notify` LauncherShell 0.4.9): справа внизу над нижней
//! панелью, не больше трёх (лишний старый уходит), новые — внизу. Тон по значку (`toneOf`),
//! значок в плашке цвета тона, текст с переносом, крестик при наведении, полоса жизни.
//! Вход — переход `toast` (сдвиг 32 пружиной), соседи съезжают за `--l-dur` (`.toast-move`),
//! уход — `toast-out` 420 мс (вправо и гаснет, затем схлопывается по высоте). Щелчок закрывает.
//!
//! Время жизни — `ms` у `notify` (0 — 2800 мс; длинные с путями зовут с 15 000). Полоса жизни
//! у 0.4.9 всегда тает за 2800 мс: `notify` не передаёт `duration`, и `--toast-life` берёт
//! умолчание — так же и здесь.

use super::Z_TOAST;
use crate::icons::{fa_glyph, icon};
use crate::liquid;
use crate::motion::{
    AnimGate, Natural, Origin, Timing, apply, hover, keyframes, mix, presence, registry, scaled,
};
use crate::paint::{Radial, ring, shadow};
use crate::scale::{u, zoom, zpx};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{
    Anchor, AnyElement, App, Bounds, Context, Element, ElementId, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, Render, Window, anchored, deferred, div,
    point, prelude::*, rgb, size,
};
use hlsr_ui::motion::CubicBezier;
use hlsr_ui::overlay::{ToastEntry, ToastFeed, ToastInput, Tone};
use std::cell::Cell;
use std::time::{Duration, Instant};

/// Жизнь тоста по умолчанию (`ms || 2800`).
pub const TOAST_MS: u64 = 2800;
/// Больше трёх — старый уходит (`toasts.shift()`).
const LIMIT: usize = 3;
const WIDTH: f32 = 344.;

/// Тосты окна: `notify` и `dismiss` зовёт оболочка, слой рисуется в корне окна.
#[derive(Default)]
pub struct Toasts {
    /// `toasts` оболочки.
    list: Vec<ToastInput>,
    /// Показанные, с уходящими.
    feed: ToastFeed,
    seq: i32,
    /// Когда добавлен последний: соседи съезжают с этого кадра.
    added: Option<Instant>,
}

impl Toasts {
    pub fn new() -> Toasts {
        Toasts::default()
    }

    /// `notify(text, icon, ms)`: значок — классы FontAwesome (нет — `fas fa-info-circle`),
    /// `ms` — время жизни (0 — [`TOAST_MS`]). Возвращает id тоста.
    pub fn notify(
        &mut self,
        text: impl Into<String>,
        icon: Option<&str>,
        ms: u64,
        cx: &mut Context<Toasts>,
    ) -> i32 {
        self.seq += 1;
        let id = self.seq;
        self.list.push(ToastInput {
            id,
            text: text.into(),
            icon: icon.unwrap_or("fas fa-info-circle").to_owned(),
        });
        if self.list.len() > LIMIT {
            self.list.remove(0);
        }
        self.sync(cx);
        let ms = if ms == 0 { TOAST_MS } else { ms };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(ms))
                .await;
            this.update(cx, |this, cx| this.dismiss(id, cx)).ok();
        })
        .detach();
        id
    }

    /// `dismissToast(id)`: тост уходит (нет такого — ничего).
    pub fn dismiss(&mut self, id: i32, cx: &mut Context<Toasts>) {
        self.list.retain(|toast| toast.id != id);
        self.sync(cx);
    }

    /// Показанные тосты (с уходящими), сверху вниз.
    #[cfg(test)]
    pub(super) fn shown(&self) -> &[ToastEntry] {
        self.feed.entries()
    }

    fn sync(&mut self, cx: &mut Context<Toasts>) {
        let changes = self.feed.sync(&self.list);
        if !changes.added.is_empty() {
            self.added = Some(cx.background_executor().now());
        }
        let still = AnimGate::still(cx);
        let leave = liquid::motion(cx.global::<Theme>(), "toast-out");
        let out = registry().keyframes_length(leave, 0, still).unwrap_or(0.);
        for id in changes.leaving {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs_f64(out / 1000.))
                    .await;
                this.update(cx, |this, cx| {
                    this.feed.remove(id);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }
}

/// Цвет тона (`--toast-tint`): info — акцент темы.
fn tint(tone: Tone, theme: &Theme) -> Hsla {
    match tone {
        Tone::Success => rgb(0x4ad07a).into(),
        Tone::Warning => rgb(0xffb37a).into(),
        Tone::Danger => rgb(0xff7a7a).into(),
        Tone::Info => theme.color("l-accent"),
    }
}

impl Render for Toasts {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let view = window.viewport_size();
        // .toasts: right 24px, bottom calc(--l-bottombar-h + 108px).
        let corner = point(
            view.width - zpx(zoom, 24.),
            view.height - zpx(zoom, theme.len("l-bottombar-h") + 108.),
        );
        let entity = cx.entity().downgrade();
        let items = self.feed.entries().iter().map(|entry| {
            let card = Card {
                entry: entry.clone(),
                tint: tint(entry.tone, &theme),
                theme: theme.clone(),
                zoom,
            };
            let id = entry.id;
            let build: Build = if entry.leaving {
                Box::new(move |height, born| leaving(card, height, born).into_any_element())
            } else {
                let entity = entity.clone();
                Box::new(move |_, born| {
                    presence(
                        ElementId::named_usize("toast-in", id as usize),
                        true,
                        "toast",
                        move || {
                            hover(
                                ElementId::named_usize("toast-hover", id as usize),
                                Timing::fast(),
                                move |t| {
                                    card.build(t, born)
                                        .id(ElementId::named_usize("toast", id as usize))
                                        .occlude()
                                        .cursor_pointer()
                                        .on_click(move |_, _, cx| {
                                            entity.update(cx, |this, cx| this.dismiss(id, cx)).ok();
                                        })
                                },
                            )
                        },
                    )
                    .origin(Origin::CENTER)
                    .appear()
                    .w(u(WIDTH))
                    .into_any_element()
                })
            };
            Slot {
                id: ElementId::named_usize("toast-slot", id as usize),
                added: self.added,
                leaving: entry.leaving,
                build: Some(build),
            }
        });
        let column = div()
            .flex()
            .flex_col()
            .items_end()
            .gap(u(8.))
            .text_color(theme.color("l-text"))
            .children(items);
        deferred(
            anchored()
                .anchor(Anchor::BottomRight)
                .position(corner)
                .child(column),
        )
        .with_priority(Z_TOAST)
    }
}

/// Карточка тоста: всё, что нужно, чтобы её построить в кадре.
struct Card {
    entry: ToastEntry,
    tint: Hsla,
    theme: Theme,
    zoom: f32,
}

impl Card {
    /// `.toasts__item` при наведении `hover` (0…1). `born` — первый кадр тоста: значок и полоса
    /// жизни играют от него и в уходе (у 0.4.9 это тот же узел, его анимации не начинаются заново).
    fn build(self, hover: f32, born: Instant) -> gpui::Div {
        let Card {
            entry,
            tint,
            theme,
            zoom,
        } = self;
        let border = theme.color("l-modal-border");
        let edge = mix(mix(border, tint, 0.28), mix(border, tint, 0.5), hover);
        let id = entry.id as usize;
        let radius_sm = theme.rem("l-radius-sm");
        let plate = keyframes(ElementId::named_usize("toast-icon", id), "toast-icon", {
            let classes = entry.icon.clone();
            move |frame| {
                div()
                    .flex_none()
                    .size(u(32.))
                    .rounded(radius_sm)
                    .bg(tint.opacity(0.16))
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(frame.opacity as f32)
                    .child(icon(&classes, 14., tint))
            }
        })
        .since(born);
        let life = keyframes(
            ElementId::named_usize("toast-life", id),
            "toast-life",
            move |frame| {
                // Полоса обрезана скруглением карточки: видна с ~6 px от углов.
                div()
                    .absolute()
                    .bottom_0()
                    .left(u(6.))
                    .right(u(6.))
                    .h(u(2.))
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(u(-6.))
                            .h_full()
                            .w(u(WIDTH * frame.scale_x as f32))
                            .bg(tint.opacity(0.6)),
                    )
            },
        )
        .since(born);
        let card = div()
            .relative()
            .flex()
            .items_center()
            .gap(u(12.))
            .w(u(WIDTH))
            .min_h(u(56.))
            .pt(u(12.))
            .pb(u(12.))
            .pl(u(12.))
            .pr(u(16.))
            .bg(theme.color("l-modal-bg"))
            .rounded(theme.rem("l-radius"))
            .overflow_hidden()
            .shadow(vec![
                ring(zoom, 1., edge),
                shadow(zoom, 0., 18., 48., 0., gpui::black().opacity(0.55)),
            ]);
        // Liquid Glass: `.toasts__item` — мягкое стекло под своей заливкой и рамкой.
        let (own, radius) = (theme.color("l-modal-bg"), theme.rem("l-radius"));
        liquid::soft(card, own, radius, vec![ring(zoom, 1., edge)], &theme)
            // radial-gradient(120% 140% at 0% 0%, тон 12 %, прозрачно 60 %).
            .child(
                Radial {
                    center: (0., 0.),
                    radius: (1.2, 1.4),
                    stops: vec![(0., 0.12), (0.6, 0.)],
                }
                .paint_rounded(WIDTH, 56., theme.len("l-radius"), tint)
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(plate)
            .child(div().flex_1().min_w_0().text_px(13., 20.).child(entry.text))
            .child(
                fa_glyph("fas fa-times", 11.)
                    .w(u(16.))
                    .flex()
                    .justify_center()
                    .opacity(hover)
                    .text_color(mix(
                        theme.color("l-text-faint"),
                        theme.color("l-text"),
                        hover,
                    )),
            )
            .child(life)
    }
}

/// `toast-out`: вправо на 56 и гаснет к 55 %, затем высота (с полями) — в ноль, отступ до
/// соседа — в −8 (gap колонки). `height` — высота тоста до ухода.
fn leaving(card: Card, height: Option<Pixels>, born: Instant) -> impl IntoElement {
    let id = card.entry.id as usize;
    let zoom = card.zoom;
    let height = height.unwrap_or(zpx(zoom, 56.));
    keyframes(
        ElementId::named_usize("toast-out", id),
        liquid::motion(&card.theme, "toast-out"),
        move |frame| {
            // `translateX(56px) scale(.96)`: сдвиг снаружи масштаба.
            let natural = Natural::new(Cell::new(Some(size(zpx(zoom, WIDTH), height))));
            let card = scaled(frame.scale, Origin::CENTER, natural, card.build(0., born));
            div()
                .w(u(WIDTH))
                .h(height * frame.height as f32)
                .mb(u(frame.margin as f32))
                .overflow_hidden()
                .child(apply(div().child(card), frame))
        },
    )
}

/// Строит тост по высоте до ухода и первому кадру тоста.
type Build = Box<dyn FnOnce(Option<Pixels>, Instant) -> AnyElement>;

/// Место тоста в колонке: помнит высоту (для схлопывания) и верх прошлого кадра. Когда
/// добавлен новый тост, остальные съезжают со старого места (`.toast-move`, FLIP).
struct Slot {
    id: ElementId,
    added: Option<Instant>,
    leaving: bool,
    build: Option<Build>,
}

#[derive(Default)]
struct SlotState {
    top: Option<Pixels>,
    height: Option<Pixels>,
    /// Последнее добавление, которое этот тост уже отыграл.
    seen: Option<Instant>,
    /// Начало сдвига и его величина.
    moving: Option<(Instant, Pixels)>,
    /// Первый кадр тоста.
    born: Option<Instant>,
}

impl Element for Slot {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let now = cx.background_executor().now();
        let (height, born) =
            window.with_element_state(id.unwrap(), |state: Option<SlotState>, _| {
                let mut state = state.unwrap_or_default();
                let born = *state.born.get_or_insert(now);
                ((state.height, born), state)
            });
        let build = self.build.take().expect("один кадр — одна сборка");
        let mut child = build(height, born);
        (child.request_layout(window, cx), child)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        let now = cx.background_executor().now();
        let tokens = &registry().tokens;
        let still = AnimGate::still(cx).any();
        let (added, leaving) = (self.added, self.leaving);
        let (offset, running) =
            window.with_element_state(id.unwrap(), |state: Option<SlotState>, _| {
                let mut state = state.unwrap_or_default();
                if state.seen != added {
                    state.seen = added;
                    if let Some(top) = state.top
                        && top != bounds.origin.y
                        && !still
                    {
                        state.moving = Some((now, top - bounds.origin.y));
                    }
                }
                state.top = Some(bounds.origin.y);
                if !leaving {
                    state.height = Some(bounds.size.height);
                }
                let offset = match state.moving {
                    Some((since, delta)) => {
                        let t = now.duration_since(since).as_secs_f64() * 1000. / tokens.l_dur;
                        if t >= 1. {
                            state.moving = None;
                            Pixels::ZERO
                        } else {
                            delta * (1. - CubicBezier::new(tokens.l_ease).solve(t) as f32)
                        }
                    }
                    None => Pixels::ZERO,
                };
                ((offset, state.moving.is_some()), state)
            });
        if running {
            crate::motion::monitor_frame(window);
        }
        window.with_element_offset(point(Pixels::ZERO, offset), |window| {
            child.prepaint(window, cx)
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        child: &mut AnyElement,
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
    }
}
