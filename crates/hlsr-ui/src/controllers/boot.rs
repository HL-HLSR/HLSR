//! Заставка (`views/Loading.vue` 0.4.9): шлюз загрузки `boot-gate-run`, события `updater-state`,
//! надписи и числа, повтор, страховка, переход в лаунчер.
//!
//! [`BootMachine`] — логика без UI и без часов: время приходит аргументом (мс), следующее
//! действие — [`Effect`]. Сверена с настоящим скриптом `Loading.vue`, выполненным в Node на
//! виртуальных часах (`tools/reference/ui/boot.js`, `tests/boot_machine.rs`). Привязка к окну,
//! таймерам и событиям — у вида (`hlsr-gpui` `shell/boot.rs`); здесь же разбор ответов шлюза и
//! событий и подписи размеров `file-format.js` ([`format_number`], [`format_size`]).

use hlsr_core::js::number::number_to_string;
use hlsr_core::js::to_fixed;

/// Заставка стоит не меньше (`MIN_SPLASH_MS`), мс.
pub const MIN_SPLASH_MS: f64 = 1200.0;
/// Если шлюз молчит, через столько заставка идёт дальше (`FAILSAFE_MS`), мс.
pub const FAILSAFE_MS: f64 = 20000.0;

/// Фаза заставки (`phase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Checking,
    Downloading,
    Verifying,
    Installing,
    Blocked,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Checking => "checking",
            Phase::Downloading => "downloading",
            Phase::Verifying => "verifying",
            Phase::Installing => "installing",
            Phase::Blocked => "blocked",
        }
    }
}

/// `action` ответа `boot-gate-run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAction {
    Proceed,
    Installing,
    Blocked,
    /// Любое другое: 0.4.7 идёт дальше.
    Other,
}

/// Ответ `boot-gate-run`: `{action, reason?, error?}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootDecision {
    pub action: GateAction,
    pub reason: Option<String>,
    pub error: Option<String>,
}

impl BootDecision {
    pub fn proceed() -> BootDecision {
        BootDecision {
            action: GateAction::Proceed,
            reason: None,
            error: None,
        }
    }
}

/// `status.progress` события `updater-state`; поля могут отсутствовать.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateProgress {
    pub percent: Option<f64>,
    pub transferred: Option<f64>,
    pub total: Option<f64>,
    pub bytes_per_second: Option<f64>,
}

/// Событие `updater-state` (то, что читает заставка).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdaterStatus {
    pub state: String,
    pub version: Option<String>,
    pub progress: Option<UpdateProgress>,
}

/// Что должен сделать вызывающий.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    /// `ipcRenderer.invoke("boot-gate-run", retry ? {retry: true} : {})`.
    RunGate { retry: bool },
    /// Запустить таймер страховки: через `after_ms` позвать [`BootMachine::failsafe`].
    ArmFailsafe { after_ms: f64 },
    /// `localStorage.setItem("wasLaunched", true)`.
    MarkLaunched,
    /// `setTimeout(() => router.push("launcher"), after_ms)`.
    Navigate { after_ms: f64 },
}

/// Надписи и числа заставки, как вычисляемые свойства `Loading.vue`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootTexts {
    /// `statusText`.
    pub status: String,
    /// `v-if="phase === 'checking' && !done"`: подпись «Проверка обновления лаунчера».
    pub checking_hint: bool,
    /// `v-if="phase === 'downloading'"`: полоса загрузки.
    pub downloading: bool,
    /// `progressPercent`, 0–100.
    pub percent: u32,
    /// `progressBytes`: «12,0 МБ / 32,0 МБ» или пусто.
    pub bytes: String,
    /// `progressSpeed`: «1,5 МБ/с» или пусто.
    pub speed: String,
    /// `v-if="phase === 'blocked'"`.
    pub blocked: bool,
    /// `bootError`.
    pub error: Option<String>,
}

/// Логика заставки.
#[derive(Debug, Clone)]
pub struct BootMachine {
    was_launched: bool,
    splash: Option<&'static str>,
    phase: Phase,
    update_version: Option<String>,
    progress: Option<UpdateProgress>,
    error: Option<String>,
    mounted_at: f64,
    done: bool,
}

/// JS-истинность числа: не `0`, не `NaN`, не `undefined`.
fn truthy(value: Option<f64>) -> Option<f64> {
    value.filter(|v| *v != 0.0 && !v.is_nan())
}

