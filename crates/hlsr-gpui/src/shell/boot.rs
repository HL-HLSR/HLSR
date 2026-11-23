//! Заставка (`views/Loading.vue`, `LoadingIndicator.vue`, `Logo.vue` 0.4.9): логотип, 24
//! мигающих треугольника со свечением, надпись с «дыханием», подпись проверки, полоса загрузки
//! обновления (проценты, размер и скорость на языке окна), «Требуется обновление» с причиной и
//! «Повторить». Надписи «Developed by» в 0.4.9 нет.
//!
//! Логика — `hlsr_ui::controllers::boot::BootMachine`; здесь часы, таймеры, канал
//! `boot-gate-run` и события `updater-state`. Заставка лежит вне `.launcher-root`: тема и
//! настройка «анимации» на неё не действуют — цвета и размеры числами из CSS 0.4.9, стрелки —
//! `--accent-color` из base.css. Положения — от середины окна (`top: calc(50% + Npx)`), высота
//! строк — `line-height: 20px` у body (base.css), у кнопки — `normal` Rubik.

use crate::image::{Raster, device_size, render_image};
use crate::kit::button::focus_handle;
use crate::layer::Layers;
use crate::motion::{Timing, hover, keyframes, mix, tween};
use crate::scale::{u, viewport, zoom};
use crate::shell::root::{AppPaths, language_of};
use crate::text::{RUBIK, TextExt, spaced};
use gpui::{
    App, ClickEvent, Context, ElementId, EventEmitter, FontWeight, Hsla, IntoElement, ObjectFit,
    Render, RenderImage, Task, Window, div, hsla, img, prelude::*, px, relative, rgb, white,
};
use hlsr_core::language::Language;
use hlsr_core::launcher_prefs::LauncherPrefs;
use hlsr_ui::controllers::boot::{BootMachine, Effect, gate_answer, gate_args, updater_status};
use hlsr_ui::render::triangle::{self, Arrow};
use hlsr_ui::services::{ServiceEvent, Services};
use hlsr_ui::{Rgba, Rgba8Image, tr};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// `--accent-color` (base.css): стрелки при любой теме.
const ACCENT: Rgba = Rgba::new(0x00, 0xab, 0xff, 255);
/// `filter: drop-shadow(0 0 27px)` стрелки (`$size * 1.5`).
const GLOW: f32 = 27.;
/// `ease` CSS.
const EASE: [f64; 4] = [0.25, 0.1, 0.25, 1.];
/// Цветной фон считается в четверть разрешения: он гладкий, GPUI растягивает его с
/// линейной выборкой.
const BACKDROP_STEP: u32 = 4;

fn white_a(alpha: f32) -> Hsla {
    hsla(0., 0., 1., alpha)
}

/// Заставка ушла (`router.push({name: "launcher"})`): на её место встаёт оболочка.
pub struct Leave;

/// Картинка стрелки и её место относительно рамки (x, y, ширина, высота в CSS px).
type Placed = (Arc<RenderImage>, [f32; 4]);

/// Растры стрелок под масштаб экрана: у привязанных к пикселям рамок 2–4 разных размера.
#[derive(Default)]
struct Arrows {
    scale: f32,
    /// Ключ — направление и размер в физических px.
    images: HashMap<(bool, u32, u32), Placed>,
}

