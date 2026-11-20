//! Параллакс фона игры (`LauncherBackground.vue:40-55, 160-330` 0.4.7): модель без Slint.
//!
//! Слой догоняет курсор критически демпфированной пружиной (`smoothDamp`, 0,32 с), по реальному
//! времени кадра, а не по доле за кадр. Передний план ходит вдвое дальше фона. Движение идёт, пока
//! окно в фокусе, не скрыто и над фоном не открыто другое окно. Цикл — аналог
//! `requestAnimationFrame`: [`Parallax::frame_pending`] говорит, нужен ли следующий кадр,
//! [`Parallax::frame`] — шаг на нём. Наклон переднего плана (1,3°) не переносится (решение 10
//! спецификации ПП5): `rotateX/Y` в Slint нет, сдвиг ×2 и увеличение фона остаются.
//!
//! Числа записываются так же, как их пишет 0.4.7 в CSS-переменные: `toFixed(2)` — поэтому
//! фикстура Node (`tools/reference/ui/parallax.js`, настоящий код компонента) сходится
//! до знака.

use hlsr_core::js::to_fixed;

/// Запас картинки за краями окна (`PARALLAX_OVERSCAN`).
pub const OVERSCAN: f64 = 0.05;
/// Увеличение фона при включённом параллаксе (`--l-bg-par-zoom`).
pub const ZOOM: f64 = 1.0 + OVERSCAN;
const EDGE_MARGIN: f64 = OVERSCAN / 2.0;
const AMPLITUDE_OF_EDGE_MARGIN: f64 = 0.72;
const AMPLITUDE: f64 = EDGE_MARGIN * AMPLITUDE_OF_EDGE_MARGIN;
const MIN_SAFE_WINDOW_PX: f64 = 240.0;
const MIN_AMPLITUDE_PX: f64 = MIN_SAFE_WINDOW_PX * EDGE_MARGIN;
/// Время, за которое слой почти догоняет курсор, с (`PARALLAX_SMOOTH_TIME`).
pub const SMOOTH_TIME: f64 = 0.32;
/// Самый большой шаг времени между кадрами, с (`PARALLAX_MAX_DT`).
pub const MAX_DT: f64 = 0.05;
/// Расстояние и скорость, меньше которых слой считается пришедшим (`PARALLAX_EPSILON`).
pub const EPSILON: f64 = 0.05;
/// Передний план ближе к зрителю и ходит вдвое дальше (`FRONT_DEPTH`).
pub const FRONT_DEPTH: f64 = 2.0;

/// Переменные, которые `LauncherBackground` пишет в стиль корня: `None` — ещё не записаны.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vars {
    /// `--l-bg-par-zoom`; нет — увеличения нет (параллакс выключен).
    pub zoom: Option<f64>,
    /// `--l-bg-par-x/y`, px.
    pub par: Option<(f64, f64)>,
    /// `--l-bg-front-x/y`, px.
    pub front: Option<(f64, f64)>,
}

/// Состояние параллакса одного фона.
#[derive(Debug, Clone)]
pub struct Parallax {
    // Свойства компонента.
    parallax: bool,
    paused: bool,
    reduced_motion: bool,
    // Состояние окна.
    window: (f64, f64),
    visible: bool,
    focused: bool,
    document_hidden: bool,
    // Пружина.
    frame_pending: bool,
    target: (f64, f64),
    current: (f64, f64),
    velocity: (f64, f64),
    last_frame: Option<f64>,
    amplitude: (f64, f64),
    vars: Vars,
}

