//! Steam в фасаде: наблюдение за подключением (`startSteamConnectionWatch`, `pushSteamAccount`),
//! аккаунт (`steamAccountState`), друзья (`steamFriends`, `friends-avatars`), здоровье Steam
//! (`steamHealthCheck`), владение играми (`steamOwnership`) и профиль в браузере.

use super::{PlatformApi, channels};
use crate::profile::steam_avatars::{FetchOptions, fetch_avatar};
use crate::profile::steam_friends::{self, ListOptions};
use hlsr_core::js;
use hlsr_delivery::manifests::{self, OwnedApp};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Duration;

/// Сколько аватаров качается одновременно.
const AVATAR_CONCURRENCY: usize = 6;
/// Больше стольких id за раз не принимается.
const MAX_AVATAR_IDS: usize = 100;

/// `^7656119\d{10}$`.
fn is_steam_id64(id: &str) -> bool {
    id.len() == 17 && id.starts_with("7656119") && id.bytes().all(|b| b.is_ascii_digit())
}

impl PlatformApi {
    /// `steamAccountState()`: аккаунт Steam или пустой ответ (`connection: "unknown"`).
    pub fn steam_account_state(&self) -> Value {
        match self.steam_account() {
            Some(account) => account.describe(),
            None => json!({
                "signedIn": false,
                "steamId64": null,
                "steamId2": null,
                "personaName": null,
                "avatarDataUri": null,
                "connection": "unknown",
            }),
        }
    }

    /// `pushSteamAccount()`: аккаунт — в окно.
    pub(crate) fn push_steam_account(&self) -> Value {
        let state = self.steam_account_state();
        self.emit_to(channels::STEAM_ACCOUNT, state.clone());
        state
    }

    /// `startSteamConnectionWatch()`: 150 мс, затем раз в 30 с; без входа — раз в 3 с первые
    /// 10 раз. На Windows во время игры опрос откладывается, при скрытом окне — тоже.
    pub(crate) fn start_steam_connection_watch(&self) {
        let me = self.me.clone();
        let timings = self.options.timings.clone();
        let Some(runtime) = self.runtime() else {
            self.logger()
                .warn("[steam] опрос подключения не запущен: нет среды tokio");
            return;
        };
        let task = runtime.spawn(async move {
            let mut delay = timings.steam_first;
            loop {
                tokio::time::sleep(delay).await;
                let Some(api) = me.upgrade() else {
                    return;
                };
                delay = api.steam_tick(&timings);
            }
        });
        if let Some(previous) = self.state().steam_timer.replace(task) {
            previous.abort();
        }
    }

    /// Один шаг опроса; ответ — пауза до следующего.
    fn steam_tick(&self, timings: &super::Timings) -> Duration {
        if !self.is_linux() && self.is_any_game_running() {
            return timings.steam_tick;
        }
        if !(self.options.window_visible)() {
            return timings.steam_tick;
        }
        if let Some(account) = self.steam_account() {
            account.refresh_connection();
        }
        let state = self.push_steam_account();
        let mut locked = self.state();
        if js::truthy(&state["signedIn"]) {
            locked.steam_miss_window = 0;
            return timings.steam_tick;
        }
        locked.steam_miss_window += 1;
        if locked.steam_miss_window > timings.steam_signed_out_fast_ticks {
            timings.steam_tick
        } else {
            timings.steam_signed_out_tick
        }
    }

    /// `steamInstallPath()`.
    fn steam_install_path(&self) -> Option<String> {
        self.steam_account()?
            .install_path()
            .filter(|path| !path.is_empty())
    }

    /// `steamFriends()`: друзья из `localconfig.vdf` без аватаров (для онлайн-профиля).
    pub fn steam_friends(&self) -> Vec<Value> {
        let Some(account) = self.steam_account() else {
            return Vec::new();
        };
        let (Some(install_path), Some(account_id)) =
            (account.install_path(), account.active_account_id())
        else {
            return Vec::new();
        };
        if install_path.is_empty() {
            return Vec::new();
        }
        let friends: Vec<Value> = steam_friends::list(&ListOptions {
            fs: self.options.steam_fs.as_ref(),
            logger: self.logger(),
            platform: self.options.platform,
            install_path: &install_path,
            account_id: account_id as f64,
            avatars: false,
        })
        .iter()
        .map(steam_friends::Friend::to_value)
        .collect();
        // `friendsCache` 0.4.7 никто не читал — не храним.
        friends
    }

