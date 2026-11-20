//! Карточка обновления лаунчера (`UpdateCard.vue`) и сценарии оболочки вокруг неё
//! (`LauncherShell.vue`: `onUpdaterState`, `startUpdateDownload`, `confirmInstallNow`,
//! `installOnQuit`, `checkForUpdates`). Состояние приходит каналом `updater-get-state` и
//! событием `updater-state` (формы — карта ПП5 §4.1); машина без Slint возвращает побочные
//! действия списком.

use super::bridge::Tr;
use super::jsv::{num, or_zero, prop, str_of, truthy};
use crate::services::Reply;
use serde_json::{Value, json};

/// Известные коды ошибок (`errorText`).
const KNOWN_ERRORS: &[&str] = &[
    "NETWORK",
    "RATE_LIMIT",
    "FEED",
    "NO_SPACE",
    "ACCESS",
    "VERIFY",
    "METADATA",
    "TIMEOUT",
    "CONFIG",
];

/// Побочное действие.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// `ipcRenderer.invoke(channel)`; ответ — методом по `tag`.
    Invoke { tag: Tag, channel: &'static str },
    /// `notify(text, icon)`: тост (ключ перевода и значок).
    Notify {
        key: &'static str,
        icon: &'static str,
    },
    /// `askConfirm({title, text, confirmLabel, danger})` перед перезапуском; ответ — методом
    /// [`UpdateState::restart_answer`].
    ConfirmRestart { busy: bool },
    /// Открыть журнал изменений лаунчера: `onWhatsNew({target: "launcher", version})`.
    WhatsNew { version: Option<String> },
}

/// Чему ответ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    /// `updater-get-state`.
    State,
    /// `updater-download`: запущена ли загрузка.
    Download,
    /// Ответ не нужен (`updater-check`, `-cancel`, `-install-now`, `-install-on-quit`).
    Ignored,
}

/// Что показывает карточка.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateView {
    pub visible: bool,
    pub can_dismiss: bool,
    pub title: String,
    pub subtitle: String,
    pub downloading: bool,
    pub percent: i32,
    pub transferred: String,
    pub speed: String,
    pub eta: String,
    pub notes: String,
    pub cancel: bool,
    pub download: bool,
    pub whats_new: bool,
    pub later: bool,
    pub install_now: bool,
    pub install_on_quit: bool,
    pub retry: bool,
}

#[derive(Debug, Clone)]
pub struct UpdateState {
    status: Value,
    dismissed: bool,
}

impl Default for UpdateState {
    fn default() -> Self {
        UpdateState::new()
    }
}

/// `(bytes / 1024 / 1024).toFixed(1).replace(".", ",")`; нуль и не число — «0».
fn mb(bytes: Option<f64>) -> String {
    match bytes {
        Some(bytes) if bytes != 0.0 && !bytes.is_nan() => {
            hlsr_core::js::to_fixed(bytes / 1024.0 / 1024.0, 1).replace('.', ",")
        }
        _ => "0".to_owned(),
    }
}

/// `String#slice(0, 400)` по кодовым единицам UTF-16.
fn slice_units(text: &str, units: usize) -> String {
    let all: Vec<u16> = text.encode_utf16().collect();
    String::from_utf16_lossy(&all[..all.len().min(units)])
}

impl UpdateState {
    /// Состояние до первого ответа (`updateStatus` оболочки): простой и без прогресса.
    pub fn new() -> UpdateState {
        UpdateState {
            status: json!({"state": "idle", "enabled": false, "currentVersion": "",
                           "version": null, "releaseNotes": null, "progress": null,
                           "error": null, "errorCode": null, "checkSource": null}),
            dismissed: false,
        }
    }

    /// `mounted`: `updater-get-state`.
    pub fn mounted(&self) -> Vec<Effect> {
        vec![Effect::Invoke {
            tag: Tag::State,
            channel: "updater-get-state",
        }]
    }

    pub fn status(&self) -> &Value {
        &self.status
    }

    pub fn current_version(&self) -> String {
        str_of(prop(&self.status, "currentVersion"))
            .unwrap_or_default()
            .to_owned()
    }

