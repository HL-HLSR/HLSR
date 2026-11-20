//! GX-набор `settings-fields.scss` 0.4.9 — разметка списка настроек «Персонализации»
//! (GhostingSettingsExtras, GameConfigEditor): разделы слева [`nav`] / [`nav_item`] с точкой
//! изменений, панель [`pane`] с шапкой [`pane_head`] и полями [`fields`], строка [`row`]
//! (`.is-changed` — полоса акцента слева, `.is-off` — бледнее, подсказка, ошибка `invalid`),
//! поле выбора [`select`] для кнопки [`super::dropdown`], концы ползунка [`slider_ends`] и счётчик
//! сброса [`count`]. Контролы строк — F8 (`toggle`, `slider`, `num_step`, `button(Look::Gx)`),
//! поле цвета — [`super::color`].
//!
//! Низкое окно (`@media (max-height: 640px)`) — [`compact`]: строки и шапка ниже, заметка —
//! одной строкой.

use super::button::{Handler, Preview, child_id, focus_handle, focus_visible, outline, white};
use crate::icons::{chevron, fa_glyph};
use crate::motion::{Timing, hover, mix, tween};
use crate::paint::shadow;
use crate::scale::{u, viewport, zoom};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::text::{MONO, RUBIK, TextExt, balance, pretty};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FontWeight, IntoElement, ParentElement,
    RenderOnce, Role, SharedString, Styled, Window, div, hsla, prelude::*, rgb,
};
use std::rc::Rc;

/// `@media (max-height: 640px)` по CSS-окну страницы.
pub fn compact(window: &Window) -> bool {
    hlsr_ui::fields::is_compact(viewport(window).1)
}

/// `.gx-nav`: колонка разделов с рамкой; прокрутку (Bar::Fields) добавляет место.
pub fn nav(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(u(2.))
        .min_h_0()
        .p(u(8.))
        .border_1()
        .border_color(theme.color("l-border"))
        .rounded(theme.rem("l-radius-sm"))
        .bg(theme.color("l-modal-nav"))
}

/// `.gx-pane`: панель полей раздела (шапка [`pane_head`] и [`fields`]).
pub fn pane(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .border_1()
        .border_color(theme.color("l-border"))
        .rounded(theme.rem("l-radius-sm"))
        .bg(theme.color("l-modal-bar"))
}

/// `.gx-fields`: прокрутка полей с полосой `settings-fields.scss` (10 px, отступ 8 у краёв).
pub fn fields(id: impl Into<ElementId>, scroll: &Scroll) -> ScrollArea {
    ScrollArea::new(id, scroll)
        .bar(Bar::Fields)
        .contain()
        .flex_1()
        .min_h_0()
}

/// `.gx-btn__count`: число изменений в кнопке сброса.
pub fn count(n: usize, theme: &Theme) -> Div {
    div()
        .min_w(u(18.))
        .px(u(6.))
        .py(u(2.))
        .rounded(u(9.))
        .bg(theme.color("accent"))
        .text_color(theme.color("accent-contrast-text"))
        .text_px(11., 11. * 1.2)
        .tnum()
        .font_weight(FontWeight::SEMIBOLD)
        .flex()
        .justify_center()
        .child(n.to_string())
}

/// `.gx-slider`: ползунок `slider` с подписями концов (`.gx-slider__end`, мин. 44 px, правый —
/// по правому краю).
pub fn slider_ends(
    left: impl Into<SharedString>,
    slider: impl IntoElement,
    right: impl Into<SharedString>,
    theme: &Theme,
) -> Div {
    let end = |text: SharedString| {
        div()
            .flex_none()
            .min_w(u(44.))
            .text_px(12., 12. * 1.15)
            .text_color(theme.color("l-text-faint"))
            .child(text)
    };
    div()
        .flex()
        .items_center()
        .gap(u(12.))
        .mt(u(2.))
        .child(end(left.into()))
        .child(div().flex_1().child(slider))
        .child(end(right.into()).flex().justify_end())
}

