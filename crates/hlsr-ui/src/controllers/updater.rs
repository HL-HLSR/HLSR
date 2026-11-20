//! Окно апдейтера (`src/updater/window/updater.js` 0.4.7): страница по событиям
//! `updater:init`/`updater:stage`/`updater:progress` и единственная кнопка («Отменить» во время
//! загрузки, «Закрыть» после ошибки и отмены). Ядро — `hlsr_update::updater::mode::UpdaterMain`;
//! здесь — только то, что видно: заголовок, версия, стадия, полоса, байты и скорость, пояснение,
//! код ошибки. Строки — `updater.*` (`src/updater/locales.js`).

use crate::tr;
use hlsr_core::js;
use hlsr_core::language::Language;
use serde_json::Value;

/// Класс полосы (`upd__fill` + `indeterminate`/`done`/`failed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillClass {
    #[default]
    Plain,
    /// Бегущий отрезок 35 % (без анимаций — вся ширина).
    Indeterminate,
    /// Зелёная, 100 %.
    Done,
    /// Красная, 100 %.
    Failed,
}

/// Полоса: класс и ширина из `style.width` (она сильнее ширины класса).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Fill {
    pub class: FillClass,
    /// `el.fill.style.width` в процентах (`percent.toFixed(1)`); `None` — не задана.
    pub inline_width: Option<f32>,
}

impl Fill {
    /// Ширина в процентах, если полоса не бегущая (у бегущей — 35 % и движение).
    pub fn width(&self) -> f32 {
        if let Some(width) = self.inline_width {
            return width;
        }
        match self.class {
            FillClass::Plain => 0.0,
            FillClass::Indeterminate => 35.0,
            FillClass::Done | FillClass::Failed => 100.0,
        }
    }

    pub fn running(&self) -> bool {
        self.class == FillClass::Indeterminate
    }
}

/// Кнопка (`actionMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdaterAction {
    Cancel,
    Close,
}

/// Что отправить ядру по нажатию (`updater:cancel` / `updater:close`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdaterCommand {
    Cancel,
    Close,
}

/// Что видно в окне.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdaterView {
    pub language: Language,
    pub title: String,
    pub subtitle: String,
    pub stage: String,
    pub fill: Fill,
    pub bytes: String,
    pub speed: String,
    pub note: String,
    pub code: String,
    /// Кнопка видна (`hidden` снят). До `updater:init` кнопка видна и пуста, как в разметке.
    pub action_shown: bool,
    pub action_label: String,
}

/// Страница окна апдейтера.
#[derive(Debug, Clone)]
pub struct UpdaterPage {
    /// Строки пришли (`updater:init`): до этого `t()` даёт пустую строку.
    strings: Option<Language>,
    view: UpdaterView,
    action: Option<UpdaterAction>,
}

impl Default for UpdaterPage {
    fn default() -> Self {
        UpdaterPage::new()
    }
}

/// `formatBytes(value)` окна апдейтера.
pub fn format_bytes(value: f64) -> String {
    let bytes = if value.is_nan() { 0.0 } else { value };
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    if bytes >= GB {
        format!("{} GB", js::to_fixed(bytes / GB, 2))
    } else if bytes >= MB {
        format!("{} MB", js::to_fixed(bytes / MB, 1))
    } else if bytes >= KB {
        format!("{} KB", js::to_fixed(bytes / KB, 0))
    } else {
        format!("{} B", js::number::number_to_string(bytes))
    }
}

/// `Number(value) || 0`.
fn number_or_zero(value: Option<&Value>) -> f64 {
    let number = js::value::to_number(value);
    if number.is_nan() { 0.0 } else { number }
}

impl UpdaterPage {
    /// Страница до `updater:init`: тексты разметки (`index.html`), пустая кнопка.
    pub fn new() -> UpdaterPage {
        UpdaterPage {
            strings: None,
            view: UpdaterView {
                language: Language::Ru,
                title: "Обновление HLSR".to_owned(),
                subtitle: String::new(),
                stage: "Скачивание обновления…".to_owned(),
                fill: Fill::default(),
                bytes: String::new(),
                speed: String::new(),
                note: String::new(),
                code: String::new(),
                action_shown: true,
                action_label: String::new(),
            },
            action: Some(UpdaterAction::Cancel),
        }
    }

    pub fn view(&self) -> &UpdaterView {
        &self.view
    }

    pub fn action(&self) -> Option<UpdaterAction> {
        self.action
    }

    /// `t(key, arg)`.
    fn t(&self, key: &str, arg: Option<&str>) -> String {
        let Some(lang) = self.strings else {
            return String::new();
        };
        let full = format!("updater.{key}");
        let value = tr(lang, &full, &[]);
        let value = if value == full { String::new() } else { value };
        match arg {
            Some(arg) => value.replacen("%0%", arg, 1),
            None => value,
        }
    }

    /// `strings[code]` — есть ли строка с таким ключом.
    fn known(&self, key: &str) -> Option<String> {
        let lang = self.strings?;
        let full = format!("updater.{key}");
        let value = tr(lang, &full, &[]);
        (value != full).then_some(value)
    }

