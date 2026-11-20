//! Экраны поверх плеера (`.dmp__screen`): нет записи, не поддерживается, рендер не готов,
//! карточки bxt-rs и FFmpeg, идёт предпросмотр (полоса, «Отмена»), ошибки предпросмотра и видео,
//! готово (большая «Играть», «отменено», «Дополнительные настройки», заметки). У вкладки
//! «Отрендеренные» — «выбери видео» или ошибка видео.
//!
//! Вид — отдельная сущность в слоте плеера: читает состояние страницы при отрисовке.

use super::panel::DemoPanel;
use super::view_settings::ViewSettings;
use super::{BtnLook, btn, btn_icon};
use crate::icons::fa_glyph;
use crate::kit::button::{focus_handle, focus_visible, outline, white};
use crate::layer::TipExt;
use crate::motion::{Timing, hover, mix};
use crate::paint::shadow;
use crate::scale::{u, zoom};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::Entity;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, Div, FontWeight, IntoElement, Render, Role, SharedString, WeakEntity,
    Window, div, prelude::*, relative,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::demo_panel::{DemoPanel as State, Job, Screen};
use hlsr_ui::controllers::demo_tool::error_text;
use serde_json::{Value, json};

pub struct DemoScreens {
    panel: WeakEntity<DemoPanel>,
    /// Экран вкладки «Отрендеренные».
    rendered: bool,
}

impl DemoScreens {
    pub fn new(panel: WeakEntity<DemoPanel>, rendered: bool) -> DemoScreens {
        DemoScreens { panel, rendered }
    }
}

/// `.dmp__screen`: колонка по центру, до 520 px, 13,5 px / 1.5, `--l-text-dim`.
fn column(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(u(12.))
        // max-width 520: дети по центру, поэтому колонка во всю ширину не видна — а переносам
        // текста нужна определённая ширина.
        .w(u(520.))
        .max_w_full()
        .text_size(u(13.5))
        .line_height(relative(1.5))
        .text_color(theme.color("l-text-dim"))
        .text_center()
}

/// Значок экрана 30 px (`--l-text-faint`, у ошибок — `--l-level-error`).
fn screen_icon(classes: &str, error: bool, theme: &Theme) -> AnyElement {
    fa_glyph(classes, 30.)
        .text_color(theme.color(if error {
            "l-level-error"
        } else {
            "l-text-faint"
        }))
        .into_any_element()
}

/// Абзац экрана: не шире колонки (переносится внутри 520 px).
fn para(text: String) -> Div {
    div().max_w_full().child(text)
}

fn title(text: String, theme: &Theme) -> AnyElement {
    div()
        .max_w_full()
        .text_px(15., 15. * 1.4)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.color("l-text"))
        .child(text)
        .into_any_element()
}

fn note(icon: Option<&str>, text: String, theme: &Theme) -> AnyElement {
    div()
        .max_w_full()
        .flex()
        .items_center()
        .gap(u(8.))
        .text_size(u(12.))
        .text_color(theme.color("l-text-faint"))
        .when_some(icon, |el, icon| el.child(fa_glyph(icon, 12.)))
        .child(div().min_w_0().child(text))
        .into_any_element()
}

/// Что нужно экранам из состояния страницы (снимок на кадр).
pub struct ScreenView {
    screen: Option<Screen>,
    rendered_media_error: bool,
    item: Value,
    render_code: Value,
    game_name: String,
    job: Option<Job>,
    preview_error: Value,
    ffmpeg: (Value, Option<String>),
    bxtrs: (Value, Option<String>),
    can_preview: bool,
    cancelled: bool,
    language: Language,
    view_settings: Entity<ViewSettings>,
}

