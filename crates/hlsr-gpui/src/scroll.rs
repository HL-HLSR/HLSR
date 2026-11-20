//! Прокрутка как у Chromium в 0.4.9 (Ф5; образец — `ui/components/scroll.slint` ветки slint).
//!
//! GPUI сдвигает содержимое (`overflow_y_scroll` + `ScrollHandle`), но полосы не рисует, а
//! колесо крутит на «строки» своего шрифта. [`ScrollArea`] даёт то, что было у Chromium:
//! - полоса `::-webkit-scrollbar` 0.4.9 ([`Bar`]): место у правого края, только когда
//!   содержимое не помещается (`overflow-y: auto`); ползунок по формулам Chromium; наведение;
//!   перетаскивание; щелчок по дорожке — на страницу;
//! - колесо: 100/3 px на строку (`wheel_step_for_lines`; строк за щелчок из ОС GPUI уже
//!   положил в событие) и плавность Chromium; `overscroll-behavior: contain` ([`ScrollArea::contain`]);
//! - плавная программная прокрутка ([`Scroll::scroll_to`]);
//! - затухание краёв списка (`mask-image` HomePanel/HomePlayers) — градиентом цвета подложки;
//! - длинные списки — `uniform_list` ([`ScrollArea::list`]).
//!
//! Плавность — замер Electron 44 (slint, 17.10): щелчок колеса — cubic-bezier(.42, 0, .58, 1)
//! за 14 − |Δ|/60 кадров по 1/60 с (от 6 до 12); программная — cubic-bezier(.42, 0, 0, 1) за
//! √|Δ| кадров, не больше 30. Δ — CSS px. Размеры полос — CSS px макета (зум окна).

use crate::scale::{u, zoom, zpx};
use crate::theme::bezier;
use gpui::{
    AnyElement, App, Bounds, DispatchPhase, ElementId, HitboxBehavior, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Refineable, RenderOnce, ScrollDelta, ScrollEvent, ScrollHandle, StatefulInteractiveElement,
    StyleRefinement, Styled, UniformList, UniformListScrollHandle, Window, canvas, div, fill,
    linear_color_stop, linear_gradient, point, prelude::FluentBuilder, size, white,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Колесо: cubic-bezier щелчка и программной прокрутки.
const WHEEL_CURVE: [f32; 4] = [0.42, 0., 0.58, 1.];
const SMOOTH_CURVE: [f32; 4] = [0.42, 0., 0., 1.];
/// Переход `--fade-top`/`--fade-bottom`: `--l-fast` (.16 с, `--l-ease`).
const FADE_TIME: Duration = Duration::from_millis(160);
const FADE_CURVE: [f32; 4] = [0.22, 1., 0.36, 1.];
/// Затухание края: сверху 32 px, снизу 48 px (`.fade-top`, `.fade-bottom`).
const FADE_TOP: f32 = 32.;
const FADE_BOTTOM: f32 = 48.;

/// Шаг колеса на строку события GPUI, CSS px. Windows: строки = щелчки × SPI_GETWHEELSCROLLLINES,
/// строка Chromium — 100/3 px. Linux: строка — щелчок, 53 px (X11).
pub(crate) fn line_step() -> f32 {
    if cfg!(windows) {
        hlsr_ui::render::wheel_step_for_lines(1)
    } else {
        hlsr_ui::render::WHEEL_STEP
    }
}

/// Полоса прокрутки (`::-webkit-scrollbar` 0.4.9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bar {
    /// App.vue: 4 px; дорожка .05, ползунок .1, наведение на ползунок — .2.
    #[default]
    Thin,
    /// HomePanel.vue, HomePlayers.vue 0.4.9: 6 px, без дорожки, радиус 3; ползунок .28, при
    /// наведении на список — .45.
    List,
    /// UiDropdown.vue: 8 px, прозрачная рамка ползунка 2 px; дорожка App.vue .05, ползунок .14.
    Dropdown,
    /// settings-fields.scss: 10 px, рамка ползунка 3 px, дорожка прозрачная с отступом 8 px
    /// сверху и снизу; ползунок .14, наведение на ползунок — .24.
    Fields,
    /// `display: none`: ни полосы, ни места под неё.
    Hidden,
}

