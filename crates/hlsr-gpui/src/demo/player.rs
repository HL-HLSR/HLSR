//! Плеер «Демо» (`DemoPlayer.vue` 0.4.9): экран с видео и слотом поверх, шкала (`timeline.rs`),
//! кнопки — играть/пауза, время, «Начало/Конец здесь» (на узком — значки), скорость (меню),
//! громкость (ползунок с заливкой), во весь экран; клавиши (пробел/K, ←/→ ±5 с, с Shift ±1 с,
//! ↑/↓ громкость, Home/End, F, Esc). Логика без окна — `controllers::demo_player`.
//!
//! Договор со страницей (точка монтирования G11): страница создаёт плеер `DemoPlayer::new`,
//! каждый кадр отдаёт пропсы [`PlayerProps`] (`set_props`: видео, метки, длина демо, интервал,
//! «Начало/Конец здесь», экран поверх видео — `overlay`, слот 0.4.9) и зовёт `pause`,
//! `exit_full`, `seek` (время клипа), `seek_demo` (время демо), `request_focus`. Плеер сообщает
//! [`PlayerEvent`]: отметка интервала, ошибка видео, «во весь экран».
//!
//! Видео — `media::video` (кадры и звук декодирует FFmpeg, решение владельца 3). Адрес
//! `hlsr-media://` переводит в файлы [`MediaSource`] приложения (`DemoApi::player_media`: файл,
//! который выдал фасад, и FFmpeg, сверенный перед запуском). Источника нет (стенд, тесты) или он
//! отказал — `@error`, как у `<video>` с адресом без файла. Пауза снаружи (уход со страницы,
//! смена демки, удаление видео) отпускает файл: процессы FFmpeg гасятся, кадр остаётся.
//!
//! Во весь экран: `demo-player-fullscreen` (окно на весь монитор) и плеер поверх всего окна
//! (`deferred`: выше шапки и колонки, ниже окон-диалогов и тостов — `.dmp.is-player-expanded`
//! z-index 30). Канал ответил не `true` — только на всё окно (HTML Fullscreen в GPUI нет).

use crate::icons::fa_glyph;
use crate::kit::button::{Look, button, focus_handle, focus_visible, outline, white};
use crate::layer::{Dismiss, Layers, TipExt, anchor_probe};
use crate::media::video::{Video, probe};
use crate::motion::{Timing, hover, mix};
use crate::paint::{ring, shadow};
use crate::scale::{u, viewport, zoom, zpx};
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyView, App, Bounds, ClickEvent, Context, DispatchPhase, ElementId, EventEmitter,
    FocusHandle, FontWeight, Global, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels, Point, Render, Role, SharedString,
    Subscription, Task, Window, anchored, canvas, deferred, div, img, prelude::*, px, relative,
    svg,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::demo_player::{Key, Player, Props, rate_text, volume_at};
use hlsr_ui::controllers::demo_tool::{ClipRange, Edge, RATES, clock};
use hlsr_ui::services::{ServiceEvent, Services};
use serde_json::{Value, json};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Канал полного экрана окна и его событие (`DemoApi`, `window_ops`).
const FULLSCREEN: &str = "demo-player-fullscreen";
const FULLSCREEN_CHANGED: &str = "demo-player-fullscreen-changed";
/// Порядок слоя плеера на всё окно: выше шапки и колонки, ниже окон (50) и тостов (60).
const Z_EXPANDED: usize = 30;
/// Пока тянут шкалу или держат стрелку, видео перематывается не чаще (каждая перемотка — новый
/// FFmpeg).
const SCRUB_EVERY: Duration = Duration::from_millis(120);

/// Пропсы плеера (`:src`, `:video`, `:markers`, `:duration`, `:range`, `:marking`) и слот.
#[derive(Clone, Default)]
pub struct PlayerProps {
    /// Адрес видео (`hlsr-media://…`); нет — шкала во времени демо.
    pub src: Option<String>,
    /// `timeline.video`: `{ range, speed, duration, width, height, fps }`.
    pub video: Option<Value>,
    /// Метки шкалы: `{ t, type, label, sourceTime }`.
    pub markers: Vec<Value>,
    /// Длина демки, когда видео нет.
    pub duration: f64,
    /// Интервал экспорта во времени демо.
    pub range: Option<ClipRange>,
    /// «Начало здесь» и «Конец здесь».
    pub marking: bool,
    /// Экран поверх видео (`#overlay`).
    pub overlay: Option<AnyView>,
}

/// События плеера (`@mark`, `@error`, `@expanded`).
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    Mark(Edge, f64),
    Error,
    Expanded(bool),
}

/// Файлы видео: FFmpeg, ffprobe и сам файл.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Media {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub file: PathBuf,
}

/// Адрес `hlsr-media://` → [`Media`]; зовётся в фоне (сверка FFmpeg читает файлы). Ставит
/// приложение; текст ошибки — в журнал.
#[derive(Clone)]
pub struct MediaSource(pub Arc<Resolve>);

/// Адрес → файлы видео или текст ошибки.
pub type Resolve = dyn Fn(&str) -> Result<Media, String> + Send + Sync;

impl Global for MediaSource {}

