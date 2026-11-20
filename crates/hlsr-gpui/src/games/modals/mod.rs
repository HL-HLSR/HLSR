//! Окна игры (0.4.9): запуск LiveSplit, «Недоступно», заметки версии, падение игры, ремонт по
//! файлам, окно FFmpeg перед «Демо» (G3). Каждое окно — свой вид со своим [`Modal`]
//! (`crate::layer`); состояние — `ShellFlows` (флаги окон, ответы), запрос LiveSplit —
//! [`ShellRoot::livesplit_request`]. Подтверждения паков и итоги — общий `ConfirmModal` и тосты.
//!
//! [`Modal`]: crate::layer::Modal
//! [`ShellRoot::livesplit_request`]: crate::shell::root::ShellRoot::livesplit_request

mod crash;
mod ffmpeg_gate;
mod livesplit_launch;
mod notes;
mod repair;
#[cfg(test)]
mod tests;
mod unavailable;

use crate::icons::fa_glyph;
use crate::kit::button::{child_id, focus_handle, focus_visible, white};
use crate::kit::outline;
use crate::layer::TipExt;
use crate::motion::{Push, Timing, hover, mix, press};
use crate::scale::{u, zoom};
use crate::shell::root::ShellContext;
use crate::text::{RUBIK, TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, ClickEvent, Context, Div, ElementId, FocusHandle, FontWeight, IntoElement,
    Render, RenderOnce, Role, SharedString, Window, div, prelude::*,
};
use hlsr_ui::controllers::shell_flows::ShellFlows;
use hlsr_ui::services::Services;
use std::rc::Rc;

/// Окна игры в порядке шаблона 0.4.9.
pub struct GameModals {
    windows: Vec<AnyView>,
}

impl GameModals {
    pub fn new(_services: &Services, cx: &mut Context<GameModals>) -> GameModals {
        GameModals {
            windows: vec![
                cx.new(livesplit_launch::LiveSplitLaunchModal::new).into(),
                cx.new(ffmpeg_gate::FfmpegGateModal::new).into(),
                cx.new(repair::RepairModal::new).into(),
                cx.new(unavailable::UnavailableModal::new).into(),
                cx.new(crash::GameCrashModal::new).into(),
                cx.new(notes::GameNotesModal::new).into(),
            ],
        }
    }
}

impl Render for GameModals {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let el = div().children(self.windows.iter().cloned());
        #[cfg(feature = "dev-hooks")]
        let el = el.child(snap_points());
        el
    }
}

/// Сцены стенда (`dev-hooks`): щелчок эталона по месту без своей цели («Что нового» уведомления
/// S5, кнопка «Запустить» инструмента), приходит в невидимую точку 1 × 1
/// у верхнего края окна (`left` CSS px) и делает то же, что то место. Убрать, когда у мест будут
/// свои цели.
#[cfg(feature = "dev-hooks")]
fn snap_points() -> impl IntoElement {
    let point = |name: &'static str, left: f32, action: fn(&ShellFlows)| {
        crate::snap::target(
            name,
            div()
                .id(name)
                .absolute()
                .top_0()
                .left(u(left))
                .size(u(1.))
                .occlude()
                .on_click(move |_, _, cx| {
                    if let Some(flows) = flows(cx) {
                        action(&flows);
                    }
                }),
        )
    };
    div()
        // Уведомление сцены `games/modal-notes`: Ghosting обновился до 1.0.9.
        .child(point("snap-whats-new", 44., |flows| {
            flows.on_whats_new(&serde_json::json!({
                "target": "game", "gameId": "ghosting-renewed", "version": "1.0.9"
            }))
        }))
        .child(point("snap-tool-launch", 48., |flows| {
            flows.on_tool_launch("livesplit")
        }))
}

/// Сценарии оболочки окна.
pub(crate) fn flows(cx: &App) -> Option<ShellFlows> {
    ShellContext::get(cx).map(|shell| shell.flows.clone())
}

/// `$t(key, ...args)`.
pub(crate) fn tr(cx: &App, key: &str, args: &[&str]) -> String {
    ShellContext::get(cx).map_or_else(|| key.to_owned(), |shell| shell.shell.tr(key, args))
}