/// Вид полосы, CSS px и прозрачность белого.
#[derive(Clone, Copy)]
struct Look {
    thickness: f32,
    /// Прозрачная рамка ползунка (`background: … padding-box`).
    border: f32,
    /// `margin-block` дорожки.
    margin: f32,
    /// Скругление видимого ползунка.
    radius: f32,
    /// Дорожка: прозрачность и скругление.
    track: Option<(f32, f32)>,
    thumb: f32,
    hot: f32,
    /// Ярче при наведении на всю область (`:hover::-webkit-scrollbar-thumb`), не на ползунок.
    area_hover: bool,
}

impl Bar {
    fn look(self) -> Look {
        let look = |thickness, border, margin, radius, track, thumb, hot, area_hover| Look {
            thickness,
            border,
            margin,
            radius,
            track,
            thumb,
            hot,
            area_hover,
        };
        match self {
            // border-radius 8 у 4 px — 2.
            Bar::Thin => look(4., 0., 0., 2., Some((0.05, 2.)), 0.10, 0.20, false),
            Bar::List => look(6., 0., 0., 3., None, 0.28, 0.45, true),
            // Радиус 4 у рамки 2: видимый фон (padding-box) — 2; дорожка App.vue: радиус 8 у 8 px — 4.
            Bar::Dropdown => look(8., 2., 0., 2., Some((0.05, 4.)), 0.14, 0.14, false),
            Bar::Fields => look(10., 3., 8., 2., None, 0.14, 0.24, false),
            Bar::Hidden => look(0., 0., 0., 0., None, 0., 0., false),
        }
    }

    /// Место полосы у правого края, CSS px (0 — полосы нет).
    pub fn thickness(self) -> f32 {
        self.look().thickness
    }
}

/// Ползунок Chromium (`ScrollbarTheme::ThumbLength/ThumbPosition`), CSS px: начало от верха
/// дорожки и длина. Длина — round(видимое/всё × дорожка); не помещается в дорожку — ползунка
/// нет; начало — round((дорожка − длина) × прокрутка / максимум), в (0, 1) — 1.
fn thumb(viewport: f32, content: f32, top: f32, track: f32, border: f32) -> Option<(f32, f32)> {
    let max = content - viewport;
    if max <= 0. {
        return None;
    }
    let length = (viewport / content.max(1.) * track)
        .round()
        .max(2. * border);
    if length > track {
        return None;
    }
    let start = top.clamp(0., max) / max * (track - length);
    let start = if start > 0. && start < 1. {
        1.
    } else {
        start.round()
    };
    Some((start, length))
}

/// Щелчок по дорожке — страница Chromium (`ScrollableArea::PageStep`): 87,5 % видимого, целые
/// px, не меньше 1.
fn page_step(viewport: f32) -> f32 {
    (viewport.round() * 0.875).floor().max(1.)
}

/// Плавность щелчка колеса: 14 − |Δ|/60 кадров по 1/60 с, от 6 до 12.
fn wheel_duration(delta: f32) -> Duration {
    Duration::from_secs_f32((14. - delta.abs() / 60.).clamp(6., 12.) / 60.)
}

/// Плавная программная прокрутка: √|Δ| кадров по 1/60 с, не больше 30.
fn smooth_duration(delta: f32) -> Duration {
    Duration::from_secs_f32(delta.abs().sqrt().min(30.) / 60.)
}

/// Переход прокрутки, px окна.
struct Motion {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
    curve: [f32; 4],
}

/// Ползунок тянут: где была мышь (y окна) и прокрутка (px окна) при нажатии.
#[derive(Clone, Copy)]
struct Drag {
    pointer: Pixels,
    top: f32,
}

/// Затухание края: 0…1 с переходом [`FADE_TIME`].
#[derive(Default)]
struct Fade {
    on: bool,
    from: f32,
    since: Option<Instant>,
}

impl Fade {
    fn value(&self, now: Instant) -> f32 {
        let target = if self.on { 1. } else { 0. };
        let Some(since) = self.since else {
            return target;
        };
        let t = now.saturating_duration_since(since).as_secs_f32() / FADE_TIME.as_secs_f32();
        if t >= 1. {
            return target;
        }
        self.from + (target - self.from) * bezier(FADE_CURVE)(t)
    }

    fn set(&mut self, on: bool, now: Instant) {
        if on != self.on {
            self.from = self.value(now);
            self.on = on;
            self.since = Some(now);
        }
    }

