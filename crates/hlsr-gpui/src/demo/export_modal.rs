//! Окно «Экспорт клипа» (`DemoExportModal.vue` 0.4.9): итог задания (файл, мета, попытки,
//! предупреждения) или ошибка с быстрыми исправлениями, «Экспорт отменён», карточки bxt-rs и
//! FFmpeg, совет вместо долгого предпросмотра, чипы настроек (пресет с оценкой времени,
//! разрешение, FPS, кодировщик), звук, проверка данных, вид в кадре, интервал, скорость, папка,
//! итог плана, ход задания, «Отмена / Сохранить клип». Логика — [`DemoExport`].
//!
//! Договор со страницей (точка монтирования G11): окно создаётся один раз
//! `DemoExportModal::new(panel, services, cx)` и рисуется страницей всегда. Открыто, пока у
//! страницы `export_open` и есть выбранная запись (`v-if="exportOpen && detail"`); каждое
//! открытие — новое состояние окна (`created()`). Пропсы 0.4.9 окно читает из `panel.state()`,
//! события отдаёт методами страницы: `close_export`, `preview_anyway`, `start_export(settings)`,
//! `cancel_job`, `install_tool`, `cancel_tool`, `set_range`, `open_result`, `export_again`.

use super::panel::DemoPanel;
use super::view_settings::{OnChange, more_link, rows};
use super::{BtnLook, OnClick, btn, btn_icon};
use crate::icons::{fa_glyph, icon};
use crate::kit::button::{focus_handle, focus_visible, outline, white};
use crate::kit::{FieldEvent, Look, TextInput, button, toggle};
use crate::layer::{Dismiss, Modal, TipExt, anchor_probe, card, listen};
use crate::motion::{Timing, hover, mix};
use crate::paint::ring;
use crate::scale::{u, viewport, zoom};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::text::{RUBIK, TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, FocusHandle, FontWeight, Hsla, IntoElement,
    Pixels, Render, Role, SharedString, Subscription, Task, Toggled, WeakEntity, Window, div,
    prelude::*, px, relative,
};
use hlsr_core::js;
use hlsr_core::language::Language;
use hlsr_ui::controllers::demo_export::{DemoExport, Edge, Fix, FixAction, PRESETS};
use hlsr_ui::controllers::demo_panel::{Answer, Job, JobKind};
use hlsr_ui::controllers::demo_tool::{ClipRange, clock, error_text};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// `watch.payload`: план и оценка — через 150 мс после последней правки.
const PLAN_DELAY: Duration = Duration::from_millis(150);
/// Ширина подписи строки (`grid-template-columns: 136px`) с промежутком 12.
const LABEL_W: f32 = 136.;
const INDENT: f32 = 148.;

/// Пропсы окна (`:item`, `:duration`, `:range`, … у `DemoPanel.vue`).
#[derive(Clone)]
struct Props {
    language: Language,
    item: Value,
    item_id: String,
    duration: f64,
    range: Option<ClipRange>,
    ffmpeg: Option<Value>,
    ffmpeg_error: Option<String>,
    bxtrs: Option<Value>,
    bxtrs_error: Option<String>,
    /// Задание экспорта этой записи.
    job: Option<Job>,
    result: Option<Value>,
    job_error: Option<Value>,
    /// Идёт любое задание.
    busy: bool,
    cancelled: bool,
    long_preview: Option<Value>,
    initial: Option<Value>,
}

impl Props {
    fn t(&self, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(self.language, key, args)
    }

    fn translate(&self) -> impl Fn(&str, &[&str]) -> String + use<> {
        let language = self.language;
        move |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args)
    }

    fn ready(status: Option<&Value>) -> bool {
        status.and_then(|status| status.get("state")) == Some(&json!("ready"))
    }
}

pub struct DemoExportModal {
    panel: WeakEntity<DemoPanel>,
    services: Services,
    modal: Modal,
    close: FocusHandle,
    s: DemoExport,
    /// Пропсы прошлого кадра: с ними окно и доигрывает уход (`Rc`: кадр не копирует их заново).
    props: Option<Rc<Props>>,
    /// Окно закрыто тогда: содержимое строится, пока идёт уход.
    closed_at: Option<Instant>,
    /// Номер открытия: ответ каналов прошлого открытия не нужен.
    generation: u64,
    /// Прошлые значения наблюдателей `watch` 0.4.9.
    seen_payload: Option<Value>,
    seen_range: Option<ClipRange>,
    seen_tools: (bool, bool),
    /// Пришли `demo-options`: `watch.payload` срабатывает и при том же значении.
    force_plan: bool,
    plan_timer: Option<Task<()>>,
    /// Поля «Начало» и «Конец» (создаются при первом открытии: полю нужно окно).
    inputs: Option<[Entity<TextInput>; 2]>,
    /// С чем сверены поля: текст, ревизия, подсказка «Конца».
    applied: Option<([String; 2], u64, String)>,
    body: Scroll,
    view_open: bool,
    view_probe: Rc<Cell<Bounds<Pixels>>>,
    /// Панель вида раскрыта: докрутить до неё, когда она будет в кадре.
    reveal_view: bool,
    _subscriptions: Vec<Subscription>,
}

impl DemoExportModal {
    pub fn new(
        panel: WeakEntity<DemoPanel>,
        services: &Services,
        cx: &mut Context<DemoExportModal>,
    ) -> DemoExportModal {
        let close = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("demo-export", cx).on_top();
        modal.focus_initial(close.clone());
        DemoExportModal {
            panel,
            services: services.clone(),
            modal,
            close,
            s: DemoExport::default(),
            props: None,
            closed_at: None,
            generation: 0,
            seen_payload: None,
            seen_range: None,
            seen_tools: (false, false),
            force_plan: false,
            plan_timer: None,
            inputs: None,
            applied: None,
            body: Scroll::new(),
            view_open: false,
            view_probe: Rc::default(),
            reveal_view: false,
            _subscriptions: Vec::new(),
        }
    }

    /// Пропсы из состояния страницы; `None` — окно закрыто или записи нет.
    fn read_props(&self, cx: &App) -> Option<Props> {
        let panel = self.panel.upgrade()?;
        let panel = panel.read(cx);
        let s = panel.state();
        if !s.export_open {
            return None;
        }
        let item = s.item()?.clone();
        let item_id = item.get("id").map(js::to_string).unwrap_or_default();
        let outcome = s.own_outcome();
        let job = s
            .job
            .clone()
            .filter(|job| job.kind == JobKind::Export && job.item_id == item_id);
        let cancelled = s
            .cancelled
            .as_ref()
            .is_some_and(|(id, kind)| *kind == JobKind::Export && *id == item_id);
        let long_preview = s.long_preview.clone().filter(|long| {
            long.get("itemId").map(js::to_string).as_deref() == Some(item_id.as_str())
        });
        Some(Props {
            language: panel.language,
            duration: s.duration(),
            range: s.range,
            ffmpeg: s.ffmpeg.clone(),
            ffmpeg_error: s.ffmpeg_error.clone(),
            bxtrs: s.bxtrs.clone(),
            bxtrs_error: s.bxtrs_error.clone(),
            job,
            result: outcome.and_then(|outcome| outcome.result.clone()),
            job_error: outcome.and_then(|outcome| outcome.error.clone()),
            busy: s.job.is_some(),
            cancelled,
            long_preview,
            initial: s.export_initial.clone(),
            item,
            item_id,
        })
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.read_props(cx) {
            Some(props) if !self.modal.is_open() => {
                self.start(props, window, cx);
                self.modal.open(window, cx);
            }
            None if self.modal.is_open() => {
                self.modal.close(window, cx);
                self.plan_timer = None;
                self.view_open = false;
                self.closed_at = Some(Instant::now());
            }
            _ => {}
        }
        cx.notify();
    }

