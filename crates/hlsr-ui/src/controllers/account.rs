//! Аккаунт в шапке (`AccountMenu.vue` 0.4.9): что показывают кнопка и меню по ответу
//! `runner-state` / событию `runner-profile-state`. Логика без интерфейса: тексты, точка
//! присутствия, значок подписчика, пункты меню (гостю первый — «Войти через Steam», вошедшему —
//! «Открыть профиль», «Настройки профиля»; карточки-шапки в меню нет); опрос раз в 30 с и при
//! фокусе окна — в виде (`hlsr-gpui` `header::account`).

use super::bridge::Tr;
use super::jsv::{field, prop, str_of, truthy};
use serde_json::Value;

/// Как часто обновляется аккаунт (`ACCOUNT_REFRESH_MS`).
pub const ACCOUNT_REFRESH_MS: u64 = 30_000;

/// Короткие имена игр (`launcherData.shortNames`) по идентификатору лаунчера.
pub fn short_name(launcher_id: &str) -> Option<&'static str> {
    Some(match launcher_id {
        "70" => "Half-Life",
        "50" => "Opposing Force",
        "130" => "Blue Shift",
        "220" => "Half-Life 2",
        "218" => "HL2 Ghosting",
        "380" => "HL2: Episode One",
        "420" => "HL2: Episode Two",
        _ => return None,
    })
}

/// Пункт меню аккаунта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountItem {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: String,
}

/// Готовое к показу: тексты кнопки и меню.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountView {
    /// `accountLoading`: ответа ещё не было или связь проверяется.
    pub loading: bool,
    pub signed_in: bool,
    /// Гость: первый пункт меню — «Войти через Steam».
    pub can_sign_in: bool,
    /// Ник с оформлением — только у вошедшего.
    pub show_nick: bool,
    /// Значок подписчика у ника (`showNick && subscriber`).
    pub subscriber: bool,
    pub name: String,
    /// `aria-label` кнопки.
    pub label: String,
    /// `title` кнопки: «ник · присутствие · состояние».
    pub hint: String,
    /// Вторая строка кнопки (`account-menu__connection`).
    pub presence_text: String,
    /// Цвет точки на аватаре: `unknown`, `checking`, `online`, `offline`.
    pub presence_state: &'static str,
    /// `account.avatarDataUri` (пусто — значок Steam).
    pub avatar: String,
    pub nick_style: String,
    pub nick_css: String,
    pub frame_css: String,
    pub items: Vec<AccountItem>,
}

/// Состояние ответов main: аккаунт, оформление, присутствие.
#[derive(Debug, Clone)]
pub struct AccountState {
    account: Value,
    nick: Option<Value>,
    /// Подписка активна сейчас (`value.subscriber === true`).
    subscriber: bool,
    frame: Option<Value>,
    presence: Value,
    known: bool,
}

impl Default for AccountState {
    fn default() -> Self {
        AccountState::new()
    }
}

/// `typeof value === "object"` и не `null`.
fn is_object(value: &Value) -> bool {
    value.is_object() || value.is_array()
}

impl AccountState {
    /// Состояние до первого ответа: `connection: "checking"`.
    pub fn new() -> AccountState {
        AccountState {
            account: serde_json::json!({"signedIn": false, "connection": "checking"}),
            nick: None,
            subscriber: false,
            frame: None,
            presence: serde_json::json!({"online": false, "gameId": null, "launcherId": null}),
            known: false,
        }
    }

    /// Ответ `runner-state` или событие `runner-profile-state` (`refreshAccount`,
    /// `onAccountPushed`): не объект — игнорируется. Истина, если состояние принято.
    pub fn apply(&mut self, value: &Value) -> bool {
        if !is_object(value) {
            return false;
        }
        self.account = match prop(value, "account") {
            Some(account) if truthy(Some(account)) => account.clone(),
            _ => serde_json::json!({"signedIn": false}),
        };
        self.nick = prop(value, "nick").filter(|v| truthy(Some(*v))).cloned();
        self.subscriber = prop(value, "subscriber") == Some(&Value::Bool(true));
        self.frame = prop(value, "avatarFrame")
            .filter(|v| truthy(Some(*v)))
            .cloned();
        if let Some(presence) = prop(value, "presence").filter(|v| truthy(Some(*v))) {
            self.presence = presence.clone();
        }
        self.known = true;
        true
    }

    fn signed_in(&self) -> bool {
        truthy(prop(&self.account, "signedIn"))
    }

    fn connection_state(&self) -> &str {
        match str_of(prop(&self.account, "connection")) {
            Some(value @ ("connected" | "elevated" | "offline" | "checking")) => value,
            _ => "unknown",
        }
    }

    fn loading(&self) -> bool {
        !self.known || str_of(prop(&self.account, "connection")) == Some("checking")
    }

    /// Вошёл ли игрок (по последнему ответу; поиску нужен только он).
    pub fn is_signed_in(&self) -> bool {
        self.signed_in()
    }