    fn moving(&self, now: Instant) -> bool {
        self.since
            .is_some_and(|since| now.saturating_duration_since(since) < FADE_TIME)
    }
}

/// Что сделало колесо.
enum Wheel {
    Scrolled,
    /// Не сдвинулось, но наружу не уходит (`contain`).
    Held,
    /// Уходит наружу.
    Pass,
}

#[derive(Default)]
struct State {
    /// У списка свой хендл; его `base_handle` — хендл и обычной области.
    list: UniformListScrollHandle,
    motion: Option<Motion>,
    drag: Option<Drag>,
    thumb_hot: bool,
    area_hot: bool,
    /// Полоса занимает место: в прошлом кадре содержимое не поместилось.
    gutter: bool,
    fades: [Fade; 2],
}

impl State {
    fn handle(&self) -> ScrollHandle {
        self.list.0.borrow().base_handle.clone()
    }

    /// scrollTop, px окна.
    fn shown(&self) -> f32 {
        -f32::from(self.handle().offset().y)
    }

    /// Наибольший scrollTop прошлого кадра, px окна.
    fn max(&self) -> f32 {
        f32::from(self.handle().max_offset().y).max(0.)
    }

    fn set_top(&self, top: f32) {
        let handle = self.handle();
        let x = handle.offset().x;
        handle.set_offset(point(x, -Pixels::from(top)));
    }

    /// Куда идёт прокрутка: конец перехода или нынешнее место.
    fn target(&self) -> f32 {
        self.motion
            .as_ref()
            .map_or(self.shown(), |motion| motion.to)
    }

    fn go(&mut self, to: f32, duration: Duration, curve: [f32; 4], now: Instant) {
        if duration.is_zero() {
            self.motion = None;
            self.set_top(to);
        } else {
            self.motion = Some(Motion {
                from: self.shown(),
                to,
                start: now,
                duration,
                curve,
            });
        }
    }

    /// Шаг перехода к `now`.
    fn step(&mut self, now: Instant) {
        let Some(motion) = &self.motion else { return };
        let t = now.saturating_duration_since(motion.start).as_secs_f32()
            / motion.duration.as_secs_f32();
        let (top, done) = if t >= 1. {
            (motion.to, true)
        } else {
            let eased = bezier(motion.curve)(t);
            (motion.from + (motion.to - motion.from) * eased, false)
        };
        self.set_top(top);
        if done {
            self.motion = None;
        }
    }

    /// Колесо: щелчки во время перехода копятся от его конца, как у Chromium.
    fn wheel(
        &mut self,
        event: &ScrollEvent,
        zoom: f32,
        contain: bool,
        instant: bool,
        now: Instant,
    ) -> Wheel {
        let (delta, smooth) = match event.delta {
            ScrollDelta::Lines(lines) => (-lines.y * line_step() * zoom, !instant),
            // Тачпад: точные px без плавности.
            ScrollDelta::Pixels(pixels) => (-f32::from(pixels.y), false),
        };
        let held = if contain { Wheel::Held } else { Wheel::Pass };
        let max = self.max();
        if delta == 0. || max <= 0. {
            return held;
        }
        let from = self.target().clamp(0., max);
        let to = (from + delta).clamp(0., max);
        if to == from {
            return held;
        }
        let duration = if smooth {
            wheel_duration((to - self.shown()) / zoom)
        } else {
            Duration::ZERO
        };
        self.go(to, duration, WHEEL_CURVE, now);
        Wheel::Scrolled
    }

    /// Полоса в CSS px: видимое, всё, прокрутка, дорожка и ползунок.
    fn geometry(&self, bar_height: f32, look: Look, zoom: f32) -> Geometry {
        let viewport = f32::from(self.handle().bounds().size.height) / zoom;
        let max = self.max() / zoom;
        let top = self.shown() / zoom;
        let track = (bar_height - 2. * look.margin).max(0.);
        Geometry {
            viewport,
            max,
            top,
            track,
            thumb: thumb(viewport, viewport + max, top, track, look.border),
        }
    }
}

struct Geometry {
    viewport: f32,
    max: f32,
    top: f32,
    track: f32,
    thumb: Option<(f32, f32)>,
}