    /// Ответ `updater-get-state`: `if (status) this.updateStatus = status`.
    pub fn state_reply(&mut self, reply: Reply) {
        if let Ok(status) = reply
            && truthy(Some(&status))
        {
            self.status = status;
        }
    }

    /// Состояние оболочки как есть (`:status="updateStatus"`, `v-if="!updateDismissed"`): карточку
    /// в окне лаунчера ведёт оболочка; пустой ответ оставляет прежнее состояние.
    pub fn show(&mut self, status: &Value, dismissed: bool) {
        if truthy(Some(status)) {
            self.status = status.clone();
        }
        self.dismissed = dismissed;
    }

    /// Событие `updater-state` (`onUpdaterState`).
    pub fn updater_state(&mut self, status: &Value) {
        let state = str_of(prop(status, "state")).unwrap_or_default();
        let manual = str_of(prop(status, "checkSource")) == Some("manual");
        if state == "checking" && manual {
            self.dismissed = false;
        }
        if state == "available" && prop(status, "version") != prop(&self.status, "version") {
            self.dismissed = false;
        }
        if state == "downloaded" || (state == "error" && manual) {
            self.dismissed = false;
        }
        self.status = status.clone();
    }

    fn state(&self) -> &str {
        str_of(prop(&self.status, "state")).unwrap_or_default()
    }

    fn card_visible(&self) -> bool {
        let state = self.state();
        matches!(
            state,
            "available" | "downloading" | "verifying" | "downloaded"
        ) || (matches!(state, "checking" | "error")
            && str_of(prop(&self.status, "checkSource")) == Some("manual"))
    }

    // ---- События карточки -----------------------------------------------------------------

    /// «Скачать»: `startUpdateDownload()`.
    pub fn download(&self) -> Vec<Effect> {
        vec![Effect::Invoke {
            tag: Tag::Download,
            channel: "updater-download",
        }]
    }

    /// Ответ `updater-download` (отказ — не запущено).
    pub fn download_reply(&self, reply: Reply) -> Vec<Effect> {
        if reply.as_ref().is_ok_and(|started| truthy(Some(started))) {
            return vec![];
        }
        vec![
            Effect::Notify {
                key: "#L_U_CHECKING",
                icon: "fas fa-sync",
            },
            Effect::Invoke {
                tag: Tag::Ignored,
                channel: "updater-check",
            },
        ]
    }

    /// «Отмена».
    pub fn cancel(&self) -> Vec<Effect> {
        vec![Effect::Invoke {
            tag: Tag::Ignored,
            channel: "updater-cancel",
        }]
    }

    /// «Повторить»: `checkForUpdates()`.
    pub fn retry(&self) -> Vec<Effect> {
        vec![Effect::Invoke {
            tag: Tag::Ignored,
            channel: "updater-check",
        }]
    }

    /// «Позже» и крестик.
    pub fn dismiss(&mut self) {
        self.dismissed = true;
    }

    /// «Перезапустить и установить»: подтверждение (`confirmInstallNow`); `busy` — идёт доставка.
    pub fn install_now(&self, busy: bool) -> Vec<Effect> {
        vec![Effect::ConfirmRestart { busy }]
    }

    /// Ответ окна подтверждения.
    pub fn restart_answer(&self, accepted: bool) -> Vec<Effect> {
        if !accepted {
            return vec![];
        }
        vec![Effect::Invoke {
            tag: Tag::Ignored,
            channel: "updater-install-now",
        }]
    }

    /// «Установить при выходе».
    pub fn install_on_quit(&mut self) -> Vec<Effect> {
        self.dismissed = true;
        vec![
            Effect::Invoke {
                tag: Tag::Ignored,
                channel: "updater-install-on-quit",
            },
            Effect::Notify {
                key: "#L_U_WILL_INSTALL_ON_QUIT",
                icon: "fas fa-check",
            },
        ]
    }

    /// «Что нового».
    pub fn whats_new(&self) -> Vec<Effect> {
        vec![Effect::WhatsNew {
            version: str_of(prop(&self.status, "version")).map(str::to_owned),
        }]
    }

