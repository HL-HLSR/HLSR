//! Каналы профиля без окна: `online-profile/ipc.js` (25 каналов `runner-*`: ошибки → `{status:
//! "error", code}`, у ошибки без кода — `RUNNER_NETWORK`) и каналы профиля из
//! `game-delivery/index.js` (`profile-get`, `friends-list`, онлайн-ветка `game-stats-get`).
//! `open-discord` и `open-external-link` — в `hlsr_delivery::api` (`DeliveryApi`).
//!
//! Аргументы — как пришли из окна (`Option<&Value>`: `None` — `undefined`), итог — то, что получило
//! бы окно (`undefined` — `null`).

use super::online_profile_service::{OnlineProfileService, OpenExternal, RunnerError};
use serde_json::{Map, Value, json};
use std::sync::LazyLock;

/// Хосты, которые открывает `runner-open-link`.
pub const OPEN_LINK_HOSTS: [&str; 7] = [
    "steamcommunity.com",
    "www.speedrun.com",
    "youtube.com",
    "www.youtube.com",
    "youtu.be",
    "twitch.tv",
    "www.twitch.tv",
];
const MAX_LINK_LENGTH: usize = 512;

/// Старые закреплённые забеги: `/runs/<id>` без игры (флаг `i`).
static LEGACY_RUN_PATH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i-u)^/runs/([a-z0-9]{8})$").expect("шаблон пути забега")
});

/// Итог канала: значение или `{status: "error", code}`.
pub fn reply(result: Result<Value, RunnerError>) -> Value {
    match result {
        Ok(value) => value,
        Err(error) => json!({ "status": "error", "code": error.ipc_code() }),
    }
}

/// Каналы профиля (`bind(ipcMain, service, openExternal)`).
#[derive(Clone)]
pub struct ProfileIpc {
    service: OnlineProfileService,
    open_external: OpenExternal,
}

impl ProfileIpc {
    /// `openExternal` — тот же, что у службы (`AppManager.js:294`).
    pub fn new(service: OnlineProfileService) -> ProfileIpc {
        let open_external = service.open_external();
        ProfileIpc {
            service,
            open_external,
        }
    }

    pub fn service(&self) -> &OnlineProfileService {
        &self.service
    }

    /// Канал по имени и аргументам окна; `None` — канала нет.
    pub async fn handle(&self, channel: &str, args: &[Value]) -> Option<Value> {
        let a = |index: usize| args.get(index);
        Some(match channel {
            "runner-state" => self.runner_state(),
            "runner-sync" => self.runner_sync().await,
            "runner-login" => self.runner_login(a(0)).await,
            "runner-login-cancel" => self.runner_login_cancel(),
            "runner-logout" => self.runner_logout().await,
            "runner-logout-all" => reply(self.service.logout_all().await),
            "runner-search" => self.runner_search(a(0)).await,
            "runner-players" => self.runner_players(a(0)).await,
            "runner-profile-view" => self.runner_profile_view(a(0)).await,
            "runner-rate" => self.runner_rate(a(0), a(1)).await,
            "runner-customize" => self.runner_customize(a(0)).await,
            "runner-speedrun-link" => self.runner_speedrun_link(a(0)).await,
            "runner-speedrun-unlink" => self.runner_speedrun_unlink().await,
            "runner-speedrun-bests" => self.runner_speedrun_bests().await,
            "runner-speedrun-pin" => self.runner_speedrun_pin(a(0)).await,
            "runner-speedrun-unpin" => self.runner_speedrun_unpin().await,
            "runner-discord-code" => self.empty_action("discord/code").await,
            "runner-discord-unlink" => self.empty_action("discord/unlink").await,
            "runner-telegram-code" => self.empty_action("telegram/code").await,
            "runner-telegram-unlink" => self.empty_action("telegram/unlink").await,
            "runner-secret-unlock" => self.runner_secret_unlock(a(0)).await,
            "runner-delete-challenge" => self.runner_delete_challenge().await,
            "runner-delete-request" => self.runner_delete_request(a(0)).await,
            "runner-delete-cancel" => self.runner_delete_cancel().await,
            "runner-open-link" => self.runner_open_link(a(0)).await,
            "profile-get" => self.profile_get(),
            _ => return None,
        })
    }