/// Состояние прокрутки одной области: вид держит его полем и отдаёт [`ScrollArea`] каждый кадр
/// (клон — то же состояние).
#[derive(Clone, Default)]
pub struct Scroll(Rc<RefCell<State>>);

impl Scroll {
    pub fn new() -> Scroll {
        Scroll::default()
    }

    /// Хендл GPUI области (сдвиг, `bounds`, `scroll_to_item` у детей).
    pub fn handle(&self) -> ScrollHandle {
        self.0.borrow().handle()
    }

    /// Хендл списка `uniform_list` (`scroll_to_item`); [`ScrollArea::list`] подключает его сам.
    pub fn list_handle(&self) -> UniformListScrollHandle {
        self.0.borrow().list.clone()
    }

    /// scrollTop, px окна.
    pub fn top(&self) -> Pixels {
        Pixels::from(self.0.borrow().shown())
    }

    /// scrollTop этого кадра: идущая плавная прокрутка — шагом к `now` (как в отрисовке
    /// области), px окна.
    pub fn top_at(&self, now: Instant) -> Pixels {
        let mut state = self.0.borrow_mut();
        state.step(now);
        Pixels::from(state.shown())
    }

    /// Наибольший scrollTop (по прошлому кадру), px окна.
    pub fn max_top(&self) -> Pixels {
        Pixels::from(self.0.borrow().max())
    }

    /// `scrollTo({top})`: сразу или плавно (`scroll-behavior: smooth`); `top` — px окна. Потом
    /// перерисовать вид (`cx.notify()`). Сверху цель не прижимается: до первой раскладки или
    /// после роста содержимого наибольший сдвиг прошлого кадра устарел, к краю её прижмёт
    /// раскладка GPUI (цель за краем — плавная прокрутка дойдёт до края раньше конца перехода).
    pub fn scroll_to(&self, top: Pixels, smooth: bool, window: &Window, cx: &App) {
        let zoom = zoom(window);
        let now = cx.background_executor().now();
        let mut state = self.0.borrow_mut();
        let to = f32::from(top).max(0.);
        let duration = if smooth && !cx.reduce_motion() {
            smooth_duration((to - state.shown()) / zoom)
        } else {
            Duration::ZERO
        };
        state.go(to, duration, SMOOTH_CURVE, now);
    }
}

/// Область прокрутки с полосой 0.4.9. Размер и место — стилями самой области (`Styled`);
/// содержимое — дети (`ParentElement`) или длинный список ([`ScrollArea::list`]).
#[derive(IntoElement)]
pub struct ScrollArea {
    id: ElementId,
    scroll: Scroll,
    bar: Bar,
    contain: bool,
    fade: Option<Hsla>,
    style: StyleRefinement,
    children: Vec<AnyElement>,
    list: Option<UniformList>,
}

impl ScrollArea {
    pub fn new(id: impl Into<ElementId>, scroll: &Scroll) -> ScrollArea {
        ScrollArea {
            id: id.into(),
            scroll: scroll.clone(),
            bar: Bar::default(),
            contain: false,
            fade: None,
            style: StyleRefinement::default(),
            children: Vec::new(),
            list: None,
        }
    }

    pub fn bar(mut self, bar: Bar) -> Self {
        self.bar = bar;
        self
    }

    /// `overscroll-behavior: contain`: колесо у края не уходит внешней области.
    pub fn contain(mut self) -> Self {
        self.contain = true;
        self
    }

    /// Затухание краёв (32 px сверху, 48 снизу), пока за краем есть содержимое: градиент цвета
    /// подложки `color` поверх (в 0.4.9 — `mask-image`, в GPUI масок нет).
    pub fn fade(mut self, color: Hsla) -> Self {
        self.fade = Some(color);
        self
    }

    /// Длинный список вместо детей; хендл [`Scroll::list_handle`] подключается сам.
    pub fn list(mut self, list: UniformList) -> Self {
        self.list = Some(list);
        self
    }
}