    /// Обновление из уведомления (`onRunUpdateAction`, цель `launcher`): карточка снова видна
    /// и загрузка начата.
    pub fn run_from_notification(&mut self) -> Vec<Effect> {
        self.dismissed = false;
        self.download()
    }

    pub fn view(&self, tr: &Tr) -> UpdateView {
        let state = self.state();
        let progress = prop(&self.status, "progress").filter(|p| truthy(Some(*p)));
        let version = str_of(prop(&self.status, "version")).unwrap_or_default();
        let current = str_of(prop(&self.status, "currentVersion")).unwrap_or_default();
        let downloading = state == "downloading";
        let percent = progress
            .and_then(|p| num(prop(p, "percent")))
            .filter(|percent| *percent != 0.0 && !percent.is_nan())
            .map(|percent| super::jsv::round(percent) as i32)
            .unwrap_or(0);
        let notes = if matches!(state, "available" | "downloaded") {
            let raw = match prop(&self.status, "releaseNotes") {
                Some(Value::String(text)) => text.clone(),
                _ => String::new(),
            };
            slice_units(&raw, 400)
        } else {
            String::new()
        };
        let title = match state {
            "checking" => tr("#L_U_CHECKING", &[]),
            "available" => tr("#L_U_AVAILABLE", &[version]),
            "downloading" => tr("#L_U_DOWNLOADING", &[]),
            "verifying" => tr("#L_B_VERIFYING", &[]),
            "downloaded" => tr("#L_U_READY", &[version]),
            "error" => tr("#L_U_ERROR", &[]),
            _ => String::new(),
        };
        let subtitle = if state == "error" {
            let code = str_of(prop(&self.status, "errorCode"));
            match code.filter(|code| KNOWN_ERRORS.contains(code)) {
                Some(code) => tr(&format!("#L_U_ERR_{code}"), &[]),
                None => tr("#L_U_ERR_UNKNOWN", &[]),
            }
        } else {
            tr("#L_U_CURRENT", &[current])
        };
        let transferred = progress.map_or(String::new(), |p| {
            format!(
                "{} / {}",
                mb(num(prop(p, "transferred"))),
                mb(num(prop(p, "total")))
            )
        });
        let speed = progress
            .filter(|p| or_zero(prop(p, "bytesPerSecond")) != 0.0)
            .map_or(String::new(), |p| {
                tr("#L_U_SPEED", &[mb(num(prop(p, "bytesPerSecond"))).as_str()])
            });
        let eta = progress
            .and_then(|p| {
                let bps = or_zero(prop(p, "bytesPerSecond"));
                let total = or_zero(prop(p, "total"));
                if bps == 0.0 || total == 0.0 {
                    return None;
                }
                let left = (total - or_zero(prop(p, "transferred"))) / bps;
                if !left.is_finite() || left <= 0.0 {
                    return None;
                }
                let minutes = (left / 60.0).floor();
                let seconds = super::jsv::round(left % 60.0);
                let shown = if minutes > 0.0 {
                    format!(
                        "{} {} {}",
                        minutes as i64,
                        tr("#L_U_MIN", &[]),
                        seconds as i64
                    )
                } else {
                    (seconds as i64).to_string()
                };
                Some(tr("#L_U_ETA", &[shown.as_str()]))
            })
            .unwrap_or_default();
        let cancellable = truthy(prop(&self.status, "cancellable"));
        let available = state == "available";
        let has_notes = !notes.is_empty();
        UpdateView {
            visible: !self.dismissed && self.card_visible(),
            can_dismiss: available || state == "error",
            title,
            subtitle,
            downloading,
            percent,
            transferred,
            speed,
            eta,
            notes,
            cancel: downloading || (state == "verifying" && cancellable),
            download: available,
            whats_new: available && has_notes,
            later: available,
            install_now: state == "downloaded",
            install_on_quit: state == "downloaded",
            retry: state == "error",
        }
    }
}

