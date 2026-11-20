//! Превью персонализации (`GhostingSettingsPreview.vue` 0.4.9, G9): кадр Ghosting или HL1 в
//! карточке с панелью масштаба ([`super::viewport`]), подписи под ним и окно во весь лаунчер.
//! У разделов «Призраки и запись» и «Эффекты и спреи» — своя сцена; призрак — в портретном окне
//! (5:8 от высоты, в низком окне — вся колонка), ширина кадра следует за окном вида.
//!
//! Логика — `PreviewController` (`hlsr-ui`): кадры рисует его поток на ЦП, сюда они приходят
//! картинкой и уходят в атлас GPUI тремя уровнями (кадр, половина, четверть: уменьшение больше
//! чем вдвое — с уменьшенной копии, как мип-уровни Skia у `drawImage`).

use super::viewport::{Viewport, ViewportProps};
use crate::image::{Raster, render_image};
use crate::layer::{Modal, TipExt, card_plain};
use crate::scale::{u, zoom};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext, Context, EventEmitter, FocusHandle, FontWeight, IntoElement,
    ParentElement, Render, RenderImage, Styled, Task, Window, div, hsla, prelude::*, rgb, rgba,
};
use hlsr_core::language::Language;
use hlsr_ui::Rgba8Image;
use hlsr_ui::controllers::preview::{
    Invoke, Pixmap, PreviewController, RenderWorker, SceneLine, State, Variant, with_worker,
};
use hlsr_ui::render::ghost_scene::Kind as SceneKind;
use hlsr_ui::services::Services;
use serde_json::Value;
use std::rc::Rc;
use std::sync::Arc;

/// Высота кадра превью.
const FRAME_HEIGHT: f64 = 900.;
/// Уровней картинки кадра: сам кадр, половина, четверть.
const LEVELS: usize = 3;

/// Что превью сообщает редактору.
pub enum PreviewEvent {
    /// `$emit('overlay', value)`: оверлей LiveSplit перетащили.
    Overlay(Value),
}

impl EventEmitter<PreviewEvent> for GhostingPreview {}

/// Что пришло от контроллера (из его вызовов и потока рисования).
enum Signal {
    State,
    Frame(Pixmap, Option<SceneKind>),
    Overlay(Value),
}

/// Вход превью от редактора (`props`); меняется — уходит в контроллер.
#[derive(Default, PartialEq)]
pub struct Inputs {
    pub value: Value,
    pub section: String,
    pub active: bool,
    pub overlay: Option<Value>,
    /// Снимок окна LiveSplit (G10); `Rc` — равенство сначала по адресу, без сверки пикселей.
    pub overlay_snapshot: Option<Rc<hlsr_ui::controllers::preview::Snapshot>>,
    pub overlay_editing: bool,
    pub hud_mod: bool,
}

/// Какой вид: в карточке или в окне во весь лаунчер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Which {
    Inline,
    Expanded,
}

pub struct GhostingPreview {
    pub(super) controller: Rc<PreviewController>,
    _worker: RenderWorker,
    pub(super) state: State,
    pub(super) language: Language,
    inputs: Inputs,
    /// Картинки кадра по уровням (кадр, ½, ¼) и сцена, чей это кадр.
    levels: [Raster; LEVELS],
    levels_scene: Option<SceneKind>,
    shown: [Option<Arc<RenderImage>>; LEVELS],
    /// Кадр, ждущий перевода, и идёт ли перевод: переводится один кадр за раз, пришедшие за это
    /// время заменяют друг друга — в работу идёт последний.
    pending: Option<(Pixmap, Option<SceneKind>)>,
    converting: bool,
    pub(super) inline: Viewport,
    pub(super) expanded: Viewport,
    modal: Modal,
    close_focus: FocusHandle,
    _signals: Task<()>,
}

/// Кадр → картинки GPUI трёх уровней (в фоне: перевод в BGRA и уменьшение).
fn levels_of(frame: Pixmap) -> Vec<Arc<RenderImage>> {
    let image = |p: &Pixmap| {
        render_image(&Rgba8Image::from_bytes(p.width(), p.height(), p.data()).premultiplied())
    };
    let mut out = vec![image(&frame)];
    let mut current = frame;
    while out.len() < LEVELS {
        let Some(next) = hlsr_ui::render::preview::halve(&current) else {
            break;
        };
        out.push(image(&next));
        current = next;
    }
    out
}