    /// `created()`: всё заново, `demo-options` и папка вывода.
    fn start(&mut self, props: Props, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        self.s = DemoExport::default();
        self.closed_at = None;
        self.view_open = false;
        self.reveal_view = false;
        self.body = Scroll::new();
        self.applied = None;
        self.force_plan = false;
        self.plan_timer = None;
        self.seen_range = props.range;
        self.seen_tools = (
            Props::ready(props.ffmpeg.as_ref()),
            Props::ready(props.bxtrs.as_ref()),
        );
        self.seen_payload = Some(self.s.payload(props.range));
        self.props = Some(Rc::new(props));
        if self.inputs.is_none() {
            let from = self.time_input(Edge::From, window, cx);
            let to = self.time_input(Edge::To, window, cx);
            self.inputs = Some([from, to]);
        }
        self.load_options(cx);
        self.call("demo-settings-get", Vec::new(), cx, |this, answer, _| {
            this.s.settings_done(&answer)
        });
    }

    /// Поле интервала `.gx-input.dex__time`: `@change` — `editRange`.
    fn time_input(
        &mut self,
        edge: Edge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let input = cx.new(|cx| TextInput::new(window, cx).align_center());
        self._subscriptions.push(cx.subscribe(
            &input,
            move |this, input, event: &FieldEvent, cx| {
                if *event == FieldEvent::Change {
                    let text = input.read(cx).text().to_owned();
                    this.edit_range(edge, &text, cx);
                }
            },
        ));
        input
    }

    /// `ipcRenderer.invoke(…)` с ответом в это открытие окна.
    fn call(
        &self,
        channel: &str,
        args: Vec<Value>,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Answer, &mut Context<Self>) + 'static,
    ) {
        let reply = self.services.invoke(channel, args);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = reply.await.map_err(|_| ());
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    done(this, result, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn panel_do(&self, cx: &mut App, f: impl FnOnce(&mut DemoPanel, &mut Context<DemoPanel>)) {
        self.panel.update(cx, f).ok();
    }

    fn load_options(&mut self, cx: &mut Context<Self>) {
        let Some(props) = self.props.as_ref() else {
            return;
        };
        self.s.options_begin();
        let initial = props.initial.clone();
        let args = vec![json!(props.item_id)];
        self.call("demo-options", args, cx, move |this, answer, cx| {
            let Some(t) = this.props.as_deref().map(Props::translate) else {
                return;
            };
            if this.s.options_done(&answer, initial.as_ref(), &t) {
                this.force_plan = true;
                this.load_plan(cx);
                this.load_estimates(cx);
            }
        });
    }

    fn load_plan(&mut self, cx: &mut Context<Self>) {
        let Some((id, range)) = self.props.as_ref().map(|p| (p.item_id.clone(), p.range)) else {
            return;
        };
        let Some(ticket) = self.s.plan_begin() else {
            return;
        };
        let args = vec![json!(id), self.s.payload(range)];
        self.call("demo-plan", args, cx, move |this, answer, _| {
            this.s.plan_done(ticket, &answer)
        });
    }

    /// Время рендера для «Лучшего качества» и Discord при текущих разрешении, FPS и интервале.
    fn load_estimates(&mut self, cx: &mut Context<Self>) {
        let Some((id, range)) = self.props.as_ref().map(|p| (p.item_id.clone(), p.range)) else {
            return;
        };
        let Some((ticket, variants)) = self.s.estimate_begin(range) else {
            return;
        };
        self.call(
            "demo-estimate",
            vec![json!(id), variants],
            cx,
            move |this, answer, _| this.s.estimate_done(ticket, &answer),
        );
    }

    /// Наблюдатели 0.4.9: FFmpeg или bxt-rs готов — `demo-options` заново; интервал сменили
    /// снаружи — поля заново; настройки изменились — план и оценка через 150 мс.
    fn watch(&mut self, cx: &mut Context<Self>) {
        let Some(props) = self.props.as_ref() else {
            return;
        };
        let tools = (
            Props::ready(props.ffmpeg.as_ref()),
            Props::ready(props.bxtrs.as_ref()),
        );
        let range = props.range;
        let ready = (tools.0 && !self.seen_tools.0) || (tools.1 && !self.seen_tools.1);
        self.seen_tools = tools;
        if range != self.seen_range {
            self.seen_range = range;
            self.s.range_changed();
        }
        if ready {
            self.load_options(cx);
        }
        let payload = self.s.payload(range);
        if std::mem::take(&mut self.force_plan) || self.seen_payload.as_ref() != Some(&payload) {
            self.seen_payload = Some(payload);
            self.plan_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PLAN_DELAY).await;
                this.update(cx, |this, cx| {
                    this.load_plan(cx);
                    this.load_estimates(cx);
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    /// Текст полей интервала — по пропсам и ревизии (`:key="'from' + rangeRev"`).
    fn sync_inputs(&mut self, inert: bool, cx: &mut Context<Self>) {
        let (Some(inputs), Some(props)) = (self.inputs.clone(), self.props.as_ref()) else {
            return;
        };
        let texts = DemoExport::range_text(props.range, props.duration);
        let key = (texts, self.s.range_rev, clock(props.duration));
        if self.applied.as_ref() != Some(&key) {
            for (input, text) in inputs.iter().zip(&key.0) {
                let text = text.clone();
                input.update(cx, |input, cx| input.set_text(text, cx));
            }
            inputs[0].update(cx, |input, cx| input.set_placeholder("0:00.0", cx));
            let placeholder = key.2.clone();
            inputs[1].update(cx, |input, cx| input.set_placeholder(placeholder, cx));
            self.applied = Some(key);
        }
        for (input, invalid) in inputs.iter().zip(self.s.invalid) {
            input.update(cx, |input, cx| {
                input.set_invalid(invalid.is_some(), cx);
                input.set_disabled(inert, cx);
            });
        }
    }

    fn edit_range(&mut self, edge: Edge, text: &str, cx: &mut Context<Self>) {
        let Some(props) = self.props.as_ref().filter(|_| self.modal.is_open()) else {
            return;
        };
        if let Some(next) = self.s.edit_range(edge, text, props.range, props.duration) {
            self.panel_do(cx, |panel, cx| panel.set_range(next, cx));
        }
        cx.notify();
    }

    fn apply_fix(&mut self, fix: &Fix, cx: &mut Context<Self>) {
        if fix.again {
            self.panel_do(cx, |panel, cx| panel.export_again(cx));
        }
        match fix.action {
            FixAction::Height(height) => self.s.pick_height(height),
            FixAction::Range(range) => {
                self.panel_do(cx, |panel, cx| panel.set_range(Some(range), cx))
            }
        }
        cx.notify();
    }

    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        self.s.action_error = None;
        self.call("demo-output-pick", Vec::new(), cx, |this, answer, _| {
            if let Some(t) = this.props.as_deref().map(Props::translate) {
                this.s.pick_folder_done(&answer, &t);
            }
        });
    }

    fn reveal(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .props
            .as_ref()
            .and_then(|props| props.result.as_ref()?.get("id").cloned())
        else {
            return;
        };
        self.s.action_error = None;
        self.call("demo-reveal", vec![id], cx, |this, answer, _| {
            if let Some(t) = this.props.as_deref().map(Props::translate) {
                this.s.reveal_done(&answer, &t);
            }
        });
    }

    fn start_export(&mut self, cx: &mut Context<Self>) {
        let Some(props) = self.props.as_ref() else {
            return;
        };
        let ffmpeg = props.ffmpeg.as_ref();
        if self.s.can_start(props.busy, props.job.is_some(), ffmpeg) {
            let payload = self.s.payload(props.range);
            self.panel_do(cx, |panel, cx| panel.start_export(payload, cx));
        }
    }

    fn toggle_view(&mut self, cx: &mut Context<Self>) {
        self.view_open = !self.view_open;
        if self.view_open {
            self.view_probe.set(Bounds::default());
            self.reveal_view = true;
        }
        cx.notify();
    }

    /// `panel.scrollIntoView({ block: "nearest" })` у раскрытой панели вида.
    fn reveal_view_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.reveal_view {
            return;
        }
        let panel = self.view_probe.get();
        if panel.size.height <= px(0.) {
            crate::motion::monitor_notify(window, cx.entity_id());
            return;
        }
        self.reveal_view = false;
        let view = self.body.handle().bounds();
        let below = panel.bottom() - view.bottom();
        if below > px(0.) {
            self.body
                .scroll_to(self.body.top() + below, false, window, cx);
            cx.notify();
        }
    }

    /// Обработчик щелчка, который зовёт метод окна.
    fn act(
        &self,
        cx: &Context<Self>,
        f: impl Fn(&mut DemoExportModal, &mut Context<DemoExportModal>) + 'static,
    ) -> OnClick {
        let view = cx.weak_entity();
        Rc::new(move |_, _, cx| {
            view.update(cx, |this, cx| {
                f(this, cx);
                cx.notify();
            })
            .ok();
        })
    }
}

impl Render for DemoExportModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let props = self.read_props(cx);
        if props.is_some() != self.modal.is_open() {
            cx.defer_in(window, Self::sync);
        }
        if let Some(props) = props.filter(|_| self.modal.is_open()) {
            self.props = Some(Rc::new(props));
            self.watch(cx);
        }
        let leaving = self
            .closed_at
            .is_some_and(|at| at.elapsed() < Duration::from_secs(1));
        let content = match self.props.clone() {
            Some(props) if self.modal.is_open() || leaving => {
                self.sync_inputs(props.job.is_some(), cx);
                self.reveal_view_panel(window, cx);
                Some(self.window_el(&props, window, cx))
            }
            _ => None,
        };
        let on_close = listen(cx, |this: &mut DemoExportModal, _, cx| {
            this.panel_do(cx, |panel, cx| panel.close_export(cx));
        });
        self.modal
            .render(window, cx, on_close, move || {
                content.unwrap_or_else(|| div().into_any_element())
            })
            .into_any_element()
    }
}

