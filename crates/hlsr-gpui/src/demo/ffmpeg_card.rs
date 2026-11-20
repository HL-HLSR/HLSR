//! Скачиваемый инструмент «Демо» (`DemoFfmpegCard.vue` 0.4.9): FFmpeg (предпросмотр и экспорт; на
//! Linux — из PATH) или bxt-rs (рендер HL1, GPL-3, ссылки на исходник и лицензию): размер, зачем
//! нужен, ошибка установки, ход загрузки с процентом, «Скачать» / «Скачать заново» / «Отмена».

use super::panel::DemoPanel;
use super::{BtnLook, btn, btn_icon};
use crate::icons::fa_glyph;
use crate::kit::button::white;
use crate::motion::{Timing, hover};
use crate::paint::ring;
use crate::scale::{u, zoom};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Div, FontWeight, IntoElement, Role, SharedString, WeakEntity, Window, div,
    prelude::*, relative,
};
use hlsr_core::js;
use hlsr_core::language::Language;
use hlsr_ui::controllers::boot::format_size;
use hlsr_ui::controllers::demo_tool::error_text;
use serde_json::{Value, json};

/// Ключи текстов карточки: заголовок, размер, зачем, «Скачать», «Загрузка».
struct Texts {
    title: &'static str,
    size: &'static str,
    why: &'static str,
    install: &'static str,
    installing: &'static str,
}

const FFMPEG: Texts = Texts {
    title: "#L_DEMO_FFMPEG_TITLE",
    size: "#L_DEMO_FFMPEG_TEXT",
    why: "#L_DEMO_FFMPEG_WHY",
    install: "#L_DEMO_FFMPEG_INSTALL",
    installing: "#L_DEMO_FFMPEG_INSTALLING",
};

const BXTRS: Texts = Texts {
    title: "#L_DEMO_BXTRS_TITLE",
    size: "#L_DEMO_BXTRS_TEXT",
    why: "#L_DEMO_BXTRS_WHY",
    install: "#L_DEMO_BXTRS_INSTALL",
    installing: "#L_DEMO_BXTRS_INSTALLING",
};