impl DemoPanel {
    fn screen_view(&self) -> ScreenView {
        let s = &self.s;
        ScreenView {
            screen: s.screen(),
            rendered_media_error: s.rendered.media_error,
            item: s.item().cloned().unwrap_or(json!({})),
            render_code: s
                .detail
                .as_ref()
                .and_then(|detail| detail.get("render"))
                .and_then(|render| render.get("code"))
                .cloned()
                .unwrap_or_default(),
            game_name: s.item_game_name(),
            job: s.job.clone(),
            preview_error: s.preview_error.clone().unwrap_or_default(),
            ffmpeg: (s.ffmpeg.clone().unwrap_or_default(), s.ffmpeg_error.clone()),
            bxtrs: (s.bxtrs.clone().unwrap_or_default(), s.bxtrs_error.clone()),
            can_preview: s.can_preview(),
            cancelled: s.preview_cancelled(),
            language: self.language,
            view_settings: self.view_settings.clone(),
        }
    }
}

impl DemoScreens {
    fn screen(&self, v: ScreenView, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let theme = cx.global::<Theme>().clone();
        let language = v.language;
        let t = move |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args);
        let tr = |key: &str| t(key, &[]);
        let weak = self.panel.clone();
        let act =
            |f: fn(&mut DemoPanel, &mut Window, &mut Context<DemoPanel>)| super::act(&weak, f);
        if self.rendered {
            let body = if v.rendered_media_error {
                column(&theme)
                    .child(screen_icon("fas fa-exclamation-triangle", true, &theme))
                    .child(title(tr("#L_DEMO_PLAYER_ERROR"), &theme))
            } else {
                column(&theme)
                    .child(screen_icon("fas fa-film", false, &theme))
                    .child(para(tr("#L_DEMO_RENDERED_PICK")))
            };
            return Some(body.into_any_element());
        }
        let text = theme.color("l-text");
        let body = match v.screen? {
            Screen::Empty => column(&theme)
                .child(screen_icon("fas fa-film", false, &theme))
                .child(para(tr("#L_DEMO_SCREEN_EMPTY"))),
            Screen::Unsupported => column(&theme)
                .child(screen_icon("fas fa-ban", false, &theme))
                .child(para(State::reason_text(&v.item, &t))),
            Screen::NotReady => {
                let waiting = v.render_code.as_str() == Some("RENDER_NOT_READY");
                let error = json!({ "code": v.render_code, "gameName": v.game_name });
                column(&theme)
                    .child(screen_icon(
                        if waiting {
                            "fas fa-hourglass-half"
                        } else {
                            "fas fa-exclamation-triangle"
                        },
                        false,
                        &theme,
                    ))
                    .child(title(error_text(&error, &t), &theme))
                    .child(para(tr("#L_DEMO_SCREEN_TIMELINE")))
            }
            Screen::Bxtrs => column(&theme).child(super::ffmpeg_card::card(
                true, v.bxtrs.0, v.bxtrs.1, language, &weak, window, cx,
            )),
            Screen::Ffmpeg => column(&theme).child(super::ffmpeg_card::card(
                false, v.ffmpeg.0, v.ffmpeg.1, language, &weak, window, cx,
            )),
            Screen::PreviewJob => {
                let job = v.job?;
                let percent = if job.percent.is_nan() {
                    0.
                } else {
                    job.percent.clamp(0., 100.)
                };
                let cancel = btn(
                    "dmp-preview-cancel",
                    BtnLook::PLAIN,
                    false,
                    None,
                    vec![
                        btn_icon("fas fa-times", text),
                        tr("#L_DEMO_JOB_CANCEL").into_any_element(),
                    ],
                    window,
                    cx,
                    act(|this, _, cx| this.cancel_job(cx)),
                );
                column(&theme)
                    .child(title(tr("#L_DEMO_PREVIEW_RUNNING"), &theme))
                    .child(para(State::job_line(&job, &t)))
                    .child(
                        div()
                            .w(u(280.))
                            .h(u(6.))
                            .rounded(u(3.))
                            .bg(white(0.12))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(percent as f32 / 100.))
                                    .bg(theme.color("accent")),
                            ),
                    )
                    .child(crate::snap::target("dmp-preview-cancel", cancel))
                    .child(note(Some("fab fa-steam"), tr("#L_DEMO_STEAM_NOTE"), &theme))
            }
            Screen::PreviewError => {
                let again = btn(
                    "dmp-preview-again",
                    BtnLook::PLAIN,
                    false,
                    None,
                    vec![
                        btn_icon("fas fa-redo", text),
                        tr("#L_DEMO_PREVIEW").into_any_element(),
                    ],
                    window,
                    cx,
                    act(|this, _, cx| this.start_preview(false, cx)),
                );
                column(&theme)
                    .child(screen_icon("fas fa-exclamation-triangle", true, &theme))
                    .child(title(error_text(&v.preview_error, &t), &theme))
                    .child(crate::snap::target("dmp-preview-again", again))
            }
            Screen::MediaError => {
                let back = btn(
                    "dmp-media-back",
                    BtnLook::PLAIN,
                    false,
                    None,
                    vec![
                        btn_icon("fas fa-arrow-left", text),
                        tr("#L_DEMO_PLAYER_BACK").into_any_element(),
                    ],
                    window,
                    cx,
                    act(|this, _, cx| this.close_media(cx)),
                );
                column(&theme)
                    .child(screen_icon("fas fa-exclamation-triangle", true, &theme))
                    .child(title(tr("#L_DEMO_PLAYER_ERROR"), &theme))
                    .child(back)
            }
            Screen::Ready => {
                let play = play_button(
                    v.can_preview,
                    tr("#L_DEMO_PREVIEW_HINT"),
                    tr("#L_DEMO_PREVIEW"),
                    window,
                    cx,
                    act(|this, _, cx| this.start_preview(false, cx)),
                );
                let cancelled = v.cancelled.then(|| {
                    div()
                        .flex()
                        .items_center()
                        .gap(u(8.))
                        .px(u(12.))
                        .py(u(4.))
                        .rounded(u(12.))
                        .bg(white(0.07))
                        .text_size(u(12.5))
                        .text_color(text)
                        .child(fa_glyph("fas fa-ban", 11.).text_color(theme.color("l-text-dim")))
                        .child(tr("#L_DEMO_PREVIEW_CANCELLED"))
                });
                column(&theme)
                    .child(crate::snap::target("dmp-play", play))
                    .children(cancelled)
                    .child(para(tr("#L_DEMO_SCREEN_READY")))
                    .child(v.view_settings)
                    .child(note(None, tr("#L_DEMO_PREVIEW_TIME"), &theme))
                    .child(note(Some("fab fa-steam"), tr("#L_DEMO_STEAM_NOTE"), &theme))
            }
        };
        Some(body.into_any_element())
    }
}

