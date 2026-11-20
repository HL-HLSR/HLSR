//! Окно «HLSR — Новая эра!» (`WelcomeModal.vue`) и решение оболочки, показывать ли его
//! (`LauncherShell.vue`: `checkWelcome`, `onWelcomeSignedIn`, `onWelcomeLater`). Без Slint.

use super::jsv::{field, prop, str_of, truthy};
use crate::services::Reply;
use serde_json::{Value, json};

/// Окно показывают один раз: после выбора папки библиотеки, если игрок ещё не вошёл через Steam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Показать окно.
    Open,
    /// Игрок уже вошёл: окно не нужно, `welcomeSeen` записать.
    MarkSeen,
    /// Ответа нет: ничего.
    Nothing,
}

/// `checkWelcome`: `welcomeSeen` уже есть — канал не спрашивается.
pub fn needs_state(welcome_seen: bool) -> bool {
    !welcome_seen
}

/// Решение по ответу `runner-state`.
pub fn decide(reply: Reply) -> Decision {
    match reply {
        Err(_) => Decision::Nothing,
        Ok(state) => {
            if truthy(field(prop(&state, "account"), "signedIn")) {
                Decision::MarkSeen
            } else {
                Decision::Open
            }
        }
    }
}

/// Побочное действие окна.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// `ipcRenderer.invoke(channel, ...args)`; ответ на вход — [`WelcomeState::login_reply`].
    Login(Value),
    /// `ipcRenderer.invoke("open-external-link", key)`.
    OpenLink(&'static str),
    /// `$emit("signed-in")`: оболочка отмечает окно виденным, закрывает его и открывает профиль.
    SignedIn,
    /// `$emit("later")`: оболочка отмечает окно виденным, закрывает его и показывает тост.
    Later,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WelcomeView {
    pub agreed: bool,
    pub busy: bool,
    pub error: bool,
    pub sign_in_enabled: bool,
}

#[derive(Debug, Default)]
pub struct WelcomeState {
    agreed: bool,
    busy: bool,
    error: bool,
}

impl WelcomeState {
    pub fn new() -> WelcomeState {
        WelcomeState::default()
    }

    pub fn set_agreed(&mut self, agreed: bool) {
        self.agreed = agreed;
    }

    /// Ссылка согласия: ключ из `links.js` (`privacy`, `terms`).
    pub fn open_link(&self, key: &str) -> Vec<Effect> {
        match key {
            "privacy" => vec![Effect::OpenLink("privacy")],
            "terms" => vec![Effect::OpenLink("terms")],
            _ => vec![],
        }
    }

    /// «Войти через Steam»: нужно согласие и свободная кнопка.
    pub fn sign_in(&mut self) -> Vec<Effect> {
        if !self.agreed || self.busy {
            return vec![];
        }
        self.busy = true;
        self.error = false;
        vec![Effect::Login(json!(true))]
    }

    /// Ответ `runner-login`: пустой ответ и `status: "error"` — ошибка, иначе вход начат.
    pub fn login_reply(&mut self, reply: Reply) -> Vec<Effect> {
        self.busy = false;
        match reply {
            Ok(result)
                if truthy(Some(&result)) && str_of(prop(&result, "status")) != Some("error") =>
            {
                vec![Effect::SignedIn]
            }
            _ => {
                self.error = true;
                vec![]
            }
        }
    }

    /// «Войти позже» и Esc.
    pub fn later(&self) -> Vec<Effect> {
        vec![Effect::Later]
    }

    pub fn view(&self) -> WelcomeView {
        WelcomeView {
            agreed: self.agreed,
            busy: self.busy,
            error: self.error,
            sign_in_enabled: self.agreed && !self.busy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_decided_by_the_seen_flag_and_the_account() {
        assert!(!needs_state(true));
        assert!(needs_state(false));
        assert_eq!(
            decide(Ok(json!({"account": {"signedIn": true}}))),
            Decision::MarkSeen
        );
        assert_eq!(
            decide(Ok(json!({"account": {"signedIn": false}}))),
            Decision::Open
        );
        assert_eq!(decide(Ok(json!({}))), Decision::Open);
        assert_eq!(decide(Err("x".into())), Decision::Nothing);
    }

    #[test]
    fn signing_in_needs_the_consent_and_runs_once() {
        let mut state = WelcomeState::new();
        assert!(state.sign_in().is_empty());
        state.set_agreed(true);
        assert!(state.view().sign_in_enabled);
        assert_eq!(state.sign_in(), vec![Effect::Login(json!(true))]);
        assert!(state.view().busy && !state.view().sign_in_enabled);
        // Пока ждём ответа — повторное нажатие ничего не делает.
        assert!(state.sign_in().is_empty());
    }

    #[test]
    fn a_failed_login_shows_the_error_and_a_retry_clears_it() {
        let mut state = WelcomeState::new();
        state.set_agreed(true);
        state.sign_in();
        assert!(state.login_reply(Ok(json!({"status": "error"}))).is_empty());
        let view = state.view();
        assert!(view.error && !view.busy);
        state.sign_in();
        assert!(!state.view().error);
        assert!(state.login_reply(Err("boom".into())).is_empty());
        assert!(state.view().error);
        state.sign_in();
        assert_eq!(
            state.login_reply(Ok(json!({"status": "pending", "userCode": "ABCD"}))),
            vec![Effect::SignedIn]
        );
        assert!(!state.view().error);
    }

    #[test]
    fn links_and_later_pass_through() {
        let state = WelcomeState::new();
        assert_eq!(
            state.open_link("privacy"),
            vec![Effect::OpenLink("privacy")]
        );
        assert_eq!(state.open_link("terms"), vec![Effect::OpenLink("terms")]);
        assert!(state.open_link("other").is_empty());
        assert_eq!(state.later(), vec![Effect::Later]);
    }
}