    fn set_indeterminate(&mut self, on: bool) {
        if on {
            self.view.fill.class = FillClass::Indeterminate;
            self.view.fill.inline_width = None;
        } else if self.view.fill.class == FillClass::Indeterminate {
            self.view.fill.class = FillClass::Plain;
        }
    }

    fn set_progress(&mut self, done: f64, total: f64) {
        self.set_indeterminate(false);
        let percent = if total > 0.0 {
            (done / total * 100.0).min(100.0)
        } else {
            0.0
        };
        self.view.fill.inline_width = js::to_fixed(percent, 1).parse().ok();
    }

    fn set_action(&mut self, action: Option<UpdaterAction>) {
        self.action = action;
        match action {
            None => self.view.action_shown = false,
            Some(action) => {
                self.view.action_shown = true;
                self.view.action_label = match action {
                    UpdaterAction::Cancel => self.t("cancel", None),
                    UpdaterAction::Close => self.t("close", None),
                };
            }
        }
    }

    /// `updater:init` `{language, targetVersion}`.
    pub fn init(&mut self, language: &str, target_version: &str) {
        let lang = if language == "en" {
            Language::En
        } else {
            Language::Ru
        };
        self.strings = Some(lang);
        self.view.language = lang;
        self.view.title = self.t("title", None);
        self.view.subtitle = self.t("subtitle", Some(target_version));
        self.view.stage = self.t("stageDownload", None);
        self.set_action(Some(UpdaterAction::Cancel));
        self.set_indeterminate(true);
    }

    /// `updater:stage` `{stage, …}`.
    pub fn stage(&mut self, payload: &Value) {
        let stage = payload.get("stage").and_then(Value::as_str).unwrap_or("");
        self.view.code.clear();
        self.view.note.clear();
        if stage == "download" {
            self.view.stage = self.t("stageDownload", None);
            self.set_action(Some(UpdaterAction::Cancel));
            self.set_progress(
                number_or_zero(payload.get("done")),
                number_or_zero(payload.get("total")),
            );
            return;
        }
        self.set_action(None);
        self.view.bytes.clear();
        self.view.speed.clear();
        match stage {
            "verify" | "wait-exit" | "install" | "relaunch" => {
                let key = match stage {
                    "verify" => "stageVerify",
                    "wait-exit" => "stageWaitExit",
                    "install" => "stageInstall",
                    _ => "stageRelaunch",
                };
                self.view.stage = self.t(key, None);
                if stage == "wait-exit" {
                    self.view.note = self.t("waitingHlsr", None);
                }
                self.set_indeterminate(true);
            }
            "done" => {
                self.set_indeterminate(false);
                self.view.fill.class = FillClass::Done;
                let version = payload
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.view.stage = self.t("done", Some(version));
                self.view.note = self.t("doneHint", None);
            }
            "cancelled" => {
                self.set_indeterminate(false);
                self.view.fill.class = FillClass::Plain;
                self.view.fill.inline_width = Some(0.0);
                self.view.stage = self.t("cancelled", None);
                self.view.note = self.t("cancelledHint", None);
                self.set_action(Some(UpdaterAction::Close));
            }
            "error" => {
                self.set_indeterminate(false);
                self.view.fill.class = FillClass::Failed;
                self.view.stage = self.t("errorTitle", None);
                let code = payload
                    .get("error")
                    .filter(|error| !error.is_null())
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_str);
                self.view.note = code
                    .and_then(|code| self.known(code))
                    .unwrap_or_else(|| self.t("errorHint", None));
                self.view.code = code.unwrap_or_default().to_owned();
                self.set_action(Some(UpdaterAction::Close));
            }
            _ => {}
        }
    }

    /// `updater:progress` `{done, total, bytesPerSecond}`.
    pub fn progress(&mut self, payload: &Value) {
        let done = number_or_zero(payload.get("done"));
        let total = number_or_zero(payload.get("total"));
        self.set_progress(done, total);
        self.view.bytes = format!(
            "{} {} {}",
            format_bytes(js::value::to_number(payload.get("done"))),
            self.t("of", None),
            format_bytes(js::value::to_number(payload.get("total")))
        );
        let speed = js::value::to_number(payload.get("bytesPerSecond"));
        self.view.speed = if speed.is_nan() || speed == 0.0 {
            String::new()
        } else {
            format!("{}{}", format_bytes(speed), self.t("perSecond", None))
        };
    }

    /// Нажатие кнопки: «Отменить» прячет кнопку и сразу пишет «Обновление отменено».
    pub fn click(&mut self) -> Option<UpdaterCommand> {
        match self.action {
            Some(UpdaterAction::Cancel) => {
                self.set_action(None);
                self.view.stage = self.t("cancelled", None);
                Some(UpdaterCommand::Cancel)
            }
            _ => Some(UpdaterCommand::Close),
        }
    }

    /// Esc: как нажатие, если кнопка есть.
    pub fn escape(&mut self) -> Option<UpdaterCommand> {
        self.action?;
        self.click()
    }
}
