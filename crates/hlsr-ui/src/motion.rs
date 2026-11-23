//! Движение 0.4.7 (решение 8 спецификации ПП5, карта §5): кривые как у Chromium,
//! реестр переходов и ключевых кадров из `data/motion.json`
//! (`tools/reference/export-motion.js` выгружает его из CSS 0.4.7) и отсчёт
//! значений по времени для `global Motion` (`ui/components/motion.slint`).
//!
//! Ключевые кадры считаются как CSS Animations: прогресс 0→1 по времени,
//! задержка с «лесенкой» по индексу, повторы, `alternate`, заполнение; кривая
//! `animation-timing-function` — на каждом интервале между ключевыми кадрами
//! своя, по свойству (неявный кадр 0 % или 100 % — исходное значение).
//! Переходы `<transition>` Vue — по свойствам: время и кривая входа и ухода,
//! прерванный переход идёт от текущих значений.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Выгрузка движения 0.4.7.
pub const MOTION_JSON: &str = include_str!("../data/motion.json");

// ---- Кривые ----------------------------------------------------------------

const BEZIER_EPSILON: f64 = 1e-7;
const MAX_NEWTON_ITERATIONS: usize = 4;

/// `cubic-bezier(x1, y1, x2, y2)` — перенос `gfx::CubicBezier` Chromium
/// (Ньютон, затем деление пополам с точностью 1e-7; за пределами [0, 1] —
/// касательные в концах).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicBezier {
    ax: f64,
    bx: f64,
    cx: f64,
    ay: f64,
    by: f64,
    cy: f64,
    start_gradient: f64,
    end_gradient: f64,
    linear: bool,
}

impl CubicBezier {
    pub fn new([x1, y1, x2, y2]: [f64; 4]) -> CubicBezier {
        let cx = 3.0 * x1;
        let bx = 3.0 * (x2 - x1) - cx;
        let cy = 3.0 * y1;
        let by = 3.0 * (y2 - y1) - cy;
        let start_gradient = if x1 > 0.0 {
            y1 / x1
        } else if y1 == 0.0 && x2 > 0.0 {
            y2 / x2
        } else if y1 == 0.0 && y2 == 0.0 {
            1.0
        } else {
            0.0
        };
        let end_gradient = if x2 < 1.0 {
            (y2 - 1.0) / (x2 - 1.0)
        } else if y2 == 1.0 && x1 < 1.0 {
            (y1 - 1.0) / (x1 - 1.0)
        } else if y2 == 1.0 && y1 == 1.0 {
            1.0
        } else {
            0.0
        };
        CubicBezier {
            ax: 1.0 - cx - bx,
            bx,
            cx,
            ay: 1.0 - cy - by,
            by,
            cy,
            start_gradient,
            end_gradient,
            linear: x1 == y1 && x2 == y2,
        }
    }

    fn sample_x(&self, t: f64) -> f64 {
        ((self.ax * t + self.bx) * t + self.cx) * t
    }

    fn sample_y(&self, t: f64) -> f64 {
        ((self.ay * t + self.by) * t + self.cy) * t
    }

    fn sample_dx(&self, t: f64) -> f64 {
        (3.0 * self.ax * t + 2.0 * self.bx) * t + self.cx
    }

    fn solve_t(&self, x: f64) -> f64 {
        let mut t2 = x;
        let mut x2 = 0.0;
        for _ in 0..MAX_NEWTON_ITERATIONS {
            x2 = self.sample_x(t2) - x;
            if x2.abs() < BEZIER_EPSILON {
                return t2;
            }
            let d2 = self.sample_dx(t2);
            if d2.abs() < BEZIER_EPSILON {
                break;
            }
            t2 -= x2 / d2;
        }
        if x2.abs() < BEZIER_EPSILON {
            return t2;
        }
        let (mut t0, mut t1) = (0.0, 1.0);
        while t0 < t1 {
            x2 = self.sample_x(t2);
            if (x2 - x).abs() < BEZIER_EPSILON {
                return t2;
            }
            if x > x2 {
                t0 = t2;
            } else {
                t1 = t2;
            }
            let next = (t1 + t0) * 0.5;
            if next == t2 {
                break;
            }
            t2 = next;
        }
        t2
    }