impl Arrows {
    fn get(
        &mut self,
        arrow: &Arrow,
        scale: f32,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Placed> {
        if self.scale != scale {
            self.scale = scale;
            for (image, _) in self.images.drain().map(|(_, entry)| entry) {
                cx.drop_image(image, Some(window));
            }
        }
        let device = |css: f64| (css * f64::from(scale)).round() as u32;
        let key = (arrow.slot.down, device(arrow.width), device(arrow.height));
        if let Some(placed) = self.images.get(&key) {
            return Some(placed.clone());
        }
        let raster = triangle::arrow_with_glow(
            arrow.width as f32,
            arrow.height as f32,
            arrow.slot.down,
            GLOW,
            ACCENT,
            scale,
        );
        if raster.pixels.width() == 0 {
            return None;
        }
        let place = [raster.x, raster.y, raster.width, raster.height];
        let placed = (render_image(&raster.pixels), place);
        self.images.insert(key, placed.clone());
        Some(placed)
    }

    fn release(&mut self, cx: &mut App) {
        for (image, _) in self.images.drain().map(|(_, entry)| entry) {
            cx.drop_image(image, None);
        }
    }
}

/// `.boot-colored`: растр под размер окна.
#[derive(Default)]
struct Backdrop {
    raster: Raster,
    size: (u32, u32),
}

impl Backdrop {
    fn image(
        &mut self,
        view: (f32, f32),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<RenderImage>> {
        let (width, height) = device_size(window, view.0, view.1);
        let size = (
            width.div_ceil(BACKDROP_STEP),
            height.div_ceil(BACKDROP_STEP),
        );
        if size != self.size {
            self.size = size;
            self.raster.set(&colored_backdrop(size, view));
        }
        self.raster.image(window, cx)
    }
}

/// Цвет стопа: каналы 0–255 и прозрачность, предумноженные (как интерполирует Chromium).
fn premultiplied(r: u8, g: u8, b: u8, a: f32) -> [f32; 4] {
    [f32::from(r) * a, f32::from(g) * a, f32::from(b) * a, a]
}

/// Градиент по стопам `(позиция, цвет)` в точке `t`; за крайними стопами — их цвет.
fn sample(stops: &[(f32, [f32; 4])], t: f32) -> [f32; 4] {
    let (first, last) = (stops[0], stops[stops.len() - 1]);
    if t <= first.0 {
        return first.1;
    }
    for pair in stops.windows(2) {
        let ((p0, c0), (p1, c1)) = (pair[0], pair[1]);
        if t <= p1 {
            let k = if p1 > p0 { (t - p0) / (p1 - p0) } else { 1. };
            return std::array::from_fn(|i| c0[i] + (c1[i] - c0[i]) * k);
        }
    }
    last.1
}

/// `.boot.boot-colored` (Loading.vue): `radial-gradient(ellipse 60% 50% at 50% 46%, …)` поверх
/// `linear-gradient(155deg, #0d1f3c, #12294e 34%, #163356 55%, #0f2440)`. `size` — растр,
/// `view` — окно в CSS px (угол линейного градиента зависит от сторон окна).
fn colored_backdrop((width, height): (u32, u32), (view_w, view_h): (f32, f32)) -> Rgba8Image {
    let linear = [
        (0., premultiplied(0x0d, 0x1f, 0x3c, 1.)),
        (0.34, premultiplied(0x12, 0x29, 0x4e, 1.)),
        (0.55, premultiplied(0x16, 0x33, 0x56, 1.)),
        (1., premultiplied(0x0f, 0x24, 0x40, 1.)),
    ];
    let radial = [
        (0., premultiplied(47, 156, 236, 0.34)),
        (0.38, premultiplied(30, 98, 172, 0.18)),
        (0.72, premultiplied(12, 36, 74, 0.)),
    ];
    // Линия градиента CSS: через центр, направление угла (0 — вверх), длина — проекция окна.
    let (sin, cos) = 155f32.to_radians().sin_cos();
    let length = (view_w * sin).abs() + (view_h * cos).abs();
    let mut image = Rgba8Image::new(width, height);
    for (index, pixel) in image.make_mut_slice().iter_mut().enumerate() {
        let x = ((index as u32 % width) as f32 + 0.5) * view_w / width as f32;
        let y = ((index as u32 / width) as f32 + 0.5) * view_h / height as f32;
        let along = ((x - view_w / 2.) * sin - (y - view_h / 2.) * cos) / length + 0.5;
        let base = sample(&linear, along);
        let dx = (x - 0.5 * view_w) / (0.6 * view_w);
        let dy = (y - 0.46 * view_h) / (0.5 * view_h);
        let glow = sample(&radial, (dx * dx + dy * dy).sqrt());
        let channel = |i: usize| (glow[i] + base[i] * (1. - glow[3])).round().clamp(0., 255.) as u8;
        *pixel = Rgba::rgb(channel(0), channel(1), channel(2));
    }
    image
}

pub struct BootScreen {
    services: Services,
    machine: BootMachine,
    language: Language,
    /// `prefs.coloredBoot !== false`.
    colored: bool,
    /// `localStorage.wasLaunched` 0.4.9 — ключ `launcher-prefs.json`; без папки данных
    /// (витрина, тесты) — только память.
    prefs: Option<LauncherPrefs>,
    start: Instant,
    /// Заставка на экране: после ухода страховка и события не нужны (`beforeUnmount`).
    mounted: bool,
    failsafe: Option<Task<()>>,
    navigate: Option<Task<()>>,
    events: Option<Task<()>>,
    arrows: Arrows,
    backdrop: Backdrop,
}

impl EventEmitter<Leave> for BootScreen {}

impl BootScreen {
    /// `data()` и `mounted()`: надпись, `wasLaunched`, слушатель `updater-state`, страховка,
    /// первый вызов шлюза.
    pub fn new(services: Services, cx: &mut Context<BootScreen>) -> BootScreen {
        let prefs = cx
            .try_global::<AppPaths>()
            .map(|paths| LauncherPrefs::open(&paths.0));
        let pref = |key: &str| prefs.as_ref().and_then(|prefs| prefs.get(key));
        let was_launched = pref("wasLaunched").is_some_and(hlsr_core::js::truthy);
        let colored = pref("coloredBoot") != Some(&Value::Bool(false));
        let events = crate::bridge::listen(cx, services.subscribe(), Self::on_event);
        let mut boot = BootScreen {
            language: language_of(&services),
            services,
            machine: BootMachine::new(was_launched, 0.),
            colored,
            prefs,
            start: cx.background_executor().now(),
            mounted: true,
            failsafe: None,
            navigate: None,
            events: Some(events),
            arrows: Arrows::default(),
            backdrop: Backdrop::default(),
        };
        let effects = boot.machine.mounted(0.);
        boot.run(effects, cx);
        boot
    }

