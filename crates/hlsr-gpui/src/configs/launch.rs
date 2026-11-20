//! Раздел «Параметры запуска» (`GameSettingsModal.vue` 0.4.9, `activeSection === 'launch'`):
//! переключатели запуска игры (у HL1 — свои), Vulkan (DXVK) у Ghosting на Windows, разрешение
//! и частота HL1, «без CPU0», язык игры, таймер, строка параметров, опции Proton (Linux).
//! Строки — `.settings-row` (подпись 14/24, подсказка 12/20, линия снизу), переключатель —
//! `.settings-switch` (`.gx-switch` с пружиной и вытягиванием при нажатии, `Toggle::spring`),
//! выбор — `.settings-select`.

use super::sections::GameSettings;
use crate::icons::chevron;
use crate::kit::input::FieldStyle;
use crate::kit::{Item, TextInput, ToggleState, dropdown, toggle};
use crate::motion::{Timing, tween};
use crate::scale::u;
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, WeakEntity,
    div, hsla, prelude::*, rgb,
};
use hlsr_ui::controllers::game_settings::{SelectView, View};

/// `.game-settings__args`: поля 12/16, 13/20, заливка .03, рамка inset `--l-border`; наведение
/// .05; фокус — рамка `--accent` и кольцо 3 px `--accent-soft`; `.invalid` — `#ff4d5e`.
pub fn args_style(theme: &Theme) -> FieldStyle {
    FieldStyle {
        height: 0.,
        pad_x: 16.,
        pad_right: None,
        pad_y: 12.,
        family: RUBIK,
        size: 13.,
        line: 20.,
        radius: theme.len("l-radius-sm"),
        bg: hsla(0., 0., 1., 0.03),
        hover_bg: hsla(0., 0., 1., 0.05),
        border: theme.color("l-border"),
        hover_border: theme.color("l-border"),
        focus_border: theme.color("accent"),
        invalid_border: rgb(0xff4d5e).into(),
        ring: true,
        glow: Some((3., theme.color("accent-soft"))),
        outline: None,
        disabled_opacity: 1.,
    }
}

/// `.settings-row`: текст слева, контрол справа; `column` — контрол под текстом.
fn row(
    theme: &Theme,
    label: String,
    hint: String,
    last: bool,
    column: bool,
    control: AnyElement,
) -> AnyElement {
    let text = div()
        .flex()
        .flex_col()
        .gap(u(4.))
        .min_w_0()
        .child(
            div()
                .text_px(14., 24.)
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.color("l-text"))
                .child(label),
        )
        .child(
            div()
                .text_px(12., 20.)
                .text_color(theme.color("l-text-faint"))
                .child(hint),
        );
    // `box-shadow: inset 0 -1px 0` — линия места не занимает.
    let line = hsla(0., 0., 1., 0.045);
    div()
        .relative()
        .flex()
        .py(u(16.))
        .when(!last, |el| {
            el.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(u(1.))
                    .bg(line),
            )
        })
        .map(|el| {
            if column {
                el.flex_col().gap(u(12.))
            } else {
                el.items_center().justify_between().gap(u(20.))
            }
        })
        .child(text)
        .child(control)
        .into_any_element()
}

/// `.settings-select`: подпись выбранного и шеврон; открыт — подложка ярче, шеврон акцентом.
fn select_toggle(theme: &Theme, label: String, state: ToggleState) -> AnyElement {
    let open = state.open;
    let chev = theme.color(if open { "l-accent" } else { "l-text-dim" });
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(u(12.))
        .min_w(u(160.))
        .px(u(14.))
        .py(u(9.))
        .rounded(u(theme.len("l-radius-sm")))
        .border_1()
        .border_color(theme.color(if open { "l-border-strong" } else { "l-border" }))
        .bg(hsla(0., 0., 1., if open { 0.08 } else { 0.05 }))
        .hover(|s| s.bg(hsla(0., 0., 1., 0.08)))
        .text_px(13., 20.)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.color("l-text"))
        .child(label)
        // `.open i { transform: rotate(180deg) }` за `--l-med`.
        .child(tween(
            "launch-chev",
            f32::from(u8::from(open)),
            Timing::med(),
            move |t| chevron(10., 180. * t, chev),
        ))
        .into_any_element()
}

fn select(
    id: &'static str,
    view: &SelectView,
    theme: &Theme,
    weak: WeakEntity<GameSettings>,
    pick: fn(&mut GameSettings, &str),
) -> AnyElement {
    let items: Vec<Item> = view
        .items
        .iter()
        .map(|item| Item::new(item.id.clone(), item.label.clone()))
        .collect();
    let (theme, label) = (theme.clone(), view.label.clone());
    dropdown(id, items, Some(SharedString::from(view.value.clone())))
        .align_right()
        .toggle(move |state| select_toggle(&theme, label, state))
        .on_select(move |item, _, cx| {
            weak.update(cx, |this, cx| {
                pick(this, &item.id);
                cx.notify();
            })
            .ok();
        })
        .into_any_element()
}