    /// Значение кривой при доле времени `x`.
    pub fn solve(&self, x: f64) -> f64 {
        if x < 0.0 {
            return self.start_gradient * x;
        }
        if x > 1.0 {
            return 1.0 + self.end_gradient * (x - 1.0);
        }
        if self.linear || x == 0.0 || x == 1.0 {
            return x;
        }
        self.sample_y(self.solve_t(x))
    }

    /// Скорость изменения значения относительно доли времени.
    pub fn slope(&self, x: f64) -> f64 {
        if self.linear {
            return 1.;
        }
        if x <= 0. {
            return self.start_gradient;
        }
        if x >= 1. {
            return self.end_gradient;
        }
        let t = self.solve_t(x);
        let dy = (3. * self.ay * t + 2. * self.by) * t + self.cy;
        dy / self.sample_dx(t)
    }
}

// ---- Кадр ------------------------------------------------------------------

/// Свойство кадра движения (`MotionFrame` в Slint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Prop {
    Opacity,
    /// translateX, px.
    X,
    /// translateY, px.
    Y,
    /// translateX в долях (100 % + `--l-swipe-gap`): ±1 — за краем (`l-slide-*`).
    Slide,
    Scale,
    ScaleX,
    /// Градусы.
    Rotation,
    /// Доля цвета состояния (цвет ключевого кадра, затемнение `l-modal`).
    Mix,
    /// Доля высоты (`toast-out`).
    Height,
    /// px.
    MinHeight,
    /// Поля по вертикали, px.
    Padding,
    /// Нижний отступ, px.
    Margin,
    /// `stroke-dashoffset`.
    Dash,
    /// `background-position` в долях (`nick-shift`).
    Shift,
    /// `hue-rotate`, градусы.
    Hue,
    Brightness,
    /// `--frame-angle`, градусы.
    Angle,
}

/// Значения кадра. Чего анимация не задаёт — исходное значение ([`Frame::REST`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub opacity: f64,
    pub x: f64,
    pub y: f64,
    pub slide: f64,
    pub scale: f64,
    pub scale_x: f64,
    pub rotation: f64,
    pub mix: f64,
    pub height: f64,
    pub min_height: f64,
    pub padding: f64,
    pub margin: f64,
    pub dash: f64,
    pub shift: f64,
    pub hue: f64,
    pub brightness: f64,
    pub angle: f64,
}

impl Frame {
    /// Исходные значения CSS: непрозрачно, без сдвигов, масштаб 1.
    pub const REST: Frame = Frame {
        opacity: 1.0,
        x: 0.0,
        y: 0.0,
        slide: 0.0,
        scale: 1.0,
        scale_x: 1.0,
        rotation: 0.0,
        mix: 0.0,
        height: 1.0,
        min_height: 0.0,
        padding: 0.0,
        margin: 0.0,
        dash: 0.0,
        shift: 0.0,
        hue: 0.0,
        brightness: 1.0,
        angle: 0.0,
    };

    pub fn get(&self, prop: Prop) -> f64 {
        *self.slot(prop)
    }

    pub fn set(&mut self, prop: Prop, value: f64) {
        *self.slot_mut(prop) = value;
    }

    fn slot(&self, prop: Prop) -> &f64 {
        match prop {
            Prop::Opacity => &self.opacity,
            Prop::X => &self.x,
            Prop::Y => &self.y,
            Prop::Slide => &self.slide,
            Prop::Scale => &self.scale,
            Prop::ScaleX => &self.scale_x,
            Prop::Rotation => &self.rotation,
            Prop::Mix => &self.mix,
            Prop::Height => &self.height,
            Prop::MinHeight => &self.min_height,
            Prop::Padding => &self.padding,
            Prop::Margin => &self.margin,
            Prop::Dash => &self.dash,
            Prop::Shift => &self.shift,
            Prop::Hue => &self.hue,
            Prop::Brightness => &self.brightness,
            Prop::Angle => &self.angle,
        }
    }

    fn slot_mut(&mut self, prop: Prop) -> &mut f64 {
        match prop {
            Prop::Opacity => &mut self.opacity,
            Prop::X => &mut self.x,
            Prop::Y => &mut self.y,
            Prop::Slide => &mut self.slide,
            Prop::Scale => &mut self.scale,
            Prop::ScaleX => &mut self.scale_x,
            Prop::Rotation => &mut self.rotation,
            Prop::Mix => &mut self.mix,
            Prop::Height => &mut self.height,
            Prop::MinHeight => &mut self.min_height,
            Prop::Padding => &mut self.padding,
            Prop::Margin => &mut self.margin,
            Prop::Dash => &mut self.dash,
            Prop::Shift => &mut self.shift,
            Prop::Hue => &mut self.hue,
            Prop::Brightness => &mut self.brightness,
            Prop::Angle => &mut self.angle,
        }
    }