/// `.gx-color code`: HEX цвета заглавными рядом с полем цвета.
pub fn code(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .font_family(MONO)
        .text_size(u(12.5))
        .text_color(theme.color("l-text-dim"))
        .child(text.into())
}

/// Шапка `.gx-pane__head`: заголовок, кнопка сброса справа, заметка во всю ширину; дети —
/// ниже заметки (`.gx-pane__link`).
#[derive(IntoElement)]
pub struct PaneHead {
    title: SharedString,
    reset: Option<AnyElement>,
    note: Option<SharedString>,
    packs: bool,
    children: Vec<AnyElement>,
}

pub fn pane_head(title: impl Into<SharedString>) -> PaneHead {
    PaneHead {
        title: title.into(),
        reset: None,
        note: None,
        packs: false,
        children: Vec::new(),
    }
}

impl PaneHead {
    /// `.gx-pane__reset`: обычно `button(_, Look::Gx)` со значком и [`count`].
    pub fn reset(mut self, reset: impl IntoElement) -> Self {
        self.reset = Some(reset.into_any_element());
        self
    }

    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// `.gx-pane--packs .gx-pane__head`: поля 8 по высоте.
    pub fn packs(mut self, packs: bool) -> Self {
        self.packs = packs;
        self
    }
}

impl ParentElement for PaneHead {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl RenderOnce for PaneHead {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let tight = compact(window);
        let faint = theme.color("l-text-faint");
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(u(3.))
            .px(u(14.))
            .pt(u(if self.packs {
                8.
            } else if tight {
                9.
            } else {
                10.
            }))
            .pb(u(if self.packs || tight { 8. } else { 10. }))
            .border_b_1()
            .border_color(theme.color("l-border"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_px(15., 15. * 1.3)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(balance(self.title)),
                    )
                    .children(self.reset),
            )
            .when_some(self.note, |head, note| {
                let note_el = div().text_px(12., 12. * 1.4).text_color(faint);
                // В одну строку (`line-clamp: 1`) переносить нечего: строка GPUI с многоточием.
                head.child(if tight {
                    note_el.clamp(1).child(note)
                } else {
                    note_el.child(pretty(note))
                })
            })
            .children(self.children)
    }
}

/// Пункт раздела `.gx-nav__item`: значок 16 px (точка изменений `__dot` на нём), подпись;
/// `active` — `--accent-soft`, значок `--accent`.
#[derive(IntoElement)]
pub struct NavItem {
    id: ElementId,
    icon: SharedString,
    label: SharedString,
    active: bool,
    dot: bool,
    preview: Preview,
    on_click: Option<Handler<ClickEvent>>,
}

pub fn nav_item(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
) -> NavItem {
    NavItem {
        id: id.into(),
        icon: icon.into(),
        label: label.into(),
        active: false,
        dot: false,
        preview: Preview::default(),
        on_click: None,
    }
}

impl NavItem {
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// В разделе есть изменённые настройки (число — в кнопке сброса).
    pub fn dot(mut self, dot: bool) -> Self {
        self.dot = dot;
        self
    }