/// Раздел целиком.
pub fn launch(
    view: &View,
    args: &Entity<TextInput>,
    proton: &Entity<TextInput>,
    weak: WeakEntity<GameSettings>,
    t: &dyn Fn(&str) -> String,
    theme: &Theme,
) -> AnyElement {
    let theme = theme.clone();
    let mut rows: Vec<AnyElement> = Vec::new();
    for item in &view.toggles {
        let (weak, key) = (weak.clone(), item.key);
        rows.push(row(
            &theme,
            item.label.clone(),
            item.hint.clone(),
            false,
            false,
            toggle(format!("gs-toggle-{key}"), item.checked)
                .spring()
                .label(item.label.clone())
                .on_change(move |on, _, cx| {
                    weak.update(cx, |this, cx| {
                        this.session().set_flag(key, *on);
                        cx.notify();
                    })
                    .ok();
                })
                .into_any_element(),
        ));
    }
    if let Some(vulkan) = &view.vulkan {
        let weak = weak.clone();
        rows.push(row(
            &theme,
            t("#L_GS_VULKAN"),
            vulkan.hint.clone(),
            false,
            false,
            toggle("gs-toggle-vulkan", vulkan.checked)
                .spring()
                .disabled(!vulkan.available)
                .label(t("#L_GS_VULKAN"))
                .on_change(move |on, _, cx| {
                    weak.update(cx, |this, cx| {
                        this.session().set_flag("vulkan", *on);
                        cx.notify();
                    })
                    .ok();
                })
                .into_any_element(),
        ));
    }
    if view.goldsrc {
        rows.push(row(
            &theme,
            t("#L_GS_RESOLUTION"),
            t("#L_GS_RESOLUTION_HINT"),
            false,
            false,
            select(
                "gs-resolution",
                &view.resolution,
                &theme,
                weak.clone(),
                |this, id| this.session().set_resolution(id),
            ),
        ));
        rows.push(row(
            &theme,
            t("#L_GS_REFRESH_RATE"),
            t("#L_GS_REFRESH_RATE_HINT"),
            false,
            false,
            select(
                "gs-refresh",
                &view.refresh,
                &theme,
                weak.clone(),
                |this, id| this.session().set_refresh_rate(id),
            ),
        ));
    }
    let flag = |key: &'static str, label: &str, hint: &str, checked: bool| {
        let weak = weak.clone();
        row(
            &theme,
            t(label),
            t(hint),
            false,
            false,
            toggle(format!("gs-toggle-{key}"), checked)
                .spring()
                .label(t(label))
                .on_change(move |on, _, cx| {
                    weak.update(cx, |this, cx| {
                        this.session().set_flag(key, *on);
                        cx.notify();
                    })
                    .ok();
                })
                .into_any_element(),
        )
    };
    rows.push(flag(
        "excludeCpu0",
        "#L_GS_NO_CPU0",
        "#L_GS_NO_CPU0_HINT",
        view.cpu0,
    ));
    if view.language_available {
        rows.push(row(
            &theme,
            t("#L_GS_LANG"),
            t("#L_GS_LANG_HINT"),
            false,
            false,
            select(
                "gs-language",
                &view.language,
                &theme,
                weak.clone(),
                |this, id| this.session().set_language(id),
            ),
        ));
    }
    rows.push(flag(
        "startTimer",
        "#L_GS_START_TIMER",
        "#L_GS_START_TIMER_HINT",
        view.timer,
    ));
    let field = |input: &Entity<TextInput>, error: &str| {
        div()
            .flex()
            .flex_col()
            .gap(u(12.))
            .child(input.clone())
            .when(!error.is_empty(), |el| {
                el.child(
                    div()
                        .text_size(u(12.))
                        .line_height(gpui::relative(1.4))
                        .text_color(rgb(0xff6b78))
                        .child(error.to_owned()),
                )
            })
            .into_any_element()
    };
    rows.push(row(
        &theme,
        t("#L_GS_ARGS"),
        view.args_hint.clone(),
        !view.linux,
        true,
        field(args, &view.args_error),
    ));
    if view.linux {
        rows.push(row(
            &theme,
            t("#L_GS_PROTON_OPTIONS"),
            t("#L_GS_PROTON_OPTIONS_HINT"),
            true,
            true,
            field(proton, &view.proton_error),
        ));
    }
    div().flex().flex_col().children(rows).into_any_element()
}