    fn with(mut self, values: &BTreeMap<Prop, f64>) -> Frame {
        for (prop, value) in values {
            self.set(*prop, *value);
        }
        self
    }
}

// ---- Реестр ----------------------------------------------------------------

/// Токены движения `.launcher-root` (launcher.css:137-161): мс, px, кривые.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Tokens {
    pub l_ease: [f64; 4],
    pub l_ease_in: [f64; 4],
    pub l_ease_spring: [f64; 4],
    pub l_ease_fade: [f64; 4],
    pub l_ease_swipe: [f64; 4],
    pub l_dur_fast: f64,
    pub l_dur: f64,
    pub l_dur_slow: f64,
    pub l_dur_fade: f64,
    pub l_dur_swipe: f64,
    pub l_dur_tab: f64,
    pub l_ink_lead: f64,
    pub l_ink_trail: f64,
    pub l_rise: f64,
    pub l_swipe_gap: f64,
}

/// `@keyframes`: дорожки по свойствам, `[доля, значение]`; `None` — исходное значение.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keyframes {
    pub source: String,
    pub tracks: BTreeMap<Prop, Vec<(f64, Option<f64>)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stagger {
    /// Шаг задержки на индекс, мс.
    pub step: f64,
    /// `min(var(--i), max)`.
    pub max: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Normal,
    Alternate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fill {
    None,
    Forwards,
    Backwards,
    Both,
}

impl Fill {
    fn backwards(self) -> bool {
        matches!(self, Fill::Backwards | Fill::Both)
    }

    fn forwards(self) -> bool {
        matches!(self, Fill::Forwards | Fill::Both)
    }
}

/// Число повторов или `infinite`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Iterations {
    Count(f64),
    Infinite,
}

impl<'de> Deserialize<'de> for Iterations {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::String(text) if text == "infinite" => Ok(Iterations::Infinite),
            serde_json::Value::Number(n) => n
                .as_f64()
                .map(Iterations::Count)
                .ok_or_else(|| serde::de::Error::custom("iterations: не число")),
            other => Err(serde::de::Error::custom(format!("iterations: {other}"))),
        }
    }
}

/// Поведение без анимаций (`.no-anim` и `prefers-reduced-motion`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StillKind {
    /// Общее правило launcher.css: длительность 0,01 мс, задержка остаётся.
    Instant,
    /// `animation: none` — исходные значения.
    None,
    /// `animation: none` и конечное значение в том же правиле.
    End,
    /// Правила не действуют (вне `.launcher-root`).
    Ignore,
    /// Другая анимация реестра (`fa-pulse` вместо вращения).
    Swap(String),
}

impl<'de> Deserialize<'de> for StillKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Ok(match text.as_str() {
            "instant" => StillKind::Instant,
            "none" => StillKind::None,
            "end" => StillKind::End,
            "ignore" => StillKind::Ignore,
            _ => StillKind::Swap(text),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StillDef {
    /// `.no-anim`: prefs `animations` выключены или программный рендерер.
    pub prefs: StillKind,
    /// `prefers-reduced-motion` ОС.
    pub os: StillKind,
}

/// Место, где играют ключевые кадры (`animation: …` в 0.4.7).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Animation {
    pub source: String,
    pub selector: String,
    pub keyframes: String,
    /// мс.
    pub duration: f64,
    pub easing: [f64; 4],
    /// мс; бывает отрицательной (фаза).
    pub delay: f64,
    pub stagger: Option<Stagger>,
    pub iterations: Iterations,
    pub direction: Direction,
    pub fill: Fill,
    /// Исходные значения свойств у этого элемента (не умолчания [`Frame::REST`]).
    pub underlying: Option<BTreeMap<Prop, f64>>,
    pub still: StillDef,
}

/// Время свойств перехода.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    /// Свойство CSS (`opacity`, `transform`, `background-color`).
    pub property: String,
    pub props: Vec<Prop>,
    pub duration: f64,
    pub easing: [f64; 4],
    pub delay: f64,
}