    pub fn preview(mut self, preview: Preview) -> Self {
        self.preview = preview;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for NavItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let [dim, text, soft, accent, focus_color, nav_bg] = [
            "l-text-dim",
            "l-text",
            "accent-soft",
            "accent",
            "accent-focus",
            "l-modal-nav",
        ]
        .map(|key| theme.color(key));
        // Liquid Glass: неактивный пункт меню редактора светлее (лежит на преломлённом кадре).
        let dim = if theme.liquid() {
            rgb(crate::liquid::NAV_TEXT).into()
        } else {
            dim
        };
        let zoom = zoom(window);
        let tight = compact(window);
        let focus = focus_handle(&self.id, window, cx);
        let ring_on = self.preview.focus || focus_visible(&focus, window);
        let NavItem {
            id,
            icon,
            label,
            active,
            dot,
            preview,
            on_click,
        } = self;
        let inner = id.clone();
        let meta = crate::live::Meta::new("tab", id.clone())
            .label(Some(label.clone()))
            .focused(focus.is_focused(window))
            .checked(active);
        let element = hover(id, Timing::fast(), move |t| {
            let t = if preview.hover { 1. } else { t };
            let (bg, color) = if active {
                (soft, text)
            } else {
                (white(0.06 * t), mix(dim, text, t))
            };
            // Точка обведена цветом под ней: у активного — тёмно-синий (#0f2a3d в 0.4.9).
            let under = if active { rgb(0x0f2a3d).into() } else { nav_bg };
            div()
                .id(inner)
                .role(Role::Tab)
                .button_text()
                .aria_selected(active)
                .aria_label(label.clone())
                .track_focus(&focus)
                .relative()
                .flex()
                .items_center()
                .gap(u(9.))
                .min_h(u(if tight { 34. } else { 38. }))
                .px(u(8.))
                .py(u(if tight { 5. } else { 7. }))
                .rounded(u(8.))
                .bg(bg)
                .text_color(color)
                .font_family(RUBIK)
                .text_px(13., 13. * 1.25)
                .cursor_pointer()
                .when_some(on_click, |el, handler| {
                    el.on_click(move |event, window, cx| handler(event, window, cx))
                })
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .w(u(16.))
                        .flex()
                        .justify_center()
                        .child(fa_glyph(&icon, 13.).when(active, |i| i.text_color(accent)))
                        .when(dot, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .top(u(-4.))
                                    .right(u(-5.))
                                    .size(u(8.))
                                    .rounded_full()
                                    .bg(accent)
                                    .shadow(vec![shadow(zoom, 0., 0., 0., 2., under)]),
                            )
                        }),
                )
                .child(div().flex_1().min_w_0().child(label))
                // outline-offset: -2px — контур внутри пункта.
                .when(ring_on, |el| {
                    el.child(outline(zoom, 8., -2., 2., focus_color))
                })
        });
        crate::live::mark(meta, element)
    }
}

/// Поле выбора `.gx-input--select` — содержимое кнопки списка ([`super::dropdown`]): значение с
/// многоточием и стрелка. `open` — рамка и фон наведения, стрелка `--l-accent`; `focus` —
/// `:focus-visible` кнопки (рамка `--accent` и кольцо `--accent-soft`, как у поля).
pub fn select(
    id: impl Into<ElementId>,
    value: impl Into<SharedString>,
    open: bool,
    focus: bool,
) -> impl IntoElement {
    let value = value.into();
    let id = id.into();
    let inner = id.clone();
    hover(id, Timing::fast(), move |t| SelectBox {
        id: inner,
        value,
        open,
        focus,
        hover: t,
    })
}

#[derive(IntoElement)]
struct SelectBox {
    id: ElementId,
    value: SharedString,
    open: bool,
    focus: bool,
    hover: f32,
}