/// Переводчик (`$t`): ключ и аргументы `%0%`…
pub type Translate<'a> = &'a dyn Fn(&str, &[&str]) -> String;

/// `formatNumber(value, digits, t)` (`file-format.js` 0.4.9): `toFixed` с десятичным знаком
/// языка («1,5» / «1.5»).
pub fn format_number(value: f64, digits: usize, t: Translate) -> String {
    to_fixed(value, digits).replacen('.', &t("#L_DECIMAL", &[]), 1)
}

/// `formatSize(bytes, t)` (`file-format.js` 0.4.9): «24,7 МБ» / «24.7 MB», меньше КБ — целые
/// байты; между числом и единицей — неразрывный пробел.
pub fn format_size(bytes: f64, t: Translate) -> String {
    // `Number(bytes) || 0`: NaN и −0 — ноль.
    let value = truthy(Some(bytes)).unwrap_or(0.0);
    if value < 1024.0 {
        return format!("{}\u{a0}{}", number_to_string(value), t("#L_B", &[]));
    }
    let (size, unit) = if value < 1048576.0 {
        (value / 1024.0, "#L_KB")
    } else if value < 1073741824.0 {
        (value / 1048576.0, "#L_MB")
    } else {
        (value / 1073741824.0, "#L_GB")
    };
    format!("{}\u{a0}{}", format_number(size, 1, t), t(unit, &[]))
}