    /// `runner-state`: снимок.
    pub fn runner_state(&self) -> Value {
        self.service.snapshot()
    }

    /// `runner-sync`.
    pub async fn runner_sync(&self) -> Value {
        self.service.sync().await
    }

    /// `runner-login(consent)`: согласие на телеметрию — только `true`.
    pub async fn runner_login(&self, consent: Option<&Value>) -> Value {
        reply(
            self.service
                .begin_login(consent == Some(&Value::Bool(true)))
                .await,
        )
    }

    /// `runner-login-cancel`.
    pub fn runner_login_cancel(&self) -> Value {
        self.service.cancel_login();
        self.service.changed();
        self.service.snapshot()
    }

    /// `runner-logout`.
    pub async fn runner_logout(&self) -> Value {
        self.service.logout().await
    }

    pub async fn runner_search(&self, query: Option<&Value>) -> Value {
        reply(self.service.search(query).await)
    }

    pub async fn runner_players(&self, query: Option<&Value>) -> Value {
        reply(self.service.players(query).await)
    }

    pub async fn runner_profile_view(&self, id: Option<&Value>) -> Value {
        reply(self.service.view(id).await)
    }

    pub async fn runner_rate(&self, id: Option<&Value>, value: Option<&Value>) -> Value {
        reply(self.service.rate(id, value).await)
    }

    pub async fn runner_customize(&self, data: Option<&Value>) -> Value {
        reply(self.service.action("customize", data).await)
    }

    /// `runner-speedrun-link(key)`: `{apiKey: key}` (без ключа — `{}`).
    pub async fn runner_speedrun_link(&self, key: Option<&Value>) -> Value {
        let body = field_body("apiKey", key);
        reply(self.service.action("speedrun/link", Some(&body)).await)
    }

    pub async fn runner_speedrun_unlink(&self) -> Value {
        reply(
            self.service
                .action("speedrun/unlink", Some(&json!({})))
                .await,
        )
    }

    pub async fn runner_speedrun_bests(&self) -> Value {
        reply(
            self.service
                .action("speedrun/bests", Some(&json!({})))
                .await,
        )
    }

    /// `runner-speedrun-pin(id)`: `{runId: id}`.
    pub async fn runner_speedrun_pin(&self, id: Option<&Value>) -> Value {
        let body = field_body("runId", id);
        reply(self.service.action("speedrun/pin", Some(&body)).await)
    }

    pub async fn runner_speedrun_unpin(&self) -> Value {
        reply(
            self.service
                .action("speedrun/unpin", Some(&json!({})))
                .await,
        )
    }

    /// `runner-secret-unlock(id)`: только `back-to-past`, `wake-up`, `night-shift`, `chronicler`.
    pub async fn runner_secret_unlock(&self, id: Option<&Value>) -> Value {
        reply(self.service.unlock_from_renderer(id).await)
    }

    /// Действие без аргументов окна: тело `{}` (привязки Discord и Telegram — только одноразовым
    /// кодом от сервера).
    async fn empty_action(&self, action: &str) -> Value {
        reply(self.service.action(action, Some(&json!({}))).await)
    }

    pub async fn runner_delete_challenge(&self) -> Value {
        reply(
            self.service
                .action("delete/challenge", Some(&json!({})))
                .await,
        )
    }

    /// `runner-delete-request({challenge, confirmed})`.
    pub async fn runner_delete_request(&self, data: Option<&Value>) -> Value {
        reply(self.service.action("delete/request", data).await)
    }

    pub async fn runner_delete_cancel(&self) -> Value {
        reply(self.service.action("delete/cancel", Some(&json!({}))).await)
    }

