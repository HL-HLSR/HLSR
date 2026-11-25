//! Инструмент в «Играх» (`ToolPanel.vue` 0.4.9) двумя местами оболочки: герой
//! ([`ToolPanel`], `hero-only`: название, подзаголовок, описание, размер загрузки) и нижняя
//! панель ([`ToolControls`], `controls-only`: отмена установки, справка, папка, удаление, главное
//! действие, полоса установки). Карточка — `currentTool` оболочки, тексты и действие —
//! [`tool_panel::view`], нажатия — сценарии оболочки (`onTool*`).

use super::hero::{subtitle, title};
use crate::icons::icon;
use crate::kit::{Look, Tone, button};
use crate::layer::{Layers, TipExt};
use crate::liquid::Footer;
use crate::motion::{Timing, hover, mix, tween};
use crate::paint::shadow;
use crate::scale::{u, viewport, zoom, zpx};
use crate::shell::root::ShellContext;
use crate::text::{RUBIK, TextExt, spaced};
use crate::theme::{Gradient, Theme};
use gpui::{
    App, Bounds, Context, Corners, Div, Edges, Font, FontWeight, Global, IntoElement, Render,
    RenderImage, SharedString, TextRun, Window, canvas, div, font, prelude::*, relative, rgba,
    size, white,
};
use hlsr_core::language::Language;
use hlsr_ui::Rgba8Image;
use hlsr_ui::controllers::shell_flows::ShellFlows;
use hlsr_ui::controllers::tool_panel::{self, ToolAction, ToolView};
use hlsr_ui::render::rail_glass::Placement;
use hlsr_ui::services::Services;
use hlsr_ui::tr;
use std::sync::Arc;

/// Язык и карточка `currentTool`; без оболочки — пустая карточка.
fn current(cx: &App) -> (Language, ToolView) {
    let Some(context) = ShellContext::get(cx) else {
        return (
            Language::En,
            tool_panel::view(Language::En, &tool_panel::empty_tool()),
        );
    };
    let language = context.shell.language();
    let card = context
        .flows
        .with_state(|state| tool_panel::current_tool(&state.tools, &state.selected_tool_id));
    (language, tool_panel::view(language, &card))
}

/// Сценарий оболочки по нажатию (`$emit` панели).
fn emit(cx: &App, act: impl FnOnce(&ShellFlows)) {
    if let Some(context) = ShellContext::get(cx) {
        act(&context.flows);
    }
}

/// Герой инструмента (`.tool-panel--hero`).
pub struct ToolPanel;

impl ToolPanel {
    pub fn new(_services: &Services, _cx: &mut Context<ToolPanel>) -> ToolPanel {
        ToolPanel
    }
}

impl Render for ToolPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let (_, view) = current(cx);
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        // `@media (max-width: 1599px)`: описание ложится на синий шарик фона — под ним тёмная
        // подложка, размытая на 12 px (`filter: blur`, σ = 12) — тенью той же формы без заливки
        // (`box-shadow` 24 px: σ = 24 / 2).
        let narrow = viewport(window).0 < 1600.;
        let description = (!view.description.is_empty()).then(|| {
            div()
                .relative()
                .text_px(14., 24.)
                .text_color(theme.color("l-text-dim"))
                .when(narrow, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(u(-10.))
                            .bottom(u(-10.))
                            .left(u(-16.))
                            .right(u(-16.))
                            .rounded(u(16.))
                            .shadow(vec![shadow(zoom, 0., 0., 24., 0., rgba(0x0a0d12cc).into())]),
                    )
                })
                .child(
                    div()
                        .relative()
                        .child(SharedString::from(view.description.clone())),
                )
        });
        let size = (!view.size_text.is_empty()).then(|| {
            div()
                .text_px(13., 20.)
                .text_color(theme.color("l-text-faint"))
                .child(SharedString::from(view.size_text.clone()))
        });
        let tool_id = view.tool_id.clone();
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(theme.rem("l-hero-gap"))
                    .pt(theme.rem("l-hero-top"))
                    .px(theme.rem("l-hero-pad"))
                    .max_w(u(640.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(title(
                                format!("tool-title:{tool_id}").into(),
                                &view.title,
                                &theme,
                            ))
                            .child(subtitle(
                                format!("tool-sub:{tool_id}").into(),
                                &view.subtitle,
                                &theme,
                            )),
                    )
                    .children(description)
                    .children(size),
            )
    }
}

/// Нижняя панель инструмента (`.tool-panel--controls`).
pub struct ToolControls;

impl ToolControls {
    pub fn new(_services: &Services, _cx: &mut Context<ToolControls>) -> ToolControls {
        ToolControls
    }
}