impl Parallax {
    /// `created()`: окно `window` (логические px; размер корня и `innerWidth/Height`), фокус окна,
    /// `prefers-reduced-motion` ОС и свойство `parallax` (настройка «анимации»).
    pub fn new(
        window: (f64, f64),
        focused: bool,
        reduced_motion: bool,
        parallax: bool,
    ) -> Parallax {
        Parallax {
            parallax,
            paused: false,
            reduced_motion,
            window: (window.0.round(), window.1.round()),
            visible: true,
            focused,
            document_hidden: false,
            frame_pending: false,
            target: (0.0, 0.0),
            current: (0.0, 0.0),
            velocity: (0.0, 0.0),
            last_frame: None,
            amplitude: (MIN_AMPLITUDE_PX, MIN_AMPLITUDE_PX),
            vars: Vars {
                zoom: None,
                par: None,
                front: None,
            },
        }
    }

    /// `mounted()`: `applyParallaxMode`.
    pub fn mount(&mut self) {
        self.apply_mode();
    }

    /// Нужен ли кадр (`requestAnimationFrame` запрошен и не снят).
    pub fn frame_pending(&self) -> bool {
        self.frame_pending
    }

    /// Записанные переменные.
    pub fn vars(&self) -> Vars {
        self.vars
    }

    /// Амплитуда по x и y, px (`ampX`, `ampY`).
    pub fn amplitude(&self) -> (f64, f64) {
        self.amplitude
    }

    /// Размер окна, px (целые, как `clientWidth`).
    pub fn window_size(&self) -> (f64, f64) {
        self.window
    }

    /// Свойство `parallax` (настройка «анимации»).
    pub fn parallax_property(&self) -> bool {
        self.parallax
    }

    /// Свойство `paused` (поверх открыто окно).
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Окно в фокусе.
    pub fn focused(&self) -> bool {
        self.focused
    }

    /// Окно показано (`window-visibility`).
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Параллакс включён: свойство и не `prefers-reduced-motion`.
    pub fn enabled(&self) -> bool {
        self.parallax && !self.reduced_motion
    }

    fn tracking(&self) -> bool {
        self.focused && !self.paused
    }

    /// Свойство `parallax` изменилось (watcher).
    pub fn set_parallax(&mut self, on: bool) {
        self.parallax = on;
        self.apply_mode();
    }

