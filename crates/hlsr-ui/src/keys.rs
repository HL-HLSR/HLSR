//! Физические клавиши и режим захвата (карта §0.9, решение 6, риск 13).
//!
//! `KeyEvent` Slint не даёт физического кода клавиши, левого и правого модификатора, кнопок
//! мыши 4/5 и колеса как клавиш. Приложение перехватывает события winit
//! (`hlsr-app/src/winit_input.rs`), переводит их в [`RawInput`] с кодом UI Events (`event.code`
//! Chromium) и отдаёт [`KeyInput::handle`]: ответ — пропустить событие в окно или поглотить.
//!
//! Захват клавиши (`GameScripts.vue:436-482` 0.4.7): пока он идёт, нажатия клавиш в окно не
//! уходят (Esc только отменяет ожидание, Tab — обычная клавиша `TAB`); кнопка мыши и колесо над
//! кнопкой захвата — клавиши `MOUSE1…5`, `MWHEELUP/DOWN`; нажатие мыши вне кнопки отменяет
//! ожидание и уходит в окно как обычно. Имена — [`crate::keyboard_layout::key_from_event`].
//!
//! Последовательность «вверх, вверх, вниз, вниз, влево, вправо, влево, вправо, B, A»
//! (`LauncherShell.vue:380`, `:675-686`) — тоже по физическим клавишам, чтобы работала и в
//! русской раскладке.

use crate::keyboard_layout::{Engine, InputEvent, key_from_event};

/// Секретная последовательность (`SECRET_SEQUENCE`).
pub const SECRET_SEQUENCE: [&str; 10] = [
    "ArrowUp",
    "ArrowUp",
    "ArrowDown",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "ArrowLeft",
    "ArrowRight",
    "KeyB",
    "KeyA",
];

/// Событие ввода окна в форме DOM: коды UI Events, кнопки `MouseEvent.button`, `deltaY`
/// колеса (вниз — больше нуля); координаты — логические пиксели окна.
#[derive(Debug, Clone, PartialEq)]
pub enum RawInput {
    KeyDown {
        code: String,
        repeat: bool,
    },
    KeyUp {
        code: String,
    },
    MouseDown {
        button: i32,
        x: f64,
        y: f64,
    },
    MouseUp {
        button: i32,
    },
    Wheel {
        delta_y: f64,
        x: f64,
        y: f64,
    },
    /// Окно получило или потеряло фокус (`focus`/`blur` окна).
    Focus(bool),
}

/// Что делать с событием.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Окно получает событие как обычно.
    Pass,
    /// Событие поглощено (`preventDefault` + `stopImmediatePropagation`).
    Consume,
}

/// Прямоугольник в логических пикселях окна.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Area {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// Итог ожидания: кто ждал и что нажато (`None` — ожидание кончилось без клавиши: Esc,
/// щелчок вне кнопки, кнопка мыши или колесо, которых движок не знает).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureEnd {
    pub owner: String,
    pub key: Option<&'static str>,
}

#[derive(Debug, Clone)]
struct Capture {
    owner: String,
    engine: Engine,
    area: Area,
}

/// Ответ на событие.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handled {
    pub verdict: Verdict,
    /// Ожидание клавиши закончилось этим событием.
    pub capture: Option<CaptureEnd>,
    /// Набрана секретная последовательность.
    pub secret: bool,
}

impl Handled {
    fn pass() -> Handled {
        Handled {
            verdict: Verdict::Pass,
            capture: None,
            secret: false,
        }
    }

    fn consume(capture: Option<CaptureEnd>) -> Handled {
        Handled {
            verdict: Verdict::Consume,
            capture,
            secret: false,
        }
    }
}

/// Последние нажатия для секретной последовательности (`keySequence`).
#[derive(Debug, Clone, Default)]
pub struct SecretSequence {
    codes: Vec<String>,
}

impl SecretSequence {
    pub fn reset(&mut self) {
        self.codes.clear();
    }

    /// `keySequence.concat(code).slice(-N)`; набрана — сброс и `true`.
    pub fn push(&mut self, code: &str) -> bool {
        self.codes.push(code.to_owned());
        if self.codes.len() > SECRET_SEQUENCE.len() {
            let extra = self.codes.len() - SECRET_SEQUENCE.len();
            self.codes.drain(..extra);
        }
        if self.codes.iter().map(String::as_str).eq(SECRET_SEQUENCE) {
            self.codes.clear();
            return true;
        }
        false
    }
}