/// Кнопка слева от действия (`.tool-panel__help/__folder/__remove`): 64 × `--l-play-h`, при
/// наведении только светлеет, при нажатии — оттенок акцента.
pub(crate) fn side_button(
    id: &'static str,
    classes: &'static str,
    label: String,
    disabled: bool,
    theme: &Theme,
    on_click: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let solid = theme.color("l-surface-solid");
    let lit = solid.blend(white().opacity(0.08));
    let pressed = solid.blend(theme.color("accent").opacity(0.16));
    let (dim, text) = (theme.color("l-text-dim"), theme.color("l-text"));
    let (height, radius) = (theme.rem("l-play-h"), theme.rem("l-radius"));
    hover(id, Timing::fast(), move |t| {
        let t = if disabled { 0. } else { t };
        let color = mix(dim, text, t);
        div()
            .id(id)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .w(u(64.))
            .h(height)
            .rounded(radius)
            .bg(mix(solid, lit, t))
            .child(
                div()
                    .w(u(24.))
                    .flex()
                    .justify_center()
                    .child(icon(classes, 18., color)),
            )
            .when(disabled, |el| el.opacity(0.5))
            .when(!disabled, |el| {
                el.cursor_pointer()
                    .active(move |s| s.bg(pressed))
                    .tip(label)
                    .on_click(move |_, _, cx| on_click(cx))
            })
    })
}

impl Render for ToolControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let (language, view) = current(cx);
        let theme = cx.global::<Theme>().clone();
        let text = |key: &str| tr(language, key, &[]);
        let id = view.tool_id.clone();
        let with = |act: fn(&ShellFlows, &str)| {
            let id = id.clone();
            move |cx: &mut App| emit(cx, |flows| act(flows, &id))
        };

        let tone = if view.action_progress {
            Tone::Progress
        } else {
            Tone::Ready
        };
        let action_text = theme.color(if view.action_progress {
            "btn-progress-text"
        } else {
            "btn-ready-text"
        });
        let play_font = theme.len("l-play-font");
        let label = view.action_label.to_uppercase();
        // `v-grid-width:8` пары «значок + текст»: ширина вверх до кратной 8 (разрядка — как у
        // [`spaced`], после каждого знака).
        let zoom = zoom(window);
        let run = TextRun {
            len: label.len(),
            font: Font {
                weight: FontWeight::BOLD,
                ..font(RUBIK)
            },
            color: action_text,
            ..Default::default()
        };
        let line = window
            .text_system()
            .layout_line(&label, zpx(zoom, play_font), &[run], None);
        let glyphs: usize = line.runs.iter().map(|run| run.glyphs.len()).sum();
        let pair = 24. + 12. + f32::from(line.width) / zoom + 0.12 * play_font * glyphs as f32;
        let pair = ((pair / 8. - 0.01).ceil() * 8.).max(0.);
        let action = view.action;
        let action_id = id.clone();
        let main = button("tool-action", Look::Tone(tone))
            .disabled(action.is_none())
            .min_w(theme.rem("l-play-w"))
            .h(theme.rem("l-play-h"))
            .px(u(28.))
            .on_click(move |_, _, cx| {
                let Some(action) = action else {
                    return;
                };
                emit(cx, |flows| match action {
                    ToolAction::Stop => flows.on_tool_stop(&action_id),
                    ToolAction::Launch => flows.on_tool_launch(&action_id),
                    ToolAction::Install => flows.on_tool_install(&action_id),
                });
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .min_w(u(pair))
                    .child(div().w(u(24.)).flex().justify_center().child(icon(
                        view.action_icon,
                        play_font,
                        action_text,
                    )))
                    .child(spaced(
                        label,
                        play_font,
                        24.,
                        FontWeight::BOLD,
                        0.12,
                        action_text,
                    )),
            );
        // Занятая кнопка 0.4.9 — `opacity: .6`; у тоновой кнопки набора .75: .75 × .8.
        let main = div()
            .when(action.is_none(), |el| el.opacity(0.8))
            .child(main);

        let bar = div()
            .relative()
            .flex()
            .items_center()
            .gap(theme.rem("l-control-gap"))
            .h(theme.rem("l-bottombar-h"))
            .pl(theme.rem("l-hero-pad"))
            .pr(theme.rem("l-edge-pad"))
            .child(div().flex_1())
            .when(view.show_cancel, |bar| {
                bar.child(side_button(
                    "tool-cancel",
                    "fas fa-times",
                    text("#L_CANCEL"),
                    false,
                    &theme,
                    with(ShellFlows::on_tool_cancel),
                ))
            })
            .when(view.has_help, |bar| {
                bar.child(side_button(
                    "tool-help",
                    "fas fa-question",
                    text("#L_TOOL_HELP"),
                    false,
                    &theme,
                    with(ShellFlows::on_tool_help),
                ))
            })
            .when(view.installed, |bar| {
                bar.child(side_button(
                    "tool-folder",
                    "fas fa-folder-open",
                    text("#L_TOOL_FOLDER"),
                    view.busy,
                    &theme,
                    with(ShellFlows::on_tool_folder),
                ))
                .child(side_button(
                    "tool-remove",
                    "fas fa-trash",
                    text("#L_TOOL_REMOVE"),
                    view.busy || view.running,
                    &theme,
                    with(ShellFlows::on_tool_remove),
                ))
            })
            .child(main);

        let accent = theme.color("l-accent");
        let progress = view.busy.then(|| {
            tween(
                "tool-progress",
                view.percent as f32,
                Timing::fast(),
                move |percent| {
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(u(4.))
                        .bg(white().opacity(0.08))
                        .overflow_hidden()
                        .child(div().h_full().w(relative(percent / 100.)).bg(accent))
                },
            )
        });

        div()
            .w_full()
            .relative()
            .flex_none()
            .child(feather(&theme, viewport(window).0))
            .child(bar)
            .children(progress)
    }
}