// ---------- вид ----------

/// Подпись строки `.dex__label`: 12 px / 600, разрядка .06em, прописные, `--l-text-dim`.
fn label(text: &str, theme: &Theme) -> impl IntoElement {
    spaced(
        text.to_uppercase(),
        12.,
        20.,
        FontWeight::SEMIBOLD,
        0.06,
        theme.color("l-text-dim"),
    )
}

/// Строка `.dex__row`: подпись 136 px и содержимое; `top` — `.dex__row--top`.
fn row(text: &str, content: impl IntoElement, top: bool, theme: &Theme) -> Div {
    div()
        .flex()
        .gap(u(12.))
        .map(|el| {
            if top {
                el.items_start()
            } else {
                el.items_center()
            }
        })
        .child(
            div()
                .flex_none()
                .w(u(LABEL_W))
                .when(top, |el| el.pt(u(2.)))
                .child(label(text, theme)),
        )
        .child(div().flex_1().min_w_0().flex().child(content))
}

/// `.dex__line`: в строку с переносом, 8 × 10.
fn line() -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(u(10.))
        .gap_y(u(8.))
}

/// `.dex__seg`: чипы на подложке `--l-chip-bg` с рамкой.
fn seg(chips: Vec<AnyElement>, theme: &Theme, zoom: f32) -> Div {
    div()
        .children(chips)
        .flex()
        .flex_wrap()
        .gap(u(3.))
        .p(u(3.))
        .rounded(theme.rem("l-radius-sm"))
        .bg(theme.color("l-chip-bg"))
        .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
}

/// `.dex__subhint`: под строкой, с отступом подписи; `error` — `--l-level-error`.
fn subhint(text: String, error: bool, theme: &Theme) -> Div {
    div()
        .mt(u(-6.))
        .ml(u(INDENT))
        .text_px(12., 12. * 1.4)
        .text_color(theme.color(if error {
            "l-level-error"
        } else {
            "l-text-faint"
        }))
        .child(text)
}

/// `.dex__hint`: 12 px `--l-text-faint`.
fn hint(text: String, theme: &Theme) -> Div {
    div()
        .text_px(12., 20.)
        .text_color(theme.color("l-text-faint"))
        .child(text)
}

/// `.dex__warn`: значок 11 px и текст `--l-level-warning` (или ошибки).
fn warn(glyph: &str, text: String, error: bool, theme: &Theme) -> Div {
    div()
        .flex()
        .items_start()
        .gap(u(8.))
        .text_px(12.5, 12.5 * 1.4)
        .text_color(theme.color(if error {
            "l-level-error"
        } else {
            "l-level-warning"
        }))
        .child(fa_glyph(glyph, 11.).mt(u(2.)))
        .child(div().min_w_0().child(text))
}

/// `.dex__notice`: строка на `--l-note-bg`; значок `--accent`; `error` — текст ошибкой.
fn notice(glyph: Option<&str>, text: String, error: bool, theme: &Theme, zoom: f32) -> Div {
    div()
        .flex()
        .items_start()
        .gap(u(10.))
        .py(u(10.))
        .px(u(12.))
        .rounded(theme.rem("l-radius-sm"))
        .bg(theme.color("l-note-bg"))
        .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
        .text_px(13., 13. * 1.45)
        .text_color(theme.color(if error { "l-level-error" } else { "l-text-dim" }))
        .children(glyph.map(|glyph| {
            fa_glyph(glyph, 13.)
                .mt(u(2.))
                .text_color(theme.color("accent"))
        }))
        .child(div().min_w_0().child(text))
}

/// Плашка `.dex__job` / `.dex__outcome`: `--l-modal-bar` с рамкой.
fn bar_box(theme: &Theme, zoom: f32) -> Div {
    div()
        .rounded(theme.rem("l-radius-sm"))
        .bg(theme.color("l-modal-bar"))
        .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
}

/// Кнопки быстрых исправлений `.dex__fixes` (`.gx-btn`).
fn fix_buttons(fixes: Vec<Fix>, cx: &mut Context<DemoExportModal>) -> Option<Div> {
    if fixes.is_empty() {
        return None;
    }
    let view = cx.weak_entity();
    Some(
        div()
            .flex()
            .flex_wrap()
            .gap(u(8.))
            .children(fixes.into_iter().map(|fix| {
                let view = view.clone();
                let name = format!("dex-fix-{}", fix.id);
                let el = button(SharedString::from(name.clone()), Look::Gx)
                    .child(fix.label.clone())
                    .on_click(move |_, _, cx| {
                        view.update(cx, |this, cx| this.apply_fix(&fix, cx)).ok();
                    });
                crate::snap::target(name, el)
            })),
    )
}