/// Пропсы для логики.
pub(super) fn props(p: &PlayerProps) -> Props<'_> {
    Props {
        src: p.src.is_some(),
        video: p.video.as_ref(),
        markers: &p.markers,
        duration: p.duration,
        range: p.range,
    }
}

pub struct DemoPlayer {
    services: Services,
    /// Язык страницы: смена языка пересоздаёт страницу с плеерами.
    language: Language,
    pub(super) props: PlayerProps,
    pub(super) s: Player,
    pub(super) focus: FocusHandle,
    /// Рамки прошлого кадра: экран (размер кадра видео и подложки слота), шкала, ряд кнопок
    /// (узкий — без подписей), кнопка скорости (щелчок по ней меню не закрывает), громкость.
    screen: Rc<Cell<Bounds<Pixels>>>,
    pub(super) track: Rc<Cell<Bounds<Pixels>>>,
    controls: Rc<Cell<Bounds<Pixels>>>,
    rate_button: Rc<Cell<Bounds<Pixels>>>,
    volume_bar: Rc<Cell<Bounds<Pixels>>>,
    /// Адрес открытого (или открываемого) видео.
    loaded: Option<String>,
    duration: f64,
    video: Option<Video>,
    /// Номер открытия: ответ прежнего — мимо.
    opening: u64,
    /// `@error` этого адреса уже отдан.
    failed: bool,
    /// Тянут ползунок громкости.
    sliding: bool,
    /// Когда видео последний раз перематывали и место, до которого не дошли (тянут шкалу).
    scrubbed: Option<Instant>,
    pub(super) unsought: Option<f64>,
    _events: Task<()>,
    _quit: Subscription,
}

impl EventEmitter<PlayerEvent> for DemoPlayer {}

impl DemoPlayer {
    pub fn new(services: &Services, cx: &mut Context<DemoPlayer>) -> DemoPlayer {
        let events = crate::bridge::listen(cx, services.subscribe(), |this, event, cx| {
            this.on_event(&event, cx)
        });
        // Выход лаунчера — не beforeUnmount: окно не возвращается из полного экрана.
        let quit = cx.on_app_quit(|this: &mut DemoPlayer, _| {
            this.s.window_full = false;
            async {}
        });
        // beforeUnmount: окно — из полного экрана, кадр — из атласа (процессы гасит drop).
        cx.on_release(|this: &mut DemoPlayer, cx| {
            if this.s.exit_full() {
                fire(&this.services, false, cx);
            }
            if let Some(mut video) = this.video.take() {
                video.release(cx);
            }
        })
        .detach();
        DemoPlayer {
            services: services.clone(),
            language: crate::shell::root::ShellContext::get(cx)
                .map_or(Language::Ru, |shell| shell.shell.language()),
            props: PlayerProps::default(),
            s: Player::default(),
            focus: cx.focus_handle(),
            screen: Rc::default(),
            track: Rc::default(),
            controls: Rc::default(),
            rate_button: Rc::default(),
            volume_bar: Rc::default(),
            loaded: None,
            duration: 0.,
            video: None,
            opening: 0,
            failed: false,
            sliding: false,
            scrubbed: None,
            unsought: None,
            _events: events,
            _quit: quit,
        }
    }

    pub fn set_props(&mut self, props: PlayerProps) {
        self.props = props;
    }