/// `.dmp__play`: круг 64 px акцентом со свечением; значок 22 px сдвинут на 4 px вправо.
fn play_button(
    enabled: bool,
    hint: String,
    label: String,
    window: &mut Window,
    cx: &mut App,
    on_click: super::OnClick,
) -> AnyElement {
    let id: gpui::ElementId = "dmp-play-btn".into();
    let zoom = zoom(window);
    let focus = focus_handle(&id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let theme = cx.global::<Theme>();
    let (bg, hot, text, accent) = (
        theme.color("accent"),
        theme.color("accent-hover"),
        theme.color("accent-contrast-text"),
        theme.color("l-accent"),
    );
    let inner = id.clone();
    hover(id, Timing::fast(), move |t| {
        let t = if enabled { t } else { 0. };
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .aria_label(SharedString::from(label))
            .tip(hint)
            .relative()
            .size(u(64.))
            .flex()
            .items_center()
            .justify_center()
            .pl(u(4.))
            .rounded_full()
            .bg(mix(bg, hot, t))
            .shadow(vec![shadow(zoom, 0., 8., 26., 0., accent.opacity(0.4))])
            .text_color(text)
            .child(fa_glyph("fas fa-play", 22.))
            .when(!enabled, |el| el.opacity(0.5))
            .when(enabled, |el| {
                el.track_focus(&focus)
                    .cursor_pointer()
                    .on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .when(ring_on, |el| {
                el.child(outline(zoom, 32., 3., 2., gpui::white()))
            })
    })
    .into_any_element()
}

impl Render for DemoScreens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(panel) = self.panel.upgrade() else {
            return div().into_any_element();
        };
        let view = panel.read(cx).screen_view();
        self.screen(view, window, cx)
            .unwrap_or_else(|| div().into_any_element())
    }
}