/// Захват клавиши и наблюдение за нажатиями. Живёт в потоке интерфейса.
#[derive(Debug, Default)]
pub struct KeyInput {
    capture: Option<Capture>,
    /// Нажатия, поглощённые захватом: их отпускание тоже не уходит в окно.
    swallowed: Vec<String>,
    secret: SecretSequence,
}

impl KeyInput {
    pub fn new() -> KeyInput {
        KeyInput::default()
    }

    /// Начать ожидание клавиши для `owner` (`startCapture`): прежнее ожидание отменяется без
    /// итога (кнопка, что ждала, просто гаснет — как `stopCapture` 0.4.7).
    pub fn start_capture(&mut self, owner: &str, engine: Engine, area: Area) {
        self.capture = Some(Capture {
            owner: owner.to_owned(),
            engine,
            area,
        });
    }

    /// Обновить область кнопки (раскладка сдвинулась).
    pub fn set_capture_area(&mut self, area: Area) {
        if let Some(capture) = &mut self.capture {
            capture.area = area;
        }
    }

    /// Прервать ожидание (`stopCapture`); вернуть, кто ждал.
    pub fn stop_capture(&mut self) -> Option<String> {
        self.capture.take().map(|capture| capture.owner)
    }

    /// Кто ждёт клавишу.
    pub fn capturing(&self) -> Option<&str> {
        self.capture.as_ref().map(|capture| capture.owner.as_str())
    }

    fn finish(&mut self, key: Option<&'static str>) -> Option<CaptureEnd> {
        self.capture.take().map(|capture| CaptureEnd {
            owner: capture.owner,
            key,
        })
    }

    /// Разобрать событие. `secret_blocked` — открыто окно поверх или фокус в поле ввода:
    /// последовательность сбрасывается (`onGlobalKeydown`).
    pub fn handle(&mut self, input: &RawInput, secret_blocked: bool) -> Handled {
        match input {
            RawInput::KeyDown { code, .. } => {
                if let Some(capture) = &self.capture {
                    let engine = capture.engine;
                    if !self.swallowed.contains(code) {
                        self.swallowed.push(code.clone());
                    }
                    // onCaptureKey: Esc только отменяет; незнакомая клавиша — ждать дальше.
                    if code == "Escape" {
                        return Handled::consume(self.finish(None));
                    }
                    let key = key_from_event(&InputEvent::Key { code }, engine);
                    let end = key.and_then(|key| self.finish(Some(key)));
                    return Handled::consume(end);
                }
                let mut handled = Handled::pass();
                if secret_blocked {
                    self.secret.reset();
                } else {
                    handled.secret = self.secret.push(code);
                }
                handled
            }
            RawInput::KeyUp { code } => {
                if let Some(at) = self.swallowed.iter().position(|known| known == code) {
                    self.swallowed.remove(at);
                    return Handled::consume(None);
                }
                Handled::pass()
            }
            RawInput::MouseDown { button, x, y } => {
                let Some(capture) = &self.capture else {
                    return Handled::pass();
                };
                // onCaptureMouse над кнопкой: кнопка мыши — клавиша (незнакомая — без итога);
                // onOutsideMouse: вне кнопки — отмена. Нажатие уходит в окно в обоих случаях
                // (preventDefault у mousedown не отменяет щелчка).
                let key = if capture.area.contains(*x, *y) {
                    key_from_event(&InputEvent::MouseButton { button: *button }, capture.engine)
                } else {
                    None
                };
                Handled {
                    verdict: Verdict::Pass,
                    capture: self.finish(key),
                    secret: false,
                }
            }
            RawInput::MouseUp { .. } => Handled::pass(),
            RawInput::Wheel { delta_y, x, y } => {
                let Some(capture) = &self.capture else {
                    return Handled::pass();
                };
                if !capture.area.contains(*x, *y) {
                    return Handled::pass();
                }
                // onCaptureWheel: колесо над кнопкой; без вертикального сдвига — без итога.
                let key = key_from_event(&InputEvent::Wheel { delta_y: *delta_y }, capture.engine);
                Handled::consume(self.finish(key))
            }
            RawInput::Focus(_) => Handled::pass(),
        }
    }
}

thread_local! {
    static LAST_DOWN: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Запомнить физическую клавишу нажатия (`KeyboardEvent.code`): перехват winit приложения видит
/// её раньше `KeyEvent` Slint, в котором кода нет.
pub fn note_key_down(code: &str) {
    LAST_DOWN.with(|last| code.clone_into(&mut last.borrow_mut()));
}

/// Физическая клавиша последнего нажатия в этом потоке; пусто — нажатий не было или их не
/// перехватывали (тестовый бэкенд, служебные окна).
pub fn last_key_down() -> String {
    LAST_DOWN.with(|last| last.borrow().clone())
}
