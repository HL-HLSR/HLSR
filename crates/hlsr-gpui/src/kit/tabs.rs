//! Вкладки `role="tablist"` 0.4.9 с подсветкой `.l-ink` (`tab-ink.js`, launcher.css): полоса под
//! выбранной вкладкой перетекает к новой; передний край (в сторону новой вкладки) идёт
//! `--l-ink-lead` (.23 с), задний догоняет за `--l-ink-trail` (.35 с), кривая `--l-ease-swipe`.
//! Новое место без смены вкладки (первый замер, размер окна, язык) — без анимации (`is-still`).
//!
//! Клавиши — как `onTabKey`: ←/→ по кругу, Home/End; выбор сразу, фокус идёт за ним; в Tab
//! стоит только выбранная (`tabindex` 0/−1). Оформление — [`TabStyle`] (окна настроек и
//! профиля, «Главная»); содержимое страниц и их `l-slide-*` — у места
//! (`motion::swap(…, SLIDE_NEXT|PREV, …)`, направление — [`direction`]).

use super::button::{Handler, child_id, focus_visible, outline};
use crate::icons::fa_glyph;
use crate::motion::{Timing, hover, mix, registry, tween};
use crate::paint::shadow;
use crate::scale::{u, zoom};
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ElementId, FocusHandle, FontWeight, Hsla, IntoElement, KeyDownEvent, RenderOnce, Role,
    SharedString, Styled, Window, div, prelude::*,
};
use std::rc::Rc;

/// Вкладка: подпись и значок (классы `<i>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub label: SharedString,
    pub icon: Option<&'static str>,
    /// Счётчик после подписи (`.profile__tab-count`, «1/3» у «Достижений» профиля).
    pub count: Option<SharedString>,
}

impl Tab {
    pub fn new(label: impl Into<SharedString>) -> Tab {
        Tab {
            label: label.into(),
            icon: None,
            count: None,
        }
    }

    pub fn icon(mut self, classes: &'static str) -> Tab {
        self.icon = Some(classes);
        self
    }

    pub fn count(mut self, count: impl Into<SharedString>) -> Tab {
        self.count = Some(count.into());
        self
    }
}

/// Оформление полосы (CSS px; вес 600 у Rubik — 700, как в Chromium).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabStyle {
    pub gap: f32,
    pub height: f32,
    pub pad_x: f32,
    pub size: f32,
    pub line: f32,
    pub icon_size: f32,
    pub icon_gap: f32,
    pub color: Hsla,
    pub hover_color: Hsla,
    pub active_color: Hsla,
    /// Цвет значка; `None` — как у подписи.
    pub icon_color: Option<Hsla>,
    pub icon_active_color: Option<Hsla>,
    pub focus_color: Hsla,
    pub focus_offset: f32,
    pub focus_radius: f32,
    /// `bottom` подсветки.
    pub ink_bottom: f32,
    /// `border-bottom: 1px` полосы.
    pub rule: Option<Hsla>,
}

impl TabStyle {
    /// `.settings-modal__tab` / `.profile__tab`: 48 px, поле 12, 13,5 px, значок 12 px
    /// `--l-text-faint` (у выбранной `--l-accent`), контур фокуса внутрь на 4 px.
    pub fn modal(theme: &Theme) -> TabStyle {
        TabStyle {
            gap: 8.,
            height: 48.,
            pad_x: 12.,
            size: 13.5,
            line: 20.,
            icon_size: 12.,
            icon_gap: 8.,
            color: theme.color("l-text-dim"),
            hover_color: theme.color("l-text"),
            active_color: theme.color("l-text"),
            icon_color: Some(theme.color("l-text-faint")),
            icon_active_color: Some(theme.color("l-accent")),
            focus_color: theme.color("l-accent"),
            focus_offset: -4.,
            focus_radius: theme.len("l-radius-sm"),
            ink_bottom: 0.,
            rule: None,
        }
    }