    /// Картинки заставки — из атласа (заставка ушла насовсем).
    pub fn release(&mut self, cx: &mut App) {
        self.arrows.release(cx);
        self.backdrop.raster.release(cx);
    }

    /// Мс от создания заставки (`Date.now() - mountedAt`).
    fn now(&self, cx: &App) -> f64 {
        let now = cx.background_executor().now();
        now.saturating_duration_since(self.start).as_secs_f64() * 1000.
    }

    fn on_event(&mut self, event: ServiceEvent, cx: &mut Context<Self>) {
        if !self.mounted || event.channel != "updater-state" {
            return;
        }
        if let Some(status) = updater_status(&event.payload) {
            self.machine.updater_state(&status);
            cx.notify();
        }
    }

    fn run(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::MarkLaunched => {
                    // Файл пишется целиком с `sync_all` в потоке окна: только когда ключ ещё не `true`.
                    if let Some(prefs) = &mut self.prefs
                        && prefs.get("wasLaunched") != Some(&Value::Bool(true))
                    {
                        prefs.set("wasLaunched", Value::Bool(true));
                    }
                }
                Effect::ArmFailsafe { after_ms } => {
                    self.failsafe = Some(Self::after(after_ms, cx, |boot, cx| {
                        if boot.mounted {
                            let effects = boot.machine.failsafe(boot.now(cx));
                            boot.run(effects, cx);
                        }
                    }));
                }
                Effect::Navigate { after_ms } => {
                    self.navigate = Some(Self::after(after_ms, cx, Self::leave));
                }
                Effect::RunGate { retry } => {
                    // Каждый вызов ждёт свой ответ, как `await` в `runGate` (повтор старый не
                    // отменяет).
                    let reply = self.services.invoke("boot-gate-run", gate_args(retry));
                    cx.spawn(async move |boot, cx| {
                        let answer = gate_answer(reply.await);
                        boot.update(cx, |boot, cx| {
                            let effects = boot.machine.gate_response(boot.now(cx), answer);
                            boot.run(effects, cx);
                        })
                        .ok();
                    })
                    .detach();
                }
            }
        }
        cx.notify();
    }