impl Styled for ScrollArea {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for ScrollArea {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ScrollArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let zoom = zoom(window);
        let now = cx.background_executor().now();
        let look = self.bar.look();
        let (gutter, fades) = {
            let mut state = self.scroll.0.borrow_mut();
            state.step(now);
            // HomePanel.onScroll: край «чистый», пока до него не больше 1 px.
            let (top, max) = (state.shown(), state.max());
            state.fades[0].set(top > zoom, now);
            state.fades[1].set(max - top > zoom, now);
            (
                state.gutter && look.thickness > 0.,
                [state.fades[0].value(now), state.fades[1].value(now)],
            )
        };
        let reserve = u(if gutter { look.thickness } else { 0. });
        let wheel = wheel_layer(self.scroll.clone(), self.contain, self.list.is_some());
        let (body, wheel_over_list) = match self.list {
            // Слушатель колеса — после списка: строки списка своих областей не держат.
            Some(list) => (
                list.track_scroll(&self.scroll.list_handle())
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .scrollbar_width(reserve)
                    .into_any_element(),
                Some(wheel),
            ),
            None => (
                div()
                    .id(self.id)
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.handle())
                    .scrollbar_width(reserve)
                    // Колесо — первым ребёнком: его слушатель идёт после вложенных областей, но
                    // до встроенного колеса GPUI этой области.
                    .child(wheel)
                    .children(self.children)
                    .into_any_element(),
                None,
            ),
        };
        // Высота окна прокрутки — от колонки корня, не `height: 100%`: taffy не разрешает процент,
        // когда корень сжат в колонке с одним `max-height`, а сама колонка стоит в блоке или по
        // центру флекса (окно экспорта «Демо»): окно прокрутки росло по содержимому.
        let mut root = div().relative().flex().flex_col();
        root.style().refine(&self.style);
        root.child(body)
            .children(wheel_over_list)
            .when(gutter, |root| {
                root.child(bar_layer(self.scroll.clone(), look))
            })
            .when_some(self.fade, |root, color| {
                root.children(
                    [(true, fades[0]), (false, fades[1])]
                        .into_iter()
                        .filter(|(_, alpha)| *alpha > 0.)
                        .map(|(top, alpha)| fade_layer(top, color, alpha)),
                )
            })
    }
}

/// Колесо и наведение области. Хитбокс — видимое окно области: обрезка прокрутки (слой внутри
/// неё) или своя коробка (слой поверх списка).
fn wheel_layer(scroll: Scroll, contain: bool, over_list: bool) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            let area = if over_list {
                bounds
            } else {
                window.content_mask().bounds
            };
            window.insert_hitbox(area, HitboxBehavior::Normal)
        },
        move |_, hitbox, window, cx| {
            let view = window.current_view();
            let now = cx.background_executor().now();
            {
                let mut state = scroll.0.borrow_mut();
                // overflow-y: auto — место под полосу появляется со следующего кадра.
                let overflows = state.max() > 0.;
                let moving =
                    state.motion.is_some() || state.fades.iter().any(|fade| fade.moving(now));
                let gutter = overflows != state.gutter;
                state.gutter = overflows;
                if gutter || moving {
                    crate::motion::monitor_frame(window);
                }
                state.area_hot = hitbox.is_hovered(window);
            }
            let hover = (scroll.clone(), hitbox.clone());
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                let (scroll, hitbox) = &hover;
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let hot = hitbox.is_hovered(window);
                let changed = std::mem::replace(&mut scroll.0.borrow_mut().area_hot, hot) != hot;
                if changed {
                    cx.notify(view);
                }
            });
            window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.should_handle_scroll(window) {
                    return;
                }
                let now = cx.background_executor().now();
                let instant = cx.reduce_motion();
                let result =
                    scroll
                        .0
                        .borrow_mut()
                        .wheel(event, zoom(window), contain, instant, now);
                match result {
                    Wheel::Scrolled => {
                        cx.stop_propagation();
                        cx.notify(view);
                    }
                    Wheel::Held => cx.stop_propagation(),
                    Wheel::Pass => {}
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .when(over_list, |layer| layer.size_full())
}