    /// `$t` плеера.
    pub(super) fn translate(&self) -> impl Fn(&str, &[&str]) -> String + use<> {
        let language = self.language;
        move |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args)
    }

    /// Пауза снаружи (уход со страницы, смена демки, удаление видео): файл отпускается.
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        // Отложенная перемотка открыла бы файл снова.
        self.unsought = None;
        if let Some(video) = &mut self.video {
            video.park();
        }
        self.s.playing = false;
        cx.notify();
    }

    /// Выйти из полного экрана (Esc, кнопка, уход со страницы).
    pub fn exit_full(&mut self, cx: &mut Context<Self>) {
        let was = self.s.expanded;
        if self.s.exit_full() {
            fire(&self.services, false, cx);
        }
        if was {
            cx.emit(PlayerEvent::Expanded(false));
        }
        cx.notify();
    }

    /// Перейти ко времени клипа.
    pub fn seek(&mut self, t: f64, cx: &mut Context<Self>) {
        if let Some(at) = self.s.seek(t, &props(&self.props)) {
            self.scrub(at, true, cx);
        }
        cx.notify();
    }

    /// Перейти ко времени демо (метка карточки).
    pub fn seek_demo(&mut self, demo_time: f64, cx: &mut Context<Self>) {
        if let Some(at) = self.s.seek_demo(demo_time, &props(&self.props)) {
            self.scrub(at, true, cx);
        }
        cx.notify();
    }

    /// Фокус плееру (готов предпросмотр).
    pub fn request_focus(&mut self, cx: &mut Context<Self>) {
        let focus = self.focus.clone();
        cx.defer(move |cx| {
            if let Some(window) = cx.active_window() {
                window
                    .update(cx, |_, window, cx| window.focus(&focus, cx))
                    .ok();
            }
        });
    }

    /// Перемотать видео; пока тянут шкалу или держат стрелку — не чаще [`SCRUB_EVERY`]
    /// (пропущенное место — по истечении шага или при отпускании), `now` — сразу.
    pub(super) fn scrub(&mut self, at: f64, now: bool, cx: &mut Context<Self>) {
        let Some(video) = &mut self.video else {
            return;
        };
        let wait = self.scrubbed.map_or(Duration::ZERO, |last| {
            SCRUB_EVERY.saturating_sub(last.elapsed())
        });
        if !now && !wait.is_zero() {
            if self.unsought.replace(at).is_none() {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    this.update(cx, |this, cx| {
                        if let Some(at) = this.unsought.take() {
                            this.scrub(at, true, cx);
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
            return;
        }
        self.unsought = None;
        self.scrubbed = Some(Instant::now());
        video.seek(at);
    }

    fn on_event(&mut self, event: &ServiceEvent, cx: &mut Context<Self>) {
        // Окно вышло из полного экрана само (Esc в окне, Win+↓, свернули): плеер на место.
        if event.channel == FULLSCREEN_CHANGED && event.payload == json!(false) {
            let was = self.s.expanded;
            self.s.window_left();
            if was && !self.s.expanded {
                cx.emit(PlayerEvent::Expanded(false));
            }
            cx.notify();
        }
    }

    /// `@error` (раз на адрес).
    fn fail(&mut self, cx: &mut Context<Self>) {
        if !std::mem::replace(&mut self.failed, true) {
            cx.emit(PlayerEvent::Error);
        }
    }

    /// Новый адрес (`watch: src`): прежнее видео закрывается, новое открывается в фоне.
    fn open(&mut self, cx: &mut Context<Self>) {
        self.loaded = self.props.src.clone();
        self.s.src_changed();
        self.unsought = None;
        self.failed = false;
        self.opening += 1;
        if let Some(mut old) = self.video.take() {
            // Кадр — из атласа вне отрисовки окна; процессы гасит drop.
            cx.defer(move |cx| old.release(cx));
        }
        let Some(url) = self.loaded.clone() else {
            return;
        };
        let Some(source) = cx.try_global::<MediaSource>().cloned() else {
            tracing::warn!("[demo] видео не открыть: нет источника файлов плеера");
            self.fail(cx);
            return;
        };
        let opening = self.opening;
        let opened = cx.background_spawn(async move {
            let media = (source.0)(&url)?;
            let info = probe(&media.ffprobe, &media.file).map_err(|error| error.0)?;
            Ok::<_, String>(Video::new(media.ffmpeg, media.file, info))
        });
        cx.spawn(async move |this, cx| {
            let opened = opened.await;
            this.update(cx, |this, cx| {
                // Адрес сменился, пока открывали: это видео не нужно (кадров в атласе у него нет).
                if this.opening != opening {
                    return;
                }
                match opened {
                    // loadedmetadata: длина, громкость, скорость.
                    Ok(mut video) => {
                        this.s.media_duration = video.info().duration;
                        video.set_volume(this.s.heard_volume() as f32);
                        video.set_speed(this.s.rate);
                        this.s.current = video.position();
                        this.video = Some(video);
                    }
                    Err(error) => {
                        tracing::warn!("[demo] видео не открылось: {error}");
                        this.fail(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let Some(video) = &mut self.video else {
            return;
        };
        if video.playing() && !video.ended() {
            video.pause();
        } else {
            video.play();
        }
        self.s.playing = video.playing();
        cx.notify();
    }

    fn set_volume(&mut self, value: f64, cx: &mut Context<Self>) {
        self.s.set_volume(value);
        self.apply_volume();
        cx.notify();
    }

    fn apply_volume(&mut self) {
        if let Some(video) = &mut self.video {
            video.set_volume(self.s.heard_volume() as f32);
        }
    }

    fn set_rate(&mut self, value: f64, cx: &mut Context<Self>) {
        self.s.set_rate(value);
        if let Some(video) = &mut self.video {
            video.set_speed(value);
        }
        cx.notify();
    }

    fn mark(&mut self, edge: Edge, cx: &mut Context<Self>) {
        let time = self.s.mark_time(&props(&self.props));
        cx.emit(PlayerEvent::Mark(edge, time));
    }

    /// Во весь экран: окно на монитор (`demo-player-fullscreen`) и плеер на всё окно; повторно —
    /// выход.
    fn toggle_full(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.s.expanded {
            self.exit_full(cx);
            return;
        }
        if std::mem::replace(&mut self.s.switching, true) {
            return;
        }
        let reply = self.services.invoke(FULLSCREEN, vec![json!(true)]);
        cx.spawn_in(window, async move |this, cx| {
            let on = matches!(reply.await, Ok(Value::Bool(true)));
            this.update_in(cx, |this, window, cx| {
                this.s.switching = false;
                this.s.entered(on);
                cx.emit(PlayerEvent::Expanded(true));
                window.focus(&this.focus, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        // Пробел и Enter у кнопки плеера — её нажатие.
        if !self.focus.is_focused(window) && matches!(key, "space" | "enter") {
            return;
        }
        let m = &event.keystroke.modifiers;
        let chord = m.control || m.alt || m.platform || m.function;
        let Some(action) = self.s.key(key, m.shift, chord, &props(&self.props)) else {
            return;
        };
        match action {
            Key::TogglePlay => self.toggle_play(cx),
            Key::Seek(t) => {
                if let Some(at) = self.s.seek(t, &props(&self.props)) {
                    self.scrub(at, false, cx);
                }
            }
            Key::Volume(value) => self.set_volume(value, cx),
            Key::Fullscreen => self.toggle_full(window, cx),
            Key::CloseMenu => self.s.rate_open = false,
        }
        // Стрелки не уходят в общий обработчик окна.
        cx.stop_propagation();
        cx.notify();
    }

    /// Esc: меню скорости, потом полный экран; иначе — дальше по окну.
    fn on_dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if self.s.rate_open {
            self.s.rate_open = false;
            cx.notify();
        } else if self.s.expanded {
            self.exit_full(cx);
        } else {
            cx.propagate();
        }
    }

    /// Громкость под курсором ползунка.
    fn slide(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let bounds = self.volume_bar.get();
        let zoom = zoom(window);
        let x = f64::from(f32::from(position.x - bounds.origin.x) / zoom);
        let width = f64::from(f32::from(bounds.size.width) / zoom);
        self.set_volume(volume_at(x, width), cx);
    }

    /// Кадр видео в рамке экрана прошлого кадра; часы и состояние — с видео.
    fn frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Arc<gpui::RenderImage>> {
        let video = self.video.as_mut()?;
        let screen = self.screen.get().size;
        if screen.width <= px(0.) || screen.height <= px(0.) {
            // Экран ещё не разложен: размер — в следующем кадре.
            crate::motion::monitor_notify(window, cx.entity_id());
            return None;
        }
        let scale = window.scale_factor();
        let device = |side: Pixels| (f32::from(side) * scale).round().max(1.) as u32;
        let image = video.frame((device(screen.width), device(screen.height)), window, cx);
        // Перемотка отложена: головка — на нужном месте, следующая стрелка — от него.
        if !self.s.dragging && self.unsought.is_none() {
            self.s.current = video.position();
        }
        self.s.playing = video.playing() && !video.ended();
        if video.error().is_some() {
            self.fail(cx);
        }
        image
    }
}

/// `demo-player-fullscreen` без ответа.
fn fire(services: &Services, on: bool, cx: &App) {
    let reply = services.invoke(FULLSCREEN, vec![json!(on)]);
    cx.background_spawn(async move {
        let _ = reply.await;
    })
    .detach();
}

/// Подложка слота `radial-gradient(ellipse at center, rgba(panel, .55), rgba(0, 0, 0, .88))`
/// над чёрным экраном: цвет панели с прозрачностью .55 → 0 к углам (маска SVG по размеру).
fn overlay_svg(w: f32, h: f32) -> String {
    format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}' viewBox='0 0 {w} {h}'>\
         <radialGradient id='g' gradientUnits='userSpaceOnUse' cx='0' cy='0' r='1' \
         gradientTransform='matrix({rx} 0 0 {ry} {cx} {cy})'>\
         <stop offset='0' stop-color='#fff' stop-opacity='0.55'/>\
         <stop offset='1' stop-color='#fff' stop-opacity='0'/></radialGradient>\
         <rect width='{w}' height='{h}' fill='url(#g)'/></svg>",
        cx = w / 2.,
        cy = h / 2.,
        rx = w / 2. * std::f32::consts::SQRT_2,
        ry = h / 2. * std::f32::consts::SQRT_2,
    )
}

/// Кнопка `.dpl__btn`: 34 px, прозрачная, при наведении — белый .08 и `--l-text`; недоступная
/// — .4; фокус с клавиатуры — контур 2 px `--l-accent`. `text` — `--text` (скорость): шире, 12,5 px.
#[allow(clippy::too_many_arguments)]
fn control(
    id: &'static str,
    glyph: Option<&'static str>,
    text: Option<String>,
    label: String,
    disabled: bool,
    window: &mut Window,
    cx: &mut Context<DemoPlayer>,
    on_click: impl Fn(&mut DemoPlayer, &mut Window, &mut Context<DemoPlayer>) + 'static,
) -> AnyElement {
    let id = ElementId::from(id);
    let focus = focus_handle(&id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let theme = cx.global::<Theme>();
    let (dim, bright, accent) = (
        theme.color("l-text-dim"),
        theme.color("l-text"),
        theme.color("l-accent"),
    );
    let zoom = zoom(window);
    let on_click = cx.listener(move |this, _: &ClickEvent, window, cx| on_click(this, window, cx));
    let meta = crate::live::Meta::new("button", id.clone())
        .label(Some(SharedString::from(label.clone())))
        .disabled(disabled)
        .focused(focus.is_focused(window));
    let inner = id.clone();
    let element = hover(id, Timing::fast(), move |t| {
        let t = if disabled { 0. } else { t };
        let wide = text.is_some();
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .aria_label(SharedString::from(label.clone()))
            .tip(label)
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .min_w(u(if wide { 48. } else { 34. }))
            .h(u(34.))
            .when(wide, |el| {
                el.px(u(8.))
                    .text_size(u(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .tnum()
            })
            .rounded(u(8.))
            .bg(white(0.08 * t))
            .text_color(mix(dim, bright, t))
            .when_some(glyph, |el, glyph| el.child(fa_glyph(glyph, 14.)))
            .when_some(text, |el, text| el.child(text))
            .when(disabled, |el| el.opacity(0.4))
            .when(!disabled, |el| {
                el.track_focus(&focus).cursor_pointer().on_click(on_click)
            })
            .when(ring_on, |el| el.child(outline(zoom, 8., 0., 2., accent)))
    });
    crate::live::mark(meta, element)
}

impl Render for DemoPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // watch: src, duration.
        if self.props.src != self.loaded {
            self.open(cx);
        }
        if self.props.duration.to_bits() != self.duration.to_bits() {
            self.duration = self.props.duration;
            self.s.duration_changed();
        }
        let image = self.frame(window, cx);
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let t = self.translate();
        let p = props(&self.props);
        let length = self.s.length(&p);
        let idle = length == 0. || length.is_nan();
        let has_src = p.src;
        let expanded = self.s.expanded;
        // На всё окно углы — как у окна (в полном экране — прямые).
        let radius = if expanded {
            Layers::radius(cx)
        } else {
            theme.rem("l-radius-sm").to_pixels(window.rem_size())
        };
        let bar_bg = theme.color("l-modal-bar");
        let (dim, text, faint, border) = (
            theme.color("l-text-dim"),
            theme.color("l-text"),
            theme.color("l-text-faint"),
            theme.color("l-border"),
        );

        let overlay = self.props.overlay.clone().map(|overlay| {
            let bounds = self.screen.get();
            let (w, h) = (
                f32::from(bounds.size.width) / zoom,
                f32::from(bounds.size.height) / zoom,
            );
            let backdrop = (w > 0. && h > 0.).then(|| {
                svg()
                    .data(overlay_svg(w, h).as_bytes())
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(u(w))
                    .h(u(h))
                    .text_color(theme.color("l-panel-rgb"))
            });
            div()
                .absolute()
                .inset_0()
                .rounded_t(radius)
                .bg(gpui::black().opacity(0.88))
                .children(backdrop)
                .child(
                    div()
                        .id("dpl-overlay")
                        .absolute()
                        .inset_0()
                        // `@click.stop @dblclick.stop`: экран под слотом нажатий не получает.
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(u(24.))
                        .overflow_y_scroll()
                        .child(overlay),
                )
        });
        let screen = div()
            .id("dpl-screen")
            .relative()
            .flex_1()
            .min_h(u(120.))
            // GPUI режет детей прямоугольником: углы скругляют сами дети.
            .rounded_t(radius)
            .bg(gpui::black())
            .when(has_src, |el| el.cursor_pointer())
            .child(anchor_probe(self.screen.clone()))
            .children(image.map(|image| {
                img(image)
                    .absolute()
                    .inset_0()
                    .size_full()
                    .rounded_t(radius)
                    .object_fit(ObjectFit::Contain)
            }))
            .children(overlay)
            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                this.s.rate_open = false;
                this.toggle_play(cx);
                window.focus(&this.focus, cx);
                if event.click_count() == 2 {
                    this.toggle_full(window, cx);
                }
                cx.notify();
            }));

        // Ряд кнопок; узкий (≤ 560 px, Full HD 125 %) — «Начало/Конец здесь» без подписей.
        let row = f32::from(self.controls.get().size.width) / zoom;
        let narrow = row > 0. && row <= 560.;
        let playing = self.s.playing;
        let play_label = t(
            if playing {
                "#L_DEMO_PAUSE"
            } else {
                "#L_DEMO_PLAY"
            },
            &[],
        );
        let play = control(
            "dpl-play",
            Some(if playing {
                "fas fa-pause"
            } else {
                "fas fa-play"
            }),
            None,
            play_label,
            !has_src,
            window,
            cx,
            |this, _, cx| this.toggle_play(cx),
        );
        let time = div()
            .flex_none()
            .ml(u(4.))
            .flex()
            .gap(u(4.))
            .text_size(u(12.5))
            .tnum()
            .whitespace_nowrap()
            .text_color(text)
            .child(clock(self.s.position(&p)))
            .child(
                div()
                    .text_color(faint)
                    .child(format!("/ {}", clock(length))),
            );
        let marks = self.props.marking.then(|| {
            let mark = |id: &'static str, key: &str, glyph: &'static str, edge: Edge| {
                let label = t(key, &[]);
                let icon = fa_glyph(glyph, 10.).into_any_element();
                let caption = (!narrow).then(|| label.clone().into_any_element());
                let children: Vec<AnyElement> = match edge {
                    Edge::From => [Some(icon), caption].into_iter().flatten().collect(),
                    Edge::To => [caption, Some(icon)].into_iter().flatten().collect(),
                };
                div().id(id).flex_none().tip(label.clone()).child(
                    button(SharedString::from(format!("{id}-btn")), Look::Gx)
                        .disabled(idle)
                        .label(label)
                        .children(children)
                        .on_click(cx.listener(move |this, _, _, cx| this.mark(edge, cx))),
                )
            };
            [
                crate::snap::target(
                    "dpl-from",
                    mark(
                        "dpl-from",
                        "#L_DEMO_SET_START",
                        "fas fa-step-backward",
                        Edge::From,
                    ),
                ),
                crate::snap::target(
                    "dpl-to",
                    mark("dpl-to", "#L_DEMO_SET_END", "fas fa-step-forward", Edge::To),
                ),
                div()
                    .flex_none()
                    .w(u(1.))
                    .h(u(20.))
                    .mx(u(4.))
                    .bg(border)
                    .into_any_element(),
            ]
        });
        let rate_menu = self.s.rate_open.then(|| {
            let current = self.s.rate;
            let decimal = t("#L_DECIMAL", &[]);
            let items = RATES.iter().map(|&value| {
                let active = value == current;
                let id = ElementId::Name(format!("dpl-rate-{value}").into());
                let inner = id.clone();
                let pick = cx.listener(move |this, _: &ClickEvent, _, cx| this.set_rate(value, cx));
                let caption = rate_text(value, &decimal);
                let accent = theme.color("accent");
                let meta = crate::live::Meta::new("option", id.clone())
                    .label(Some(SharedString::from(caption.clone())))
                    .checked(active);
                let option = hover(id, Timing::fast(), move |h| {
                    div()
                        .id(inner)
                        .role(Role::ListBoxOption)
                        .button_text()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(u(30.))
                        .px(u(8.))
                        .rounded(u(6.))
                        .bg(white(0.07 * h))
                        .text_color(if active { text } else { mix(dim, text, h) })
                        .text_size(u(13.))
                        .tnum()
                        .cursor_pointer()
                        .child(caption)
                        .when(active, |el| {
                            el.child(fa_glyph("fas fa-check", 11.).text_color(accent))
                        })
                        .on_click(pick)
                });
                crate::live::mark(meta, option)
            });
            let inside = self.rate_button.clone();
            div()
                .id("dpl-rate-menu")
                .absolute()
                .right_0()
                .bottom(u(42.))
                .flex()
                .flex_col()
                .min_w(u(168.))
                .p(u(6.))
                .rounded(theme.rem("l-radius-sm"))
                .bg(theme.color("l-surface-solid"))
                .shadow(vec![
                    shadow(zoom, 0., 12., 32., 0., gpui::black().opacity(0.55)),
                    ring(zoom, 1., border),
                ])
                .role(Role::ListBox)
                .occlude()
                // Нажатие мимо меню и его кнопки закрывает меню (`onDocumentDown`).
                .on_mouse_down_out(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if !inside.get().contains(&event.position) {
                        this.s.rate_open = false;
                        cx.notify();
                    }
                }))
                .child(div().pt(u(4.)).px(u(8.)).pb(u(6.)).child(spaced(
                    t("#L_DEMO_RATE", &[]).to_uppercase(),
                    11.,
                    20.,
                    FontWeight::SEMIBOLD,
                    0.06,
                    faint,
                )))
                .children(items)
        });
        let rate = div()
            .relative()
            .flex_none()
            .child(
                div()
                    .relative()
                    .child(anchor_probe(self.rate_button.clone()))
                    .child(control(
                        "dpl-rate",
                        None,
                        Some(rate_text(self.s.rate, &t("#L_DECIMAL", &[]))),
                        t("#L_DEMO_RATE", &[]),
                        !has_src,
                        window,
                        cx,
                        |this, _, cx| {
                            this.s.rate_open = !this.s.rate_open;
                            cx.notify();
                        },
                    )),
            )
            .children(rate_menu);

        let mute_label = t(
            if self.s.muted {
                "#L_DEMO_UNMUTE"
            } else {
                "#L_DEMO_MUTE"
            },
            &[],
        );
        let mute = control(
            "dpl-mute",
            Some(self.s.volume_icon()),
            None,
            mute_label,
            false,
            window,
            cx,
            |this, _, cx| {
                this.s.toggle_mute();
                this.apply_volume();
                cx.notify();
            },
        );
        let volume = self.volume_slider(window, cx);
        let full_label = t(
            if expanded {
                "#L_DEMO_FULLSCREEN_EXIT"
            } else {
                "#L_DEMO_FULLSCREEN"
            },
            &[],
        );
        let full = control(
            "dpl-full",
            Some(if expanded {
                "fas fa-compress"
            } else {
                "fas fa-expand"
            }),
            None,
            full_label,
            false,
            window,
            cx,
            |this, window, cx| this.toggle_full(window, cx),
        );
        let controls = div()
            .flex_none()
            .relative()
            .flex()
            .items_center()
            .gap(u(6.))
            .min_h(u(48.))
            .pt(u(2.))
            .px(u(10.))
            .pb(u(8.))
            .rounded_b(radius)
            .bg(bar_bg)
            .child(anchor_probe(self.controls.clone()))
            .child(crate::snap::target("dpl-play", play))
            .child(time)
            .child(div().flex_1())
            .children(marks.into_iter().flatten())
            .child(rate)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(u(2.))
                    .child(mute)
                    .child(volume),
            )
            .child(crate::snap::target("dpl-full", full));
        let timeline = self.timeline(window, cx);

        let ring_on = focus_visible(&self.focus, window);
        let root = div()
            .id("dpl")
            .track_focus(&self.focus)
            .role(Role::Group)
            .aria_label(SharedString::from(t("#L_DEMO_TITLE", &[])))
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(Self::on_dismiss))
            .flex()
            .flex_col()
            .min_h_0()
            .rounded(radius)
            .bg(gpui::black())
            .shadow(vec![if ring_on {
                ring(zoom, 2., theme.color("l-accent"))
            } else {
                ring(zoom, 1., border)
            }])
            .overflow_hidden()
            .child(screen)
            .child(timeline)
            .child(controls);
        if !expanded {
            return div().size_full().child(root.size_full()).into_any_element();
        }
        // `.is-expanded`: на всё окно поверх шапки и колонки; полоса шапки под плеером окно не
        // таскает.
        let view = window.viewport_size();
        div()
            .size_full()
            .child(
                deferred(
                    anchored()
                        .position(Point::default())
                        .child(root.occlude().w(view.width).h(view.height)),
                )
                .with_priority(Z_EXPANDED),
            )
            .into_any_element()
    }
}

impl DemoPlayer {
    /// Ползунок громкости (`<input type="range" step="0.05">`): дорожка 4 px с заливкой
    /// `--l-text-dim` до значения, бегунок 12 px; тянут и за пределами (захват мыши окном).
    fn volume_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let zoom = zoom(window);
        let id = ElementId::from("dpl-volume");
        let focus = focus_handle(&id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let theme = cx.global::<Theme>().clone();
        let fill = self.s.heard_volume().clamp(0., 1.) as f32;
        // @media (max-width: 1400px).
        let width = if viewport(window).0 <= 1400. {
            56.
        } else {
            76.
        };
        let sliding = self.sliding;
        let player = cx.entity().downgrade();
        let capture = canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                if !sliding {
                    return;
                }
                let moved = player.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let (position, released) = (
                        event.position,
                        event.pressed_button != Some(MouseButton::Left),
                    );
                    moved
                        .update(cx, |this, cx| {
                            if released {
                                this.sliding = false;
                                cx.notify();
                            } else {
                                this.slide(position, window, cx);
                            }
                        })
                        .ok();
                });
                let up = player.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        up.update(cx, |this, cx| {
                            this.sliding = false;
                            cx.notify();
                        })
                        .ok();
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let label = self.translate()("#L_DEMO_VOLUME", &[]);
        let meta = crate::live::Meta::new("slider", id.clone())
            .label(Some(SharedString::from(label.clone())))
            .focused(focus.is_focused(window))
            .value(format!("{fill}"));
        let slider = div()
            .id(id)
            .role(Role::Slider)
            .aria_label(SharedString::from(label))
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(1.)
            .aria_numeric_value(f64::from(fill))
            .track_focus(&focus)
            .relative()
            .flex_none()
            .w(u(width))
            .h(u(20.))
            .mr(u(6.))
            .cursor_pointer()
            .child(anchor_probe(self.volume_bar.clone()))
            .child(
                div()
                    .absolute()
                    .top(u(8.))
                    .left_0()
                    .right_0()
                    .h(u(4.))
                    .rounded(u(2.))
                    .bg(white(0.14))
                    .child(
                        div()
                            .h_full()
                            .w(relative(fill))
                            .rounded_l(u(2.))
                            .bg(theme.color("l-text-dim")),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(u(4.))
                    .left(zpx(
                        zoom,
                        fill * (f32::from(self.volume_bar.get().size.width) / zoom - 12.).max(0.),
                    ))
                    .size(u(12.))
                    .rounded_full()
                    .bg(gpui::white())
                    .when(ring_on, |el| {
                        el.shadow(vec![shadow(
                            zoom,
                            0.,
                            0.,
                            0.,
                            3.,
                            theme.color("accent-soft"),
                        )])
                    }),
            )
            .child(capture)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.sliding = true;
                    this.slide(event.position, window, cx);
                }),
            )
            // Стрелки, Home/End — у самого ползунка; пробел, K и F он глотает, как `<input>`.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let volume = this.s.heard_volume();
                let next = match event.keystroke.key.as_str() {
                    "right" | "up" | "pageup" => volume + 0.05,
                    "left" | "down" | "pagedown" => volume - 0.05,
                    "home" => 0.,
                    "end" => 1.,
                    "space" | "k" | "f" => {
                        cx.stop_propagation();
                        return;
                    }
                    _ => return,
                };
                cx.stop_propagation();
                this.set_volume((next.clamp(0., 1.) * 20.).round() / 20., cx);
            }));
        crate::live::mark(meta, slider)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{calls, click, fixtures, invoke, panel, section, settle};
    use super::{DemoPlayer, Media, MediaSource, Video};
    use crate::app::LauncherRoot;
    use crate::scale::zpx;
    use gpui::{Modifiers, TestAppContext, point};
    use hlsr_ui::controllers::demo_tool::ClipRange;
    use hlsr_ui::services::Services;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[gpui::test]
    fn the_player_snaps_marks_goes_fullscreen_and_reports_a_broken_video(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let mut answers = invoke();
        answers["demo-player-fullscreen"] = json!({"$args": [[[true], true], [[false], false]]});
        let fx = fixtures(answers);
        let services: Services = fx.clone();
        let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
        cx.run_until_parked();
        section(cx, true);

        // Без видео шкала — время демо (144,46 с): нажатие в 3 px от метки 25,85 — на метку.
        let track = crate::snap::bounds_of("dpl-track").expect("шкала");
        let at = track.origin.x + track.size.width * (25.85 / 144.46) + zpx(1., 3.);
        cx.simulate_click(point(at, track.center().y), Modifiers::none());
        settle(cx, 4);
        click(cx, "dpl-from");
        let range = panel().read_with(cx, |panel, _| panel.state().range);
        assert_eq!(range.map(|range| range.from), Some(25.85), "{range:?}");
        // Дальше от метки — время под курсором (до мс).
        let at = track.origin.x + track.size.width * 0.5;
        cx.simulate_click(point(at, track.center().y), Modifiers::none());
        settle(cx, 4);
        click(cx, "dpl-to");
        let range = panel().read_with(cx, |panel, _| panel.state().range);
        assert_eq!(
            range,
            Some(ClipRange {
                from: 25.85,
                to: 72.23
            })
        );

        // Во весь экран: окно на монитор и плеер на всё окно; окно вышло само — плеер на место.
        let expanded = |cx: &mut gpui::VisualTestContext| {
            panel().read_with(cx, |panel, _| panel.state().player_expanded)
        };
        click(cx, "dpl-full");
        assert!(expanded(cx));
        fx.emit("demo-player-fullscreen-changed", json!(false));
        settle(cx, 4);
        assert!(!expanded(cx));
        // Снова и Esc: окно отпускается.
        click(cx, "dpl-full");
        assert!(expanded(cx));
        cx.simulate_keystrokes("escape");
        settle(cx, 4);
        assert!(!expanded(cx));
        assert_eq!(
            calls(&fx, "demo-player-fullscreen"),
            [json!([true]), json!([true]), json!([false])]
        );

        // Видео, которое не открыть (нет источника файлов): `@error` — экран ошибки видео.
        click(cx, "dmp-tab-rendered");
        click(cx, "dmp-video-0");
        settle(cx, 4);
        let broken = panel().read_with(cx, |panel, _| panel.state().rendered.media_error);
        assert!(broken);
    }

    /// Шаги окна в реальном времени (FFmpeg идёт по настоящим часам), пока `done` не верно (≤ 5 с).
    fn wait(cx: &mut gpui::VisualTestContext, done: impl Fn(&DemoPlayer) -> bool) -> bool {
        let begun = std::time::Instant::now();
        while begun.elapsed() < std::time::Duration::from_secs(5) {
            settle(cx, 1);
            let player = panel().read_with(cx, |panel, _| panel.rendered_player.clone());
            if player.read_with(cx, |player, _| done(player)) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    #[gpui::test]
    fn a_real_mp4_plays_and_a_pause_from_the_page_lets_the_file_go(cx: &mut TestAppContext) {
        let ffmpeg =
            std::env::var_os("HLSR_DEMO_FFMPEG").map_or(PathBuf::from("ffmpeg"), PathBuf::from);
        let ffprobe = ffmpeg.with_file_name(format!("ffprobe{}", std::env::consts::EXE_SUFFIX));
        let dir = std::env::temp_dir().join(format!("hlsr-player-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.mp4");
        // 2 с 160×90, звук — тишина.
        let made = std::process::Command::new(&ffmpeg)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc2=size=160x90:rate=25:duration=2")
            .args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", "2"])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
            .arg(&file)
            .status();
        if !made.is_ok_and(|status| status.success()) {
            eprintln!("FFmpeg нет — проверка пропущена");
            return;
        }
        cx.update(crate::init);
        let media = Media {
            ffmpeg,
            ffprobe,
            file,
        };
        cx.update(|cx| {
            cx.set_global(MediaSource(Arc::new(move |url: &str| {
                (url == "hlsr-media://r/1")
                    .then(|| media.clone())
                    .ok_or_else(|| url.to_owned())
            })))
        });
        let fx = fixtures(invoke());
        let services: Services = fx.clone();
        let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
        cx.run_until_parked();
        section(cx, true);
        click(cx, "dmp-tab-rendered");
        click(cx, "dmp-video-0");

        // Открыто: длина файла вместо timeline.video (20 с), кадр на экране.
        assert!(
            wait(cx, |player| player.video.is_some()),
            "видео не открылось"
        );
        assert!(wait(cx, |player| (player.s.media_duration - 2.).abs() < 0.1));
        click(cx, "dpl-play");
        assert!(wait(cx, |player| player.s.playing && player.s.current > 0.3));
        let pids = |cx: &mut gpui::VisualTestContext| {
            let player = panel().read_with(cx, |panel, _| panel.rendered_player.clone());
            player.read_with(cx, |player, _| {
                player.video.as_ref().map(Video::pids).unwrap_or_default()
            })
        };
        assert!(!pids(cx).is_empty());
        // Уход на «Демки» ставит паузу снаружи: процессы FFmpeg погашены, видео и место — те же.
        click(cx, "dmp-tab-demos");
        assert!(pids(cx).is_empty());
        let player = panel().read_with(cx, |panel, _| panel.rendered_player.clone());
        let (playing, at) = player.read_with(cx, |player, _| (player.s.playing, player.s.current));
        assert!(!playing && at > 0.3, "{at}");
        let panel_error = panel().read_with(cx, |panel, _| panel.state().rendered.media_error);
        assert!(!panel_error);
        std::fs::remove_dir_all(&dir).ok();
    }
}