    /// `setTimeout(then, ms)`.
    fn after(
        ms: f64,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut BootScreen, &mut Context<BootScreen>) + 'static,
    ) -> Task<()> {
        let timer = cx
            .background_executor()
            .timer(Duration::from_secs_f64(ms / 1000.));
        cx.spawn(async move |boot, cx| {
            timer.await;
            boot.update(cx, then).ok();
        })
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        let effects = self.machine.retry();
        self.run(effects, cx);
    }

    /// Переход в лаунчер и `beforeUnmount`.
    fn leave(&mut self, cx: &mut Context<Self>) {
        self.mounted = false;
        self.failsafe = None;
        self.events = None;
        cx.emit(Leave);
    }

    fn tr(&self, key: &str) -> String {
        tr(self.language, key, &[])
    }

    /// `Logo` (`position: absolute`): кольцо 48 с рамкой 4 и точкой 20 (под курсором 26 за
    /// .2 с), «HLSR» 22/28 с разрядкой 6 px, «BETA» 10 px у его верха.
    fn logo(&self) -> impl IntoElement + use<> {
        div()
            .absolute()
            .top_0()
            .left_0()
            .p(u(32.))
            .flex()
            .items_center()
            .child(hover("boot-logo", Timing::new(200., EASE), |t| {
                div().size(u(48.)).flex_none().child(
                    crate::paint::logo_mark(4., 20., (20. + 6. * t) / 20., white()).size_full(),
                )
            }))
            .child(
                div()
                    .ml(u(16.))
                    .flex()
                    .items_center()
                    .child(spaced(
                        "HLSR",
                        22.,
                        28.,
                        FontWeight::BOLD,
                        6. / 22.,
                        white(),
                    ))
                    .child(div().self_start().mt(u(2.)).ml(u(2.)).child(spaced(
                        "BETA",
                        10.,
                        10.,
                        FontWeight::BOLD,
                        0.08,
                        white_a(0.5),
                    ))),
            )
    }

    /// `LoadingIndicator`: стрелки по центру окна, у каждой — мигание со своей задержкой.
    fn indicator(
        &mut self,
        view: (f32, f32),
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<impl IntoElement + use<>> {
        let scale = zoom(window) * window.scale_factor();
        let layout = triangle::layout((f64::from(view.0), f64::from(view.1)), f64::from(scale));
        let mut arrows = Vec::with_capacity(layout.len());
        for (at, arrow) in layout.iter().enumerate() {
            let arrow = arrow.snapped(f64::from(scale));
            let Some((image, [x, y, width, height])) = self.arrows.get(&arrow, scale, window, cx)
            else {
                continue;
            };
            let name = if arrow.slot.outer {
                "loading-arrow-outer"
            } else {
                "loading-arrow-inner"
            };
            let (left, top) = (arrow.x as f32 + x, arrow.y as f32 + y);
            let index = usize::try_from(arrow.slot.index).unwrap_or(0);
            arrows.push(
                keyframes(("boot-arrow", at), name, move |frame| {
                    img(image)
                        .absolute()
                        .left(u(left))
                        .top(u(top))
                        .w(u(width))
                        .h(u(height))
                        .object_fit(ObjectFit::Fill)
                        .opacity(frame.opacity as f32)
                })
                .index(index),
            );
        }
        arrows
    }

    /// `.boot-progress`: полоса 6 px (ширина — .25 с `ease`) и строка «проценты, размер, скорость».
    fn progress(
        &self,
        view: (f32, f32),
        percent: u32,
        meta: Vec<String>,
    ) -> impl IntoElement + use<> {
        let width = 420f32.min(view.0 * 0.7);
        div()
            .absolute()
            .top(u(view.1 / 2. + 170.))
            .left(u((view.0 - width) / 2.))
            .w(u(width))
            .flex()
            .flex_col()
            .gap(u(8.))
            .child(
                div()
                    .h(u(6.))
                    .rounded(u(3.))
                    .bg(white_a(0.12))
                    .overflow_hidden()
                    .child(tween(
                        "boot-progress",
                        percent as f32 / 100.,
                        Timing::new(250., EASE),
                        |share| div().h_full().w(relative(share)).rounded(u(3.)).bg(white()),
                    )),
            )
            .child(
                div().flex().justify_between().gap(u(12.)).children(
                    meta.into_iter().map(|text| {
                        spaced(text, 12., 20., FontWeight::NORMAL, 0.05, white_a(0.55))
                    }),
                ),
            )
    }

    /// `.boot-blocked`: причина, текст ошибки и «Повторить» (фон .08, под курсором .16 за .16 с;
    /// контура фокуса нет — `*:focus { outline: none }` в App.vue).
    fn blocked(
        &self,
        view: (f32, f32),
        error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let width = 520f32.min(view.0 * 0.8);
        let id = ElementId::from("boot-retry");
        let focus = focus_handle(&id, window, cx);
        let on_click = cx.listener(|boot, _: &ClickEvent, _, cx| boot.retry(cx));
        let label = self.tr("#L_B_RETRY");
        let button = hover("boot-retry-hover", Timing::new(160., EASE), move |t| {
            div()
                .id(id)
                .track_focus(&focus)
                .mt(u(6.))
                .px(u(26.))
                .py(u(9.))
                .border_1()
                .border_color(white_a(0.3))
                .rounded(u(8.))
                .bg(mix(white_a(0.08), white_a(0.16), t))
                .cursor_pointer()
                .text_px(13., 15.)
                .font_weight(FontWeight::BOLD)
                .button_text()
                .child(label)
                .on_click(on_click)
        });
        div()
            .absolute()
            .top(u(view.1 / 2. + 170.))
            .left(u((view.0 - width) / 2.))
            .w(u(width))
            .flex()
            .flex_col()
            .items_center()
            .gap(u(10.))
            .child(
                div()
                    .w_full()
                    .text_center()
                    .text_px(13.5, 20.25)
                    .text_color(rgb(0xffb3ba))
                    .child(self.tr("#L_B_BLOCKED")),
            )
            .children(error.map(|error| {
                div()
                    .w_full()
                    .text_center()
                    .text_px(12., 20.)
                    .text_color(white_a(0.45))
                    .child(error)
            }))
            .child(button)
    }
}

