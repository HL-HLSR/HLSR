//! Список разделов слева (`.game-settings__tabs` embedded 0.4.9): пункт — значок и подпись,
//! выбранный — подложкой акцента и значком акцентом; под открытой «Персонализацией» — её
//! подразделы (мельче, с отступом, точка у изменённых). Полосы-указателя у списка нет
//! (`measureInk`: «у списка слева полосы нет»).

use super::sections::GameSettings;
use crate::icons::fa_glyph;
use crate::kit::button::{child_id, focus_handle, focus_visible, outline};
use crate::liquid;
use crate::motion::{Timing, hover, mix, tween};
use crate::scale::{u, zoom};
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, IntoElement, ParentElement, Role,
    Styled, WeakEntity, Window, div, prelude::*, rgb,
};
use hlsr_ui::controllers::game_settings::TabView;

/// Подразделы редактора внешнего вида (`personalNav`): выбранный и пункты.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PersonalNav {
    pub active: String,
    pub items: Vec<NavItem>,
}

/// Подраздел: значок, подпись (уже переведённая), число правок (точка).
#[derive(Debug, Clone, PartialEq)]
pub struct NavItem {
    pub id: String,
    pub icon: String,
    pub label: String,
    pub changes: usize,
}

/// Цвета пункта в покое и при наведении: невыбранного и выбранного (`.is-active`).
#[derive(Clone, Copy)]
struct Paint {
    text: Hsla,
    hover_text: Hsla,
    bg: Hsla,
    hover_bg: Hsla,
}