impl GhostingPreview {
    /// Превью `variant` игры `game_id`; `inputs` — первое значение редактора.
    pub fn new(
        variant: Variant,
        game_id: &str,
        services: Services,
        language: Language,
        inputs: Inputs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> GhostingPreview {
        let env = crate::env::GpuiEnv::new(cx);
        let invoke: Invoke = {
            let env = env.clone();
            Rc::new(move |channel, args, reply| {
                let answer = services.invoke(channel, args);
                hlsr_ui::env::Env::spawn_local(
                    &*env,
                    Box::pin(async move {
                        // Отказ main — как пустой ответ (`catch` 0.4.9: «файл недоступен»).
                        reply(answer.await.unwrap_or(Value::Null));
                    }),
                );
            })
        };
        let controller = PreviewController::new(variant, game_id, invoke);
        let worker = with_worker(&controller, hlsr_ui::env::dispatch(&*env));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Signal>();
        {
            let tx = tx.clone();
            controller.on_state(move |_| {
                let _ = tx.send(Signal::State);
            });
        }
        {
            let tx = tx.clone();
            controller.on_frame(move |frame, scene| {
                let _ = tx.send(Signal::Frame(frame, scene));
            });
        }
        controller.on_overlay(move |value| {
            let _ = tx.send(Signal::Overlay(value));
        });
        let signals = cx.spawn(async move |this, cx| {
            while let Some(signal) = rx.recv().await {
                if this.update(cx, |this, cx| this.signal(signal, cx)).is_err() {
                    break;
                }
            }
        });
        cx.on_release(|this: &mut GhostingPreview, cx| {
            for raster in &mut this.levels {
                raster.release(cx);
            }
        })
        .detach();
        let mut this = GhostingPreview {
            state: controller.state(),
            controller,
            _worker: worker,
            language,
            inputs: Inputs::default(),
            levels: Default::default(),
            levels_scene: None,
            shown: Default::default(),
            pending: None,
            converting: false,
            inline: Viewport::new(cx.focus_handle().tab_stop(true)),
            expanded: Viewport::new(cx.focus_handle().tab_stop(true)),
            // `.gp__backdrop`: rgba(2, 6, 11, .88); `<Teleport v-if>` без `<transition>`.
            modal: Modal::new("gp-dialog", cx)
                .scrim(rgba(0x02060be0).into())
                .instant(),
            close_focus: cx.focus_handle().tab_stop(true),
            _signals: signals,
        };
        this.set_inputs(inputs);
        // Экран монитора (`window.screen × devicePixelRatio`): пока main не сказал разрешение игры.
        let display = window.display(cx).map(|display| {
            let size = display.bounds().size;
            let scale = window.scale_factor();
            (
                (f32::from(size.width) * scale).round() as u32,
                (f32::from(size.height) * scale).round() as u32,
            )
        });
        this.controller.mount(cx.reduce_motion(), display);
        this
    }

    pub(super) fn t(&self, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(self.language, key, args)
    }

    /// Раздел без кадра или страница ушла (`active` false).
    pub fn set_active(&mut self, active: bool) {
        if self.inputs.active != active {
            self.inputs.active = active;
            self.controller.set_active(active);
        }
    }

    /// Новые `props` редактора: в контроллер уходит только изменившееся (у оверлея нет своей
    /// проверки на равенство — он перерисовал бы кадр).
    pub fn set_inputs(&mut self, inputs: Inputs) {
        if inputs == self.inputs {
            return;
        }
        let c = &self.controller;
        // Включение — последним: первый кадр после возврата уже по новому разделу и значению
        // (у 0.4.9 `queueRender` лишь ставит rAF, кадр рисуется по свойствам на момент rAF).
        // Иначе кадр прежнего раздела (HUD) встал бы в рамку сцены, пока рисуется сцена.
        if !inputs.active && self.inputs.active {
            c.set_active(false);
        }
        if inputs.hud_mod != self.inputs.hud_mod {
            c.set_hud_mod(inputs.hud_mod);
        }
        if inputs.section != self.inputs.section {
            c.set_section(&inputs.section);
        }
        if inputs.value != self.inputs.value {
            c.set_value(inputs.value.clone());
        }
        if inputs.overlay != self.inputs.overlay
            || inputs.overlay_snapshot != self.inputs.overlay_snapshot
            || inputs.overlay_editing != self.inputs.overlay_editing
        {
            c.set_overlay(
                inputs.overlay.clone(),
                inputs.overlay_snapshot.clone(),
                inputs.overlay_editing,
            );
        }
        if inputs.active && !self.inputs.active {
            c.set_active(true);
        }
        self.inputs = inputs;
    }

    fn signal(&mut self, signal: Signal, cx: &mut Context<Self>) {
        match signal {
            Signal::State => {
                let state = self.controller.state();
                if state.frame_width != self.state.frame_width {
                    // Другая ширина кадра: виды снова показывают его целиком (`watch frameWidth`).
                    let width = f64::from(state.frame_width);
                    self.inline.set_frame(width);
                    self.expanded.set_frame(width);
                }
                self.state = state;
                cx.notify();
            }
            Signal::Frame(frame, scene) => {
                self.pending = Some((frame, scene));
                if !self.converting {
                    self.convert_next(cx);
                }
            }
            Signal::Overlay(value) => cx.emit(PreviewEvent::Overlay(value)),
        }
    }

    /// Последний пришедший кадр — в перевод; готов — следующий, если пришёл.
    fn convert_next(&mut self, cx: &mut Context<Self>) {
        let Some((frame, scene)) = self.pending.take() else {
            self.converting = false;
            return;
        };
        self.converting = true;
        let convert = cx.background_spawn(async move { levels_of(frame) });
        cx.spawn(async move |this, cx| {
            let levels = convert.await;
            this.update(cx, |this, cx| {
                for (i, raster) in this.levels.iter_mut().enumerate() {
                    match levels.get(i) {
                        Some(image) => raster.set_render(image.clone()),
                        None => raster.clear(),
                    }
                }
                this.levels_scene = scene;
                cx.notify();
                this.convert_next(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Картинки уровней для этого кадра окна (прежние уходят из атласа). Кадр другой сцены
    /// (HUD в окне призрака, пока рисуется сцена раздела) не показывается: у 0.4.9 кадр раздела
    /// рисуется в том же rAF, чужого в окне не бывает.
    pub(super) fn level_images(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> [Option<Arc<RenderImage>>; LEVELS] {
        let own = self.levels_scene == self.state.scene;
        for (i, raster) in self.levels.iter_mut().enumerate() {
            self.shown[i] = raster.image(window, cx).filter(|_| own);
        }
        self.shown.clone()
    }

    pub(super) fn viewport(&mut self, which: Which) -> &mut Viewport {
        match which {
            Which::Inline => &mut self.inline,
            Which::Expanded => &mut self.expanded,
        }
    }

    /// «Развернуть» (`openExpanded`): только когда кадр готов.
    pub(super) fn open_expanded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.ready {
            return;
        }
        self.controller.open_expanded();
        let width = f64::from(self.state.frame_width);
        self.expanded.set_frame(width);
        self.expanded.reset();
        self.modal.focus_initial(self.close_focus.clone());
        self.modal.open(window, cx);
        cx.notify();
    }

    pub(super) fn close_expanded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.controller.close_expanded();
        self.modal.close(window, cx);
        cx.notify();
    }

    /// Строки под кадром (`caption`, `screenText`, `sceneLines`, подсказка).
    fn legend(&self) -> Vec<(String, bool)> {
        let s = &self.state;
        let mut lines = Vec::new();
        if !s.caption.is_empty() {
            lines.push((self.t(s.caption, &[]), s.caption_error));
        }
        if let Some((key, size)) = &s.screen {
            lines.push((self.t(key, &[size]), false));
        }
        for line in &s.scene_lines {
            let text = match line {
                SceneLine::Ghost(distance) => {
                    self.t("#L_GS_X_SCENE_GHOST", &[&distance.to_string()])
                }
                SceneLine::Key(key) => self.t(key, &[]),
                SceneLine::SpraySize { units, texels } => {
                    // `Number(units.toFixed(2)).toLocaleString(locale)`.
                    let units = hlsr_ui::intl::format_number(self.language, *units);
                    let texels = texels.to_string();
                    self.t(
                        "#L_GS_X_SCENE_SPRAY_SIZE",
                        &[&units, &units, &texels, &texels],
                    )
                }
            };
            lines.push((text, false));
        }
        lines.push((self.t("#L_GS_X_PREVIEW_HELP", &[]), false));
        lines
    }

    /// Окно во весь лаунчер (`.gp__backdrop`, `.gp__dialog`).
    fn dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Закрытое окно: вид кадра не нужен (`Modal::render` — всё равно: окно уходит в этом кадре).
        if !self.modal.is_open() {
            return self
                .modal
                .render(window, cx, |_, _| {}, || gpui::Empty)
                .into_any_element();
        }
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let viewport = window.viewport_size();
        let (vw, vh) = (
            f32::from(viewport.width) / zoom,
            f32::from(viewport.height) / zoom,
        );
        let aspect = self.state.frame_width as f32 / FRAME_HEIGHT as f32;
        // Ширина — чтобы кадр занял высоту, что осталась от шапки и панели (--gp-chrome 118).
        let width = 1500f32.min(vw - 40.).min((vh - 40. - 118.) * aspect + 26.);
        let note = if self.state.caption.is_empty() {
            self.t("#L_GS_X_PREVIEW_HELP", &[])
        } else {
            self.t(self.state.caption, &[])
        };
        let note_error = !self.state.caption.is_empty() && self.state.caption_error;
        let title = self.t("#L_GS_X_OVERLAY_TITLE", &[]);
        let close = self.t("#L_GS_X_PREVIEW_CLOSE", &[]);
        let view = self.render_viewport(
            Which::Expanded,
            ViewportProps {
                framed: false,
                fill: false,
                expandable: false,
                wrap: false,
            },
            window,
            cx,
        );
        let close_focus = self.close_focus.clone();
        let ring = crate::kit::button::focus_visible(&close_focus, window);
        let closing = cx.entity().downgrade();
        let head = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(14.))
            .pt(u(8.))
            .pr(u(8.))
            .pb(u(8.))
            .pl(u(18.))
            .border_b_1()
            .border_color(theme.color("l-modal-border"))
            .bg(theme.color("l-modal-bar"))
            .text_px(14., 14. * 1.2)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.color("l-text"))
                    .child(title),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_px(12., 12. * 1.2)
                    .text_color(if note_error {
                        rgb(0xefa5a5).into()
                    } else {
                        theme.color("l-text-faint")
                    })
                    .child(note),
            )
            .child(crate::snap::target(
                "gp-close",
                crate::motion::hover("gp-close", crate::motion::Timing::fast(), {
                    let dim = theme.color("l-text-dim");
                    let focus_color = theme.color("accent-focus");
                    let close_focus = close_focus.clone();
                    let label = close.clone();
                    move |t| {
                        div()
                            .id("gp-close-btn")
                            .track_focus(&close_focus)
                            .relative()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(u(34.))
                            .rounded(u(8.))
                            .cursor_pointer()
                            .bg(hsla(0., 0., 1., 0.08 * t))
                            .text_color(crate::motion::mix(dim, hsla(0., 0., 1., 1.), t))
                            .aria_label(label.clone())
                            .tip(label.clone())
                            .child(crate::icons::fa_glyph("fas fa-times", 15.))
                            .when(ring, |el| {
                                el.child(crate::kit::button::outline(zoom, 8., 2., 2., focus_color))
                            })
                            .on_click({
                                let closing = closing.clone();
                                move |_, window, cx| {
                                    closing
                                        .update(cx, |this, cx| this.close_expanded(window, cx))
                                        .ok();
                                }
                            })
                    }
                }),
            ));
        // Окна превью в правилах Liquid Glass нет: без стекла.
        let body = card_plain(&theme, window)
            .w(u(width))
            .max_h_full()
            .min_h_0()
            .overflow_hidden()
            .child(head)
            .child(div().flex_none().p(u(12.)).child(view))
            .into_any_element();
        let on_close = cx.entity().downgrade();
        self.modal
            .render(
                window,
                cx,
                move |window, cx| {
                    on_close
                        .update(cx, |this, cx| this.close_expanded(window, cx))
                        .ok();
                },
                move || {
                    div()
                        .p(u(20.))
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(body)
                },
            )
            .into_any_element()
    }
}

impl Render for GhostingPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let portrait = self.state.portrait;
        let legend = self.legend();
        let view = self.render_viewport(
            Which::Inline,
            ViewportProps {
                framed: true,
                fill: portrait,
                expandable: true,
                wrap: true,
            },
            window,
            cx,
        );
        // `@media (max-height: 760px)`: низкое окно — портрет на всю колонку.
        let low = f32::from(window.viewport_size().height) / zoom <= 760.;
        let lines = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(u(2.))
            .pt(u(8.))
            .px(u(12.))
            .pb(u(9.))
            .border_t_1()
            .border_color(theme.color("l-border"))
            .text_px(12., 12. * 1.45)
            .text_color(theme.color("l-text-faint"))
            .children(legend.into_iter().map(|(text, error)| {
                div()
                    .when(error, |el| el.text_color(rgb(0xefa5a5)))
                    .child(crate::text::pretty(text))
            }));
        let dialog = self.dialog(window, cx);
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .max_h_full()
            .overflow_hidden()
            .border_1()
            .border_color(theme.color("l-border"))
            .rounded(theme.rem("l-radius-sm"))
            .bg(theme.color("l-modal-bar"))
            .text_color(theme.color("l-text"))
            .map(|el| {
                if !portrait {
                    el.w_full()
                } else if low {
                    el.w_full().h_full()
                } else {
                    el.h_full().max_w_full().aspect_ratio(5. / 8.)
                }
            })
            .child(view)
            .child(lines)
            .child(dialog)
    }
}