impl RenderOnce for SelectBox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let zoom = zoom(window);
        let t = if self.open { 1. } else { self.hover };
        let border = if self.focus {
            theme.color("accent")
        } else {
            mix(theme.color("l-border"), theme.color("l-border-strong"), t)
        };
        // `.open .gx-field__chev { transform: rotate(180deg); color: var(--l-accent) }`:
        // `transition: transform var(--l-med), color var(--l-fast)`.
        let hovered = mix(theme.color("l-text-dim"), theme.color("l-text"), self.hover);
        let accent = theme.color("l-accent");
        let opened = if self.open { 1. } else { 0. };
        let (color_id, turn_id) = (child_id(&self.id, "chev"), child_id(&self.id, "chev-turn"));
        let chev = tween(color_id, opened, Timing::fast(), move |lit| {
            tween(turn_id, opened, Timing::med(), move |turn| {
                chevron(10., 180. * turn, mix(hovered, accent, lit))
            })
        });
        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(u(8.))
            .w_full()
            .h(u(34.))
            .px(u(12.))
            .border_1()
            .border_color(border)
            .rounded(theme.rem("l-radius-sm"))
            .bg(mix(white(0.05), white(0.07), t))
            .when(self.focus, |el| {
                el.shadow(vec![shadow(
                    zoom,
                    0.,
                    0.,
                    0.,
                    2.,
                    theme.color("accent-soft"),
                )])
            })
            .text_color(theme.color("l-text"))
            .font_family(RUBIK)
            .text_px(13., 32.)
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.value),
            )
            .child(
                div()
                    .flex_none()
                    .w(u(12.))
                    .flex()
                    .justify_center()
                    .child(chev),
            )
    }
}

/// Строка настройки `.gx-row`: подпись и контрол в строку, ниже подсказка, ошибка, что-то во
/// всю ширину (ползунок) и `needs` (что включает настройку). `changed` — полоса акцента слева
/// (`--l-fast`), `off` — всё, кроме `needs`, бледнее (.45); сами контролы выключает место.
#[derive(IntoElement)]
pub struct Row {
    id: ElementId,
    label: SharedString,
    control: Option<AnyElement>,
    hint: Option<SharedString>,
    error: Option<SharedString>,
    needs: Option<SharedString>,
    below: Vec<AnyElement>,
    changed: bool,
    off: bool,
    card: bool,
    last: bool,
    stack: bool,
    on_label: Option<Handler<ClickEvent>>,
}