/// Кнопка списка: подложка и цвет за `--l-fast` и при наведении, и при выборе
/// (`transition: background var(--l-fast), color var(--l-fast)`), контур фокуса внутрь на 2 px.
/// `build` получает ход выбора 0…1 (значок — своим правилом).
#[allow(clippy::too_many_arguments)]
fn item(
    id: ElementId,
    idle: Paint,
    on: Paint,
    active: bool,
    window: &mut Window,
    cx: &mut App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    build: impl FnOnce(gpui::Stateful<gpui::Div>, f32) -> gpui::Stateful<gpui::Div> + 'static,
) -> AnyElement {
    let theme = cx.global::<Theme>();
    let radius = theme.len("l-radius-sm");
    let focus_color = theme.color("l-accent");
    let zoom = zoom(window);
    let focus = focus_handle(&id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let (hover_id, inner) = (id.clone(), id.clone());
    tween(
        child_id(&id, "on"),
        f32::from(u8::from(active)),
        Timing::fast(),
        move |a| {
            hover(hover_id, Timing::fast(), move |t| {
                let bg = mix(
                    mix(idle.bg, on.bg, a),
                    mix(idle.hover_bg, on.hover_bg, a),
                    t,
                );
                let text = mix(
                    mix(idle.text, on.text, a),
                    mix(idle.hover_text, on.hover_text, a),
                    t,
                );
                let el = div()
                    .id(inner)
                    .role(Role::Button)
                    .button_text()
                    .relative()
                    .flex()
                    .items_center()
                    .rounded(u(radius))
                    .bg(bg)
                    .text_color(text)
                    .font_family(RUBIK)
                    .cursor_pointer()
                    .track_focus(&focus)
                    .on_click(on_click);
                build(el, a).when(ring_on, |el| {
                    el.child(outline(zoom, radius, -2., 2., focus_color))
                })
            })
        },
    )
    .into_any_element()
}

/// Текст и значок неактивного пункта: в Liquid Glass светлее (пункты лежат на преломлённом кадре).
fn idle_colors(theme: &Theme) -> (Hsla, Hsla) {
    if theme.liquid() {
        (rgb(liquid::NAV_TEXT).into(), rgb(liquid::NAV_ICON).into())
    } else {
        (theme.color("l-text-dim"), theme.color("l-text-faint"))
    }
}

/// Список разделов и подразделов.
pub fn nav(
    tabs: &[TabView],
    active_id: &str,
    personal: &PersonalNav,
    weak: WeakEntity<GameSettings>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.global::<Theme>().clone();
    let (dim, faint) = idle_colors(&theme);
    let mut list = div().flex().flex_col().gap(u(2.));
    for tab in tabs {
        let active = tab.id == active_id;
        let idle = Paint {
            text: dim,
            hover_text: theme.color("l-text"),
            bg: theme.color("l-chip-hover").opacity(0.),
            hover_bg: theme.color("l-chip-hover"),
        };
        let on = Paint {
            text: theme.color("l-text"),
            hover_text: theme.color("l-text"),
            bg: theme.color("accent-soft"),
            hover_bg: theme.color("accent-soft"),
        };
        // Значок раздела — `i { transition: color var(--l-fast) }`.
        let accent = theme.color("l-accent");
        let (on_tab, id) = (weak.clone(), tab.id);
        let label = tab.label.clone();
        let icon = tab.icon;
        let button = item(
            ElementId::Name(format!("gs-tab-{id}").into()),
            idle,
            on,
            active,
            window,
            cx,
            move |_, window, cx| {
                on_tab
                    .update(cx, |this, cx| this.select_section(id, window, cx))
                    .ok();
            },
            move |el, a| {
                el.gap(u(10.))
                    .min_h(u(40.))
                    .px(u(12.))
                    .py(u(8.))
                    .text_px(13.5, 20.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(
                        div()
                            .w(u(16.))
                            .flex()
                            .justify_center()
                            .text_color(mix(faint, accent, a))
                            .child(fa_glyph(icon, 13.)),
                    )
                    .child(div().flex_1().min_w_0().child(label))
            },
        );
        // Якорь обучения `gs-<раздел>` (`:data-tour="'gs-' + s.id"`).
        let button = super::anchors::anchor(format!("gs-{id}"), button);
        list = list.child(crate::snap::target(format!("gs-{id}"), button));
        if tab.id == "personal" && active && !personal.items.is_empty() {
            list = list.child(subtabs(personal, &weak, &theme, window, cx));
        }
    }
    list.into_any_element()
}

fn subtabs(
    personal: &PersonalNav,
    weak: &WeakEntity<GameSettings>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let mut group = div().flex().flex_col().gap(u(1.)).mt(u(2.)).mb(u(6.));
    let (dim, _) = idle_colors(theme);
    for entry in &personal.items {
        let active = entry.id == personal.active;
        let idle = Paint {
            text: dim,
            hover_text: theme.color("l-text"),
            bg: theme.color("l-chip-hover").opacity(0.),
            hover_bg: theme.color("l-chip-hover"),
        };
        let on = Paint {
            text: theme.color("l-text"),
            hover_text: theme.color("l-text"),
            bg: theme.color("l-chip-hover"),
            hover_bg: theme.color("l-chip-hover"),
        };
        // У значка подраздела своего перехода нет: цвет сразу.
        let icon_color = theme.color(if active { "l-accent" } else { "l-text-faint" });
        let weak = weak.clone();
        let (id, icon, label, changes) = (
            entry.id.clone(),
            entry.icon.clone(),
            entry.label.clone(),
            entry.changes,
        );
        let dot = theme.color("l-accent");
        let target = id.clone();
        let name = format!("gx-tab-{id}");
        let button = item(
            ElementId::Name(format!("gx-tab-{id}").into()),
            idle,
            on,
            active,
            window,
            cx,
            move |_, _, cx| {
                weak.update(cx, |this, cx| this.select_personal(&target, cx))
                    .ok();
            },
            move |el, _| {
                el.gap(u(7.))
                    .min_h(u(30.))
                    .pl(u(22.))
                    .pr(u(6.))
                    .py(u(5.))
                    .text_px(12.5, 16.)
                    .when(active, |el| el.font_weight(FontWeight::SEMIBOLD))
                    .child(
                        div()
                            .w(u(14.))
                            .flex()
                            .justify_center()
                            .text_color(icon_color)
                            .child(fa_glyph(&icon, 11.)),
                    )
                    // `.game-settings__subtab-label { text-wrap: balance }`.
                    .child(div().min_w_0().child(crate::text::balance(label)))
                    .when(changes > 0, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .ml_auto()
                                .size(u(6.))
                                .rounded_full()
                                .bg(dot),
                        )
                    })
            },
        );
        group = group.child(crate::snap::target(name, button));
    }
    group.into_any_element()
}