    /// `runner-open-link(url)`: только https на хостах Steam, speedrun.com, YouTube, Twitch, без
    /// учётных данных и порта; старые `/runs/<id>` speedrun.com — через его API.
    pub async fn runner_open_link(&self, value: Option<&Value>) -> Value {
        reply(self.open_link(value).await)
    }

    async fn open_link(&self, value: Option<&Value>) -> Result<Value, RunnerError> {
        let Some(text) = value
            .and_then(Value::as_str)
            .filter(|text| hlsr_core::js::text::utf16_len(text) <= MAX_LINK_LENGTH)
        else {
            return Err(RunnerError::plain("RUNNER_INVALID_URL"));
        };
        let mut url = url::Url::parse(text).map_err(|_| RunnerError::invalid_url())?;
        let host = url.host_str().unwrap_or("").to_owned();
        if url.scheme() != "https"
            || !OPEN_LINK_HOSTS.contains(&host.as_str())
            || !url.username().is_empty()
            || url.password().is_some_and(|password| !password.is_empty())
            || url.port().is_some()
        {
            return Err(RunnerError::plain("RUNNER_INVALID_URL"));
        }
        // Старые закреплённые забеги ведут на /runs/<id> без игры — такой страницы на
        // speedrun.com нет, настоящую ссылку знает его API.
        let legacy = (host == "www.speedrun.com")
            .then(|| {
                LEGACY_RUN_PATH
                    .captures(url.path())
                    .map(|found| found[1].to_owned())
            })
            .flatten();
        if let Some(id) = legacy {
            let link = self.service.speedrun_run_link(&id).await?;
            url = url::Url::parse(&link).map_err(|_| RunnerError::invalid_url())?;
        }
        (self.open_external)(url.as_str().to_owned())
            .await
            .map_err(RunnerError::plain)?;
        Ok(Value::Null)
    }

    /// `profile-get` (`game-delivery/index.js:1166`): снимок профиля.
    pub fn profile_get(&self) -> Value {
        self.service.snapshot()
    }

    /// `friends-list({force})` (`game-delivery/index.js:1170`): ошибка не превращается в
    /// `{status: "error"}` — окно получает отказ.
    pub async fn friends_list(&self, options: Option<&Value>) -> Result<Value, RunnerError> {
        let force = options
            .and_then(|options| options.get("force"))
            .is_some_and(|force| *force == Value::Bool(true));
        self.service.friends(force).await
    }

    /// Онлайн-ветка `game-stats-get` (`game-delivery/index.js:1032-1037`): при входе — игра из
    /// снимка сервера (`source: "server"`); `None` — профиля нет (дальше — `GameStatsStore`).
    pub fn game_stats_from_server(&self, game_id: &str) -> Option<Value> {
        if !self.service.has_profile() {
            return None;
        }
        let snapshot = self.service.snapshot();
        let mut out = Map::new();
        out.insert("available".to_owned(), json!(true));
        out.insert("gameId".to_owned(), json!(game_id));
        out.insert("totalPlaySeconds".to_owned(), json!(0));
        out.insert("lastLaunchAt".to_owned(), Value::Null);
        out.insert("launchCount".to_owned(), json!(0));
        out.insert(
            "isRunning".to_owned(),
            json!(self.service.is_game_active(game_id)),
        );
        if let Some(Value::Object(remote)) =
            snapshot.get("games").and_then(|games| games.get(game_id))
        {
            for (key, value) in remote {
                out.insert(key.clone(), value.clone());
            }
        }
        out.insert("source".to_owned(), json!("server"));
        Some(Value::Object(out))
    }
}

/// `{key: value}`; `undefined` — пустой объект (как `JSON.stringify`).
fn field_body(key: &str, value: Option<&Value>) -> Value {
    let mut body = Map::new();
    if let Some(value) = value {
        body.insert(key.to_owned(), value.clone());
    }
    Value::Object(body)
}
