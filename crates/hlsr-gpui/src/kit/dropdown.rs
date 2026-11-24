//! Выпадающий список `UiDropdown.vue` 0.4.9 вместо `<select>`: кнопка (её вид — у места,
//! [`Dropdown::toggle`]; поле GX — [`super::gx::select`]) и меню [`popup`] слоя F7.
//!
//! - Клавиши у кнопки ([`MenuNav`]): ↓/↑ открывают на первом/последнем доступном пункте, дальше
//!   по кругу с пропуском недоступных, Home/End, Enter и пробел — выбрать (у закрытого — открыть),
//!   Esc закрывает только список, Tab закрывает и идёт дальше; поиск по первым буквам (700 мс).
//!   Фокус всё время на кнопке (`aria-activedescendant`), пункт с клавиатуры держится в виду.
//! - Место — `place_dropdown` 0.4.9: переворот вверх/вниз, ширина по пунктам (шаг 4) и плюс
//!   полоса, если список листается; на низком окне, где в прокручиваемой области места нет ни
//!   вверх, ни вниз, — выход за неё в пределах окна ([`Dropdown::clip`]).
//! - Пункт: подпись с многоточием (или своё содержимое [`Dropdown::item_view`]), значок `badge` и
//!   галочка у выбранного — вместе (0.4.9), `separated` — линия над пунктом, заголовок `heading`;
//!   появление `l-rise-in` лесенкой; `title` — подсказка `hint` или подпись.
//!
//! События — [`Dropdown::on_select`] (`input` + `select` 0.4.9) с выбранным пунктом.

use super::button::{Handler, child_id, focus_visible, white};
use crate::icons::fa_glyph;
use crate::layer::{
    MenuKey, MenuNav, Outcome, Place, PopLook, TipExt, anchor_probe, menu_keys, popup_scroll,
};
use crate::motion::{Timing, hover, mix, rise_in};
use crate::paint::ring;
use crate::scale::{u, zoom};
use crate::text::{RUBIK, TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, ClickEvent, ElementId, FocusHandle, FontWeight, IntoElement,
    ParentElement, Pixels, RenderOnce, Role, SharedString, Styled, Window, canvas, div, hsla,
    point, prelude::*, rgb,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Пункт списка (`items` 0.4.9).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    pub id: SharedString,
    pub label: SharedString,
    /// `title` пункта (иначе — подпись).
    pub hint: Option<SharedString>,
    /// Пометка справа («скоро», размер эпизода).
    pub badge: Option<SharedString>,
    pub disabled: bool,
    /// Линия над пунктом.
    pub separated: bool,
}

impl Item {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Item {
        Item {
            id: id.into(),
            label: label.into(),
            ..Item::default()
        }
    }
}

/// Что видно кнопке: список открыт, фокус с клавиатуры (`:focus-visible`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToggleState {
    pub open: bool,
    pub focus: bool,
}

type ToggleView = Box<dyn FnOnce(ToggleState) -> AnyElement>;
/// Содержимое пункта по пункту и его подсветке ([`Dropdown::item_view`]).
type ItemView = Rc<dyn Fn(&Item, f32) -> AnyElement>;

#[derive(IntoElement)]
pub struct Dropdown {
    id: ElementId,
    items: Vec<Item>,
    value: Option<SharedString>,
    up: bool,
    align_right: bool,
    heading: Option<SharedString>,
    clip: Option<Bounds<Pixels>>,
    label: Option<SharedString>,
    toggle: Option<ToggleView>,
    item_view: Option<ItemView>,
    on_select: Option<Handler<Item>>,
    preview_open: bool,
}

/// Список `items`, выбран пункт с id `value`.
pub fn dropdown(
    id: impl Into<ElementId>,
    items: Vec<Item>,
    value: Option<SharedString>,
) -> Dropdown {
    Dropdown {
        id: id.into(),
        items,
        value,
        up: false,
        align_right: false,
        heading: None,
        clip: None,
        label: None,
        toggle: None,
        item_view: None,
        on_select: None,
        preview_open: false,
    }
}