    /// `friends-avatars(ids)`: аватары из кеша Steam, недостающие — из профиля Steam (6 запросов
    /// одновременно, кеш `avatar-cache`).
    pub async fn friends_avatars(&self, ids: &Value) -> Value {
        let install_path = self.steam_install_path();
        let Some(ids) = ids.as_array().filter(|ids| ids.len() <= MAX_AVATAR_IDS) else {
            return json!({});
        };
        let ids: Vec<String> = ids
            .iter()
            .filter_map(Value::as_str)
            .filter(|id| is_steam_id64(id))
            .map(str::to_owned)
            .collect();

        let mut out = Map::new();
        if let Some(install_path) = &install_path {
            let found = steam_friends::avatars_for(
                self.options.steam_fs.as_ref(),
                self.options.platform,
                install_path,
                &ids,
            )
            .await;
            for (id, uri) in found.iter() {
                out.insert(id.to_owned(), json!(uri));
            }
        }

        let missing: Vec<String> = ids
            .iter()
            .filter(|id| !out.get(id.as_str()).is_some_and(js::truthy))
            .cloned()
            .collect();
        if missing.is_empty() {
            return Value::Object(out);
        }

        let cache_dir = self
            .options
            .user_data_path
            .as_ref()
            .map(|dir| dir.join("avatar-cache").to_string_lossy().into_owned());
        let options = Arc::new(FetchOptions {
            cache_dir,
            get: self.options.avatar_get.clone(),
            platform: self.options.platform,
        });
        let missing = Arc::new(missing);
        let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut workers = tokio::task::JoinSet::new();
        for _ in 0..AVATAR_CONCURRENCY.min(missing.len()) {
            let (missing, next, options) = (missing.clone(), next.clone(), options.clone());
            workers.spawn(async move {
                let mut found = Vec::new();
                loop {
                    let at = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let Some(id) = missing.get(at) else {
                        return found;
                    };
                    if let Some(uri) = fetch_avatar(id, &options).await {
                        found.push((id.clone(), uri));
                    }
                }
            });
        }
        while let Some(result) = workers.join_next().await {
            for (id, uri) in result.unwrap_or_default() {
                out.insert(id, json!(uri));
            }
        }
        Value::Object(out)
    }

    /// `steam-health-get({force})`.
    pub fn steam_health_get(&self, options: &Value) -> Value {
        let force = js::truthy(options.get("force").unwrap_or(&Value::Null));
        self.steam_health_check(force)
    }

    /// `steamHealthCheck(force)`: состояние Steam (кеш 60 с) и событие `steam-health`.
    pub fn steam_health_check(&self, force: bool) -> Value {
        let now = self.now_ms();
        {
            let state = self.state();
            let checked_at = state.steam_health["checkedAt"].as_i64().unwrap_or(0);
            if !force
                && checked_at != 0
                && now - checked_at < self.options.timings.steam_health_cache_ms
            {
                return state.steam_health.clone();
            }
        }

        let Some(session) = self.ensure_session() else {
            let health =
                json!({"status": "unknown", "code": "STEAM_CHECK_FAILED", "checkedAt": now});
            self.state().steam_health = health.clone();
            return health;
        };

        // В Linux `LaunchService` проверяет Proton, а здоровье Steam — только поиск Steam.
        let result = if self.is_linux() {
            session
                .steam
                .account
                .as_ref()
                .map_or(Value::Null, |account| account.describe_steam_only())
        } else {
            session.launcher.describe_availability("ghosting-renewed")
        };
        let ok = result.get("ok").is_some_and(js::truthy);
        let code = result
            .get("code")
            .filter(|code| js::truthy(code))
            .cloned()
            .unwrap_or(Value::Null);
        let detail = match result.get("steam").filter(|steam| js::truthy(steam)) {
            Some(steam) => {
                let text = |key: &str| {
                    steam
                        .get(key)
                        .filter(|value| js::truthy(value))
                        .map(|value| js::to_template_string(Some(value)))
                };
                [
                    text("installPath"),
                    text("pid").map(|pid| format!("PID {pid}")),
                    text("source").map(|source| format!("опознан: {source}")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ")
            }
            None => result
                .get("message")
                .filter(|message| js::truthy(message))
                .map(|message| js::to_template_string(Some(message)))
                .unwrap_or_default(),
        };
        let health = json!({
            "status": if ok { "available" } else { "unavailable" },
            "code": code,
            "detail": detail,
            "checkedAt": now,
        });
        self.state().steam_health = health.clone();
        self.emit_to(channels::STEAM_HEALTH, health.clone());
        health
    }

    /// `requiredSteamApps()`: игры Steam, которые нужны на аккаунте (диагностика).
    pub fn required_steam_apps(&self) -> Vec<OwnedApp> {
        manifests::required_owned_apps()
    }

    /// `steamOwnership(appId, options)`: `{appId, owned, code, message?}` (диагностика).
    pub async fn steam_ownership(&self, app_id: &Value, options: Option<&Value>) -> Value {
        let depot = self
            .ensure_session()
            .and_then(|session| session.steam.depot);
        let Some(depot) = depot else {
            return json!({"appId": app_id, "owned": null, "code": "STEAM_CHECK_FAILED"});
        };
        let timeout_ms = options
            .and_then(|options| options.get("timeoutMs"))
            .and_then(Value::as_f64)
            .filter(|ms| ms.is_finite() && *ms > 0.0)
            .map(|ms| ms as u64);
        depot.ownership_of(app_id, timeout_ms).await.to_value()
    }

    /// `steam-open-profile`: профиль Steam в браузере.
    pub fn steam_open_profile(&self) -> Value {
        let account = self.steam_account_state();
        let steam_id64 = js::to_template_string(account.get("steamId64"));
        let valid = steam_id64.len() == 17 && steam_id64.bytes().all(|b| b.is_ascii_digit());
        if !js::truthy(&account["signedIn"]) || !valid {
            return json!({"status": "unknown-account"});
        }
        // Как 0.4.7: openExternal без ожидания — отказ оболочки ответ не меняет.
        let _ = self
            .options
            .shell
            .open_external(&format!("https://steamcommunity.com/profiles/{steam_id64}"));
        json!({"status": "ok"})
    }
}