/// Полоса: дорожка, ползунок, мышь. Одна неподвижная коробка на всю высоту области.
fn bar_layer(scroll: Scroll, look: Look) -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, _| {
            let view = window.current_view();
            let scale = zoom(window);
            let height = f32::from(bounds.size.height) / scale;
            let at = |css: f32| zpx(scale, css);
            {
                let state = scroll.0.borrow();
                let geometry = state.geometry(height, look, scale);
                if let Some((alpha, radius)) = look.track {
                    let track = Bounds::new(
                        point(bounds.left(), bounds.top() + at(look.margin)),
                        size(bounds.size.width, at(geometry.track)),
                    );
                    window.paint_quad(fill(track, white().opacity(alpha)).corner_radii(at(radius)));
                }
                if let Some((start, length)) = geometry.thumb {
                    let hot = if look.area_hover {
                        state.area_hot || state.drag.is_some()
                    } else {
                        state.thumb_hot || state.drag.is_some()
                    };
                    let alpha = if hot { look.hot } else { look.thumb };
                    let thumb = Bounds::new(
                        point(
                            bounds.left() + at(look.border),
                            bounds.top() + at(look.margin + start + look.border),
                        ),
                        size(
                            at(look.thickness - 2. * look.border),
                            at((length - 2. * look.border).max(0.)),
                        ),
                    );
                    window.paint_quad(
                        fill(thumb, white().opacity(alpha)).corner_radii(at(look.radius)),
                    );
                }
            }
            // Где ползунок у точки `y` окна: выше, на нём, ниже (`None` — ползунка нет).
            let place = {
                let scroll = scroll.clone();
                move |y: Pixels, zoom: f32| {
                    let state = scroll.0.borrow();
                    let geometry = state.geometry(height, look, zoom);
                    let (start, length) = geometry.thumb?;
                    let local = f32::from(y - bounds.top()) / zoom - look.margin;
                    Some(local.partial_cmp(&start).map_or(0, |order| {
                        if order.is_lt() {
                            -1
                        } else if local < start + length {
                            0
                        } else {
                            1
                        }
                    }))
                }
            };
            let down = (scroll.clone(), hitbox.clone(), place.clone());
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                let (scroll, hitbox, place) = &down;
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                cx.stop_propagation();
                window.prevent_default();
                let zoom = zoom(window);
                let Some(side) = place(event.position.y, zoom) else {
                    return;
                };
                {
                    let mut state = scroll.0.borrow_mut();
                    if side == 0 {
                        state.motion = None;
                        state.drag = Some(Drag {
                            pointer: event.position.y,
                            top: state.shown(),
                        });
                        window.capture_pointer(hitbox.id);
                    } else {
                        let geometry = state.geometry(height, look, zoom);
                        let page = page_step(geometry.viewport) * side as f32;
                        let to = (geometry.top + page).clamp(0., geometry.max) * zoom;
                        state.go(
                            to,
                            Duration::ZERO,
                            SMOOTH_CURVE,
                            cx.background_executor().now(),
                        );
                    }
                }
                cx.notify(view);
            });
            let moved = (scroll.clone(), hitbox.clone());
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                let (scroll, hitbox) = &moved;
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let zoom = zoom(window);
                let drag = scroll.0.borrow().drag;
                if let Some(drag) = drag {
                    let state = scroll.0.borrow();
                    let geometry = state.geometry(height, look, zoom);
                    if let Some((_, length)) = geometry.thumb
                        && geometry.track > length
                    {
                        let dy = f32::from(event.position.y - drag.pointer) / zoom;
                        let top = drag.top / zoom + dy * geometry.max / (geometry.track - length);
                        state.set_top(top.clamp(0., geometry.max) * zoom);
                        drop(state);
                        cx.notify(view);
                    }
                    return;
                }
                let hot = hitbox.is_hovered(window) && place(event.position.y, zoom) == Some(0);
                let changed = std::mem::replace(&mut scroll.0.borrow_mut().thumb_hot, hot) != hot;
                if changed {
                    cx.notify(view);
                }
            });
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                if scroll.0.borrow_mut().drag.take().is_some() {
                    window.release_pointer();
                    cx.notify(view);
                }
            });
        },
    )
    .absolute()
    .top_0()
    .right_0()
    .h_full()
    .w(u(look.thickness))
}