    /// `.home-panel__tab`: полоса 42 px с линией снизу, промежуток 28, поле 2, 14 px;
    /// подсветка на линии (`bottom: -1px`).
    pub fn home(theme: &Theme) -> TabStyle {
        TabStyle {
            gap: 28.,
            height: 41.,
            pad_x: 2.,
            size: 14.,
            icon_color: None,
            icon_active_color: None,
            focus_offset: -2.,
            focus_radius: 0.,
            ink_bottom: -1.,
            rule: Some(theme.color("l-border")),
            ..TabStyle::modal(theme)
        }
    }
}

/// `direction()` tab-ink.js: правее или та же — `true` (`next`).
pub fn direction(from: usize, to: usize) -> bool {
    to >= from
}

/// Класс подсветки: `is-still`, `is-next`, `is-prev`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Motion {
    #[default]
    Still,
    Next,
    Prev,
}

#[derive(Default)]
struct TabsState {
    handles: Vec<FocusHandle>,
    /// `inkBox`: отступы вкладок от краёв полосы, CSS px (прошлый кадр).
    edges: Vec<(f32, f32)>,
    last: Option<usize>,
    motion: Motion,
}

#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    tabs: Vec<Tab>,
    active: usize,
    style: TabStyle,
    label: Option<SharedString>,
    focus_preview: bool,
    on_select: Option<Handler<usize>>,
}

pub fn tabs(id: impl Into<ElementId>, tabs: Vec<Tab>, active: usize, style: TabStyle) -> Tabs {
    Tabs {
        id: id.into(),
        tabs,
        active,
        style,
        label: None,
        focus_preview: false,
        on_select: None,
    }
}

impl Tabs {
    /// `aria-label` полосы.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Контур `:focus-visible` на выбранной вкладке (витрина).
    pub fn focus_preview(mut self, on: bool) -> Self {
        self.focus_preview = on;
        self
    }

    /// Выбрана вкладка (щелчок или клавиша); место меняет `active`.
    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Tabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Tabs {
            id,
            tabs,
            active,
            style,
            label,
            focus_preview,
            on_select,
        } = self;
        let count = tabs.len();
        let active = active.min(count.saturating_sub(1));
        let state = window.use_keyed_state(child_id(&id, "tabs"), cx, |_, _| TabsState::default());
        let (handles, ink, motion) = state.update(cx, |s, cx| {
            s.handles
                .resize_with(count, || cx.focus_handle().tab_stop(true));
            if s.last != Some(active) {
                s.motion = match s.last {
                    Some(prev) if !s.edges.is_empty() => {
                        if direction(prev, active) {
                            Motion::Next
                        } else {
                            Motion::Prev
                        }
                    }
                    _ => Motion::Still,
                };
                s.last = Some(active);
            }
            (s.handles.clone(), s.edges.get(active).copied(), s.motion)
        });
        let theme = cx.global::<Theme>();
        let accent = theme.color("l-accent");
        let zoom = zoom(window);
        let tokens = &registry().tokens;
        let timing = |ms| Timing::new(ms, tokens.l_ease_swipe);
        let (lead, trail) = (timing(tokens.l_ink_lead), timing(tokens.l_ink_trail));
        // CSS: передний край — lead, задний — trail; is-still — без перехода.
        let (left_t, right_t) = match motion {
            Motion::Still => (timing(0.), timing(0.)),
            Motion::Next => (trail, lead),
            Motion::Prev => (lead, trail),
        };
        let ink = ink.map(|(left, right)| {
            let right_id = child_id(&id, "ink-right");
            let selector = format!("{id}/ink");
            tween(child_id(&id, "ink-left"), left, left_t, move |left| {
                tween(right_id, right, right_t, move |right| {
                    div()
                        .debug_selector(|| selector)
                        .absolute()
                        .left(u(left))
                        .right(u(right))
                        .bottom(u(style.ink_bottom))
                        .h(u(2.))
                        .rounded_t(u(2.))
                        .bg(accent)
                        .shadow(vec![shadow(zoom, 0., 0., 12., 0., accent.opacity(0.6))])
                })
            })
        });

