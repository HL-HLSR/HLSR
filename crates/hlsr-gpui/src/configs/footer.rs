//! Общий подвал разделов (`.settings-modal__footer.game-settings__footer` embedded 0.4.9): слева
//! импорт, экспорт и папка страницы (слот `footer-start`), значок «Убрать импортированный cfg»
//! у «Клавиатуры»; справа заметка (файл изменён снаружи, правленые скрипты, «применится после
//! сохранения», итог импорта) и кнопки «Перечитать» / «Удалить» / «Сбросить», «Отмена»,
//! «Сохранить». И кнопка `.settings-btn` (обычная, `ghost`, `primary`, компактная, значок),
//! общая для страницы.

use super::sections::GameSettings;
use crate::icons::fa_glyph;
use crate::kit::Handler;
use crate::kit::button::{child_id, focus_handle, focus_visible, outline, white};
use crate::layer::TipExt;
use crate::motion::{Push, Timing, hover, mix, press, tween};
use crate::paint::shadow;
use crate::scale::{u, zoom};
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, IntoElement, ParentElement, RenderOnce,
    Role, SharedString, Styled, WeakEntity, Window, div, prelude::*, relative,
};
use hlsr_ui::controllers::game_settings::FooterView;
use std::rc::Rc;

/// Вид `.settings-btn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtnLook {
    Plain,
    /// `.ghost`: без заливки, текст тусклее.
    Ghost,
    /// `.primary`: заливка акцентом, свечение при наведении.
    Primary,
}

#[derive(IntoElement)]
pub struct SettingsBtn {
    id: ElementId,
    look: BtnLook,
    compact: bool,
    icon_only: bool,
    disabled: bool,
    fade: bool,
    label: Option<SharedString>,
    children: Vec<AnyElement>,
    on_click: Option<Handler<ClickEvent>>,
}

/// Кнопка `.settings-btn`: 48 px, поля 11/19, рамка `--l-border-strong`, 13,5 px / 600; нажатие
/// `scale(.97)` `--l-fast` (так у всех `.settings-btn` с ним: конфиги, игра, `.pz-btn`, настройки).
pub fn settings_btn(id: impl Into<ElementId>, look: BtnLook) -> SettingsBtn {
    SettingsBtn {
        id: id.into(),
        look,
        compact: false,
        icon_only: false,
        disabled: false,
        fade: false,
        label: None,
        children: Vec::new(),
        on_click: None,
    }
}