/// Событие `updater-state` как значение для тестов и сцен.
pub fn status_json(state: &str, extra: Value) -> Value {
    let mut status = json!({"state": state, "enabled": true, "currentVersion": "0.4.7",
                            "version": null, "releaseNotes": null, "progress": null,
                            "error": null, "errorCode": null, "checkSource": null});
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        status[key] = value;
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use hlsr_core::language::Language;
    use std::rc::Rc;

    fn en() -> Tr {
        Rc::new(|key, args| crate::tr(Language::En, key, args))
    }

    fn t(key: &str, args: &[&str]) -> String {
        crate::tr(Language::En, key, args)
    }

    fn state(status: Value) -> UpdateState {
        let mut state = UpdateState::new();
        state.state_reply(Ok(status));
        state
    }

    #[test]
    fn the_card_shows_for_the_four_active_states_and_for_manual_checks_and_errors() {
        for (name, manual, visible) in [
            ("idle", false, false),
            ("checking", false, false),
            ("checking", true, true),
            ("available", false, true),
            ("downloading", false, true),
            ("verifying", false, true),
            ("downloaded", false, true),
            ("error", false, false),
            ("error", true, true),
        ] {
            let source = if manual { json!("manual") } else { Value::Null };
            let card = state(status_json(name, json!({"checkSource": source})));
            assert_eq!(card.view(&en()).visible, visible, "{name} {manual}");
        }
    }

    #[test]
    fn dismissal_is_undone_by_a_new_version_a_manual_check_a_finished_download_or_a_manual_error() {
        let mut card = state(status_json("available", json!({"version": "0.5.0"})));
        card.dismiss();
        assert!(!card.view(&en()).visible);
        // Та же версия — остаётся закрытой.
        card.updater_state(&status_json("available", json!({"version": "0.5.0"})));
        assert!(!card.view(&en()).visible);
        card.updater_state(&status_json("available", json!({"version": "0.6.0"})));
        assert!(card.view(&en()).visible);
        card.dismiss();
        card.updater_state(&status_json("downloading", json!({"version": "0.6.0"})));
        assert!(!card.view(&en()).visible);
        card.updater_state(&status_json("downloaded", json!({"version": "0.6.0"})));
        assert!(card.view(&en()).visible);
        card.dismiss();
        card.updater_state(&status_json("error", json!({"checkSource": "auto"})));
        assert!(!card.view(&en()).visible);
        card.updater_state(&status_json("error", json!({"checkSource": "manual"})));
        assert!(card.view(&en()).visible);
        card.dismiss();
        card.updater_state(&status_json("checking", json!({"checkSource": "manual"})));
        assert!(card.view(&en()).visible);
    }

    #[test]
    fn an_available_update_offers_download_later_and_notes() {
        let card = state(status_json(
            "available",
            json!({"version": "0.5.0", "releaseNotes": "Fixes\nMore fixes"}),
        ));
        let view = card.view(&en());
        assert_eq!(view.title, t("#L_U_AVAILABLE", &["0.5.0"]));
        assert_eq!(view.subtitle, t("#L_U_CURRENT", &["0.4.7"]));
        assert!(view.download && view.later && view.whats_new && view.can_dismiss);
        assert!(!view.cancel && !view.install_now && !view.retry);
        assert_eq!(view.notes, "Fixes\nMore fixes");
    }

    #[test]
    fn notes_are_cut_at_400_units() {
        let long = "я".repeat(500);
        let card = state(status_json(
            "available",
            json!({"version": "1", "releaseNotes": long}),
        ));
        assert_eq!(card.view(&en()).notes.chars().count(), 400);
    }

    #[test]
    fn a_download_shows_progress_speed_and_time_left() {
        let card = state(status_json(
            "downloading",
            json!({"version": "0.5.0", "progress": {"percent": 33.4, "transferred": 10_485_760,
                   "total": 31_457_280, "bytesPerSecond": 1_048_576}}),
        ));
        let view = card.view(&en());
        assert!(view.downloading && view.cancel && !view.download);
        assert_eq!(view.percent, 33);
        assert_eq!(view.transferred, "10,0 / 30,0");
        assert_eq!(view.speed, t("#L_U_SPEED", &["1,0"]));
        // 20 МБ при 1 МБ/с — 20 секунд.
        assert_eq!(view.eta, t("#L_U_ETA", &["20"]));
        let slow = state(status_json(
            "downloading",
            json!({"progress": {"percent": 1, "transferred": 0, "total": 31_457_280, "bytesPerSecond": 262_144}}),
        ));
        // 120 секунд: «2 мин 0».
        let shown = format!("2 {} 0", t("#L_U_MIN", &[]));
        assert_eq!(slow.view(&en()).eta, t("#L_U_ETA", &[shown.as_str()]));
        let none = state(status_json(
            "downloading",
            json!({"progress": {"percent": 0, "transferred": 0, "total": 0, "bytesPerSecond": 0}}),
        ));
        let view = none.view(&en());
        assert_eq!(
            (view.percent, view.speed.as_str(), view.eta.as_str()),
            (0, "", "")
        );
        assert_eq!(view.transferred, "0 / 0");
    }

    #[test]
    fn verifying_can_be_cancelled_only_when_main_says_so() {
        let plain = state(status_json("verifying", json!({})));
        assert!(!plain.view(&en()).cancel);
        let cancellable = state(status_json("verifying", json!({"cancellable": true})));
        assert!(cancellable.view(&en()).cancel);
        assert_eq!(cancellable.view(&en()).title, t("#L_B_VERIFYING", &[]));
    }

    #[test]
    fn a_downloaded_update_offers_restart_and_install_on_quit() {
        let card = state(status_json("downloaded", json!({"version": "0.5.0"})));
        let view = card.view(&en());
        assert_eq!(view.title, t("#L_U_READY", &["0.5.0"]));
        assert!(view.install_now && view.install_on_quit && !view.later);
    }

    #[test]
    fn errors_name_their_code_or_the_unknown_one() {
        let known = state(status_json(
            "error",
            json!({"errorCode": "NO_SPACE", "checkSource": "manual"}),
        ));
        let view = known.view(&en());
        assert_eq!(view.title, t("#L_U_ERROR", &[]));
        assert_eq!(view.subtitle, t("#L_U_ERR_NO_SPACE", &[]));
        assert!(view.retry && view.can_dismiss);
        let unknown = state(status_json(
            "error",
            json!({"errorCode": "WEIRD", "checkSource": "manual"}),
        ));
        assert_eq!(unknown.view(&en()).subtitle, t("#L_U_ERR_UNKNOWN", &[]));
    }

    #[test]
    fn a_failed_download_start_falls_back_to_a_check_with_a_toast() {
        let card = UpdateState::new();
        assert_eq!(
            card.download(),
            vec![Effect::Invoke {
                tag: Tag::Download,
                channel: "updater-download"
            }]
        );
        assert!(card.download_reply(Ok(json!(true))).is_empty());
        let effects = card.download_reply(Ok(json!(false)));
        assert_eq!(
            effects,
            vec![
                Effect::Notify {
                    key: "#L_U_CHECKING",
                    icon: "fas fa-sync"
                },
                Effect::Invoke {
                    tag: Tag::Ignored,
                    channel: "updater-check"
                }
            ]
        );
        assert_eq!(card.download_reply(Err("x".into())).len(), 2);
    }

    #[test]
    fn restarting_asks_first_and_installing_on_quit_dismisses_with_a_toast() {
        let mut card = state(status_json("downloaded", json!({"version": "0.5.0"})));
        assert_eq!(
            card.install_now(true),
            vec![Effect::ConfirmRestart { busy: true }]
        );
        assert!(card.restart_answer(false).is_empty());
        assert_eq!(
            card.restart_answer(true),
            vec![Effect::Invoke {
                tag: Tag::Ignored,
                channel: "updater-install-now"
            }]
        );
        let effects = card.install_on_quit();
        assert_eq!(effects.len(), 2);
        assert!(!card.view(&en()).visible);
    }

    #[test]
    fn whats_new_and_the_notification_route_carry_the_launcher_version() {
        let mut card = state(status_json(
            "available",
            json!({"version": "0.5.0", "releaseNotes": " a \n\n b\n"}),
        ));
        assert_eq!(
            card.whats_new(),
            vec![Effect::WhatsNew {
                version: Some("0.5.0".into())
            }]
        );
        card.dismiss();
        let effects = card.run_from_notification();
        assert_eq!(effects.len(), 1);
        assert!(card.view(&en()).visible);
    }
}