pub fn row(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Row {
    Row {
        id: id.into(),
        label: label.into(),
        control: None,
        hint: None,
        error: None,
        needs: None,
        below: Vec::new(),
        changed: false,
        off: false,
        card: false,
        last: false,
        stack: false,
        on_label: None,
    }
}

impl Row {
    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.control = Some(control.into_any_element());
        self
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// `.gx-row__error` (поле `invalid`).
    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self
    }

    pub fn needs(mut self, needs: impl Into<SharedString>) -> Self {
        self.needs = Some(needs.into());
        self
    }

    /// Во всю ширину под строкой (`.gx-slider`).
    pub fn below(mut self, el: impl IntoElement) -> Self {
        self.below.push(el.into_any_element());
        self
    }

    pub fn changed(mut self, changed: bool) -> Self {
        self.changed = changed;
        self
    }

    /// `.is-off` (`<fieldset disabled>`): строка .45, подпись не нажимается. Контрол строки — без
    /// обработчиков, но не `disabled`: у `toggle`/`slider` `disabled` даёт свои .45, вышло бы
    /// .2 вместо .45.
    pub fn off(mut self, off: bool) -> Self {
        self.off = off;
        self
    }

    /// `.gc .gx-row` (GameConfigEditor.vue): карточка со скруглёнными углами, поля 12 по
    /// бокам, линия снизу с отступами 12 (её нет у последней, изменённой и под курсором),
    /// изменённая — заливка акцентом 7 % (под курсором 10 %), метка 12…12 от краёв и 4 слева.
    pub fn card(mut self, last: bool) -> Self {
        self.card = true;
        self.last = last;
        self
    }

    /// `.gx-pane--packs .gx-row` (паки HL1): поля 6 по высоте, подпись над контролом во всю ширину.
    pub fn stack(mut self) -> Self {
        self.stack = true;
        self
    }

    /// Щелчок по подписи (`<label for>`): переключить, открыть список, поставить фокус в поле.
    pub fn on_label(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_label = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Row {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let [text, faint, accent] =
            ["l-text", "l-text-faint", "accent"].map(|key| theme.color(key));
        let tight = compact(window);
        // :focus-within — фокус у контрола строки.
        let within = window
            .use_keyed_state(child_id(&self.id, "within"), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let focused = within.contains_focused(window, cx);
        let Row {
            id,
            label,
            control,
            hint,
            error,
            needs,
            below,
            changed,
            off,
            card,
            last,
            stack,
            on_label,
        } = self;
        let note = |color| div().text_px(12., 12. * 1.4).text_color(color);
        let fade = move |el: Div| el.when(off, |el| el.opacity(0.45));
        let main = fade(if stack {
            div().flex().flex_col().gap(u(6.)).min_h(u(28.))
        } else {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(u(12.))
                .min_h(u(28.))
        })
        .child(
            div()
                .id(child_id(&id, "label"))
                .flex_1()
                .min_w_0()
                .text_color(text)
                .text_px(13.5, 13.5 * 1.3)
                .font_weight(FontWeight::MEDIUM)
                .when(!off, |el| el.cursor_pointer())
                .when_some(on_label.filter(|_| !off), |el, handler| {
                    el.on_click(move |event, window, cx| handler(event, window, cx))
                })
                .child(label),
        )
        .children(control);
        let mark = tween(
            child_id(&id, "mark"),
            f32::from(u8::from(changed)),
            Timing::fast(),
            move |t| {
                let inset = if card { 12. } else { 10. };
                div()
                    .absolute()
                    .top(u(inset))
                    .bottom(u(inset))
                    .left(u(if card { 4. } else { 0. }))
                    .w(u(3.))
                    .rounded_tr(u(3.))
                    .rounded_br(u(3.))
                    .when(card, |el| el.rounded(u(3.)))
                    .bg(accent)
                    .opacity(t)
            },
        );
        let inner = id.clone();
        hover(id, Timing::fast(), move |t| {
            let t = if focused { 1. } else { t };
            let line = hsla(0., 0., 1., 0.055);
            let row = div()
                .id(inner)
                .track_focus(&within)
                .relative()
                .flex()
                .flex_col()
                .gap(u(6.))
                .min_w_0()
                .py(u(if stack {
                    6.
                } else if tight {
                    8.
                } else {
                    10.
                }));
            let row = if card {
                let bg = if changed {
                    accent.opacity(0.07 + 0.03 * t)
                } else {
                    white(0.04 * t)
                };
                // ::after — линия с отступами 12, гаснет под курсором и у изменённой.
                let rule = (!last && !changed).then(|| {
                    div()
                        .absolute()
                        .left(u(12.))
                        .right(u(12.))
                        .bottom_0()
                        .h(gpui::px(1.))
                        .bg(line.opacity(1. - t))
                });
                row.px(u(12.)).rounded(u(8.)).bg(bg).children(rule)
            } else {
                row.px(u(14.))
                    .border_b_1()
                    .border_color(line)
                    .bg(white(0.025 * t))
            };
            row.child(mark)
                .child(main)
                // `.gx-row__hint` — `text-wrap: pretty`, `__error` и `__needs` — обычный перенос.
                .when_some(hint, |el, hint| {
                    el.child(fade(note(faint).child(pretty(hint))))
                })
                .when_some(error, |el, error| {
                    el.child(fade(note(rgb(0xff8a96).into()).child(error)))
                })
                .children(below.into_iter().map(|el| fade(div()).child(el)))
                .when_some(needs, |el, needs| el.child(note(faint).child(needs)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};

    struct Form;

    impl Render for Form {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = cx.global::<Theme>();
            pane(theme).child(pane_head("Призраки").note("…")).child(
                row("r", "Цвет")
                    .changed(true)
                    .off(true)
                    .hint("подсказка")
                    .needs("нужно")
                    .control(select("s", "Вкл", false, true)),
            )
        }
    }

    #[gpui::test]
    fn rows_render_in_every_state(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (_, cx) = cx.add_window_view(|_, _| Form);
        cx.run_until_parked();
    }
}