impl Dropdown {
    /// `direction="up"`.
    pub fn up(mut self) -> Self {
        self.up = true;
        self
    }

    /// `align="right"`.
    pub fn align_right(mut self) -> Self {
        self.align_right = true;
        self
    }

    /// Подпись над пунктами (`heading`).
    pub fn heading(mut self, heading: impl Into<SharedString>) -> Self {
        self.heading = Some(heading.into());
        self
    }

    /// Прокручиваемая область, где стоит кнопка (px окна; рамка — [`anchor_probe`] в области).
    pub fn clip(mut self, clip: Bounds<Pixels>) -> Self {
        self.clip = Some(clip);
        self
    }

    /// `aria-label` кнопки.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Содержимое кнопки (слот `toggle`).
    pub fn toggle<E: IntoElement>(mut self, view: impl FnOnce(ToggleState) -> E + 'static) -> Self {
        self.toggle = Some(Box::new(move |state| view(state).into_any_element()));
        self
    }

    /// Содержимое пункта вместо подписи (слот `item`); значок и галочка — свои. Второй аргумент —
    /// подсветка пункта 0…1 для правил слота `.ui-dropdown__item:hover:not(:disabled) …`,
    /// `:focus-visible …` и `.selected …`: наведение (с переходом `--l-fast`), пункт с клавиатуры
    /// или выбранный.
    pub fn item_view<E: IntoElement>(mut self, view: impl Fn(&Item, f32) -> E + 'static) -> Self {
        self.item_view = Some(Rc::new(move |item, lit| view(item, lit).into_any_element()));
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&Item, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// Открыт с первого кадра (витрина и сцены без мыши).
    pub fn preview_open(mut self) -> Self {
        self.preview_open = true;
        self
    }
}

struct State {
    nav: MenuNav,
    focus: FocusHandle,
    anchor: Rc<Cell<Bounds<Pixels>>>,
    /// Прокрутка меню: новая на каждое открытие (`scrollTop` 0, как у нового меню 0.4.9).
    scroll: crate::scroll::Scroll,
    /// Пункт, который надо показать прокруткой (с клавиатуры).
    reveal: Rc<Cell<Option<usize>>>,
    /// Рамки пунктов в прошлом кадре (px окна).
    rows: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

impl State {
    fn set_open(&mut self, open: bool) {
        if open && !self.nav.open {
            self.scroll = crate::scroll::Scroll::new();
        }
        if open {
            self.nav.show();
        } else {
            self.nav.hide();
        }
    }
}

/// `focusOption`: прокрутить меню, чтобы пункт `row` был виден целиком (рамки в px окна).
fn reveal_offset(view: Bounds<Pixels>, row: Bounds<Pixels>, offset: Pixels) -> Pixels {
    if row.top() < view.top() {
        offset + (view.top() - row.top())
    } else if row.bottom() > view.bottom() {
        offset - (row.bottom() - view.bottom())
    } else {
        offset
    }
}

/// `l-rise-in` пункта: задержка `min(i, 8) × 16 мс` на ступенях реестра по 24 мс.
/// shortcut: ступень ближайшая к 16·i (ошибка ≤ 8 мс), своя лесенка — если сверка заметит.
fn rise_step(index: usize) -> usize {
    (index.min(8) * 2 + 1) / 3
}

fn emit(handler: &Option<Handler<Item>>, item: &Item, window: &mut Window, cx: &mut App) {
    if let Some(handler) = handler {
        handler(item, window, cx);
    }
}

impl RenderOnce for Dropdown {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Dropdown {
            id,
            items,
            value,
            up,
            align_right,
            heading,
            clip,
            label,
            toggle,
            item_view,
            on_select,
            preview_open,
        } = self;
        let owner = window.current_view();
        let state = window.use_keyed_state(child_id(&id, "dropdown"), cx, |_, cx| State {
            nav: {
                let mut nav = MenuNav::default();
                if preview_open {
                    nav.show();
                }
                nav
            },
            focus: cx.focus_handle().tab_stop(true),
            anchor: Rc::default(),
            scroll: crate::scroll::Scroll::new(),
            reveal: Rc::default(),
            rows: Rc::default(),
        });
        let (open, active, focus, anchor, scroll, reveal, rows) = {
            let s = state.read(cx);
            (
                s.nav.open,
                s.nav.active,
                s.focus.clone(),
                s.anchor.clone(),
                s.scroll.clone(),
                s.reveal.clone(),
                s.rows.clone(),
            )
        };
        let theme = cx.global::<Theme>();
        let zoom = zoom(window);
        let selected = items
            .iter()
            .position(|item| Some(&item.id) == value.as_ref())
            .map_or(-1, |index| index as i32);
        let items = Rc::new(items);