const FOOTER_TEXTURE_WIDTH: u32 = 256;
const FOOTER_TEXTURE_HEIGHT: u32 = 64;

fn footer_raster(fill: &Gradient, view_width: f32, lead: f32) -> Rgba8Image {
    let (width, height) = (FOOTER_TEXTURE_WIDTH, FOOTER_TEXTURE_HEIGHT);
    let rect = Placement {
        x: 0.,
        y: 0.,
        width: view_width,
        height: height as f32,
    };
    let footer = Footer::new(rect, fill, lead);
    let mut raster = Rgba8Image::new(width + 2, height + 2);
    // По краям дублируем тексель, чтобы фильтр атласа брал цвет того же градиента.
    for (row, pixels) in raster
        .make_mut_slice()
        .chunks_exact_mut((width + 2) as usize)
        .enumerate()
    {
        let y = (row.saturating_sub(1) as u32).min(height - 1) as f32 + 0.5;
        for (col, pixel) in pixels.iter_mut().enumerate() {
            let x = (col.saturating_sub(1) as u32).min(width - 1) as f32 + 0.5;
            *pixel = footer.at(x / width as f32 * view_width, y);
        }
    }
    raster
}

#[derive(Default)]
struct FooterTexture(Option<(Gradient, f32, Arc<RenderImage>)>);

impl Global for FooterTexture {}

impl FooterTexture {
    fn image(
        fill: &Gradient,
        width: f32,
        lead: f32,
        window: &mut Window,
        cx: &mut App,
    ) -> Arc<RenderImage> {
        let texture = &mut cx.default_global::<Self>().0;
        if let Some((previous, old_width, image)) = texture
            && previous == fill
            && *old_width == width
        {
            return image.clone();
        }
        let image = crate::image::render_image(&footer_raster(fill, width, lead));
        if let Some((_, _, old)) = texture.replace((fill.clone(), width, image.clone())) {
            cx.drop_image(old, Some(window));
        }
        image
    }
}

/// `::before` панели: градиент снизу вверх, умноженный на плавную маску слева.
pub(crate) fn feather(theme: &Theme, view_width: f32) -> Div {
    // --l-footer-lead-in: clamp(150px, 13vw, 260px).
    let view_width = view_width.max(1.);
    let lead = (view_width * 0.13).clamp(150., 260.);
    let fill = theme.gradient("l-footer-fill");
    let liquid = crate::liquid::active(theme);
    // Liquid Glass: слой идёт под стекло кнопок панели (они рисуются позже в том же кадре).
    let gradient = canvas(
        move |bounds, window, cx| {
            if liquid {
                crate::liquid::footer(cx, bounds, zoom(window), &fill, lead);
            }
            FooterTexture::image(&fill, view_width, lead, window, cx)
        },
        move |bounds, image, window, cx| {
            // Размер растра зависит от окна, а видимую область задаёт панель. Её правый
            // нижний угол совпадает с углом окна и использует тот же радиус.
            let body = Bounds::new(
                bounds.origin,
                size(zpx(zoom(window), view_width), bounds.size.height),
            );
            let pad_x = body.size.width / FOOTER_TEXTURE_WIDTH as f32;
            let pad_y = body.size.height / FOOTER_TEXTURE_HEIGHT as f32;
            let padded = body.extend(Edges {
                left: pad_x,
                right: pad_x,
                top: pad_y,
                bottom: pad_y,
            });
            let corners = Corners {
                bottom_right: Layers::radius(cx),
                ..Default::default()
            };
            window
                .paint_image(bounds, padded, corners, image, 0, false)
                .ok();
        },
    )
    .absolute()
    .inset_0();
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .top(u(-theme.len("l-footer-feather")))
        .child(gradient)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footer_feathers_without_gaps_before_the_full_fill() {
        let fill = Theme::new(crate::theme::Table::embedded()).gradient("l-footer-fill");
        let raster = footer_raster(&fill, 1600., 208.);
        let stride = raster.width() as usize;
        let top = &raster.as_slice()[stride..2 * stride];
        let bottom = &raster.as_slice()[raster.as_slice().len() - 2 * stride..][..stride];

        assert!(bottom[1].a < 5);
        assert!(bottom[17].a > 100 && bottom[17].a < 130);
        assert_eq!(bottom[35].a, bottom[stride - 2].a);
        assert!(bottom[35].a > 240 && top[35].a < 15);
        assert!(bottom.windows(2).all(|pair| pair[0].a <= pair[1].a));
    }
}