/// Обработчик щелчка окна.
pub(crate) type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// Шапка `.settings-modal__header` («Недоступно», заметки версии): заголовок 15 px / 600
/// прописными с разрядкой .09em, крестик 36 × 36 (`--l-text-dim`, при наведении .08 и белый),
/// линия `--l-border` снизу; поля 16 18 16 24.
pub(crate) fn header(
    title: &str,
    close: &FocusHandle,
    ring: bool,
    on_close: OnClick,
    window: &Window,
    cx: &App,
) -> Div {
    let theme = cx.global::<Theme>();
    let (text, dim, accent) = (
        theme.color("l-text"),
        theme.color("l-text-dim"),
        theme.color("accent-focus"),
    );
    let label = tr(cx, "#L_CLOSE", &[]);
    // Фокус из скрипта до первого щелчка мышью — с контуром (:focus-visible Chromium).
    let ring = (ring && close.is_focused(window)) || focus_visible(close, window);
    let zoom = zoom(window);
    let close = close.clone();
    let button = hover("game-modal-close", Timing::fast(), move |t| {
        div()
            .id("game-modal-close-button")
            .role(Role::Button)
            .button_text()
            .aria_label(label.clone())
            .tip(label)
            .track_focus(&close)
            .relative()
            .size(u(36.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(u(8.))
            .bg(white(0.08 * t))
            .text_color(mix(dim, gpui::white(), t))
            .cursor_pointer()
            .on_click(move |event, window, cx| on_close(event, window, cx))
            .child(fa_glyph("fas fa-times", 17.))
            .when(ring, |el| el.child(outline(zoom, 8., 2., 2., accent)))
    });
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .pt(u(16.))
        .pb(u(16.))
        .pl(u(24.))
        .pr(u(18.))
        .border_b_1()
        .border_color(theme.color("l-border"))
        .child(spaced(
            title.to_uppercase(),
            15.,
            20.,
            FontWeight::SEMIBOLD,
            0.09,
            text,
        ))
        .child(button)
}

/// Вид кнопки окон игры.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Look {
    /// `.settings-btn` («Недоступно»): поля 9 × 16, 13 px / 500, значок `--l-text-dim`.
    Settings,
    /// `.settings-btn.primary` (опрос о конфиге): заливка .14 и при наведении тоже.
    SettingsPrimary,
    /// `.crash__btn`: поля 10 × 18, 13,5 px / 600, без переноса.
    Crash,
    /// `.crash__btn.primary`: `--accent`, при наведении `brightness(1.08)`, не уже 110.
    CrashPrimary,
    /// `.rp__btn` (ремонт): поля 5 × 12, 12,5 px / 600.
    Small,
}

/// Кнопка окон игры: фон .05 (при наведении .1), рамка `--l-border-strong`, значок слева;
/// контур `:focus-visible`. Нажатие `scale(.97)` `--l-fast` — только у `.crash__btn` (у
/// `.settings-btn` окон опроса и «Недоступно» и у `.rp__btn` его нет).
#[derive(IntoElement)]
pub(crate) struct GameButton {
    id: ElementId,
    icon: Option<&'static str>,
    label: SharedString,
    look: Look,
    focus: Option<FocusHandle>,
    ring: bool,
    on_click: Option<OnClick>,
}

pub(crate) fn game_button(
    id: impl Into<ElementId>,
    icon: Option<&'static str>,
    label: impl Into<SharedString>,
    look: Look,
) -> GameButton {
    GameButton {
        id: id.into(),
        icon,
        label: label.into(),
        look,
        focus: None,
        ring: false,
        on_click: None,
    }
}

impl GameButton {
    pub(crate) fn focus(mut self, handle: &FocusHandle) -> Self {
        self.focus = Some(handle.clone());
        self
    }

    /// Контур фокуса без клавиатуры (фокус из скрипта до первого щелчка мышью).
    pub(crate) fn ring(mut self, ring: bool) -> Self {
        self.ring = ring;
        self
    }

    pub(crate) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for GameButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let (text, dim, strong) = (
            theme.color("l-text"),
            theme.color("l-text-dim"),
            theme.color("l-border-strong"),
        );
        let (accent, contrast) = (theme.color("accent"), theme.color("accent-contrast-text"));
        let focus_color = theme.color("accent-focus");
        let radius = theme.len("l-radius-sm");
        let zoom = zoom(window);
        let GameButton {
            id,
            icon,
            label,
            look,
            focus,
            ring,
            on_click,
        } = self;
        let focus = focus.unwrap_or_else(|| focus_handle(&id, window, cx));
        let ring = (ring && focus.is_focused(window)) || focus_visible(&focus, window);
        let inner = id.clone();
        let press_id = child_id(&id, "press");
        let pushes = matches!(look, Look::Crash | Look::CrashPrimary);
        let (py, pxx, size, line, weight, gap) = match look {
            Look::Settings | Look::SettingsPrimary => (9., 16., 13., 16., FontWeight::MEDIUM, 9.),
            Look::Crash | Look::CrashPrimary => (10., 18., 13.5, 16., FontWeight::SEMIBOLD, 8.),
            Look::Small => (5., 12., 12.5, 15., FontWeight::SEMIBOLD, 8.),
        };
        hover(id, Timing::fast(), move |t| {
            let primary = look == Look::CrashPrimary;
            let (bg, color) = if primary {
                (mix(accent, brightness(accent, 1.08), t), contrast)
            } else if look == Look::SettingsPrimary {
                (white(0.14), text)
            } else {
                (mix(white(0.05), white(0.1), t), text)
            };
            let settings = matches!(look, Look::Settings | Look::SettingsPrimary);
            let icon_color = if settings { dim } else { color };
            let el = div()
                .id(inner)
                .role(Role::Button)
                .button_text()
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .gap(u(gap))
                .py(u(py))
                .px(u(pxx))
                .border_1()
                .border_color(if primary {
                    gpui::transparent_black()
                } else {
                    strong
                })
                .rounded(u(radius))
                .bg(bg)
                .font_family(RUBIK)
                .text_px(size, line)
                .font_weight(weight)
                .text_color(color)
                .whitespace_nowrap()
                .when(primary, |el| el.min_w(u(110.)).ml_auto())
                .track_focus(&focus)
                .cursor_pointer()
                .when_some(on_click, |el, handler| {
                    el.on_click(move |event, window, cx| handler(event, window, cx))
                })
                .when_some(icon, |el, icon| {
                    el.child(fa_glyph(icon, size).text_color(icon_color))
                })
                .child(label)
                .when(ring, |el| {
                    el.child(outline(zoom, radius, 2., 2., focus_color))
                });
            if pushes {
                press(press_id, Push::fast(0.97), el).into_any_element()
            } else {
                el.into_any_element()
            }
        })
    }
}

/// `filter: brightness(k)`.
pub(crate) fn brightness(color: gpui::Hsla, k: f32) -> gpui::Hsla {
    let c = gpui::Rgba::from(color);
    gpui::Rgba {
        r: (c.r * k).min(1.),
        g: (c.g * k).min(1.),
        b: (c.b * k).min(1.),
        a: c.a,
    }
    .into()
}