impl SettingsBtn {
    /// `.cfgp__compact`: 32 px, поля 3/12, 12,5 px.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// Только значок: ширина 48, без полей (`.game-settings__icon-btn`, узкое окно).
    pub fn icon_only(mut self, on: bool) -> Self {
        self.icon_only = on;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// `.pz-btn`: `opacity` в его `transition` — выключение и включение за `--l-fast` (у
    /// `.settings-btn` прозрачность меняется сразу).
    pub fn fade(mut self) -> Self {
        self.fade = true;
        self
    }

    /// `aria-label`.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
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

impl ParentElement for SettingsBtn {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for SettingsBtn {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let zoom = zoom(window);
        let focus = focus_handle(&self.id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let theme = cx.global::<Theme>();
        let focus_color = theme.color("l-accent");
        let radius = theme.len("l-radius-sm");
        let (bg, hover_bg, pressed_bg, text, hover_text, border) = match self.look {
            BtnLook::Plain => (
                white(0.05),
                white(0.1),
                white(0.1),
                theme.color("l-text"),
                theme.color("l-text"),
                theme.color("l-border-strong"),
            ),
            BtnLook::Ghost => (
                white(0.),
                white(0.06),
                white(0.06),
                theme.color("l-text-dim"),
                theme.color("l-text"),
                theme.color("l-border-strong"),
            ),
            BtnLook::Primary => (
                theme.color("accent"),
                theme.color("accent-hover"),
                theme.color("accent-pressed"),
                theme.color("accent-contrast-text"),
                theme.color("accent-contrast-text"),
                gpui::transparent_black(),
            ),
        };
        let glow = (self.look == BtnLook::Primary).then(|| theme.rgba("l-accent", 0.35));
        let live = !self.disabled;
        let SettingsBtn {
            id,
            compact,
            icon_only,
            fade,
            label,
            children,
            on_click,
            ..
        } = self;
        let inner = id.clone();
        let press_id = child_id(&id, "press");
        let live_id = fade.then(|| child_id(&id, "live"));
        let meta = crate::live::Meta::new("button", id.clone())
            .label(label.clone())
            .disabled(!live)
            .focused(focus.is_focused(window));
        // `l` — ход включения 0…1: прозрачность `:disabled` и наведение (`:hover:not(:disabled)`).
        let body = move |l: f32| {
            hover(id, Timing::fast(), move |t| {
                let t = t * l;
                let base = div()
                    .id(inner)
                    .role(Role::Button)
                    .button_text()
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .gap(u(8.))
                    .font_family(RUBIK)
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .border_1()
                    .border_color(border)
                    .rounded(u(radius))
                    .bg(mix(bg, hover_bg, t))
                    .text_color(mix(text, hover_text, t))
                    .when_some(glow.filter(|_| live && t > 0.), |el, glow| {
                        el.shadow(vec![shadow(zoom, 0., 6., 20., 0., glow.opacity(t))])
                    })
                    .when_some(label, |el, label| el.aria_label(label));
                let base = if compact {
                    base.min_h(u(32.)).px(u(12.)).py(u(3.)).text_px(12.5, 20.)
                } else if icon_only {
                    base.h(u(48.)).w(u(48.)).text_px(13.5, 24.)
                } else {
                    base.min_h(u(48.)).px(u(19.)).py(u(11.)).text_px(13.5, 24.)
                };
                let el = base
                    .children(children)
                    .when(l < 1., |el| el.opacity(0.45 + 0.55 * l))
                    .when(live, |el| {
                        el.track_focus(&focus)
                            .cursor_pointer()
                            .active(move |s| s.bg(pressed_bg))
                            .when_some(on_click, |el, handler| {
                                el.on_click(move |event, window, cx| handler(event, window, cx))
                            })
                    })
                    .when(ring_on, |el| {
                        el.child(outline(zoom, radius, 2., 2., focus_color))
                    });
                press(press_id, Push::fast(0.97), el).live(live)
            })
        };
        // Без `.fade()` прозрачность встаёт сразу — свой `tween` не нужен.
        let target = f32::from(u8::from(live));
        let element = match live_id {
            Some(live_id) => tween(live_id, target, Timing::fast(), body).into_any_element(),
            None => body(target).into_any_element(),
        };
        crate::live::mark(meta, element)
    }
}

/// Значок и подпись кнопки (`<i>` + `<span>`); подпись можно спрятать (узкое окно).
pub fn icon_label(icon: &str, text: String, show_text: bool) -> Vec<AnyElement> {
    let mut parts = vec![fa_glyph(icon, 13.5).into_any_element()];
    if show_text {
        parts.push(div().child(text).into_any_element());
    }
    parts
}

/// Подвал под разделом. `start` — слот страницы (импорт, экспорт, папка).
pub fn footer(
    view: &FooterView,
    start: AnyElement,
    weak: WeakEntity<GameSettings>,
    t: &dyn Fn(&str) -> String,
    theme: &Theme,
) -> AnyElement {
    let act = |f: fn(&mut GameSettings)| {
        let weak = weak.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            weak.update(cx, |this, cx| {
                f(this);
                cx.notify();
            })
            .ok();
        }
    };
    let mut end = div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_end()
        .gap_x(u(12.))
        .gap_y(u(8.))
        .min_w_0()
        .flex_1()
        .ml_auto();
    if !view.note.is_empty() {
        let color = if view.note_error {
            theme.color("l-level-error")
        } else {
            theme.color("l-text-faint")
        };
        end = end.child(
            div()
                .id("gs-note")
                .flex_1()
                .min_w(u(120.))
                .max_w(u(340.))
                .text_size(u(12.))
                .line_height(relative(1.4))
                .text_right()
                .text_color(color)
                .when(!view.note_details.is_empty(), |el| {
                    el.tip(view.note_details.clone())
                })
                .child(crate::text::pretty(view.note.clone())),
        );
    }
    if view.file_changed {
        end = end.child(
            settings_btn("gs-reload", BtnLook::Ghost)
                .on_click(act(GameSettings::reload_after_change))
                .child(t("#L_GS_CFG_CHANGED_RELOAD")),
        );
    } else if view.edited {
        end = end
            .child(
                settings_btn("gs-edited-delete", BtnLook::Plain)
                    .on_click(act(GameSettings::delete_edited))
                    .child(t("#L_GS_SCRIPT_EDITED_DELETE")),
            )
            .child(
                settings_btn("gs-edited-cancel", BtnLook::Ghost)
                    .label(t("#L_CANCEL"))
                    .on_click(act(GameSettings::dismiss_edited))
                    .child(fa_glyph("fas fa-times", 13.5)),
            );
    }
    if view.saves {
        end = end
            .child(
                settings_btn("gs-reset", BtnLook::Ghost)
                    .on_click(act(GameSettings::reset))
                    .child(t("#L_RESET")),
            )
            .child(
                settings_btn("gs-cancel", BtnLook::Plain)
                    .on_click(act(GameSettings::discard))
                    .child(t("#L_CANCEL")),
            )
            .child(
                settings_btn("gs-save", BtnLook::Primary)
                    .disabled(!view.save_enabled)
                    .on_click(act(GameSettings::submit))
                    .child(t("#L_SAVE")),
            );
    }
    div()
        .relative()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(u(12.))
        .pt(u(16.))
        .flex_none()
        // `box-shadow: inset 0 1px 0 var(--l-modal-border)` — линия сверху, места не занимает.
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(u(1.))
                .bg(theme.color("l-modal-border")),
        )
        .child(start)
        .when(view.remove_imported, |el| {
            el.child(
                settings_btn("gs-remove-imported", BtnLook::Ghost)
                    .icon_only(true)
                    .disabled(view.remove_imported_busy)
                    .label(t("#L_KB_IMPORT_REMOVE"))
                    .on_click(act(GameSettings::remove_imported))
                    .child(fa_glyph("fas fa-file-circle-xmark", 13.5)),
            )
        })
        .child(end)
        .into_any_element()
}