/// Переход `<transition name="…">`: `hidden` — enter-from, `leave-to` — конец ухода.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub source: String,
    pub shown: BTreeMap<Prop, f64>,
    pub hidden: BTreeMap<Prop, f64>,
    #[serde(rename = "leave-to")]
    pub leave_to: BTreeMap<Prop, f64>,
    pub enter: Vec<Timing>,
    pub leave: Vec<Timing>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub duration: f64,
    pub easing: [f64; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InkEdges {
    pub left: Edge,
    pub right: Edge,
}

/// `.l-ink` (launcher.css:556-571): края подсветки вкладок; `still` — без перехода.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ink {
    pub default: InkEdges,
    pub next: InkEdges,
    pub prev: InkEdges,
    pub still: Option<InkEdges>,
}

/// Кроссфейд смены темы (`::view-transition-*`).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeFade {
    pub duration: f64,
    pub easing: [f64; 4],
}

/// `data/motion.json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub tokens: Tokens,
    pub keyframes: BTreeMap<String, Keyframes>,
    pub animations: BTreeMap<String, Animation>,
    pub transitions: BTreeMap<String, Transition>,
    pub ink: Ink,
    pub theme: ThemeFade,
}

/// Реестр движения (разбирается один раз).
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        serde_json::from_str(MOTION_JSON).expect("data/motion.json: выгрузка export-motion.js")
    })
}

// ---- Отсчёт ----------------------------------------------------------------

/// Что выключает движение: `.no-anim` (prefs `animations`, программный
/// рендерер) и `prefers-reduced-motion` ОС.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Still {
    pub no_anim: bool,
    pub os_reduced: bool,
}

impl Still {
    pub const MOVING: Still = Still {
        no_anim: false,
        os_reduced: false,
    };

    pub fn any(self) -> bool {
        self.no_anim || self.os_reduced
    }
}

/// Время от начала анимации, мс. `None` — «в конце»: переход и разовая
/// анимация завершены, бесконечная — в t = 0 (снимки `Motion.frozen`).
pub type Elapsed = Option<f64>;

/// Что делать с анимацией в режиме `still`.
fn still_kind(def: &Animation, still: Still) -> Option<&StillKind> {
    if still.os_reduced {
        Some(&def.still.os)
    } else if still.no_anim {
        Some(&def.still.prefs)
    } else {
        None
    }
}

/// Как играет анимация при этом режиме.
enum Plan<'a> {
    /// Исходные значения, без движения.
    Rest,
    /// Конечные значения.
    End,
    /// Отсчёт по времени (`instant` — длительность 0).
    Run { def: &'a Animation, instant: bool },
}

impl Registry {
    pub fn animation(&self, id: &str) -> Option<&Animation> {
        self.animations.get(id)
    }

    fn plan<'a>(&'a self, def: &'a Animation, still: Still) -> Plan<'a> {
        match still_kind(def, still) {
            None | Some(StillKind::Ignore) => Plan::Run {
                def,
                instant: false,
            },
            Some(StillKind::None) => Plan::Rest,
            Some(StillKind::End) => Plan::End,
            // Бесконечная анимация за 0,01 мс в 0.4.7 мелькает; не повторяем —
            // элемент стоит (отступление, отчёт G5).
            Some(StillKind::Instant) if def.iterations == Iterations::Infinite => Plan::Rest,
            Some(StillKind::Instant) => Plan::Run { def, instant: true },
            Some(StillKind::Swap(other)) => match self.animations.get(other.as_str()) {
                Some(swapped) => Plan::Run {
                    def: swapped,
                    instant: false,
                },
                None => Plan::Rest,
            },
        }
    }

    /// Кадр анимации `id` (`ANIMATIONS` export-motion.js). `playing: false` —
    /// анимации нет (класс снят): исходные значения. Незнакомый id — исходные.
    pub fn keyframes_frame(
        &self,
        id: &str,
        elapsed: Elapsed,
        index: i32,
        still: Still,
        playing: bool,
    ) -> Frame {
        let Some(def) = self.animations.get(id) else {
            return Frame::REST;
        };
        let rest = rest_of(def);
        if !playing {
            return rest;
        }
        match self.plan(def, still) {
            Plan::Rest => rest,
            Plan::End => self.sample(def, Some(f64::INFINITY), index, false, true),
            Plan::Run { def, instant } => self.sample(def, elapsed, index, instant, false),
        }
    }