/// Затухание края: цвет подложки у края → прозрачный.
fn fade_layer(top: bool, color: Hsla, alpha: f32) -> impl IntoElement {
    let edge = linear_color_stop(color, 0.);
    let clear = linear_color_stop(color.opacity(0.), 1.);
    // Угол как в CSS: 180° — сверху вниз, 0° — снизу вверх.
    let (angle, height) = if top {
        (180., FADE_TOP)
    } else {
        (0., FADE_BOTTOM)
    };
    div()
        .absolute()
        .left_0()
        .right_0()
        .h(u(height))
        .when(top, |layer| layer.top_0())
        .when(!top, |layer| layer.bottom_0())
        .bg(linear_gradient(angle, edge, clear))
        .opacity(alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Modifiers, Render, TestAppContext};

    fn at(x: f32, y: f32) -> gpui::Point<Pixels> {
        point(zpx(1., x), zpx(1., y))
    }

    fn wheel(position: gpui::Point<Pixels>) -> ScrollEvent {
        // Щелчок колеса на Windows: 3 строки (SPI_GETWHEELSCROLLLINES по умолчанию).
        ScrollEvent {
            position,
            delta: ScrollDelta::Lines(point(0., -3.)),
            ..Default::default()
        }
    }

    /// Внешняя область, в ней сверху внутренняя (300 из 1000 px) и 3000 px ниже.
    struct Nested {
        outer: Scroll,
        inner: Scroll,
    }

    impl Render for Nested {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            ScrollArea::new("outer", &self.outer)
                .size_full()
                .child(
                    ScrollArea::new("inner", &self.inner)
                        .h(u(300.))
                        .child(div().h(u(1000.))),
                )
                .child(div().h(u(3000.)))
        }
    }

    #[gpui::test]
    fn wheel_scrolls_the_inner_area_then_chains_out(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|_, _| Nested {
            outer: Scroll::new(),
            inner: Scroll::new(),
        });
        cx.run_until_parked();
        let (outer, inner) = view.read_with(cx, |view, _| (view.outer.clone(), view.inner.clone()));
        let notch = 3. * line_step();
        cx.simulate_event(wheel(at(50., 50.)));
        assert_eq!(f32::from(inner.top()), notch);
        assert_eq!(f32::from(outer.top()), 0.);
        while inner.top() < inner.max_top() {
            cx.simulate_event(wheel(at(50., 50.)));
        }
        assert_eq!(f32::from(outer.top()), 0.);
        // Внутренняя у края — колесо уходит внешней (contain нет).
        cx.simulate_event(wheel(at(50., 50.)));
        assert_eq!(f32::from(outer.top()), notch);
        assert_eq!(inner.top(), inner.max_top());
    }

    struct Rows {
        scroll: Scroll,
    }

    impl Render for Rows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let rows = gpui::uniform_list("rows", 500, |range, _, _| {
                range.map(|_| div().h(u(20.))).collect::<Vec<_>>()
            });
            ScrollArea::new("rows", &self.scroll).size_full().list(rows)
        }
    }

    #[gpui::test]
    fn wheel_scrolls_a_uniform_list_by_chromium_steps(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|_, _| Rows {
            scroll: Scroll::new(),
        });
        cx.run_until_parked();
        let scroll = view.read_with(cx, |view, _| view.scroll.clone());
        cx.simulate_event(wheel(at(50., 50.)));
        cx.simulate_event(wheel(at(50., 50.)));
        assert_eq!(f32::from(scroll.top()), 6. * line_step());
    }

    struct Single {
        scroll: Scroll,
    }

    impl Render for Single {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            ScrollArea::new("single", &self.scroll)
                .bar(Bar::Thin)
                .size_full()
                .child(div().h(u(4000.)))
        }
    }

    #[gpui::test]
    fn thumb_drags_and_track_pages(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| Single {
            scroll: Scroll::new(),
        });
        cx.run_until_parked();
        // Место под полосу — со второго кадра.
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let scroll = view.read_with(cx, |view, _| view.scroll.clone());
        assert!(scroll.0.borrow().gutter);
        let bounds = scroll.handle().bounds();
        let viewport = f32::from(bounds.size.height);
        let max = f32::from(scroll.max_top());
        let (start, length) = thumb(viewport, viewport + max, 0., viewport, 0.).unwrap();
        let x = bounds.right() - zpx(1., 2.);
        let y = bounds.top() + zpx(1., start + length / 2.);
        cx.simulate_mouse_down(point(x, y), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(
            point(x, y + zpx(1., 50.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.simulate_mouse_up(
            point(x, y + zpx(1., 50.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        let dragged = 50. * max / (viewport - length);
        assert!(
            (f32::from(scroll.top()) - dragged).abs() < 0.01,
            "{:?}",
            scroll.top()
        );
        // Щелчок по дорожке ниже ползунка — страница вниз.
        cx.simulate_click(point(x, bounds.bottom() - zpx(1., 2.)), Modifiers::none());
        let paged = dragged + page_step(viewport);
        assert!(
            (f32::from(scroll.top()) - paged).abs() < 0.01,
            "{:?}",
            scroll.top()
        );
    }

    /// Окно-колонка только с `max-height` в блоке по центру подложки (DemoExportModal
    /// `.dex__window`): шапка, тело `flex: 1 1 auto; min-height: 0`, низ.
    struct Capped {
        scroll: Scroll,
    }

    impl Render for Capped {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let card = div()
                .flex()
                .flex_col()
                .w(u(300.))
                .max_h(u(300.))
                .overflow_hidden()
                .pt(u(22.))
                .gap(u(14.))
                .child(div().flex_none().h(u(40.)))
                .child(
                    ScrollArea::new("capped", &self.scroll)
                        .flex_auto()
                        .min_h_0()
                        .mx(u(-24.))
                        .child(div().flex().flex_col().px(u(24.)).child(div().h(u(1000.)))),
                )
                .child(div().id("foot").flex_none().h(u(40.)));
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(div().id("wrap").child(card))
        }
    }

    #[gpui::test]
    fn area_shrunk_by_a_capped_column_scrolls(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|_, _| Capped {
            scroll: Scroll::new(),
        });
        cx.run_until_parked();
        let scroll = view.read_with(cx, |view, _| view.scroll.clone());
        let bounds = scroll.handle().bounds();
        assert_eq!(f32::from(bounds.size.height), 300. - 22. - 80. - 28.);
        assert_eq!(f32::from(scroll.max_top()), 1000. - 170.);
        cx.simulate_event(wheel(bounds.center()));
        assert_eq!(f32::from(scroll.top()), 3. * line_step());
    }

    #[test]
    fn thumb_follows_chromium_rounding() {
        // Половина видна: ползунок — половина дорожки; вверху — 0, внизу — до конца.
        assert_eq!(thumb(300., 600., 0., 300., 0.), Some((0., 150.)));
        assert_eq!(thumb(300., 600., 300., 300., 0.), Some((150., 150.)));
        // Едва сдвинуто: начало не 0, а 1 px.
        assert_eq!(thumb(300., 600., 0.5, 300., 0.), Some((1., 150.)));
        // Всё помещается — ползунка нет; рамка не даёт ползунку стать короче двух рамок.
        assert_eq!(thumb(300., 300., 0., 300., 0.), None);
        assert_eq!(thumb(10., 100_000., 0., 10., 3.), Some((0., 6.)));
        assert_eq!(thumb(10., 100_000., 0., 4., 3.), None);
    }

    #[test]
    fn page_and_wheel_timing_match_chromium() {
        assert_eq!(page_step(400.), 350.);
        assert_eq!(page_step(0.4), 1.);
        // 100 px за щелчок: 14 − 100/60 ≈ 12,3 → 12 кадров; 600 px — 6 кадров.
        assert_eq!(wheel_duration(100.), Duration::from_secs_f32(12. / 60.));
        assert_eq!(wheel_duration(-600.), Duration::from_secs_f32(6. / 60.));
        assert_eq!(smooth_duration(144.), Duration::from_secs_f32(12. / 60.));
        assert_eq!(smooth_duration(10_000.), Duration::from_secs_f32(30. / 60.));
    }

    #[test]
    fn list_bar_is_six_px_with_area_hover() {
        let look = Bar::List.look();
        assert_eq!((look.thickness, look.thumb, look.hot), (6., 0.28, 0.45));
        assert!(look.area_hover && look.track.is_none());
        assert_eq!(Bar::Hidden.thickness(), 0.);
    }

    #[test]
    fn fade_eases_over_160_ms() {
        let start = Instant::now();
        let mut fade = Fade::default();
        assert_eq!(fade.value(start), 0.);
        fade.set(true, start);
        let half = fade.value(start + FADE_TIME / 2);
        assert!(half > 0.5 && half < 1., "{half}");
        assert!(fade.moving(start + FADE_TIME / 2));
        assert_eq!(fade.value(start + FADE_TIME), 1.);
        assert!(!fade.moving(start + FADE_TIME));
    }
}