/// Карточка инструмента: `status` — `demo-ffmpeg-status`/`demo-bxtrs-status`, `error` — код
/// последней неудачной установки. Ширина — до 460 px (окно экспорта снимает предел: max_w).
pub fn card(
    bxtrs: bool,
    status: Value,
    error: Option<String>,
    language: Language,
    panel: &WeakEntity<DemoPanel>,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let theme = cx.global::<Theme>().clone();
    let zoom = zoom(window);
    let t = move |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args);
    let texts = if bxtrs { &BXTRS } else { &FFMPEG };
    let get = |name: &str| status.get(name);
    let installable = get("installable") != Some(&Value::Bool(false));
    let state = get("state").and_then(Value::as_str).unwrap_or_default();
    let installing = state == "installing";
    let broken = state == "error";
    let percent = get("percent")
        .and_then(Value::as_f64)
        .unwrap_or(0.)
        .round()
        .clamp(0., 100.);
    let size = format_size(get("sizeBytes").and_then(Value::as_f64).unwrap_or(0.), &t);
    let version = if installable {
        get("pinnedVersion")
            .filter(|version| js::truthy(version))
            .map_or_else(|| "9.0.2".to_owned(), js::to_string)
    } else {
        String::new()
    };
    let title = t(texts.title, &[&version]).trim().to_owned();
    let problem = error
        .or_else(|| {
            broken
                .then(|| get("code").and_then(Value::as_str).map(str::to_owned))
                .flatten()
        })
        .filter(|code| !code.is_empty() && code != "CANCELLED")
        .map(|code| error_text(&json!({ "code": code }), &t));
    let (dim, faint, text, accent) = (
        theme.color("l-text-dim"),
        theme.color("l-text-faint"),
        theme.color("l-text"),
        theme.color("accent"),
    );
    let line = |text: String, color| {
        div()
            .text_px(12.5, 12.5 * 1.45)
            .text_color(color)
            .child(text)
    };
    let mut body = div().flex_1().min_w_0().flex().flex_col().gap(u(4.)).child(
        div()
            .text_px(14., 20.)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(text)
            .child(title),
    );
    if installable {
        body = body
            .child(line(t(texts.size, &[&size]), dim))
            .child(line(t(texts.why, &[]), faint));
        let links: Vec<AnyElement> = [
            ("sourceLink", "fas fa-code", "#L_DEMO_BXTRS_SOURCE"),
            ("licenseLink", "fas fa-file-alt", "#L_DEMO_BXTRS_LICENSE"),
        ]
        .into_iter()
        .filter_map(|(field, glyph, label)| {
            let key = get(field)
                .filter(|key| js::truthy(key))
                .map(js::to_string)?;
            let panel = panel.clone();
            let id = SharedString::from(format!("dff-{field}"));
            let inner = id.clone();
            let label = t(label, &[]);
            Some(
                hover(id, Timing::fast(), move |hot| {
                    div()
                        .id(inner)
                        .role(Role::Link)
                        // `.dff__link` 0.4.9 — `<button>`: разрядки `body` нет.
                        .button_text()
                        .flex()
                        .items_center()
                        .gap(u(6.))
                        .cursor_pointer()
                        .text_size(u(12.5))
                        .text_color(accent)
                        .when(hot > 0.5, |el| el.underline())
                        .child(fa_glyph(glyph, 11.))
                        .child(label)
                        .on_click(move |_, _, cx| {
                            let key = key.clone();
                            panel.update(cx, |panel, cx| panel.open_link(key, cx)).ok();
                        })
                })
                .into_any_element(),
            )
        })
        .collect();
        if !links.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(u(16.))
                    .gap_y(u(4.))
                    .children(links),
            );
        }
    } else {
        let key = if bxtrs {
            "#L_DEMO_ERR_RENDER_NOT_READY"
        } else {
            "#L_DEMO_FFMPEG_LINUX"
        };
        body = body.child(line(t(key, &[]), dim));
    }
    if let Some(problem) = problem {
        body = body.child(line(problem, theme.color("l-level-error")));
    }
    if installing {
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(u(6.))
                .mt(u(6.))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_size(u(12.))
                        .text_color(dim)
                        .child(t(texts.installing, &[]))
                        .child(div().tnum().child(format!("{percent}%"))),
                )
                .child(
                    div()
                        .h(u(4.))
                        .rounded(u(2.))
                        .bg(white(0.1))
                        .overflow_hidden()
                        .child(div().h_full().w(relative(percent as f32 / 100.)).bg(accent)),
                ),
        );
    }
    if installable {
        let look = |primary| BtnLook {
            primary,
            height: 34.,
            px: 14.,
            push: false,
        };
        let action = if installing {
            btn(
                if bxtrs {
                    "dff-bxtrs-cancel"
                } else {
                    "dff-ffmpeg-cancel"
                },
                look(false),
                false,
                None,
                vec![
                    btn_icon("fas fa-times", text),
                    t("#L_DEMO_JOB_CANCEL", &[]).into_any_element(),
                ],
                window,
                cx,
                super::act(panel, move |this, _, cx| this.cancel_tool(bxtrs, cx)),
            )
        } else {
            btn(
                if bxtrs {
                    "dff-bxtrs-install"
                } else {
                    "dff-ffmpeg-install"
                },
                look(true),
                false,
                None,
                vec![
                    btn_icon("fas fa-download", theme.color("accent-contrast-text")),
                    t(
                        if broken {
                            "#L_DEMO_FFMPEG_REINSTALL"
                        } else {
                            texts.install
                        },
                        &[],
                    )
                    .into_any_element(),
                ],
                window,
                cx,
                super::act(panel, move |this, _, cx| this.install_tool(bxtrs, cx)),
            )
        };
        let name = if installing {
            "dff-cancel"
        } else {
            "dff-install"
        };
        body = body.child(
            div()
                .flex()
                .gap(u(8.))
                .mt(u(10.))
                .child(crate::snap::target(name, action)),
        );
    }
    div()
        .flex()
        .gap(u(14.))
        .w_full()
        .max_w(u(460.))
        .py(u(16.))
        .px(u(18.))
        .rounded(theme.rem("l-radius-sm"))
        .bg(theme.color("l-modal-bg"))
        .shadow(vec![ring(zoom, 1., theme.color("l-modal-border"))])
        .text_left()
        .child(
            div()
                .flex_none()
                .size(u(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(theme.color("accent-soft"))
                .text_color(accent)
                .child(fa_glyph("fas fa-download", 14.)),
        )
        .child(body)
}