    /// Длительность до конца, мс: `Some` — разовая; `None` — бесконечная или
    /// без движения в этом режиме (см. [`Registry::keyframes_moving`]).
    pub fn keyframes_length(&self, id: &str, index: i32, still: Still) -> Option<f64> {
        let def = self.animations.get(id)?;
        match self.plan(def, still) {
            Plan::Rest | Plan::End => Some(0.0),
            Plan::Run { def, instant } => match def.iterations {
                Iterations::Infinite => None,
                Iterations::Count(count) => {
                    let duration = if instant { 0.0 } else { def.duration };
                    Some((delay_of(def, index) + duration * count).max(0.0))
                }
            },
        }
    }

    /// Цикл бесконечной анимации `id` в этом режиме, мс: `(начало, длина)` — с начала кадр в
    /// момент `t` и `t + длина` один и тот же (`alternate` — туда и обратно). `None` — анимация
    /// не бесконечная или стоит.
    pub fn keyframes_cycle(&self, id: &str, still: Still) -> Option<(f64, f64)> {
        let def = self.animations.get(id)?;
        match self.plan(def, still) {
            Plan::Run {
                def,
                instant: false,
            } if def.iterations == Iterations::Infinite && def.duration > 0.0 => {
                let turns = match def.direction {
                    Direction::Normal => 1.0,
                    Direction::Alternate => 2.0,
                };
                Some((delay_of(def, 0).max(0.0), def.duration * turns))
            }
            _ => None,
        }
    }

    /// Анимация `id` в этом режиме меняется со временем.
    pub fn keyframes_moving(&self, id: &str, still: Still) -> bool {
        self.animations
            .get(id)
            .is_some_and(|def| matches!(self.plan(def, still), Plan::Run { .. }))
    }

    fn sample(
        &self,
        def: &Animation,
        elapsed: Elapsed,
        index: i32,
        instant: bool,
        end: bool,
    ) -> Frame {
        let rest = rest_of(def);
        let Some(keyframes) = self.keyframes.get(def.keyframes.as_str()) else {
            return rest;
        };
        let duration = if instant { 0.0 } else { def.duration };
        let delay = delay_of(def, index);
        let infinite = def.iterations == Iterations::Infinite;
        let time = match elapsed {
            Some(t) => t,
            None if infinite => 0.0,
            None => f64::INFINITY,
        };
        let Some(progress) = directed_progress(def, duration, delay, time, end) else {
            return rest;
        };
        let curve = CubicBezier::new(def.easing);
        let mut frame = rest;
        for (prop, stops) in &keyframes.tracks {
            let underlying = rest.get(*prop);
            frame.set(*prop, interpolate(stops, progress, underlying, &curve));
        }
        frame
    }

    /// Кадр перехода `name`: вход (`entering`) или уход в момент `elapsed`.
    /// `origin` — значения в момент смены фазы, когда прерван идущий переход.
    pub fn transition_frame(
        &self,
        name: &str,
        entering: bool,
        elapsed: Elapsed,
        still: Still,
        origin: Option<Frame>,
    ) -> Frame {
        let Some(def) = self.transitions.get(name) else {
            return Frame::REST;
        };
        let shown = Frame::REST.with(&def.shown);
        let (canonical, to, timings) = if entering {
            (Frame::REST.with(&def.hidden), shown, &def.enter)
        } else {
            (shown, Frame::REST.with(&def.leave_to), &def.leave)
        };
        let from = origin.unwrap_or(canonical);
        // Свойство без перехода сразу встаёт в новое значение.
        let mut frame = to;
        for timing in timings {
            let duration = if still.any() { 0.0 } else { timing.duration };
            let progress = match elapsed {
                None => 1.0,
                Some(t) if t < timing.delay => 0.0,
                Some(_) if duration <= 0.0 => 1.0,
                Some(t) => ((t - timing.delay) / duration).clamp(0.0, 1.0),
            };
            let eased = CubicBezier::new(timing.easing).solve(progress);
            for prop in &timing.props {
                frame.set(*prop, lerp(from.get(*prop), to.get(*prop), eased));
            }
        }
        frame
    }