    /// Свойство `paused` (поверх открыто окно): при открытии фон замирает (watcher).
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if paused {
            self.freeze();
        }
    }

    /// `prefers-reduced-motion` ОС: в 0.4.7 читается один раз при создании; здесь — тоже,
    /// метод нужен для запуска и тестов.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    /// `mousemove` окна, координаты — логические px окна (`clientX/Y`).
    pub fn pointer_moved(&mut self, x: f64, y: f64) {
        // Chromium присылает mousemove и неактивному окну, когда курсор проходит над ним: без
        // фокуса фон стоит на месте.
        if !self.enabled() || !self.tracking() {
            return;
        }
        let width = if self.window.0 == 0.0 {
            1.0
        } else {
            self.window.0
        };
        let height = if self.window.1 == 0.0 {
            1.0
        } else {
            self.window.1
        };
        let nx = (x / width) * 2.0 - 1.0;
        let ny = (y / height) * 2.0 - 1.0;
        self.target = (
            -clamp(nx, -1.0, 1.0) * self.amplitude.0,
            -clamp(ny, -1.0, 1.0) * self.amplitude.1,
        );
        self.start_loop();
    }

    /// `mouseleave` документа: цель — центр.
    pub fn pointer_left(&mut self) {
        if !self.tracking() {
            return;
        }
        self.target = (0.0, 0.0);
        self.start_loop();
    }

    /// `focus`/`blur` окна. Потеря фокуса замораживает фон там, где он был.
    pub fn focus(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.freeze();
        }
    }

    /// `window-visibility`: окно спрятано или показано.
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if visible {
            self.start_loop();
        } else {
            self.stop_loop();
        }
    }

    /// `visibilitychange` документа.
    pub fn set_document_hidden(&mut self, hidden: bool) {
        self.document_hidden = hidden;
        if hidden {
            self.stop_loop();
        } else {
            self.start_loop();
        }
    }

    /// `resize`: размер окна изменился.
    pub fn resized(&mut self, width: f64, height: f64) {
        self.window = (width.round(), height.round());
        if !self.enabled() {
            return;
        }
        self.measure();
        self.target = (
            clamp(self.target.0, -self.amplitude.0, self.amplitude.0),
            clamp(self.target.1, -self.amplitude.1, self.amplitude.1),
        );
        self.start_loop();
    }

    /// Шаг на кадре `requestAnimationFrame`; `now_ms` — время кадра (`DOMHighResTimeStamp`).
    pub fn frame(&mut self, now_ms: f64) {
        self.frame_pending = false;
        let last = self.last_frame;
        self.last_frame = Some(now_ms);
        // Первый кадр после паузы считается обычным, а не скачком за всё время простоя.
        let dt = match last {
            None => 1.0 / 60.0,
            Some(last) => ((now_ms - last) / 1000.0).clamp(0.0, MAX_DT),
        };

        let x = smooth_damp(self.current.0, self.target.0, self.velocity.0, dt);
        let y = smooth_damp(self.current.1, self.target.1, self.velocity.1, dt);
        self.current = (x.0, y.0);
        self.velocity = (x.1, y.1);

        let settled = (self.target.0 - self.current.0).abs() < EPSILON
            && (self.target.1 - self.current.1).abs() < EPSILON
            && self.velocity.0.abs() < EPSILON
            && self.velocity.1.abs() < EPSILON;
        if settled {
            self.current = self.target;
            self.velocity = (0.0, 0.0);
            self.last_frame = None;
            self.write();
            return;
        }
        self.write();
        self.start_loop();
    }

    /// `applyParallaxMode`.
    fn apply_mode(&mut self) {
        if !self.enabled() {
            self.stop_loop();
            self.target = (0.0, 0.0);
            self.current = (0.0, 0.0);
            self.velocity = (0.0, 0.0);
            self.vars = Vars {
                zoom: None,
                par: Some((0.0, 0.0)),
                front: Some((0.0, 0.0)),
            };
            return;
        }
        self.vars.zoom = Some(ZOOM);
        self.measure();
        self.start_loop();
    }

    fn measure(&mut self) {
        let (width, height) = self.window;
        self.amplitude = (
            f64::max(MIN_AMPLITUDE_PX, width * AMPLITUDE),
            f64::max(MIN_AMPLITUDE_PX, height * AMPLITUDE),
        );
    }

    fn start_loop(&mut self) {
        if self.frame_pending || !self.enabled() {
            return;
        }
        if !self.tracking() || !self.visible || self.document_hidden {
            return;
        }
        self.frame_pending = true;
    }

    fn stop_loop(&mut self) {
        self.last_frame = None;
        self.frame_pending = false;
    }

    /// Фон замирает там, где был, без возврата в центр, и потом снова следует за курсором с
    /// того же места.
    fn freeze(&mut self) {
        self.velocity = (0.0, 0.0);
        self.target = self.current;
        self.stop_loop();
    }

    /// `write()`: переменные как `toFixed(2)` 0.4.7.
    fn write(&mut self) {
        let fixed = |value: f64| -> f64 { to_fixed(value, 2).parse().unwrap_or(0.0) };
        self.vars.par = Some((fixed(self.current.0), fixed(self.current.1)));
        self.vars.front = Some((
            fixed(self.current.0 * FRONT_DEPTH),
            fixed(self.current.1 * FRONT_DEPTH),
        ));
    }
}

/// Критически демпфированная пружина (Game Programming Gems 4, как `Mathf.SmoothDamp`):
/// слой плавно разгоняется и плавно тормозит у цели, без перелёта. Возвращает значение и
/// скорость.
fn smooth_damp(current: f64, target: f64, velocity: f64, dt: f64) -> (f64, f64) {
    let omega = 2.0 / SMOOTH_TIME;
    let x = omega * dt;
    let decay = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let change = current - target;
    let temp = (velocity + omega * change) * dt;
    (
        target + (change + temp) * decay,
        (velocity - omega * temp) * decay,
    )
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}