    /// SteamID64 вошедшего (`account.steamId64`), пусто — нет.
    pub fn steam_id(&self) -> String {
        str_of(prop(&self.account, "steamId64"))
            .unwrap_or_default()
            .to_owned()
    }

    pub fn view(&self, tr: &Tr) -> AccountView {
        let loading = self.loading();
        let signed_in = self.signed_in();
        let known = self.known;
        let guest = known && !signed_in;
        let connection = self.connection_state();
        let t = |key: &str| tr(key, &[]);

        let name = if loading {
            t("#L_SB_STEAM_LOADING")
        } else if !signed_in {
            t("#L_RUNNER_LOGIN")
        } else {
            match str_of(prop(&self.account, "personaName")).filter(|name| !name.is_empty()) {
                Some(name) => name.to_owned(),
                None => t("#L_SB_STEAM_ACCOUNT"),
            }
        };
        let show_nick = !loading && signed_in;
        let label = if !loading && !signed_in {
            t("#L_RUNNER_LOGIN")
        } else {
            name.clone()
        };
        let status_text = if guest {
            t("#L_RUNNER_GUEST")
        } else if loading {
            t("#L_SB_STEAM_LOADING")
        } else {
            match connection {
                "connected" => t("#L_RUNNER_ONLINE"),
                "checking" => t("#L_ST_CHECKING"),
                "elevated" => t("#L_ST_ELEVATED"),
                "offline" => t("#L_RUNNER_OFFLINE"),
                _ => t("#L_ST_UNKNOWN"),
            }
        };
        let online = truthy(prop(&self.presence, "online"));
        let presence_state = if guest {
            "unknown"
        } else if loading {
            "checking"
        } else if online {
            "online"
        } else {
            "offline"
        };
        let presence_text = if guest {
            t("#L_RUNNER_GUEST")
        } else if loading {
            t("#L_ST_CHECKING")
        } else {
            let game = str_of(prop(&self.presence, "launcherId")).and_then(short_name);
            match game {
                Some(game) => tr("#L_PRESENCE_PLAYING", &[game]),
                None => t(if online {
                    "#L_PRESENCE_ONLINE"
                } else {
                    "#L_PRESENCE_OFFLINE"
                }),
            }
        };
        let can_sign_in = known && !loading && !signed_in;
        // Гостю первый пункт — вход (открывает тот же экран входа, что и профиль).
        let mut items = vec![if can_sign_in {
            AccountItem {
                id: "open-profile",
                icon: "fab fa-steam",
                label: t("#L_RUNNER_LOGIN"),
            }
        } else {
            AccountItem {
                id: "open-profile",
                icon: "fas fa-user",
                label: t("#L_SB_MENU_OPEN_PROFILE"),
            }
        }];
        if signed_in {
            items.push(AccountItem {
                id: "profile-settings",
                icon: "fas fa-sliders-h",
                label: t("#L_SB_MENU_PROFILE_SETTINGS"),
            });
        }
        items.push(AccountItem {
            id: "friends",
            icon: "fas fa-user-friends",
            label: t("#L_FRIENDS_TITLE"),
        });
        let hint = format!(
            "{label} · {presence_text}{}",
            if signed_in {
                format!(" · {status_text}")
            } else {
                String::new()
            }
        );
        let text = |value: Option<&Value>, key: &str| {
            str_of(field(value, key)).unwrap_or_default().to_owned()
        };
        AccountView {
            loading,
            signed_in,
            can_sign_in,
            show_nick,
            subscriber: show_nick && self.subscriber,
            name,
            label,
            hint,
            presence_text,
            presence_state,
            avatar: str_of(prop(&self.account, "avatarDataUri"))
                .unwrap_or_default()
                .to_owned(),
            nick_style: text(self.nick.as_ref(), "style"),
            nick_css: text(self.nick.as_ref(), "css"),
            frame_css: text(self.frame.as_ref(), "css"),
            items,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hlsr_core::language::Language;
    use serde_json::json;
    use std::rc::Rc;

    fn en() -> Tr {
        Rc::new(|key, args| crate::tr(Language::En, key, args))
    }

    /// Строка английского перевода (тесты не повторяют тексты локали).
    fn t(key: &str) -> String {
        crate::tr(Language::En, key, &[])
    }

    fn signed_in(connection: &str) -> Value {
        json!({
            "status": "signed-in",
            "account": {"signedIn": true, "steamId64": "76561199155902676", "personaName": "Kanoyo",
                        "avatarDataUri": null, "connection": connection},
            "nick": {"style": "developer", "css": null},
            "avatarFrame": {"css": "border: 2px solid red"},
            "presence": {"online": true, "gameId": "ghosting-renewed", "launcherId": "218"},
        })
    }

    #[test]
    fn until_the_first_answer_the_account_is_loading() {
        let state = AccountState::new();
        let view = state.view(&en());
        assert!(view.loading && !view.can_sign_in && !view.show_nick);
        assert_eq!(view.name, t("#L_SB_STEAM_LOADING"));
        assert_eq!(view.presence_state, "checking");
        assert_eq!(view.presence_text, t("#L_ST_CHECKING"));
        let items: Vec<_> = view.items.iter().map(|item| (item.id, item.icon)).collect();
        assert_eq!(
            items,
            [
                ("open-profile", "fas fa-user"),
                ("friends", "fas fa-user-friends")
            ]
        );
        assert_eq!(view.items[0].label, t("#L_SB_MENU_OPEN_PROFILE"));
        assert_eq!(view.items[1].label, t("#L_FRIENDS_TITLE"));
    }

    #[test]
    fn a_guest_signs_in_from_the_first_item_and_has_no_nick() {
        let mut state = AccountState::new();
        assert!(state.apply(&json!({"status": "signed-out", "account": {"signedIn": false}})));
        let view = state.view(&en());
        assert!(view.can_sign_in && !view.signed_in && !view.show_nick);
        assert_eq!(view.name, t("#L_RUNNER_LOGIN"));
        assert_eq!(view.label, t("#L_RUNNER_LOGIN"));
        assert_eq!(view.presence_state, "unknown");
        assert_eq!(view.presence_text, t("#L_RUNNER_GUEST"));
        let items: Vec<_> = view.items.iter().map(|item| (item.id, item.icon)).collect();
        assert_eq!(
            items,
            [
                ("open-profile", "fab fa-steam"),
                ("friends", "fas fa-user-friends")
            ]
        );
        assert_eq!(view.items[0].label, t("#L_RUNNER_LOGIN"));
        assert_eq!(
            view.hint,
            format!("{} · {}", t("#L_RUNNER_LOGIN"), t("#L_RUNNER_GUEST"))
        );
    }

    #[test]
    fn a_signed_in_player_shows_the_nick_presence_and_game() {
        let mut state = AccountState::new();
        state.apply(&signed_in("connected"));
        let view = state.view(&en());
        assert!(view.show_nick && view.signed_in && !view.can_sign_in);
        assert_eq!(view.name, "Kanoyo");
        assert_eq!(view.presence_state, "online");
        let playing = crate::tr(Language::En, "#L_PRESENCE_PLAYING", &["HL2 Ghosting"]);
        assert_eq!(view.presence_text, playing);
        assert!(!view.subscriber);
        let items: Vec<_> = view.items.iter().map(|item| item.id).collect();
        assert_eq!(items, ["open-profile", "profile-settings", "friends"]);
        assert_eq!(view.items[1].icon, "fas fa-sliders-h");
        assert_eq!(view.items[1].label, t("#L_SB_MENU_PROFILE_SETTINGS"));
        assert_eq!(
            view.hint,
            format!("Kanoyo · {playing} · {}", t("#L_RUNNER_ONLINE"))
        );
        assert_eq!(view.nick_style, "developer");
        assert_eq!(view.frame_css, "border: 2px solid red");
        assert_eq!(state.steam_id(), "76561199155902676");
    }

    #[test]
    fn connection_states_pick_their_texts() {
        for (connection, status, loading) in [
            ("checking", "#L_SB_STEAM_LOADING", true),
            ("elevated", "#L_ST_ELEVATED", false),
            ("offline", "#L_RUNNER_OFFLINE", false),
            ("weird", "#L_ST_UNKNOWN", false),
        ] {
            let mut state = AccountState::new();
            state.apply(&signed_in(connection));
            let view = state.view(&en());
            assert_eq!(view.loading, loading, "{connection}");
            assert!(view.hint.ends_with(&t(status)), "{connection}");
        }
    }

    #[test]
    fn only_a_signed_in_subscriber_shows_the_badge() {
        let mut state = AccountState::new();
        let mut value = signed_in("connected");
        value["subscriber"] = json!(true);
        state.apply(&value);
        assert!(state.view(&en()).subscriber);
        // Пока связь проверяется, ника с оформлением нет — и значка тоже.
        value["account"]["connection"] = json!("checking");
        state.apply(&value);
        assert!(!state.view(&en()).subscriber);
        value["account"]["connection"] = json!("connected");
        value["subscriber"] = json!("true");
        state.apply(&value);
        assert!(!state.view(&en()).subscriber);
    }

    #[test]
    fn replies_that_are_not_objects_change_nothing() {
        let mut state = AccountState::new();
        assert!(!state.apply(&Value::Null));
        assert!(!state.apply(&json!("x")));
        assert!(state.view(&en()).loading);
        // Ответ без account — гость.
        assert!(state.apply(&json!({"status": "signed-out"})));
        assert!(state.view(&en()).can_sign_in);
    }

    #[test]
    fn presence_without_a_known_game_is_plain_online_or_offline() {
        let mut state = AccountState::new();
        let mut value = signed_in("connected");
        value["presence"] = json!({"online": false, "gameId": null, "launcherId": null});
        state.apply(&value);
        let view = state.view(&en());
        assert_eq!(view.presence_state, "offline");
        assert_eq!(view.presence_text, t("#L_PRESENCE_OFFLINE"));
        value["presence"] = json!({"online": true, "gameId": null, "launcherId": "999"});
        state.apply(&value);
        assert_eq!(state.view(&en()).presence_text, t("#L_PRESENCE_ONLINE"));
    }
}