    /// Длительность фазы перехода, мс (с задержкой).
    pub fn transition_length(&self, name: &str, entering: bool, still: Still) -> f64 {
        let Some(def) = self.transitions.get(name) else {
            return 0.0;
        };
        let timings = if entering { &def.enter } else { &def.leave };
        timings
            .iter()
            .map(|t| t.delay + if still.any() { 0.0 } else { t.duration })
            .fold(0.0, f64::max)
    }

    /// Непрозрачность снимка старой темы поверх новой (1 → 0).
    pub fn theme_fade_opacity(&self, elapsed: Elapsed) -> f64 {
        let progress = match elapsed {
            None => 1.0,
            Some(t) => (t / self.theme.duration).clamp(0.0, 1.0),
        };
        1.0 - CubicBezier::new(self.theme.easing).solve(progress)
    }
}

fn rest_of(def: &Animation) -> Frame {
    match &def.underlying {
        Some(values) => Frame::REST.with(values),
        None => Frame::REST,
    }
}

/// `animation-delay` с «лесенкой»: `base + step × min(i, max)`.
fn delay_of(def: &Animation, index: i32) -> f64 {
    let step = match def.stagger {
        Some(Stagger { step, max }) => {
            let i = index.max(0) as u32;
            step * f64::from(max.map_or(i, |max| i.min(max)))
        }
        None => 0.0,
    };
    def.delay + step
}

/// Прогресс итерации с учётом направления (Web Animations, «directed progress»);
/// `None` — эффекта нет (вне активной фазы без заполнения).
fn directed_progress(
    def: &Animation,
    duration: f64,
    delay: f64,
    time: f64,
    end: bool,
) -> Option<f64> {
    let count = match def.iterations {
        Iterations::Count(count) => count,
        Iterations::Infinite => f64::INFINITY,
    };
    let active = duration * count;
    let (iteration, progress) = if end {
        end_point(count)
    } else if time < delay {
        if !def.fill.backwards() {
            return None;
        }
        (0.0, 0.0)
    } else if active.is_finite() && time - delay >= active {
        if !def.fill.forwards() {
            return None;
        }
        end_point(count)
    } else if duration <= 0.0 {
        (0.0, 0.0)
    } else {
        let overall = (time - delay) / duration;
        let iteration = overall.floor();
        (iteration, overall - iteration)
    };
    let reversed = def.direction == Direction::Alternate && iteration.rem_euclid(2.0) == 1.0;
    Some(if reversed { 1.0 - progress } else { progress })
}

/// Конец активной фазы: последняя итерация в точке 1 (целое число повторов).
fn end_point(count: f64) -> (f64, f64) {
    if count.fract() == 0.0 {
        ((count - 1.0).max(0.0), 1.0)
    } else {
        (count.floor(), count.fract())
    }
}

/// Значение дорожки в точке `progress` с кривой на каждом интервале.
fn interpolate(
    stops: &[(f64, Option<f64>)],
    progress: f64,
    underlying: f64,
    curve: &CubicBezier,
) -> f64 {
    let value = |stop: &(f64, Option<f64>)| stop.1.unwrap_or(underlying);
    let Some(last) = stops.last() else {
        return underlying;
    };
    if stops.len() == 1 {
        return value(last);
    }
    let mut index = stops.len() - 2;
    for (i, pair) in stops.windows(2).enumerate() {
        if progress < pair[1].0 {
            index = i;
            break;
        }
    }
    let (a, b) = (&stops[index], &stops[index + 1]);
    let span = b.0 - a.0;
    let local = if span > 0.0 {
        (progress - a.0) / span
    } else {
        1.0
    };
    lerp(value(a), value(b), curve.solve(local))
}

/// a → b; концы — точно (без ошибки округления).
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    if t == 0.0 {
        a
    } else if t == 1.0 {
        b
    } else {
        a + (b - a) * t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_parses() {
        let registry = registry();
        assert!(registry.animations.contains_key("toast-out"));
        assert!(registry.transitions.contains_key("l-pop"));
    }

    #[test]
    fn linear_and_endpoints() {
        let linear = CubicBezier::new([0.0, 0.0, 1.0, 1.0]);
        assert_eq!(linear.solve(0.37), 0.37);
        let ease = CubicBezier::new([0.25, 0.1, 0.25, 1.0]);
        assert_eq!(ease.solve(0.0), 0.0);
        assert!((ease.solve(1.0) - 1.0).abs() < 1e-9);
    }
}