        // ---- пункты (строятся лениво: только пока меню открыто или закрывается) ----
        let colors = Colors::new(theme);
        let on_dismiss = {
            let state = state.clone();
            move |_window: &mut Window, cx: &mut App| {
                state.update(cx, |s, _| s.set_open(false));
                cx.notify(owner);
            }
        };
        let menu_id = id.clone();
        let menu_items = items.clone();
        let menu_state = state.clone();
        let menu_on_select = on_select.clone();
        let menu_scroll = scroll.clone();
        let menu_reveal = reveal.clone();
        let menu = popup_scroll(
            child_id(&id, "menu"),
            open,
            Place {
                anchor: anchor.get(),
                clip,
                up,
                align_right,
            },
            PopLook::DROPDOWN,
            Some(scroll),
            window,
            cx,
            on_dismiss,
            move || {
                rows.borrow_mut()
                    .resize(menu_items.len(), Bounds::default());
                let mut children: Vec<AnyElement> =
                    Vec::with_capacity(menu_items.len() + usize::from(heading.is_some()) + 1);
                if let Some(heading) = heading {
                    children.push(
                        rise_in(
                            child_id(&menu_id, "heading"),
                            0,
                            div().flex_none().px(u(12.)).py(u(4.)).child(spaced(
                                heading.to_uppercase(),
                                11.,
                                16.,
                                FontWeight::BOLD,
                                0.1,
                                colors.faint,
                            )),
                        )
                        .into_any_element(),
                    );
                }
                let item_prefix: SharedString = format!("{menu_id}/item").into();
                let rise_prefix: SharedString = format!("{menu_id}/rise").into();
                for (index, item) in menu_items.iter().enumerate() {
                    let pick = {
                        let (state, items, on_select) = (
                            menu_state.clone(),
                            menu_items.clone(),
                            menu_on_select.clone(),
                        );
                        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                            state.update(cx, |s, _| s.set_open(false));
                            emit(&on_select, &items[index], window, cx);
                            cx.notify(owner);
                        }
                    };
                    let row = item_row(RowSpec {
                        id: ElementId::NamedInteger(item_prefix.clone(), index as u64),
                        item: item.clone(),
                        selected: index as i32 == selected,
                        active: index as i32 == active,
                        view: item_view.clone(),
                        probe: rows.clone(),
                        index,
                        colors,
                        zoom,
                        pick: Rc::new(pick),
                    });
                    children.push(
                        rise_in(
                            ElementId::NamedInteger(rise_prefix.clone(), index as u64),
                            rise_step(index),
                            div().flex_none().child(row),
                        )
                        .into_any_element(),
                    );
                }
                // Последним: пункты этого кадра уже размечены — показать пункт с клавиатуры.
                let shown = menu_scroll.handle();
                let wanted = menu_reveal;
                let marks = rows;
                children.push(
                    canvas(
                        move |_, window, _| {
                            let Some(index) = wanted.get() else { return };
                            let view = shown.bounds();
                            let row = marks.borrow().get(index).copied().unwrap_or_default();
                            // Меню ещё меряется (прокрутка не подключена) — до следующего кадра.
                            if view.size.height <= Pixels::ZERO || row.size.height <= Pixels::ZERO {
                                return;
                            }
                            wanted.set(None);
                            let offset = shown.offset();
                            let next = reveal_offset(view, row, offset.y);
                            if next != offset.y {
                                shown.set_offset(point(offset.x, next));
                                crate::motion::monitor_frame(window);
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_0()
                    .into_any_element(),
                );
                div()
                    .id("listbox")
                    .role(Role::ListBox)
                    .flex()
                    .flex_col()
                    .gap(u(4.))
                    .children(children)
            },
        );

        // ---- кнопка ----
        let focused = focus_visible(&focus, window);
        let content = toggle.map(|view| {
            view(ToggleState {
                open,
                focus: focused,
            })
        });
        let on_key = {
            let (state, items, on_select, reveal) = (
                state.clone(),
                items.clone(),
                on_select.clone(),
                reveal.clone(),
            );
            move |key: &MenuKey, window: &mut Window, cx: &mut App| {
                let labels: Vec<SharedString> = items.iter().map(|i| i.label.clone()).collect();
                let disabled: Vec<bool> = items.iter().map(|i| i.disabled).collect();
                let now = cx.background_executor().now();
                let outcome = state.update(cx, |s, _| {
                    let was_open = s.nav.open;
                    let outcome = s.nav.key(key, now, &labels, &disabled, selected);
                    if s.nav.open && !was_open {
                        s.scroll = crate::scroll::Scroll::new();
                    }
                    if s.nav.open && s.nav.active >= 0 {
                        reveal.set(Some(s.nav.active as usize));
                    }
                    outcome
                });
                if let Outcome::Pick(index) = outcome.settle(cx) {
                    emit(&on_select, &items[index], window, cx);
                }
                if outcome != Outcome::Pass {
                    cx.notify(owner);
                }
            }
        };
        let click = state.clone();
        let mut meta = crate::live::Meta::new("dropdown", id.clone())
            .label(label.clone())
            .focused(focus.is_focused(window))
            .checked(open);
        if let Some(item) = usize::try_from(selected).ok().and_then(|i| items.get(i)) {
            meta = meta.value(item.label.clone());
        }
        let element = menu_keys(
            div()
                .id(id)
                .role(Role::ComboBox)
                .button_text()
                .aria_expanded(open)
                .when_some(label, |el, label| el.aria_label(label))
                .track_focus(&focus)
                .relative()
                .flex()
                .items_center()
                .cursor_pointer()
                .on_click(move |event, _window, cx| {
                    // Enter и пробел у кнопки — клавиши меню (MenuPick), не щелчок.
                    if event.is_keyboard() {
                        return;
                    }
                    click.update(cx, |s, _| {
                        let open = !s.nav.open;
                        s.set_open(open);
                    });
                    cx.notify(owner);
                })
                .children(content)
                .child(anchor_probe(anchor))
                .child(menu),
            on_key,
        );
        crate::live::mark(meta, element)
    }
}

/// Цвета пункта из темы.
#[derive(Clone, Copy)]
struct Colors {
    dim: gpui::Hsla,
    text: gpui::Hsla,
    faint: gpui::Hsla,
    soft: gpui::Hsla,
    edge: gpui::Hsla,
    accent: gpui::Hsla,
    line: gpui::Hsla,
    radius: gpui::Rems,
}

impl Colors {
    fn new(theme: &Theme) -> Colors {
        Colors {
            dim: theme.color("l-text-dim"),
            text: theme.color("l-text"),
            faint: theme.color("l-text-faint"),
            soft: theme.color("l-accent-soft"),
            // color-mix(in srgb, var(--l-accent) 45%, var(--l-border))
            edge: mix(theme.color("l-border"), theme.color("l-accent"), 0.45),
            accent: theme.color("accent"),
            line: theme.color("l-modal-border"),
            radius: theme.rem("l-radius-sm"),
        }
    }
}

struct RowSpec {
    id: ElementId,
    item: Item,
    selected: bool,
    /// Пункт с клавиатуры (`:focus-visible`).
    active: bool,
    view: Option<ItemView>,
    probe: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    index: usize,
    colors: Colors,
    zoom: f32,
    pick: Handler<ClickEvent>,
}

/// `.ui-dropdown__item`: поля 8 8 8 12, 13 px / 500, строка 24; наведение и пункт с клавиатуры —
/// фон .05 и контур акцента, нажатие .08, выбранный — `--l-accent-soft` (наведение его
/// перекрывает: у `:hover:not(:disabled)` вес больше), недоступный — `--l-text-faint`.
fn item_row(spec: RowSpec) -> impl IntoElement {
    let RowSpec {
        id,
        item,
        selected,
        active,
        view,
        probe,
        index,
        colors: c,
        zoom,
        pick,
    } = spec;
    let live = !item.disabled;
    let inner = id.clone();
    let meta = crate::live::Meta::new("option", id.clone())
        .label(Some(item.label.clone()))
        .disabled(!live)
        .checked(selected);
    let element = hover(id, Timing::fast(), move |t| {
        let t = if live { t } else { 0. };
        let base = if selected {
            c.soft
        } else if active {
            white(0.05)
        } else {
            white(0.)
        };
        let lit = t.max(f32::from(u8::from(active)));
        let color = if !live {
            c.faint
        } else if selected || active {
            c.text
        } else {
            mix(c.dim, c.text, t)
        };
        let title = item.hint.clone().unwrap_or_else(|| item.label.clone());
        let label = match &view {
            Some(view) => view(&item, lit.max(f32::from(u8::from(selected)))),
            None => div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(item.label.clone())
                .into_any_element(),
        };
        let badge = item.badge.clone().map(|badge| {
            let peach = rgb(0xffb37a);
            div()
                .flex_none()
                .px(u(8.))
                .rounded(u(6.))
                .bg(hsla(26. / 360., 1., 0.74, 0.12))
                .shadow(vec![ring(zoom, 1., hsla(26. / 360., 1., 0.74, 0.35))])
                .child(spaced(
                    badge.to_uppercase(),
                    10.5,
                    20.,
                    FontWeight::BOLD,
                    0.08,
                    peach,
                ))
        });
        div()
            .id(inner)
            .role(Role::ListBoxOption)
            .button_text()
            .aria_selected(selected)
            .aria_label(item.label.clone())
            .relative()
            .flex()
            .items_center()
            .gap(u(12.))
            .pt(u(8.))
            .pb(u(8.))
            .pr(u(8.))
            .pl(u(12.))
            .when(item.separated, |row| row.mt(u(8.)))
            .rounded(c.radius)
            .bg(mix(base, white(0.05), t))
            .when(lit > 0., |row| {
                row.shadow(vec![ring(zoom, 1., c.edge.opacity(lit))])
            })
            .font_family(RUBIK)
            .text_px(13., 24.)
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .tip(title)
            .when(live, |row| {
                row.cursor_pointer()
                    .active(|s| s.bg(white(0.08)))
                    .on_click(move |event, window, cx| pick(event, window, cx))
            })
            .when(item.separated, |row| {
                row.child(
                    div()
                        .absolute()
                        .left(u(4.))
                        .right(u(4.))
                        .top(u(-7.))
                        .h(gpui::px(1.))
                        .bg(c.line),
                )
            })
            .child(label)
            .children(badge)
            .when(selected, |row| {
                row.child(
                    div()
                        .flex_none()
                        .w(u(24.))
                        .flex()
                        .justify_center()
                        .text_color(c.accent)
                        .child(fa_glyph("fas fa-check", 11.).line_height(u(24.))),
                )
            })
            .child(
                canvas(
                    move |bounds, _, _| {
                        if let Some(slot) = probe.borrow_mut().get_mut(index) {
                            *slot = bounds;
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
    });
    crate::live::mark(meta, element)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::zpx;
    use gpui::{Context, Entity, Modifiers, Render, TestAppContext, VisualTestContext, size};

    const LABELS: [&str; 5] = ["Германия", "Россия", "Румыния", "Украина", "Узбекистан"];

    struct Form {
        value: Option<SharedString>,
        picked: Vec<SharedString>,
        open: Rc<Cell<bool>>,
    }

    impl Render for Form {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let items = LABELS
                .iter()
                .enumerate()
                .map(|(i, label)| Item {
                    disabled: i == 1,
                    ..Item::new(format!("c{i}"), *label)
                })
                .collect();
            let shown = self.open.clone();
            div().size_full().p(u(20.)).child(
                dropdown("dd", items, self.value.clone())
                    .toggle(move |state| {
                        shown.set(state.open);
                        div().w(u(160.)).h(u(34.))
                    })
                    .on_select(cx.listener(|this, item: &Item, _, cx| {
                        this.value = Some(item.id.clone());
                        this.picked.push(item.id.clone());
                        cx.notify();
                    })),
            )
        }
    }

    fn open(cx: &mut TestAppContext) -> (Entity<Form>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (form, cx) = cx.add_window_view(|_, _| Form {
            value: Some("c2".into()),
            picked: Vec::new(),
            open: Rc::default(),
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.focus_next(cx));
        (form, cx)
    }

    /// Кадр монитора: часы теста идут на кадр (движение просит кадры в такт монитора окна).
    fn next_frame(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(17));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }

    /// Выбранные пункты и открыт ли список.
    fn seen(form: &Entity<Form>, cx: &mut VisualTestContext) -> (Vec<SharedString>, bool) {
        next_frame(cx);
        form.read_with(cx, |form, _| (form.picked.clone(), form.open.get()))
    }

    fn ids(ids: &[&str]) -> Vec<SharedString> {
        ids.iter()
            .map(|id| SharedString::from(id.to_string()))
            .collect()
    }

    #[gpui::test]
    fn arrows_skip_disabled_and_enter_picks(cx: &mut TestAppContext) {
        let (form, cx) = open(cx);
        // ↓ открывает на первом, ↓ — мимо недоступной «России», End, ↓ по кругу, ↑↑.
        cx.simulate_keystrokes("down down end down up up");
        assert_eq!(seen(&form, cx), (ids(&[]), true));
        cx.simulate_keystrokes("enter");
        assert_eq!(seen(&form, cx), (ids(&["c3"]), false));
        // ↑ у закрытого — последний доступный; Home — первый.
        cx.simulate_keystrokes("up enter");
        cx.simulate_keystrokes("down home enter");
        assert_eq!(seen(&form, cx), (ids(&["c3", "c4", "c0"]), false));
        // Esc закрывает только список, ничего не выбрано.
        cx.simulate_keystrokes("down escape");
        assert_eq!(seen(&form, cx), (ids(&["c3", "c4", "c0"]), false));
    }

    #[gpui::test]
    fn typeahead_opens_on_the_match_and_repeats_cycle(cx: &mut TestAppContext) {
        let (form, cx) = open(cx);
        // С выбранной «Румынии»: «у» — «Украина», ещё «у» — «Узбекистан».
        cx.simulate_input("у");
        assert_eq!(seen(&form, cx), (ids(&[]), true));
        cx.simulate_input("у");
        cx.simulate_keystrokes("space");
        assert_eq!(seen(&form, cx), (ids(&["c4"]), false));
        // Tab закрывает список.
        cx.simulate_keystrokes("down");
        assert!(seen(&form, cx).1);
        cx.simulate_keystrokes("tab");
        assert!(!seen(&form, cx).1);
    }

    #[gpui::test]
    fn click_opens_and_picks_with_the_mouse(cx: &mut TestAppContext) {
        let (form, cx) = open(cx);
        let none = Modifiers::default();
        let at = |x, y| point(zpx(1., x), zpx(1., y));
        cx.simulate_click(at(40., 30.), none);
        assert_eq!(seen(&form, cx), (ids(&[]), true));
        // Меню меряется кадр, потом встаёт под кнопкой (20 + 34 + 8): щелчок по «Германии».
        next_frame(cx);
        cx.simulate_click(at(60., 80.), none);
        assert_eq!(seen(&form, cx), (ids(&["c0"]), false));
    }

    /// Щелчок драйвера сверки через 0,6 с после открытия: движение, 60 мс, нажатие, 60 мс,
    /// отпускание — с кадрами между ними (наведение пункта идёт переходом).
    #[gpui::test]
    fn slow_click_soon_after_open_picks(cx: &mut TestAppContext) {
        let (form, cx) = open(cx);
        let none = Modifiers::default();
        let at = |x, y| point(zpx(1., x), zpx(1., y));
        let wait = |cx: &mut VisualTestContext, ms: u64| {
            for _ in 0..ms.div_ceil(17) {
                next_frame(cx);
            }
        };
        cx.simulate_click(at(40., 30.), none);
        wait(cx, 600);
        // «Украина» — четвёртый пункт: 62 + 8 + 3 × (40 + 4) + 20.
        let item = at(60., 222.);
        cx.simulate_mouse_move(item, None, none);
        wait(cx, 60);
        cx.simulate_mouse_down(item, gpui::MouseButton::Left, none);
        wait(cx, 60);
        cx.simulate_mouse_up(item, gpui::MouseButton::Left, none);
        assert_eq!(seen(&form, cx), (ids(&["c3"]), false));
    }

    /// Слоты пункта видят подсветку: выбранный — 1, прочие без мыши и клавиатуры — 0.
    #[gpui::test]
    fn item_view_lights_the_selected_item(cx: &mut TestAppContext) {
        type Seen = Rc<RefCell<Vec<(SharedString, f32)>>>;
        struct Lit(Seen);
        impl Render for Lit {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let seen = self.0.clone();
                let items = vec![Item::new("a", "A"), Item::new("b", "B")];
                div().size_full().p(u(20.)).child(
                    dropdown("dd", items, Some("b".into()))
                        .preview_open()
                        .toggle(|_| div().w(u(160.)).h(u(34.)))
                        .item_view(move |item, lit| {
                            seen.borrow_mut().push((item.id.clone(), lit));
                            div()
                        }),
                )
            }
        }
        cx.update(crate::init);
        let seen = Seen::default();
        let shared = seen.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Lit(shared));
        next_frame(cx);
        next_frame(cx);
        let last = |id: &str| {
            seen.borrow()
                .iter()
                .rev()
                .find(|(seen, _)| seen.as_ref() == id)
                .map(|(_, lit)| *lit)
        };
        assert_eq!((last("a"), last("b")), (Some(0.), Some(1.)));
    }

    #[test]
    fn reveal_scrolls_the_least_and_rise_steps_round() {
        let p = |v| zpx(1., v);
        let b = |y: f32, h: f32| Bounds::new(point(p(0.), p(y)), size(p(10.), p(h)));
        let view = b(100., 200.);
        assert_eq!(reveal_offset(view, b(150., 40.), p(-30.)), p(-30.));
        assert_eq!(reveal_offset(view, b(80., 40.), p(-30.)), p(-10.));
        assert_eq!(reveal_offset(view, b(290., 40.), p(-30.)), p(-60.));
        let steps: Vec<usize> = (0..10).map(rise_step).collect();
        assert_eq!(steps, [0, 1, 1, 2, 3, 3, 4, 5, 5, 5]);
    }
}