/// Чип `.dex__chip`: 30 px, 12,5 / 600; выбранный — `--accent-soft` с рамкой `--accent-border`;
/// недоступный — .35; `num` — `.dex__chip--num`. `on_click` нет — недоступен (или форма
/// `inert`).
#[allow(clippy::too_many_arguments)]
fn chip(
    id: SharedString,
    active: bool,
    disabled: bool,
    num: bool,
    title: Option<String>,
    children: Vec<AnyElement>,
    window: &mut Window,
    cx: &mut App,
    on_click: Option<OnClick>,
) -> AnyElement {
    let zoom = zoom(window);
    let focus = focus_handle(&id.clone().into(), window, cx);
    let ring_on = focus_visible(&focus, window);
    let theme = cx.global::<Theme>();
    let (dim, text, hot, soft, border, accent) = (
        theme.color("l-text-dim"),
        theme.color("l-text"),
        theme.color("l-chip-hover"),
        theme.color("accent-soft"),
        theme.color("accent-border"),
        theme.color("l-accent"),
    );
    let on_click = on_click.filter(|_| !disabled);
    let inner = id.clone();
    hover(id, Timing::fast(), move |t| {
        let t = if disabled || active { 0. } else { t };
        div()
            .id(inner)
            .role(Role::RadioButton)
            .button_text()
            .aria_toggled(if active {
                Toggled::True
            } else {
                Toggled::False
            })
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(7.))
            .h(u(30.))
            .map(|el| {
                if num {
                    el.min_w(u(46.)).justify_center().px(u(8.)).tnum()
                } else {
                    el.px(u(12.))
                }
            })
            .rounded(u(7.))
            .text_px(12.5, 20.)
            .font_weight(FontWeight::SEMIBOLD)
            .whitespace_nowrap()
            .map(|el| {
                if active {
                    el.bg(soft)
                        .shadow(vec![ring(zoom, 1., border)])
                        .text_color(text)
                } else {
                    el.bg(mix(gpui::transparent_black(), hot, t))
                        .text_color(mix(dim, text, t))
                }
            })
            .children(children)
            .when(disabled, |el| el.opacity(0.35))
            .when_some(title, |el, title| el.tip(title))
            .when_some(on_click, |el, on_click| {
                el.track_focus(&focus)
                    .cursor_pointer()
                    .on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .when(ring_on, |el| el.child(outline(zoom, 7., 1., 2., accent)))
    })
    .into_any_element()
}

/// Кнопка `.dex__btn` (`.dmp__btn` страницы с полями 18 и кеглем 13,5).
#[allow(clippy::too_many_arguments)]
fn dex_btn(
    id: &'static str,
    primary: bool,
    disabled: bool,
    title: Option<String>,
    glyph: Option<&str>,
    text: String,
    window: &mut Window,
    cx: &mut App,
    on_click: OnClick,
) -> AnyElement {
    let theme = cx.global::<Theme>();
    let color = theme.color(if primary {
        "accent-contrast-text"
    } else {
        "l-text"
    });
    let mut children: Vec<AnyElement> = glyph.map(|g| btn_icon(g, color)).into_iter().collect();
    children.push(div().text_size(u(13.5)).child(text).into_any_element());
    let look = BtnLook {
        primary,
        height: 40.,
        px: 18.,
        push: true,
    };
    crate::snap::target(
        id,
        btn(id, look, disabled, title, children, window, cx, on_click),
    )
}

impl DemoExportModal {
    fn window_el(&mut self, p: &Props, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let (vw, vh) = viewport(window);
        let head = self.head(p, &theme, window);
        let body = self.body_el(p, &theme, window, cx);
        let actions = self.actions(p, &theme, window, cx);
        card(&theme, window)
            .id("dex-window")
            .role(Role::Dialog)
            .aria_label(SharedString::from(p.t("#L_DEMO_EX_TITLE", &[])))
            .w(u((vw - 80.).min(680.)))
            .max_h(u(vh - 60.))
            .overflow_hidden()
            .pt(u(22.))
            .px(u(24.))
            .pb(u(20.))
            .gap(u(14.))
            .font_family(RUBIK)
            .text_size(u(14.))
            .line_height(u(20.))
            .text_color(theme.color("l-text"))
            .child(head)
            .child(body)
            .child(actions)
            .into_any_element()
    }

    fn head(&self, p: &Props, theme: &Theme, window: &mut Window) -> AnyElement {
        let zoom = zoom(window);
        let name = p.item.get("name").map(js::to_string).unwrap_or_default();
        let label = p.t("#L_CLOSE", &[]);
        let focus = self.close.clone();
        let ring_on = focus_visible(&focus, window);
        let (dim, text, accent) = (
            theme.color("l-text-dim"),
            theme.color("l-text"),
            theme.color("l-accent"),
        );
        let radius = theme.len("l-radius-sm");
        let on_click = super::act(&self.panel, |panel, _, cx| panel.close_export(cx));
        let close = hover("dex-close", Timing::fast(), move |t| {
            div()
                .id("dex-close-btn")
                .role(Role::Button)
                .button_text()
                .aria_label(SharedString::from(label.clone()))
                .tip(label)
                .track_focus(&focus)
                .relative()
                .flex_none()
                .size(u(32.))
                .mt(u(-4.))
                .mr(u(-6.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(u(radius))
                .bg(white(0.08 * t))
                .text_color(mix(dim, text, t))
                .cursor_pointer()
                .child(fa_glyph("fas fa-times", 13.33))
                .on_click(move |event, window, cx| on_click(event, window, cx))
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 0., 2., accent))
                })
        });
        div()
            .flex_none()
            .flex()
            .items_start()
            .gap(u(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(u(4.))
                    .child(spaced(
                        p.t("#L_DEMO_EX_TITLE", &[]).to_uppercase(),
                        15.,
                        20.,
                        FontWeight::SEMIBOLD,
                        0.09,
                        text,
                    ))
                    .child(
                        div()
                            .id("dex-name")
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_px(13., 20.)
                            .text_color(dim)
                            .tip(name.clone())
                            .child(name),
                    ),
            )
            .child(crate::snap::target("dex-close", close))
            .into_any_element()
    }

    fn body_el(
        &mut self,
        p: &Props,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = zoom(window);
        let t = p.translate();
        let mut col = div().flex().flex_col().gap(u(14.)).px(u(24.));
        if let Some(result) = &p.result {
            col = col.child(self.outcome(p, Ok(result), theme, zoom, cx));
        } else if let Some(error) = &p.job_error {
            col = col.child(self.outcome(p, Err(error), theme, zoom, cx));
        } else {
            if p.cancelled && p.job.is_none() {
                col = col.child(notice(
                    Some("fas fa-ban"),
                    p.t("#L_DEMO_EXPORT_CANCELLED", &[]),
                    false,
                    theme,
                    zoom,
                ));
            }
            // Рендер HL1 ждёт bxt-rs: вместо строки «не готово» — карточка скачивания.
            if self.s.bxtrs_needed(p.bxtrs.as_ref()) {
                let status = p.bxtrs.clone().unwrap_or_default();
                col = col.child(
                    super::ffmpeg_card::card(
                        true,
                        status,
                        p.bxtrs_error.clone(),
                        p.language,
                        &self.panel,
                        window,
                        cx,
                    )
                    .max_w(relative(1.)),
                );
            } else if self.s.render_blocked() {
                let code = self
                    .s
                    .options
                    .as_ref()
                    .and_then(|options| options.pointer("/render/code"))
                    .map(js::to_string)
                    .unwrap_or_default();
                col = col.child(notice(
                    Some("fas fa-info-circle"),
                    error_text(&json!({ "code": code }), &t),
                    false,
                    theme,
                    zoom,
                ));
            }
            if let Some(ffmpeg) = p.ffmpeg.clone().filter(|f| !Props::ready(Some(f))) {
                col = col.child(
                    super::ffmpeg_card::card(
                        false,
                        ffmpeg,
                        p.ffmpeg_error.clone(),
                        p.language,
                        &self.panel,
                        window,
                        cx,
                    )
                    .max_w(relative(1.)),
                );
            }
            if self.s.loading {
                let dim = theme.color("l-text-dim");
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(10.))
                        .py(u(24.))
                        .text_px(13., 20.)
                        .text_color(dim)
                        .child(icon("fas fa-spinner fa-spin", 13., dim))
                        .child(p.t("#L_DEMO_EX_LOADING", &[])),
                );
            } else if let Some(error) = self.s.options_error.clone() {
                col = col.child(notice(None, error, true, theme, zoom));
            } else if self.s.options.is_some() {
                col = col.child(self.form(p, theme, window, cx));
            }
            if let Some(job) = &p.job {
                col = col.child(job_box(job, &t, theme, zoom));
            }
        }
        if let Some(error) = self.s.action_error.clone() {
            col = col.child(notice(None, error, true, theme, zoom));
        }
        let outcome = p.result.is_some() || p.job_error.is_some();
        if self.s.steam_note(outcome) {
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .text_px(12., 12. * 1.4)
                    .text_color(theme.color("l-text-faint"))
                    .child(fa_glyph("fab fa-steam", 14.))
                    .child(p.t("#L_DEMO_STEAM_NOTE", &[])),
            );
        }
        // Тело листается, шапка и кнопки внизу всегда на виду; полоса — у края окна.
        ScrollArea::new("dex-body", &self.body)
            .bar(Bar::Fields)
            .contain()
            .flex_auto()
            .min_h_0()
            .mx(u(-24.))
            .child(col)
            .into_any_element()
    }

    /// Итог задания: «Клип готов» (файл, мета, попытки, предупреждения) или «Клип не сохранился»
    /// с текстом ошибки и исправлениями.
    fn outcome(
        &self,
        p: &Props,
        outcome: Result<&Value, &Value>,
        theme: &Theme,
        zoom: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = p.translate();
        let (level, glyph) = match outcome {
            Ok(_) => ("l-level-ok", "fas fa-check"),
            Err(_) => ("l-level-error", "fas fa-exclamation"),
        };
        let color = theme.color(level);
        let dim = theme.color("l-text-dim");
        let meta = |text: String| div().text_px(12.5, 12.5 * 1.45).text_color(dim).child(text);
        let mut body = div().flex_1().min_w_0().flex().flex_col().gap(u(4.));
        match outcome {
            Ok(result) => {
                let get = |name: &str| result.get(name).map(js::to_string).unwrap_or_default();
                body = body
                    .child(
                        div()
                            .text_px(15., 20.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(p.t("#L_DEMO_EX_DONE", &[])),
                    )
                    .child(
                        div()
                            .id("dex-outcome-file")
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_px(13., 20.)
                            .text_color(theme.color("l-text"))
                            .tip(get("file"))
                            .child(get("fileName")),
                    )
                    .child(meta(DemoExport::result_meta(result, &t)));
                let attempts = js::value::to_number(result.get("attempts"));
                if attempts > 1. {
                    let attempts = js::to_string(&result["attempts"]);
                    body = body.child(meta(p.t("#L_DEMO_EX_ATTEMPTS", &[&attempts])));
                }
                body = body.children(
                    DemoExport::result_warnings(result)
                        .into_iter()
                        .map(|key| warn("fas fa-exclamation-triangle", t(key, &[]), false, theme)),
                );
            }
            Err(error) => {
                let fixes = self.s.fixes(Some(error), p.range, p.duration, &t);
                body = body
                    .child(
                        div()
                            .text_px(15., 20.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(p.t("#L_DEMO_EX_FAILED", &[])),
                    )
                    .child(meta(error_text(error, &t)))
                    .children(fix_buttons(fixes, cx));
            }
        }
        bar_box(theme, zoom)
            .flex()
            .gap(u(14.))
            .py(u(14.))
            .px(u(16.))
            .child(
                div()
                    .flex_none()
                    .size(u(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(color.opacity(0.18))
                    .text_color(color)
                    .child(fa_glyph(glyph, 15.)),
            )
            .child(body)
            .into_any_element()
    }

    /// Форма настроек; во время задания — `inert` (бледнее, без нажатий).
    fn form(
        &mut self,
        p: &Props,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = zoom(window);
        let t = p.translate();
        let inert = p.job.is_some();
        let s = &self.s;
        let mut form = div().relative().flex().flex_col().gap(u(12.));

        // Открыто вместо долгого предпросмотра: сколько он займёт и совет сразу отрендерить.
        if let Some(long) = p.long_preview.as_ref().filter(|_| !inert) {
            form = form.child(long_box(long, &t, theme, zoom));
        }

        // Пресет с оценкой времени.
        let mut presets = Vec::new();
        for (id, key, glyph) in PRESETS {
            let active = s.settings.preset == id;
            let accent = theme.color("accent");
            let mut children = vec![
                fa_glyph(glyph, 12.)
                    .when(active, |el| el.text_color(accent))
                    .into_any_element(),
                t(key, &[]).into_any_element(),
            ];
            if let Some(estimate) = s.estimate_of(id) {
                let faint = theme.color("l-text-faint");
                let color = if active {
                    theme.color("l-text").opacity(0.75)
                } else {
                    faint
                };
                children.push(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .tnum()
                        .text_color(color)
                        .child(DemoExport::estimate_label(estimate, &t))
                        .into_any_element(),
                );
            }
            let on_click = (!inert).then(|| {
                self.act(cx, move |this, _| {
                    this.s.settings.preset = id.to_owned();
                })
            });
            let name = format!("dex-preset-{id}");
            presets.push(crate::snap::target(
                name.clone(),
                chip(
                    name.into(),
                    active,
                    false,
                    false,
                    None,
                    children,
                    window,
                    cx,
                    on_click,
                ),
            ));
        }
        form = form.child(row(
            &t("#L_DEMO_EX_PRESET", &[]),
            seg(presets, theme, zoom),
            false,
            theme,
        ));

        // Разрешение и FPS.
        let resolutions: Vec<AnyElement> = s
            .resolution_choices()
            .into_iter()
            .map(|res| {
                let height = res.height;
                let on_click =
                    (!inert).then(|| self.act(cx, move |this, _| this.s.pick_height(height)));
                let name = format!("dex-res-{}", res.label);
                crate::snap::target(
                    name.clone(),
                    chip(
                        name.into(),
                        s.settings.height == height,
                        !res.allowed,
                        false,
                        None,
                        vec![res.label.into_any_element()],
                        window,
                        cx,
                        on_click,
                    ),
                )
            })
            .collect();
        let rates: Vec<AnyElement> = s
            .fps_choices()
            .into_iter()
            .map(|(fps, allowed)| {
                let on_click =
                    (!inert).then(|| self.act(cx, move |this, _| this.s.settings.fps = fps));
                let text = format!("{} FPS", js::number::number_to_string(fps));
                let name = format!("dex-fps-{fps}");
                crate::snap::target(
                    name.clone(),
                    chip(
                        name.into(),
                        s.settings.fps == fps,
                        !allowed,
                        false,
                        None,
                        vec![text.into_any_element()],
                        window,
                        cx,
                        on_click,
                    ),
                )
            })
            .collect();
        form = form.child(row(
            &t("#L_DEMO_EX_VIDEO", &[]),
            line()
                .child(seg(resolutions, theme, zoom))
                .child(seg(rates, theme, zoom)),
            false,
            theme,
        ));
        let max_height = js::value::to_number(
            s.options
                .as_ref()
                .and_then(|options| options.pointer("/capabilities/maxHeight")),
        );
        if max_height < 2160. {
            form = form.child(subhint(t("#L_DEMO_EX_RES_LIMIT", &[]), false, theme));
        }

        // Кодировщик.
        let modes: Vec<AnyElement> = s
            .encoder_modes()
            .into_iter()
            .map(|(id, key, available)| {
                let on_click =
                    (!inert).then(|| self.act(cx, move |this, _| this.s.pick_encoder(id)));
                let name = format!("dex-enc-{id}");
                crate::snap::target(
                    name.clone(),
                    chip(
                        name.into(),
                        s.settings.encoder == id,
                        !available,
                        false,
                        (!available).then(|| t("#L_DEMO_ERR_NO_GPU", &[])),
                        vec![t(key, &[]).into_any_element()],
                        window,
                        cx,
                        on_click,
                    ),
                )
            })
            .collect();
        let encoder = s.encoder_label();
        form = form.child(row(
            &t("#L_DEMO_EX_ENCODER", &[]),
            line()
                .child(seg(modes, theme, zoom))
                .when(!encoder.is_empty(), |el| {
                    el.child(hint(t("#L_DEMO_EX_ENC_WILL", &[&encoder]), theme))
                }),
            false,
            theme,
        ));

        // Звук.
        let audio = s.audio_available();
        let weak = cx.weak_entity();
        let sound = toggle("dex-audio", s.settings.audio && audio)
            .disabled(!audio)
            .label(t("#L_DEMO_EX_SOUND", &[]))
            .when(!inert, |el| {
                el.on_change(move |on, _, cx| {
                    weak.update(cx, |this, cx| {
                        this.s.settings.audio = *on;
                        cx.notify();
                    })
                    .ok();
                })
            });
        form = form.child(row(
            &t("#L_DEMO_EX_SOUND", &[]),
            line()
                .child(crate::snap::target("dex-audio", sound))
                .when(!audio, |el| {
                    el.child(hint(t("#L_DEMO_EX_AUDIO_NA", &[]), theme))
                }),
            false,
            theme,
        ));

        // Надпись проверки данных демки: только если адаптер её умеет (Ghosting).
        if s.integrity_available() {
            let weak = cx.weak_entity();
            let integrity = toggle("dex-integrity", s.settings.integrity)
                .label(t("#L_DEMO_EX_INTEGRITY", &[]))
                .when(!inert, |el| {
                    el.on_change(move |on, _, cx| {
                        weak.update(cx, |this, cx| {
                            this.s.settings.integrity = *on;
                            cx.notify();
                        })
                        .ok();
                    })
                });
            form = form
                .child(row(
                    "GH DEMO DATA",
                    line().child(crate::snap::target("dex-integrity", integrity)),
                    false,
                    theme,
                ))
                .child(subhint(t("#L_DEMO_EX_INTEGRITY_HINT", &[]), false, theme));
        }

        // Как у предпросмотра: KeyHUD, HUD игры, конфиг и HUD игрока — где игра их умеет.
        form = form.child(self.view_el(p, inert, theme, zoom, cx));

        // Интервал.
        if let Some([from, to]) = self.inputs.clone() {
            let reset = button("dex-range-reset", Look::Gx)
                .disabled(p.range.is_none())
                .child(p.t("#L_DEMO_RANGE_RESET", &[]))
                .on_click({
                    let panel = self.panel.clone();
                    move |_, _, cx| {
                        panel.update(cx, |panel, cx| panel.set_range(None, cx)).ok();
                    }
                });
            form = form.child(row(
                &t("#L_DEMO_EX_RANGE", &[]),
                line()
                    .child(crate::snap::target(
                        "dex-range-from",
                        div().w(u(104.)).child(from),
                    ))
                    .child(
                        fa_glyph("fas fa-long-arrow-alt-right", 13.)
                            .text_color(theme.color("l-text-faint")),
                    )
                    .child(crate::snap::target(
                        "dex-range-to",
                        div().w(u(104.)).child(to),
                    ))
                    .child(crate::snap::target("dex-range-reset", reset)),
                false,
                theme,
            ));
        }
        if let Some(problem) = self.s.range_problem() {
            form = form.child(subhint(t(problem, &[&clock(p.duration)]), true, theme));
        }

        // Скорость клипа.
        let s = &self.s;
        let speeds: Vec<AnyElement> = s
            .speeds()
            .into_iter()
            .map(|speed| {
                let on_click =
                    (!inert).then(|| self.act(cx, move |this, _| this.s.settings.speed = speed));
                let text =
                    js::number::number_to_string(speed).replace('.', &t("#L_DECIMAL", &[])) + "×";
                let name = format!("dex-speed-{speed}");
                crate::snap::target(
                    name.clone(),
                    chip(
                        name.into(),
                        s.settings.speed == speed,
                        false,
                        true,
                        None,
                        vec![text.into_any_element()],
                        window,
                        cx,
                        on_click,
                    ),
                )
            })
            .collect();
        form = form
            .child(row(
                &t("#L_DEMO_EX_SPEED", &[]),
                seg(speeds, theme, zoom)
                    .id("dex-speeds")
                    .tip(t("#L_DEMO_EX_SPEED_HINT", &[])),
                false,
                theme,
            ))
            .child(subhint(t("#L_DEMO_EX_SPEED_HINT", &[]), false, theme));

        // Папка вывода: длинный путь режется слева — конец пути (сама папка) важнее начала.
        let folder = button("dex-folder", Look::Gx)
            .child(fa_glyph("fas fa-folder-open", 12.))
            .child(p.t("#L_DEMO_EX_FOLDER_CHANGE", &[]))
            .on_click({
                let view = cx.weak_entity();
                move |_, _, cx| {
                    view.update(cx, |this, cx| this.pick_folder(cx)).ok();
                }
            });
        let dir = s.output_dir.clone();
        form = form.child(row(
            &t("#L_DEMO_EX_FOLDER", &[]),
            line()
                .child(
                    div()
                        .id("dex-path")
                        .flex_1()
                        .flex_basis(u(200.))
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_start()
                        .text_px(12.5, 20.)
                        .text_color(theme.color("l-text-dim"))
                        .tip(dir.clone())
                        .child(dir),
                )
                .child(crate::snap::target("dex-folder", folder)),
            false,
            theme,
        ));

        // Итог плана.
        let current = s.estimate_of(&s.settings.preset).cloned();
        if s.plan.is_some() || s.plan_error.is_some() || current.is_some() {
            let mut estimate = div().flex_1().min_w_0().flex().flex_col().gap(u(6.));
            if let Some(text) = s.estimate_text(&t) {
                estimate = estimate.child(
                    div()
                        .text_px(13.5, 20.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .tnum()
                        .text_color(theme.color("l-text"))
                        .child(text),
                );
            }
            if let Some(current) = current {
                let time = DemoExport::estimate_label(&current, &t);
                estimate = estimate.child(hint(t("#L_DEMO_EST_TIME", &[&time]), theme));
            }
            if let Some(text) = s.bitrate_text(&t) {
                estimate = estimate.child(hint(text, theme));
            }
            estimate = estimate.children(
                s.plan_warnings()
                    .into_iter()
                    .map(|key| warn("fas fa-exclamation-triangle", t(key, &[]), false, theme)),
            );
            if let Some(error) = &s.plan_error {
                estimate = estimate.child(warn(
                    "fas fa-exclamation-circle",
                    error_text(error, &t),
                    true,
                    theme,
                ));
            }
            let fixes = s.fixes(None, p.range, p.duration, &t);
            estimate = estimate.children(fix_buttons(fixes, cx));
            form = form.child(row(&t("#L_DEMO_EX_ESTIMATE", &[]), estimate, true, theme));
        }

        form.when(inert, |el| {
            el.opacity(0.55).child(
                div()
                    .id("dex-inert")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude(),
            )
        })
        .into_any_element()
    }

    /// «Дополнительные настройки» в потоке формы (`DemoViewSettings` без `popover`).
    fn view_el(
        &self,
        p: &Props,
        inert: bool,
        theme: &Theme,
        zoom: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = cx.weak_entity();
        let link = more_link(
            "dex-more",
            p.t("#L_DEMO_MORE", &[]),
            self.view_open,
            None,
            Rc::new(move |_, _, cx| {
                view.update(cx, |this, cx| this.toggle_view(cx)).ok();
            }),
        );
        let panel = self.view_open.then(|| {
            let on_change = (!inert).then(|| {
                let view = cx.weak_entity();
                Rc::new(
                    move |key: &'static str, on: bool, _: &mut Window, cx: &mut App| {
                        view.update(cx, |this, cx| {
                            this.s.set_view(key, on);
                            cx.notify();
                        })
                        .ok();
                    },
                ) as OnChange
            });
            let hl1 = p.item.get("game") == Some(&json!("hl1"));
            let s = &self.s;
            div()
                .relative()
                .w_full()
                .max_w(u(440.))
                .flex()
                .flex_col()
                .gap(u(10.))
                .py(u(12.))
                .px(u(14.))
                .rounded(theme.rem("l-radius-sm"))
                .bg(theme.color("l-modal-bg"))
                .shadow(vec![ring(zoom, 1., theme.color("l-modal-border"))])
                .children(rows(
                    "dex-view",
                    p.language,
                    hl1,
                    |key| s.view_available(key),
                    |key| s.view(key),
                    on_change,
                    theme,
                ))
                .child(anchor_probe(self.view_probe.clone()))
        });
        div()
            .id("dex-view")
            .ml(u(INDENT))
            .flex()
            .flex_col()
            .items_start()
            .gap(u(10.))
            // Esc внутри панели закрывает её, а не окно (`@keydown.esc` у `.dvs`).
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| {
                if this.view_open {
                    this.view_open = false;
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
            .child(crate::snap::target("dex-more", link))
            .children(panel)
            .into_any_element()
    }

    /// Кнопки внизу: по итогу, по ошибке или «Отмена / Сохранить клип».
    fn actions(
        &self,
        p: &Props,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spacer = || div().flex_1().into_any_element();
        let panel = &self.panel;
        let mut buttons: Vec<AnyElement> = Vec::new();
        if let Some(result) = p.result.clone() {
            buttons.push(dex_btn(
                "dex-again",
                false,
                false,
                None,
                Some("fas fa-redo"),
                p.t("#L_DEMO_EX_AGAIN", &[]),
                window,
                cx,
                super::act(panel, |panel, _, cx| panel.export_again(cx)),
            ));
            buttons.push(spacer());
            let reveal = self.act(cx, |this, cx| this.reveal(cx));
            buttons.push(dex_btn(
                "dex-reveal",
                false,
                false,
                None,
                Some("fas fa-folder-open"),
                p.t("#L_DEMO_EX_REVEAL", &[]),
                window,
                cx,
                reveal,
            ));
            buttons.push(dex_btn(
                "dex-open",
                true,
                false,
                None,
                Some("fas fa-play"),
                p.t("#L_DEMO_EX_OPEN_PLAYER", &[]),
                window,
                cx,
                super::act(panel, move |panel, _, cx| {
                    panel.open_result(result.clone(), cx)
                }),
            ));
        } else if p.job_error.is_some() {
            buttons.push(spacer());
            buttons.push(dex_btn(
                "dex-cancel",
                false,
                false,
                None,
                None,
                p.t("#L_CLOSE", &[]),
                window,
                cx,
                super::act(panel, |panel, _, cx| panel.close_export(cx)),
            ));
            buttons.push(dex_btn(
                "dex-retry",
                true,
                false,
                None,
                Some("fas fa-redo"),
                p.t("#L_DEMO_EX_RETRY", &[]),
                window,
                cx,
                super::act(panel, |panel, _, cx| panel.export_again(cx)),
            ));
        } else {
            let job = p.job.is_some();
            buttons.push(spacer());
            buttons.push(dex_btn(
                "dex-cancel",
                false,
                false,
                None,
                None,
                p.t(if job { "#L_CLOSE" } else { "#L_CANCEL" }, &[]),
                window,
                cx,
                super::act(panel, |panel, _, cx| panel.close_export(cx)),
            ));
            if job {
                buttons.push(dex_btn(
                    "dex-cancel-job",
                    false,
                    false,
                    None,
                    Some("fas fa-stop"),
                    p.t("#L_DEMO_JOB_CANCEL", &[]),
                    window,
                    cx,
                    super::act(panel, |panel, _, cx| panel.cancel_job(cx)),
                ));
            } else {
                if p.long_preview.is_some() {
                    buttons.push(dex_btn(
                        "dex-preview-anyway",
                        false,
                        false,
                        None,
                        Some("fas fa-play"),
                        p.t("#L_DEMO_LONG_PREVIEW", &[]),
                        window,
                        cx,
                        super::act(panel, |panel, _, cx| panel.preview_anyway(cx)),
                    ));
                }
                let t = p.translate();
                let ffmpeg = p.ffmpeg.as_ref();
                let can = self.s.can_start(p.busy, job, ffmpeg);
                let blocker = self.s.start_blocker(p.busy, job, ffmpeg, &t);
                let key = if p.long_preview.is_some() {
                    "#L_DEMO_LONG_RENDER"
                } else {
                    "#L_DEMO_EX_START"
                };
                let start = self.act(cx, |this, cx| this.start_export(cx));
                buttons.push(dex_btn(
                    "dex-start",
                    true,
                    !can,
                    blocker,
                    Some("fas fa-file-video"),
                    p.t(key, &[]),
                    window,
                    cx,
                    start,
                ));
            }
        }
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(u(10.))
            .pt(u(14.))
            .border_t_1()
            .border_color(theme.color("l-border"))
            .children(buttons)
            .into_any_element()
    }
}

/// Ход задания `.dex__job`: фаза и сегмент, процент, полоса 6 px.
fn job_box(job: &Job, t: &dyn Fn(&str, &[&str]) -> String, theme: &Theme, zoom: f32) -> Div {
    let percent = if job.percent.is_nan() {
        0.
    } else {
        job.percent
    };
    bar_box(theme, zoom)
        .flex()
        .flex_col()
        .gap(u(8.))
        .py(u(12.))
        .px(u(14.))
        .child(
            div()
                .flex()
                .justify_between()
                .gap(u(12.))
                .text_px(13., 20.)
                .child(
                    div()
                        .text_color(theme.color("l-text"))
                        .child(DemoExport::job_text(job, t)),
                )
                .child(
                    div()
                        .tnum()
                        .text_color(theme.color("l-text-dim"))
                        .child(format!(
                            "{}%",
                            js::number::number_to_string(percent.round())
                        )),
                ),
        )
        .child(
            div()
                .h(u(6.))
                .rounded(u(3.))
                .bg(white(0.1))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .rounded(u(3.))
                        .w(relative((percent as f32 / 100.).clamp(0., 1.)))
                        .bg(theme.color("accent")),
                ),
        )
}

/// Совет вместо долгого предпросмотра `.dex__long`.
fn long_box(long: &Value, t: &dyn Fn(&str, &[&str]) -> String, theme: &Theme, zoom: f32) -> Div {
    let accent: Hsla = theme.color("accent");
    let seconds = js::value::to_number(long.get("seconds"));
    let range = clock(js::value::to_number(long.get("range")));
    let segments = js::value::to_number(long.get("segments"));
    let text = if segments > 1. {
        t(
            "#L_DEMO_LONG_TEXT",
            &[&range, &js::number::number_to_string(segments)],
        )
    } else {
        t("#L_DEMO_LONG_TEXT_ONE", &[&range])
    };
    let duration = hlsr_ui::controllers::demo_tool::duration_text(seconds).text(t);
    div()
        .flex()
        .items_start()
        .gap(u(12.))
        .py(u(12.))
        .px(u(14.))
        .rounded(theme.rem("l-radius-sm"))
        .bg(accent.opacity(0.12))
        .shadow(vec![ring(zoom, 1., accent.opacity(0.35))])
        .text_px(13., 13. * 1.45)
        .text_color(theme.color("l-text-dim"))
        .child(
            fa_glyph("fas fa-hourglass-half", 13.)
                .mt(u(3.))
                .text_color(accent),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(u(4.))
                .child(
                    div()
                        .text_px(14., 14. * 1.45)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.color("l-text"))
                        .child(t("#L_DEMO_LONG_TITLE", &[&duration])),
                )
                .child(div().child(text)),
        )
}

#[cfg(test)]
mod tests {
    //! Окно экспорта на фикстурах: каналы `demo-options`, `demo-settings-get`, `demo-plan`,
    //! `demo-estimate` (сразу и через 150 мс), `demo-output-pick`, `demo-reveal` с аргументами
    //! 0.4.9; задание — через страницу.

    use super::super::tests::{calls, click, fixtures, invoke, panel, section, settle};
    use crate::app::LauncherRoot;
    use crate::scale::zpx;
    use gpui::{ScrollDelta, ScrollEvent, TestAppContext, point, size};
    use hlsr_ui::services::Services;
    use serde_json::json;

    /// Ответы главного процесса окну: варианты, план, папка.
    fn answers() -> serde_json::Value {
        let mut answers = invoke();
        let modes = json!({"encoder": "h264_nvenc", "combos": [
            {"height": 720, "width": 1280, "fps": 60}, {"height": 1080, "width": 1920, "fps": 60}]});
        answers["demo-options"] = json!({"status": "ok", "id": "gh", "duration": 144.46,
            "render": {"ok": true, "code": null},
            "capabilities": {"maxHeight": 1080, "fps": [60], "audio": true, "integrity": true,
                "keyhud": true, "hud": false, "ownConfig": true, "ownHud": true},
            "keyhud": {"available": true, "code": null},
            "resolutions": [{"height": 720, "width": 1280, "label": "720p"},
                {"height": 1080, "width": 1920, "label": "1080p"}],
            "speed": {"min": 0.25, "max": 4},
            "modes": {"auto": modes, "gpu": modes, "cpu": null},
            "defaults": {"preset": "quality", "height": 1080, "fps": 60, "speed": 1}});
        answers["demo-plan"] = json!({"status": "ok", "plan": {"mode": "quality", "width": 1920,
            "height": 1080, "fps": 60, "outputDuration": 144.46, "estimatedBytes": 52428800,
            "warnings": []}});
        answers["demo-settings-get"] =
            json!({"status": "ok", "outputDir": "C:\\Videos\\HLSR", "preview": {"keyhud": false}});
        answers["demo-output-pick"] = json!({"status": "ok", "outputDir": "D:\\Clips"});
        answers["demo-reveal"] = json!({"status": "ok"});
        answers
    }

    #[gpui::test]
    fn the_export_window_talks_to_the_main_process_like_0_4_9(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let fx = fixtures(answers());
        let services: Services = fx.clone();
        let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
        cx.run_until_parked();
        section(cx, true);

        // created(): demo-options и папка; ответ — сразу план и оценка, через 150 мс — ещё раз.
        click(cx, "dmp-export");
        settle(cx, 2);
        assert_eq!(calls(&fx, "demo-options"), [json!(["gh"])]);
        assert_eq!(calls(&fx, "demo-settings-get").len(), 2, "страница и окно");
        let payload = json!({"preset": "quality", "height": 1080, "fps": 60, "audio": true,
            "keyhud": false, "integrity": false, "encoder": "auto", "speed": 1, "hud": true,
            "ownConfig": true, "ownHud": true, "range": null});
        assert_eq!(calls(&fx, "demo-plan"), [json!(["gh", payload])]);
        let mut discord = payload.clone();
        discord["preset"] = json!("discord");
        assert_eq!(
            calls(&fx, "demo-estimate"),
            [json!(["gh", [payload, discord]])]
        );
        settle(cx, 12);
        assert_eq!(calls(&fx, "demo-plan").len(), 2);
        assert_eq!(calls(&fx, "demo-estimate").len(), 2);

        // Чип пресета и выбор папки.
        click(cx, "dex-preset-discord");
        settle(cx, 12);
        assert_eq!(calls(&fx, "demo-plan").len(), 3);
        assert_eq!(
            calls(&fx, "demo-plan").last().unwrap()[1]["preset"],
            "discord"
        );
        click(cx, "dex-folder");
        assert_eq!(calls(&fx, "demo-output-pick"), [json!([])]);

        // «Сохранить клип» — задание страницы; ход, «Отменить», итог.
        click(cx, "dex-start");
        assert_eq!(calls(&fx, "demo-export-start"), [json!(["gh", discord])]);
        fx.emit(
            "demo-job-progress",
            json!({"jobId": "e1", "kind": "export", "phase": "render", "percent": 40,
                "segment": {"index": 1, "count": 1}}),
        );
        settle(cx, 4);
        click(cx, "dex-cancel-job");
        assert_eq!(calls(&fx, "demo-cancel"), [json!(["e1"])]);
        fx.emit(
            "demo-job-done",
            json!({"jobId": "e1", "kind": "export", "result": {"id": "r1",
                "file": "D:\\Clips\\gh.mp4", "fileName": "gh.mp4", "size": 9437184,
                "encoder": "h264_nvenc", "attempts": 2,
                "video": {"width": 1280, "height": 720, "fps": 60, "duration": 41.5}}}),
        );
        settle(cx, 4);
        click(cx, "dex-reveal");
        assert_eq!(calls(&fx, "demo-reveal"), [json!(["r1"])]);
        click(cx, "dex-open");
        let open = panel().read_with(cx, |panel, _| panel.state().export_open);
        assert!(!open, "«Открыть в плеере» закрывает окно");
    }

    /// 1600 × 900, раскрыты «Дополнительные настройки»: окно не выше 840, тело длиннее своего
    /// окна и листается колесом, а не растёт под кнопки внизу (`.dex__body { overflow-y: auto }`).
    #[gpui::test]
    fn the_body_scrolls_when_more_settings_overflow(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let fx = fixtures(answers());
        let services: Services = fx.clone();
        let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
        cx.simulate_resize(size(zpx(1., 1600.), zpx(1., 900.)));
        cx.update(|_, cx| cx.set_reduce_motion(true));
        cx.run_until_parked();
        section(cx, true);
        click(cx, "dmp-export");
        settle(cx, 12);
        click(cx, "dex-more");
        settle(cx, 12);
        let bounds = |name: &str| crate::snap::bounds_of(name).expect(name);
        let start = bounds("dex-start");
        assert!(
            start.bottom() <= zpx(1., 870.),
            "окно в пределах 900 − 30: {start:?}"
        );
        // Раскрытие прокрутило тело к панели вида; наверх и проверить, что колесо листает.
        let preset = bounds("dex-preset-discord").center();
        for _ in 0..20 {
            cx.simulate_event(ScrollEvent {
                position: preset,
                delta: ScrollDelta::Lines(point(0., 3.)),
                ..Default::default()
            });
        }
        settle(cx, 4);
        // Наверху тела «Папка» ещё за нижним краем тела (ниже верха кнопок, скрыта обрезкой):
        // тело длиннее своего окна — есть что листать; без прокрутки колесо ниже её не двигает.
        let top = bounds("dex-folder");
        assert!(
            top.top() >= start.top(),
            "наверху тела «Папка» за краем тела: {top:?}"
        );
        let wheel_at = bounds("dex-preset-discord").center();
        cx.simulate_event(ScrollEvent {
            position: wheel_at,
            delta: ScrollDelta::Lines(point(0., -3.)),
            ..Default::default()
        });
        settle(cx, 4);
        let moved = bounds("dex-folder");
        assert!(
            moved.top() < top.top(),
            "колесо листает тело: {top:?} → {moved:?}"
        );
    }
}