impl BootMachine {
    /// `data()`: заставка создана в момент `now_ms`; `was_launched` — было ли `wasLaunched`.
    pub fn new(was_launched: bool, now_ms: f64) -> BootMachine {
        BootMachine {
            was_launched,
            splash: None,
            phase: Phase::Checking,
            update_version: None,
            progress: None,
            error: None,
            mounted_at: now_ms,
            done: false,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn done(&self) -> bool {
        self.done
    }

    pub fn update_version(&self) -> Option<&str> {
        self.update_version.as_deref()
    }

    /// Ключ надписи «Добро пожаловать»; до `mounted()` его нет.
    pub fn splash_key(&self) -> Option<&'static str> {
        self.splash
    }

    /// `mounted()`: надпись, `wasLaunched`, страховка, первый вызов шлюза.
    pub fn mounted(&mut self, _now_ms: f64) -> Vec<Effect> {
        self.splash = Some(if self.was_launched {
            "#UI_WELCOME_BACK"
        } else {
            "#UI_WELCOME_HLSR"
        });
        vec![
            Effect::MarkLaunched,
            Effect::ArmFailsafe {
                after_ms: FAILSAFE_MS,
            },
            Effect::RunGate { retry: false },
        ]
    }

    /// Ответ `boot-gate-run`: `Ok(None)` — пустой ответ, `Err` — сбой канала
    /// (`{action: "proceed", reason: "ipc-error"}`).
    pub fn gate_response(
        &mut self,
        now_ms: f64,
        response: Result<Option<BootDecision>, ()>,
    ) -> Vec<Effect> {
        let decision = match response {
            Ok(decision) => decision,
            Err(()) => Some(BootDecision {
                action: GateAction::Proceed,
                reason: Some("ipc-error".to_owned()),
                error: None,
            }),
        };
        let Some(decision) = decision else {
            return self.proceed(now_ms);
        };
        match decision.action {
            GateAction::Installing => {
                self.phase = Phase::Installing;
                Vec::new()
            }
            GateAction::Blocked => {
                self.phase = Phase::Blocked;
                // `decision.error || null`: пустая строка — тоже «нет».
                self.error = decision.error.filter(|error| !error.is_empty());
                Vec::new()
            }
            GateAction::Proceed | GateAction::Other => self.proceed(now_ms),
        }
    }

    /// Событие `updater-state` (`onUpdaterState`).
    pub fn updater_state(&mut self, status: &UpdaterStatus) {
        if let Some(version) = status.version.as_ref().filter(|v| !v.is_empty()) {
            self.update_version = Some(version.clone());
        }
        match status.state.as_str() {
            "downloading" => {
                self.phase = Phase::Downloading;
                self.progress = status.progress.clone();
            }
            "verifying" => self.phase = Phase::Verifying,
            _ => {}
        }
    }

    /// Кнопка «Повторить».
    pub fn retry(&mut self) -> Vec<Effect> {
        self.phase = Phase::Checking;
        self.error = None;
        self.progress = None;
        vec![Effect::RunGate { retry: true }]
    }

    /// Сработала страховка: шлюз молчит.
    pub fn failsafe(&mut self, now_ms: f64) -> Vec<Effect> {
        if self.phase == Phase::Checking {
            return self.proceed(now_ms);
        }
        Vec::new()
    }

    /// `proceed()`: в лаунчер, но не раньше минимума от создания заставки.
    fn proceed(&mut self, now_ms: f64) -> Vec<Effect> {
        if self.done {
            return Vec::new();
        }
        self.done = true;
        let elapsed = now_ms - self.mounted_at;
        vec![Effect::Navigate {
            after_ms: f64::max(0.0, MIN_SPLASH_MS - elapsed),
        }]
    }

    /// Надписи и числа; `t(ключ, аргументы)` — переводчик (`$t`).
    pub fn texts(&self, t: &dyn Fn(&str, &[&str]) -> String) -> BootTexts {
        let status = match self.phase {
            Phase::Downloading => t(
                "#L_B_DOWNLOADING",
                &[self.update_version.as_deref().unwrap_or("")],
            ),
            Phase::Verifying => t("#L_B_VERIFYING", &[]),
            Phase::Installing => t("#L_B_INSTALLING_UPDATE", &[]),
            Phase::Blocked => t("#L_B_BLOCKED_TITLE", &[]),
            Phase::Checking => self.splash.map(|key| t(key, &[])).unwrap_or_default(),
        };
        let progress = self.progress.as_ref();
        // progressPercent: Math.max(0, Math.min(100, Math.round(percent))).
        let percent = progress
            .and_then(|p| truthy(p.percent))
            .map_or(0.0, |percent| js_round(percent).clamp(0.0, 100.0));
        // progressBytes: нет total — пусто; transferred || 0.
        let bytes = progress
            .and_then(|p| truthy(p.total).map(|total| (p, total)))
            .map(|(p, total)| {
                let transferred = truthy(p.transferred).unwrap_or(0.0);
                format!(
                    "{} / {}",
                    format_size(transferred, t),
                    format_size(total, t)
                )
            })
            .unwrap_or_default();
        // progressSpeed: мегабайты в секунду, formatNumber(…, 1).
        let speed = progress
            .and_then(|p| truthy(p.bytes_per_second))
            .map(|speed| {
                let megabytes = format_number(speed / (1024.0 * 1024.0), 1, t);
                t("#L_U_SPEED", &[&megabytes])
            })
            .unwrap_or_default();
        BootTexts {
            status,
            checking_hint: self.phase == Phase::Checking && !self.done,
            downloading: self.phase == Phase::Downloading,
            percent: percent as u32,
            bytes,
            speed,
            blocked: self.phase == Phase::Blocked,
            error: self.error.clone(),
        }
    }
}

/// `Math.round`: половина — вверх (к +∞).
fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

// ---- Ответы канала -----------------------------------------------------------

use crate::services::Reply;
use hlsr_core::js::truthy as js_truthy;
use serde_json::{Value, json};

/// Ответ шлюза: `Ok(None)` — пустой ответ, `Err` — сбой канала.
pub type GateAnswer = Result<Option<BootDecision>, ()>;

/// Аргументы `ipcRenderer.invoke("boot-gate-run", retry ? {retry: true} : {})`.
pub fn gate_args(retry: bool) -> Vec<Value> {
    vec![if retry {
        json!({"retry": true})
    } else {
        json!({})
    }]
}

/// Ответ канала → ответ шлюза: отказ — сбой канала (`ipc-error`), пустой — «идти дальше».
// Текст отказа `catch` в `runGate` не читает: `Err(())` — только «сбой».
#[allow(clippy::result_unit_err)]
pub fn gate_answer(reply: Reply) -> GateAnswer {
    let Ok(value) = reply else { return Err(()) };
    if !js_truthy(&value) {
        return Ok(None);
    }
    let action = match value.get("action").and_then(Value::as_str) {
        Some("installing") => GateAction::Installing,
        Some("blocked") => GateAction::Blocked,
        Some("proceed") => GateAction::Proceed,
        _ => GateAction::Other,
    };
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(Some(BootDecision {
        action,
        reason: text("reason"),
        error: text("error"),
    }))
}

/// Событие `updater-state`: то, что читает заставка; пустое — не обрабатывается
/// (`if (!status) return`).
pub fn updater_status(payload: &Value) -> Option<UpdaterStatus> {
    if !js_truthy(payload) {
        return None;
    }
    let number = |object: &Value, key: &str| object.get(key).and_then(Value::as_f64);
    Some(UpdaterStatus {
        state: payload
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        version: payload
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        progress: payload
            .get("progress")
            .filter(|progress| progress.is_object())
            .map(|progress| UpdateProgress {
                percent: number(progress, "percent"),
                transferred: number(progress, "transferred"),
                total: number(progress, "total"),
                bytes_per_second: number(progress, "bytesPerSecond"),
            }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(value: Value) -> GateAnswer {
        gate_answer(Ok(value))
    }

    #[test]
    fn gate_answers_follow_apply_decision() {
        // `if (!decision) return this.proceed()`: пустой ответ — идти дальше.
        for empty in [Value::Null, json!(false), json!(0), json!("")] {
            assert_eq!(answer(empty), Ok(None));
        }
        assert_eq!(
            answer(json!({"action": "proceed", "reason": "disabled"})),
            Ok(Some(BootDecision {
                action: GateAction::Proceed,
                reason: Some("disabled".into()),
                error: None,
            }))
        );
        assert_eq!(
            answer(json!({"action": "installing"}))
                .unwrap()
                .unwrap()
                .action,
            GateAction::Installing
        );
        let blocked = answer(json!({"action": "blocked", "error": "ECONNREFUSED"}))
            .unwrap()
            .unwrap();
        assert_eq!(blocked.action, GateAction::Blocked);
        assert_eq!(blocked.error.as_deref(), Some("ECONNREFUSED"));
        // Незнакомое действие и ответ не объектом — как «proceed» (`decision.action` не совпало).
        assert_eq!(
            answer(json!({"action": "later"})).unwrap().unwrap().action,
            GateAction::Other
        );
        assert_eq!(
            answer(json!("blocked")).unwrap().unwrap().action,
            GateAction::Other
        );
        // Отказ канала — сбой `ipc-error`.
        assert_eq!(gate_answer(Err("нет канала".into())), Err(()));
    }

    #[test]
    fn sizes_follow_file_format_js() {
        let ru = |key: &str, _: &[&str]| {
            match key {
                "#L_DECIMAL" => ",",
                "#L_B" => "Б",
                "#L_KB" => "КБ",
                "#L_MB" => "МБ",
                "#L_GB" => "ГБ",
                _ => key,
            }
            .to_owned()
        };
        let size = |bytes| format_size(bytes, &ru);
        assert_eq!(size(f64::NAN), "0\u{a0}Б");
        assert_eq!(size(-0.0), "0\u{a0}Б");
        assert_eq!(size(512.0), "512\u{a0}Б");
        assert_eq!(size(1023.5), "1023.5\u{a0}Б", "меньше КБ — число как в JS");
        assert_eq!(size(1024.0), "1,0\u{a0}КБ");
        assert_eq!(size(12582912.0), "12,0\u{a0}МБ");
        assert_eq!(size(1073741824.0 * 1.25), "1,3\u{a0}ГБ");
        assert_eq!(format_number(1.5, 1, &|_, _| ".".into()), "1.5");
        assert_eq!(gate_args(false), [json!({})]);
        assert_eq!(gate_args(true), [json!({"retry": true})]);
    }

    #[test]
    fn updater_events_keep_only_what_the_splash_reads() {
        assert_eq!(updater_status(&Value::Null), None, "if (!status) return");
        let status = updater_status(&json!({
            "state": "downloading",
            "version": "0.4.8",
            "progress": {"percent": 37, "transferred": 12582912, "total": 33554432, "bytesPerSecond": 1572864},
            "enabled": true,
        }))
        .unwrap();
        assert_eq!(status.state, "downloading");
        assert_eq!(status.version.as_deref(), Some("0.4.8"));
        let progress = status.progress.unwrap();
        assert_eq!(
            (
                progress.percent,
                progress.transferred,
                progress.total,
                progress.bytes_per_second
            ),
            (
                Some(37.0),
                Some(12582912.0),
                Some(33554432.0),
                Some(1572864.0)
            )
        );
        // `progress: null` и пропущенные поля — «нет».
        let idle = updater_status(&json!({"state": "idle", "progress": null})).unwrap();
        assert_eq!((idle.version, idle.progress), (None, None));
        let partial = updater_status(&json!({"state": "downloading", "progress": {"percent": 5}}))
            .unwrap()
            .progress
            .unwrap();
        assert_eq!((partial.percent, partial.total), (Some(5.0), None));
    }
}