        let mut bar = div().on_children_prepainted(move |bounds, window, cx| {
            let Some(probe) = bounds.get(count) else {
                return;
            };
            let zoom = crate::scale::zoom(window);
            let edges: Vec<(f32, f32)> = bounds[..count]
                .iter()
                .map(|tab| {
                    (
                        f32::from(tab.left() - probe.left()) / zoom,
                        f32::from(probe.right() - tab.right()) / zoom,
                    )
                })
                .collect();
            let moved = state.update(cx, |s, _| {
                if s.edges == edges {
                    return false;
                }
                // Новое место без смены вкладки — без анимации (measureInk('still')).
                if !s.edges.is_empty() {
                    s.motion = Motion::Still;
                }
                s.edges = edges;
                true
            });
            if moved {
                crate::motion::monitor_frame(window);
            }
        });
        for (index, tab) in tabs.into_iter().enumerate() {
            let selected = index == active;
            let handle = handles[index].clone().tab_stop(selected);
            let ring_on = (focus_preview && selected) || focus_visible(&handle, window);
            let on_select = on_select.clone();
            let selector = format!("{id}/tab-{index}");
            let meta = crate::live::Meta::new("tab", SharedString::from(format!("{id}-{index}")))
                .label(Some(tab.label.clone()))
                .focused(handle.is_focused(window))
                .checked(selected);
            // Цвет идёт `--l-fast` (у «Главной» перехода нет — здесь он тоже есть).
            let tab = hover(("tab-hover", index), Timing::fast(), move |t| {
                let color = if selected {
                    style.active_color
                } else {
                    mix(style.color, style.hover_color, t)
                };
                let icon_color = if selected {
                    style.icon_active_color
                } else {
                    style.icon_color
                };
                div()
                    .id(("tab", index))
                    .debug_selector(|| selector)
                    .role(Role::Tab)
                    .button_text()
                    .aria_selected(selected)
                    .aria_label(tab.label.clone())
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(style.icon_gap))
                    .h(u(style.height))
                    .px(u(style.pad_x))
                    .font_family(RUBIK)
                    .text_size(u(style.size))
                    .line_height(u(style.line))
                    .font_weight(FontWeight::BOLD)
                    .whitespace_nowrap()
                    .text_color(color)
                    .track_focus(&handle)
                    .cursor_pointer()
                    .when_some(tab.icon, |el, icon| {
                        el.child(
                            fa_glyph(icon, style.icon_size)
                                .when_some(icon_color, |el, color| el.text_color(color)),
                        )
                    })
                    .child(tab.label)
                    // `.profile__tab-count`: от 20 px, поля 6, скругление 10, 11/18, `--l-text-dim`.
                    .when_some(tab.count, |el, count| {
                        el.child(
                            div()
                                .min_w(u(20.))
                                .px(u(6.))
                                .rounded(u(10.))
                                .bg(gpui::hsla(0., 0., 1., 0.07))
                                .text_size(u(11.))
                                .line_height(u(18.))
                                .text_color(style.color)
                                .flex()
                                .justify_center()
                                .child(count),
                        )
                    })
                    .when_some(on_select, |el, handler| {
                        el.on_click(move |_, window, cx| handler(&index, window, cx))
                    })
                    .when(ring_on, |el| {
                        el.child(outline(
                            zoom,
                            style.focus_radius,
                            style.focus_offset,
                            2.,
                            style.focus_color,
                        ))
                    })
            });
            // Цель шагов стенда `<id>-<номер>`: раскладка и рамка — самой вкладки.
            bar = bar.child(crate::snap::target(
                format!("{id}-{index}"),
                crate::live::mark(meta, tab),
            ));
        }
        // Замер полосы (`inkBox` считает от её краёв) и сама подсветка.
        bar = bar.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .when_some(ink, |el, ink| el.child(ink)),
        );
        let keys = handles;
        bar.id(id)
            .role(Role::TabList)
            .when_some(label, |el, label| el.aria_label(label))
            .relative()
            .flex()
            .gap(u(style.gap))
            .when_some(style.rule, |el, color| el.border_b_1().border_color(color))
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if count == 0 {
                    return;
                }
                let next = match event.keystroke.key.as_str() {
                    "right" => (active + 1) % count,
                    "left" => (active + count - 1) % count,
                    "home" => 0,
                    "end" => count - 1,
                    _ => return,
                };
                cx.stop_propagation();
                window.focus(&keys[next], cx);
                if let Some(handler) = &on_select {
                    handler(&next, window, cx);
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext};
    use std::time::Duration;

    struct Bar {
        active: usize,
        picked: Vec<usize>,
    }

    impl Render for Bar {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let style = TabStyle::modal(cx.global::<Theme>());
            let list = vec![
                Tab::new("Общие").icon("fas fa-sliders-h"),
                Tab::new("Загрузки").icon("fas fa-download"),
                Tab::new("Интерфейс").icon("fas fa-palette"),
            ];
            div()
                .size_full()
                .p(u(20.))
                .child(tabs("bar", list, self.active, style).on_select(cx.listener(
                    |this, index: &usize, _, cx| {
                        this.picked.push(*index);
                        this.active = *index;
                        cx.notify();
                    },
                )))
        }
    }

    fn open(cx: &mut TestAppContext) -> (gpui::Entity<Bar>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (bar, cx) = cx.add_window_view(|_, _| Bar {
            active: 0,
            picked: Vec::new(),
        });
        // Первый кадр меряет вкладки, второй ставит подсветку.
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        (bar, cx)
    }

    fn settle(cx: &mut VisualTestContext, ms: u64) {
        cx.executor().advance_clock(Duration::from_millis(ms));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn keys_cycle_and_focus_follows(cx: &mut TestAppContext) {
        let (bar, cx) = open(cx);
        // Tab в GPUI не привязан (обход фокуса — F7): следующая остановка Tab напрямую.
        cx.update(|window, cx| window.focus_next(cx));
        cx.simulate_keystrokes("right");
        cx.simulate_keystrokes("right");
        cx.simulate_keystrokes("right");
        cx.simulate_keystrokes("left");
        cx.simulate_keystrokes("end");
        cx.simulate_keystrokes("home");
        // Фокус шёл за выбором: стрелки продолжают от новой вкладки.
        bar.update(cx, |bar, _| {
            assert_eq!(bar.picked, [1, 2, 0, 2, 2, 0]);
            assert_eq!(bar.active, 0);
        });
        // Чужие клавиши полоса не берёт.
        cx.simulate_keystrokes("up a");
        bar.update(cx, |bar, _| assert_eq!(bar.picked.len(), 6));
    }

    #[gpui::test]
    fn click_selects_and_ink_flows_lead_first(cx: &mut TestAppContext) {
        let (bar, cx) = open(cx);
        let first = cx.debug_bounds("bar/tab-0").unwrap();
        let third = cx.debug_bounds("bar/tab-2").unwrap();
        let ink = cx.debug_bounds("bar/ink").unwrap();
        // Первый замер — сразу на месте.
        assert_eq!(ink.left(), first.left());
        assert_eq!(ink.right(), first.right());

        cx.simulate_click(third.center(), Modifiers::default());
        bar.update(cx, |bar, _| assert_eq!(bar.picked, [2]));
        settle(cx, 120);
        let mid = cx.debug_bounds("bar/ink").unwrap();
        // Вправо: правый край (lead .23 с) прошёл больше пути, чем левый (trail .35 с).
        let right =
            f32::from(mid.right() - first.right()) / f32::from(third.right() - first.right());
        let left = f32::from(mid.left() - first.left()) / f32::from(third.left() - first.left());
        assert!(right > left && left > 0. && right < 1., "{left} {right}");
        settle(cx, 400);
        let end = cx.debug_bounds("bar/ink").unwrap();
        assert_eq!((end.left(), end.right()), (third.left(), third.right()));

        // Мышью по уже выбранной — выбор тот же, подсветка стоит.
        cx.simulate_mouse_down(third.center(), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(third.center(), MouseButton::Left, Modifiers::default());
        bar.update(cx, |bar, _| assert_eq!(bar.picked, [2, 2]));
    }
}