impl Render for BootScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = viewport(window);
        let middle = view.1 / 2.;
        let radius = Layers::radius(cx);
        let language = self.language;
        let texts = self.machine.texts(&|key, args| tr(language, key, args));
        let backdrop = if self.colored {
            self.backdrop.image(view, window, cx)
        } else {
            None
        };
        let arrows = self.indicator(view, window, cx);
        let status = texts.status;
        let meta: Vec<String> = [format!("{}%", texts.percent), texts.bytes, texts.speed]
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect();
        div()
            .id("boot")
            .size_full()
            .relative()
            .font_family(RUBIK)
            .text_color(white())
            .children(backdrop.map(|image| {
                img(image)
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Fill)
            }))
            .child(self.logo())
            .children(arrows)
            // .loading-status: середина строки 20 px на 50% + 130px; «дыхание» 2 с.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(u(middle + 120.))
                    .flex()
                    .justify_center()
                    .child(keyframes("boot-status", "loading-status", move |frame| {
                        div().opacity(frame.opacity as f32).child(spaced(
                            status,
                            16.,
                            20.,
                            FontWeight::MEDIUM,
                            0.1,
                            white(),
                        ))
                    })),
            )
            .when(texts.checking_hint, |root| {
                root.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(u(middle + 162.))
                        .flex()
                        .justify_center()
                        .child(spaced(
                            self.tr("#L_U_CHECKING"),
                            12.,
                            20.,
                            FontWeight::NORMAL,
                            0.08,
                            white_a(0.35),
                        )),
                )
            })
            .when(texts.downloading, |root| {
                root.child(self.progress(view, texts.percent, meta))
            })
            .when(texts.blocked, |root| {
                root.child(self.blocked(view, texts.error, window, cx))
            })
            // #app::after: контур окна; в полном экране нет.
            .when(radius > px(0.), |root| {
                root.child(crate::window::frame_border(radius))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::LauncherRoot;
    use gpui::TestAppContext;
    use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
    use serde_json::json;

    fn fixtures(gate: Value) -> Arc<FixtureServices> {
        let scene: Scene = serde_json::from_value(json!({
            "id": "t", "route": "/", "platform": "win32",
            "window": {"width": 1600, "height": 900},
            "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
            "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
            "store": {"settings": {"config": {"language": null}}},
            "invoke": {"boot-gate-run": gate, "window-command": {"ok": true, "fullscreen": false}}
        }))
        .unwrap();
        let variant = Variant {
            theme: "hlsr".into(),
            lang: "ru".into(),
            scale: 1.0,
            zoom: None,
            rail: None,
        };
        Arc::new(FixtureServices::new(&scene, &variant).unwrap())
    }

    fn sent(fx: &FixtureServices, kind: &str, channel: &str) -> Vec<Value> {
        fx.log()
            .into_iter()
            .filter(|entry| entry["kind"] == kind && entry["channel"] == channel)
            .map(|entry| entry["args"].clone())
            .collect()
    }

    #[gpui::test]
    fn proceeds_to_the_shell_not_before_the_minimum(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let fx = fixtures(json!({"action": "proceed"}));
        let services: Services = fx.clone();
        let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::new(services, cx));
        cx.run_until_parked();
        // preload.js: путь библиотеки; App.vue: `ready`, затем шлюз заставки.
        let channels: Vec<Value> = fx.log().iter().map(|e| e["channel"].clone()).collect();
        assert_eq!(
            channels,
            [
                json!("app-default-library-path"),
                json!("ready"),
                json!("boot-gate-run")
            ]
        );
        assert_eq!(sent(&fx, "invoke", "boot-gate-run"), [json!([{}])]);
        // Оболочка встаёт через 1 200 мс от монтирования заставки, не раньше.
        let shell_mounted = || !sent(&fx, "send", "support-thanks-ready").is_empty();
        cx.executor().advance_clock(Duration::from_millis(1100));
        cx.run_until_parked();
        assert!(!shell_mounted());
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        assert!(shell_mounted());
        assert_eq!(sent(&fx, "send", "ready").len(), 1);
    }

    #[gpui::test]
    fn blocked_waits_and_retries(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let fx = fixtures(json!({"action": "blocked", "error": "ECONNREFUSED"}));
        let services: Services = fx.clone();
        let (root, cx) = cx.add_window_view(|_, cx| LauncherRoot::new(services, cx));
        cx.run_until_parked();
        // Заблокировано: ни перехода, ни страховки.
        cx.executor().advance_clock(Duration::from_secs(25));
        cx.run_until_parked();
        assert!(sent(&fx, "send", "support-thanks-ready").is_empty());
        // «Повторить» — шлюз с retry; события `updater-state` идут в полосу загрузки.
        let boot = root.read_with(cx, |root, _| root.boot().unwrap());
        boot.update(cx, |boot, cx| boot.retry(cx));
        cx.run_until_parked();
        assert_eq!(
            sent(&fx, "invoke", "boot-gate-run"),
            [json!([{}]), json!([{"retry": true}])]
        );
        fx.emit(
            "updater-state",
            json!({"state": "downloading", "version": "0.4.10",
                   "progress": {"percent": 37, "transferred": 12582912, "total": 33554432, "bytesPerSecond": 1572864}}),
        );
        cx.run_until_parked();
        let texts = boot.read_with(cx, |boot, _| {
            boot.machine.texts(&|key, args| tr(Language::Ru, key, args))
        });
        assert_eq!(texts.bytes, "12,0\u{a0}МБ / 32,0\u{a0}МБ");
        assert_eq!(texts.speed, "1,5 МБ/с");
    }

    #[test]
    fn colored_backdrop_matches_css_corners_and_glow() {
        let image = colored_backdrop((160, 90), (1600., 900.));
        let at = |x: u32, y: u32| image.as_slice()[(y * 160 + x) as usize];
        // Угол сверху слева — начало линейного градиента 155° (#0d1f3c), свечения там нет.
        let corner = at(0, 0);
        assert!(
            corner.r.abs_diff(0x0d) <= 1 && corner.b.abs_diff(0x3c) <= 2,
            "{corner:?}"
        );
        // Середина свечения (50%, 46%): голубое поверх синего.
        let glow = at(80, 41);
        assert!(glow.b > 0x80 && glow.g > 0x50, "{glow:?}");
    }
}
