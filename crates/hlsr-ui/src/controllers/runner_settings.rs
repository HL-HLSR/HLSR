//! Настройки профиля (`RunnerSettings.vue` 0.4.9, раздел «Настройки» окна профиля): показ
//! SteamID, приватность, «О себе» (статус, местоимения, страна, ссылки), заготовки подсветки
//! ([`style_presets`], `StylePresetPicker.vue`) и Telegram по коду, speedrun.com, Discord по коду
//! («Как привязать» — 5 шагов, дата буста), выход (и на всех устройствах) и удаление аккаунта с
//! отсчётом. Машина без вида: методы возвращают побочные действия ([`Effect`]), вид
//! (`hlsr-gpui::profile::runner_settings`) выполняет их — вызовы каналов, события окну профиля,
//! таймеры.

pub mod style_presets;

use super::bridge::Tr;
use super::jsv::{is_str, or_zero, prop, str_of, truthy};
use super::profile::{medal_of, place_label};
use super::wall_clock::WallClock;
use crate::intl::{self, DateStyle};
use hlsr_core::js;
use hlsr_core::language::Language;
use hlsr_online::shared::{link_guard, profile_links, pronouns};
use serde_json::{Map, Value, json};

/// Как часто тикает отсчёт удаления.
pub const COUNTDOWN_TICK_MS: u64 = 250;

/// Поля формы «О себе».
const FIELDS: [&str; 6] = [
    "status", "pronouns", "country", "discord", "youtube", "twitch",
];

/// Самый длинный статус (`statusLength > 80`).
pub const STATUS_MAX: usize = 80;

/// Ссылки формы.
pub struct LinkSpec {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
    pub max: i32,
    pub placeholder: &'static str,
    pub hint: Option<&'static str>,
    pub bad: Option<&'static str>,
}

/// `LINKS`.
pub const LINKS: [LinkSpec; 3] = [
    LinkSpec {
        id: "discord",
        icon: "fab fa-discord",
        label: "#L_RUNNER_DISCORD",
        max: 32,
        placeholder: "username",
        hint: None,
        bad: None,
    },
    LinkSpec {
        id: "twitch",
        icon: "fab fa-twitch",
        label: "#L_RUNNER_TWITCH",
        max: 256,
        placeholder: "twitch.tv/…",
        hint: Some("#L_RUNNER_TWITCH_HINT"),
        bad: Some("#L_RUNNER_TWITCH_BAD"),
    },
    LinkSpec {
        id: "youtube",
        icon: "fab fa-youtube",
        label: "#L_RUNNER_YOUTUBE",
        max: 256,
        placeholder: "youtube.com/@…",
        hint: Some("#L_RUNNER_YOUTUBE_HINT"),
        bad: Some("#L_RUNNER_YOUTUBE_BAD"),
    },
];

/// Переключатели приватности (`privacyItems`).
pub const PRIVACY: [(&str, &str, &str); 2] = [
    (
        "hideFromPlayers",
        "#L_RUNNER_SHOW_IN_PLAYERS",
        "#L_RUNNER_SHOW_IN_PLAYERS_HINT",
    ),
    (
        "hideGame",
        "#L_RUNNER_SHOW_GAME",
        "#L_RUNNER_SHOW_GAME_HINT",
    ),
];

/// Чему ответ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    Privacy(String),
    Save,
    LinkSpeedrun,
    UnlinkSpeedrun,
    LoadRuns,
    PinRun,
    UnpinRun,
    Logout,
    LogoutAll,
    PrepareDelete,
    ConfirmDelete,
    CancelDelete,
    /// `public-style-presets-get`.
    Presets,
    /// Выбор заготовки подсветки (`StylePresetPicker.choose`).
    Style,
    /// «Привязать» Telegram с живым кодом: сначала `runner-sync`.
    TelegramSync,
    TelegramCode,
    /// `support-open("bot", код)`.
    TelegramOpen,
    TelegramUnlink,
    /// «Обновить» у Telegram (`runner-sync`).
    TelegramRefresh,
    DiscordCode,
    DiscordUnlink,
    /// Фокус вернулся в окно, пока код Discord на экране (`runner-sync`).
    DiscordSync,
}

/// Код привязки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Telegram,
    Discord,
}

/// Побочное действие.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// `request(channel, value)`: `ipcRenderer.invoke(channel, copy of value)`.
    Request {
        tag: Tag,
        channel: &'static str,
        args: Vec<Value>,
    },
    /// `$emit("updated", snapshot)`: окно профиля принимает снимок.
    Updated(Value),
    /// `ipcRenderer.invoke("runner-open-link", url).catch(() => {})`.
    OpenLink(String),
    /// Отсчёт удаления: таймер включён или выключен.
    Countdown(bool),
    /// Срок кода: через столько мс код пропадает ([`SettingsState::code_expired`]); `None` —
    /// таймер снят.
    CodeTimer(Code, Option<u64>),
}

/// Строка подсказки: текст и `is-error`.
pub type Hint = (String, bool);

/// Строка Telegram (в группе подсветки).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TelegramView {
    pub linked: bool,
    pub label: String,
    /// Подсказки под подписью: сообщение, код и срок или пояснение.
    pub hints: Vec<Hint>,
    pub busy: bool,
}

/// Группа Discord.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscordView {
    pub linked: bool,
    pub label: String,
    pub hint: String,
    /// Код выдан: кнопка «Скопировать команду» вместо «Привязать».
    pub code: bool,
    /// Команда для буфера (`discordCommand`).
    pub command: String,
    pub busy: bool,
    pub help: bool,
    /// Шаги «Как привязать»; у шага со ссылкой «Открыть сервер» — истина.
    pub steps: Vec<(String, bool)>,
    pub invite: &'static str,
    pub message: String,
}

/// Строка списка забегов.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunRow {
    pub id: String,
    pub game: String,
    pub category: String,
    pub place_label: String,
    pub medal: &'static str,
    pub time: String,
    pub pinned: bool,
}

/// Переключатель приватности.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrivacyRow {
    pub flag: &'static str,
    pub label: String,
    pub hint: String,
    pub error: bool,
    /// Включено — «показывать» (`!hidden(flag)`).
    pub on: bool,
    pub disabled: bool,
}

/// Поле ссылки.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkRow {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: String,
    pub value: String,
    pub max: i32,
    pub placeholder: &'static str,
    pub hint: String,
    pub invalid: bool,
}

/// Всё, что показывает раздел.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsView {
    pub steam_id: String,
    pub id_length: i32,
    pub id_revealed: bool,
    pub show_id: bool,
    pub privacy: Vec<PrivacyRow>,
    pub status: String,
    pub status_muted: bool,
    pub status_hint: String,
    pub status_error: bool,
    pub pronouns: String,
    pub pronouns_label: String,
    pub pronouns_items: Vec<(String, String)>,
    pub country: String,
    pub country_label: String,
    pub country_items: Vec<(String, String)>,
    pub links: Vec<LinkRow>,
    pub message: String,
    pub message_ok: bool,
    pub busy: bool,
    pub dirty: bool,
    pub can_save: bool,
    pub speedrun_linked: bool,
    pub speedrun_name: String,
    pub speedrun_hint: String,
    pub pinned: bool,
    pub runs: Vec<RunRow>,
    pub runs_loaded: bool,
    pub speedrun_message: String,
    pub deletion_scheduled: bool,
    pub deletion_hint: String,
    pub confirming: bool,
    pub agreed: bool,
    pub countdown: i32,
    pub account_message: String,
    /// Группа «Подсветка ника и рамка» (заготовки и Telegram).
    pub style_group: bool,
    pub telegram: TelegramView,
    pub discord: DiscordView,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Form {
    status: String,
    pronouns: String,
    country: String,
    discord: String,
    youtube: String,
    twitch: String,
}

impl Form {
    fn get(&self, id: &str) -> &str {
        match id {
            "status" => &self.status,
            "pronouns" => &self.pronouns,
            "country" => &self.country,
            "discord" => &self.discord,
            "youtube" => &self.youtube,
            _ => &self.twitch,
        }
    }

    fn set(&mut self, id: &str, value: String) {
        match id {
            "status" => self.status = value,
            "pronouns" => self.pronouns = value,
            "country" => self.country = value,
            "discord" => self.discord = value,
            "youtube" => self.youtube = value,
            _ => self.twitch = value,
        }
    }
}

/// Состояние раздела (`data()` `RunnerSettings.vue`).
#[derive(Debug, Clone)]
pub struct SettingsState {
    id_revealed: bool,
    form: Form,
    dirty: bool,
    busy: bool,
    /// Ключ сообщения под формой и его тон («ok» — истина).
    form_message: Option<(&'static str, bool)>,
    speedrun_key: String,
    speedrun_message: Option<&'static str>,
    runs: Vec<Value>,
    runs_loaded: bool,
    account_message: Option<&'static str>,
    delete_confirmation: Option<Value>,
    delete_agreed: bool,
    countdown: i64,
    countdown_running: bool,
    privacy_pending: Vec<(String, bool)>,
    privacy_error: Option<String>,
    // Свойства окна профиля.
    account: Value,
    details: Value,
    deletion_at: Option<String>,
    hide_steam_id: bool,
    mute: Option<Value>,
    // 0.4.9: права поддержавшего, заготовки, привязки по коду.
    perks: Value,
    subscriber_badge: bool,
    style_preset: Option<f64>,
    telegram: Value,
    discord_link: Value,
    style_presets: Vec<Value>,
    picker: style_presets::Picker,
    telegram_busy: bool,
    telegram_message: Option<&'static str>,
    /// Тон сообщения Telegram: «error» — истина.
    telegram_error: bool,
    telegram_code: Option<String>,
    /// Конец срока кода, unix-секунды.
    telegram_expires: f64,
    discord_busy: bool,
    discord_code: Option<String>,
    discord_help: bool,
    discord_expires: f64,
    discord_message: Option<&'static str>,
}

/// Результат `request()`: ответ или ключ ошибки.
type Outcome = Result<Value, &'static str>;

impl SettingsState {
    pub fn new() -> SettingsState {
        let mut state = SettingsState {
            id_revealed: false,
            form: Form::default(),
            dirty: false,
            busy: false,
            form_message: None,
            speedrun_key: String::new(),
            speedrun_message: None,
            runs: Vec::new(),
            runs_loaded: false,
            account_message: None,
            delete_confirmation: None,
            delete_agreed: false,
            countdown: 20,
            countdown_running: false,
            privacy_pending: Vec::new(),
            privacy_error: None,
            account: json!({}),
            details: json!({}),
            deletion_at: None,
            hide_steam_id: false,
            mute: None,
            perks: json!({"styles": false, "subscriber": null}),
            subscriber_badge: false,
            style_preset: None,
            telegram: json!({"linked": false, "username": null}),
            discord_link: json!({"linked": false, "username": null, "boosterSince": null}),
            style_presets: Vec::new(),
            picker: style_presets::Picker::default(),
            telegram_busy: false,
            telegram_message: None,
            telegram_error: false,
            telegram_code: None,
            telegram_expires: 0.0,
            discord_busy: false,
            discord_code: None,
            discord_help: false,
            discord_expires: 0.0,
            discord_message: None,
        };
        state.reset_form();
        state
    }

    /// Свойства 0.4.9 `perks`, `subscriberBadge`, `stylePreset`, `telegram`, `discordLink`.
    /// Привязка пришла (`watch` `discordLink.linked`, `telegram.linked`) — код больше не нужен.
    pub fn set_support(
        &mut self,
        perks: &Value,
        subscriber: bool,
        style_preset: Option<&Value>,
        telegram: &Value,
        discord_link: &Value,
    ) -> Vec<Effect> {
        let was = (linked(&self.telegram), linked(&self.discord_link));
        self.perks = perks.clone();
        self.subscriber_badge = subscriber;
        self.style_preset = style_preset.and_then(Value::as_f64);
        self.telegram = telegram.clone();
        self.discord_link = discord_link.clone();
        if !was.0 && linked(&self.telegram) {
            self.telegram_message = None;
            self.telegram_code = None;
        }
        if !was.1 && linked(&self.discord_link) && self.discord_code.is_some() {
            return self.drop_discord_code();
        }
        Vec::new()
    }

    // ---- Свойства окна ---------------------------------------------------------------------

    /// Свойства `account`, `details`, `deletionAt`, `mute` от окна профиля; форма сбрасывается к
    /// `details`, если её не правили (`watch details`).
    pub fn set_props(
        &mut self,
        account: &Value,
        details: &Value,
        deletion_at: Option<&str>,
        mute: Option<&Value>,
    ) {
        self.account = account.clone();
        self.deletion_at = deletion_at.map(str::to_owned);
        self.mute = mute.cloned();
        if &self.details != details {
            self.details = details.clone();
            if !self.dirty {
                self.reset_form();
            }
        }
    }

    pub fn set_hide_steam_id(&mut self, hide: bool) {
        self.hide_steam_id = hide;
    }

    /// `watch active`: уходя с вкладки, SteamID64 снова скрыт.
    pub fn set_active(&mut self, active: bool) {
        if !active {
            self.id_revealed = false;
        }
    }

    /// Окно потеряло фокус или скрыто (`hideId`).
    pub fn hide_id(&mut self) {
        self.id_revealed = false;
    }

    pub fn toggle_id_revealed(&mut self) {
        self.id_revealed = !self.id_revealed;
    }

    pub fn id_revealed(&self) -> bool {
        self.id_revealed
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    // ---- Форма -----------------------------------------------------------------------------

    /// `resetForm()`.
    pub fn reset_form(&mut self) {
        let mut form = Form::default();
        for field in FIELDS {
            form.set(
                field,
                str_of(prop(&self.details, field))
                    .unwrap_or_default()
                    .to_owned(),
            );
        }
        form.pronouns = pronouns::normalize_pronouns(Some(&form.pronouns)).to_owned();
        self.form = form;
        self.dirty = false;
    }

    /// `touch()`: поле правили.
    fn touch(&mut self) {
        self.dirty = true;
        self.form_message = None;
    }

    /// `@input` поля: статус или ссылка.
    pub fn set_field(&mut self, id: &str, text: &str) {
        self.form.set(id, text.to_owned());
        self.touch();
    }

    pub fn set_pronouns(&mut self, id: &str) {
        self.form.pronouns = pronouns::normalize_pronouns(Some(id)).to_owned();
        self.touch();
    }

    pub fn set_country(&mut self, code: &str) {
        self.form.country = code.to_owned();
        self.touch();
    }

    pub fn field(&self, id: &str) -> &str {
        self.form.get(id)
    }

    /// `statusLength`: символы, а не единицы UTF-16 (`Array.from`).
    pub fn status_length(&self) -> usize {
        self.form.status.chars().count()
    }

    fn status_has_link(&self) -> bool {
        link_guard::contains_link(&Value::String(self.form.status.clone()))
    }

    /// `linkInvalid[id]`: ссылка не того сервиса.
    pub fn link_invalid(&self, id: &str) -> bool {
        matches!(id, "youtube" | "twitch")
            && profile_links::normalize(id, &Value::String(self.form.get(id).to_owned())).is_none()
    }

    /// `tidyLink(id)`: после ввода поле показывает, что уйдёт на сервер, — полный адрес канала.
    pub fn tidy_link(&mut self, id: &str) -> bool {
        let url = profile_links::normalize(id, &Value::String(self.form.get(id).to_owned()));
        match url {
            Some(url) if url != self.form.get(id) => {
                self.form.set(id, url);
                true
            }
            _ => false,
        }
    }

    fn can_save(&self) -> bool {
        !self.busy
            && self.dirty
            && self.status_length() <= STATUS_MAX
            && !self.status_has_link()
            && !self.link_invalid("youtube")
            && !self.link_invalid("twitch")
    }

    // ---- Запросы ---------------------------------------------------------------------------

    /// `request(channel, value)` до ответа: `busy`.
    fn request(&mut self, tag: Tag, channel: &'static str, args: Vec<Value>) -> Vec<Effect> {
        self.busy = true;
        vec![Effect::Request { tag, channel, args }]
    }

    /// Ответ `request`: ошибка или результат; снимок уходит окну сразу (до продолжения).
    fn outcome(&mut self, reply: Result<Value, String>, effects: &mut Vec<Effect>) -> Outcome {
        let outcome = match reply {
            Err(_) => Err("#L_RUNNER_NETWORK"),
            Ok(result) => {
                if is_str(prop(&result, "code"), "RUNNER_SPEEDRUN_TAKEN") {
                    Err("#L_RUNNER_SR_TAKEN")
                } else if !truthy(Some(&result)) || str_of(prop(&result, "status")) == Some("error")
                {
                    Err("#L_RUNNER_FORM_ERROR")
                } else {
                    if let Some(snapshot) = prop(&result, "snapshot").filter(|s| truthy(Some(*s))) {
                        effects.push(Effect::Updated(snapshot.clone()));
                    }
                    Ok(result)
                }
            }
        };
        self.busy = false;
        outcome
    }

    /// `setPrivacy(flag, value)`: переключатель сразу показывает новое значение.
    pub fn set_privacy(&mut self, flag: &str, hidden: bool) -> Vec<Effect> {
        self.privacy_pending.retain(|(known, _)| known != flag);
        self.privacy_pending.push((flag.to_owned(), hidden));
        self.privacy_error = None;
        let mut body = Map::new();
        body.insert(flag.to_owned(), Value::Bool(hidden));
        self.request(
            Tag::Privacy(flag.to_owned()),
            "runner-customize",
            vec![Value::Object(body)],
        )
    }

    fn hidden(&self, flag: &str) -> bool {
        match self.privacy_pending.iter().find(|(known, _)| known == flag) {
            Some((_, value)) => *value,
            None => prop(&self.details, flag) == Some(&Value::Bool(true)),
        }
    }

    /// `save()`: форма уходит одной кнопкой; ссылки — полными адресами.
    pub fn save(&mut self) -> Vec<Effect> {
        let mut body = Map::new();
        for field in FIELDS {
            body.insert(field.to_owned(), json!(self.form.get(field)));
        }
        for id in ["youtube", "twitch"] {
            match profile_links::normalize(id, &Value::String(self.form.get(id).to_owned())) {
                Some(url) => {
                    body.insert(id.to_owned(), json!(url));
                }
                None => return Vec::new(),
            }
        }
        self.request(Tag::Save, "runner-customize", vec![Value::Object(body)])
    }

    /// `revert`: «Вернуть».
    pub fn revert(&mut self) {
        self.reset_form();
    }

    pub fn set_speedrun_key(&mut self, key: &str) {
        self.speedrun_key = key.to_owned();
    }

    /// Ключ в поле (стирается, уходя на сервер).
    pub fn speedrun_key(&self) -> &str {
        &self.speedrun_key
    }

    /// `linkSpeedrun()`: ключ уходит на сервер один раз и стирается из поля.
    pub fn link_speedrun(&mut self) -> Vec<Effect> {
        let key = self.speedrun_key.trim().to_owned();
        if key.is_empty() || self.busy {
            return Vec::new();
        }
        self.speedrun_key.clear();
        self.speedrun_message = None;
        self.request(Tag::LinkSpeedrun, "runner-speedrun-link", vec![json!(key)])
    }

    pub fn unlink_speedrun(&mut self) -> Vec<Effect> {
        self.request(
            Tag::UnlinkSpeedrun,
            "runner-speedrun-unlink",
            vec![Value::Null],
        )
    }

    pub fn load_runs(&mut self) -> Vec<Effect> {
        self.request(Tag::LoadRuns, "runner-speedrun-bests", vec![Value::Null])
    }

    pub fn pin_run(&mut self, id: &str) -> Vec<Effect> {
        self.request(Tag::PinRun, "runner-speedrun-pin", vec![json!(id)])
    }

    pub fn unpin_run(&mut self) -> Vec<Effect> {
        self.request(Tag::UnpinRun, "runner-speedrun-unpin", vec![Value::Null])
    }

    pub fn logout(&mut self) -> Vec<Effect> {
        self.request(Tag::Logout, "runner-logout", vec![Value::Null])
    }

    /// «Выйти везде» (`logoutAll`).
    pub fn logout_all(&mut self) -> Vec<Effect> {
        self.request(Tag::LogoutAll, "runner-logout-all", vec![Value::Null])
    }

    // ---- Заготовки, Telegram, Discord (0.4.9) ---------------------------------------------

    /// `loadPresets(force)`: при создании раздела; `force` — заготовку скрыли или удалили.
    pub fn load_presets(&self, force: bool) -> Vec<Effect> {
        let args = if force {
            vec![json!({"force": true})]
        } else {
            Vec::new()
        };
        send(Tag::Presets, "public-style-presets-get", args)
    }

    /// `codeAction(channel, busy, message)` до ответа.
    fn code_action(&mut self, tag: Tag, channel: &'static str) -> Vec<Effect> {
        if matches!(tag, Tag::TelegramCode | Tag::TelegramUnlink) {
            self.telegram_busy = true;
            self.telegram_message = None;
        } else {
            self.discord_busy = true;
            self.discord_message = None;
        }
        send(tag, channel, Vec::new())
    }

    /// Ответ `codeAction`: результат или `None` (сообщение строки уже стоит). Напрямую, а не
    /// через `request()`: тот теряет код ошибки, а 429 нужен.
    fn code_outcome(
        &mut self,
        code: Code,
        reply: Result<Value, String>,
        effects: &mut Vec<Effect>,
    ) -> Option<Value> {
        let (busy, message) = match code {
            Code::Telegram => (&mut self.telegram_busy, &mut self.telegram_message),
            Code::Discord => (&mut self.discord_busy, &mut self.discord_message),
        };
        *busy = false;
        let result = match reply {
            Ok(result) if truthy(Some(&result)) => result,
            _ => {
                *message = Some("#L_RUNNER_NETWORK");
                return None;
            }
        };
        if is_str(prop(&result, "status"), "error") {
            *message = Some(match str_of(prop(&result, "code")) {
                Some("RUNNER_HTTP_429") => "#L_RUNNER_DISCORD_RATE",
                Some("RUNNER_NETWORK") => "#L_RUNNER_NETWORK",
                _ => "#L_RUNNER_FORM_ERROR",
            });
            return None;
        }
        if let Some(snapshot) = prop(&result, "snapshot").filter(|s| truthy(Some(*s))) {
            effects.push(Effect::Updated(snapshot.clone()));
        }
        Some(result)
    }

    /// «Привязать» Telegram (`linkTelegram`): код у сервера (живёт 10 минут), затем бот с ним.
    /// Пока код жив, сначала синхронизация: игрок мог уже привязаться этим кодом.
    pub fn link_telegram(&mut self, now_ms: i64) -> Vec<Effect> {
        self.telegram_message = None;
        if self.telegram_code.is_some() && (now_ms as f64) / 1000.0 < self.telegram_expires {
            return send(Tag::TelegramSync, "runner-sync", Vec::new());
        }
        self.telegram_code = None;
        self.code_action(Tag::TelegramCode, "runner-telegram-code")
    }

    fn open_bot(&self) -> Vec<Effect> {
        send(
            Tag::TelegramOpen,
            "support-open",
            vec![json!("bot"), json!(self.telegram_code)],
        )
    }

    /// «Отвязать» Telegram: сервер снимает все Telegram профиля.
    pub fn unlink_telegram(&mut self) -> Vec<Effect> {
        self.telegram_code = None;
        self.code_action(Tag::TelegramUnlink, "runner-telegram-unlink")
    }

    /// «Обновить» у Telegram: синхронизация профиля сейчас.
    pub fn refresh_telegram(&mut self) -> Vec<Effect> {
        if self.telegram_busy {
            return Vec::new();
        }
        self.telegram_busy = true;
        self.telegram_message = None;
        send(Tag::TelegramRefresh, "runner-sync", Vec::new())
    }

    /// «Привязать» Discord (`requestDiscordCode`).
    pub fn request_discord_code(&mut self) -> Vec<Effect> {
        self.code_action(Tag::DiscordCode, "runner-discord-code")
    }

    /// `dropDiscordCode()`: код и его таймер сняты, синхронизации по фокусу больше нет.
    fn drop_discord_code(&mut self) -> Vec<Effect> {
        self.discord_code = None;
        vec![Effect::CodeTimer(Code::Discord, None)]
    }

    pub fn unlink_discord(&mut self) -> Vec<Effect> {
        let mut effects = self.drop_discord_code();
        effects.extend(self.code_action(Tag::DiscordUnlink, "runner-discord-unlink"));
        effects
    }

    /// Фокус вернулся в окно (`syncDiscord`): пока код на экране, игрок мог уже ввести его.
    pub fn window_focused(&mut self) -> Vec<Effect> {
        if self.discord_code.is_none() {
            return Vec::new();
        }
        send(Tag::DiscordSync, "runner-sync", Vec::new())
    }

    /// «Как привязать»: шаги по щелчку.
    pub fn toggle_discord_help(&mut self) {
        self.discord_help = !self.discord_help;
    }

    /// Срок кода вышел: код пропадает, «Привязать» снова возьмёт новый.
    pub fn code_expired(&mut self, code: Code) {
        match code {
            Code::Telegram => self.telegram_code = None,
            Code::Discord => self.discord_code = None,
        }
    }

    /// Ответы каналов 0.4.9, которые идут мимо `request()`.
    fn answer_support(
        &mut self,
        tag: &Tag,
        reply: Result<Value, String>,
        now_ms: i64,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        match tag {
            Tag::Presets => {
                // При сбое — последний список main; без списка группа держится на профиле.
                if let Some(list) = reply
                    .ok()
                    .as_ref()
                    .and_then(|r| prop(r, "presets"))
                    .and_then(Value::as_array)
                {
                    self.style_presets = list.clone();
                }
            }
            Tag::Style => effects = self.style_answer(reply),
            Tag::TelegramSync => {
                let snapshot = reply.ok().unwrap_or(Value::Null);
                if truthy(prop(&snapshot, "account")) {
                    effects.push(Effect::Updated(snapshot.clone()));
                }
                if linked(prop(&snapshot, "telegram").unwrap_or(&Value::Null)) {
                    self.telegram_code = None;
                    return effects;
                }
                effects.extend(self.open_bot());
            }
            Tag::TelegramCode => {
                let result = self.code_outcome(Code::Telegram, reply, &mut effects);
                let Some((code, expires)) = result.as_ref().and_then(code_of) else {
                    if result.is_some() {
                        self.telegram_message = Some("#L_RUNNER_FORM_ERROR");
                    }
                    self.telegram_error = true;
                    return effects;
                };
                self.telegram_code = Some(code);
                self.telegram_expires = expires;
                effects.push(timer(Code::Telegram, expires, now_ms));
                effects.extend(self.open_bot());
            }
            Tag::TelegramOpen => {
                if !matches!(&reply, Ok(r) if is_str(prop(r, "status"), "ok")) {
                    self.telegram_error = true;
                    self.telegram_message = Some("#L_RUNNER_TELEGRAM_OPEN_ERROR");
                }
            }
            Tag::TelegramUnlink => {
                if self
                    .code_outcome(Code::Telegram, reply, &mut effects)
                    .is_none()
                {
                    self.telegram_error = true;
                }
            }
            Tag::TelegramRefresh => {
                self.telegram_busy = false;
                match reply {
                    Ok(snapshot) if truthy(prop(&snapshot, "account")) => {
                        effects.push(Effect::Updated(snapshot.clone()));
                        if truthy(prop(&snapshot, "error")) {
                            self.telegram_error = true;
                            self.telegram_message = Some("#L_RUNNER_NETWORK");
                        } else if !linked(prop(&snapshot, "telegram").unwrap_or(&Value::Null)) {
                            self.telegram_error = false;
                            self.telegram_message = Some("#L_RUNNER_TELEGRAM_NOT_YET");
                        }
                    }
                    _ => {
                        self.telegram_error = true;
                        self.telegram_message = Some("#L_RUNNER_NETWORK");
                    }
                }
            }
            Tag::DiscordCode => {
                let result = self.code_outcome(Code::Discord, reply, &mut effects);
                // Как в 0.4.9: и отказ сервера (429 тоже) здесь кончается общим текстом.
                let Some((code, expires)) = result.as_ref().and_then(code_of) else {
                    self.discord_message = Some("#L_RUNNER_FORM_ERROR");
                    return effects;
                };
                self.discord_code = Some(code);
                self.discord_expires = expires;
                effects.push(timer(Code::Discord, expires, now_ms));
            }
            Tag::DiscordUnlink => {
                self.code_outcome(Code::Discord, reply, &mut effects);
            }
            Tag::DiscordSync => {
                if let Ok(snapshot) = &reply
                    && truthy(prop(snapshot, "account"))
                {
                    effects.push(Effect::Updated(snapshot.clone()));
                }
                if self.discord_code.is_some() && (now_ms as f64) / 1000.0 >= self.discord_expires {
                    effects.extend(self.drop_discord_code());
                }
            }
            _ => {}
        }
        effects
    }

    /// Группа «Подсветка ника и рамка»: есть заготовки (новый сервер) или профиль сам говорит о
    /// поддержке — право, подписка, привязка Telegram.
    fn style_group(&self) -> bool {
        !self.style_presets.is_empty()
            || prop(&self.perks, "styles") == Some(&Value::Bool(true))
            || truthy(prop(&self.perks, "subscriber"))
            || linked(&self.telegram)
    }

    fn telegram_view(&self, lang: Language, wall: &WallClock, tr: &Tr) -> TelegramView {
        let t = |key: &str| tr(key, &[]);
        let is_linked = linked(&self.telegram);
        let label = match str_of(prop(&self.telegram, "username")).filter(|u| !u.is_empty()) {
            _ if !is_linked => t("#L_RUNNER_TELEGRAM_NOT_LINKED"),
            Some(name) => tr(
                "#L_RUNNER_TELEGRAM_LINKED_AS",
                &[format!("@{name}").as_str()],
            ),
            None => t("#L_RUNNER_TELEGRAM_LINKED"),
        };
        // «Привязка пока не найдена» рядом с «Telegram: привязан» была бы неправдой.
        let note = self
            .telegram_message
            .filter(|key| !(*key == "#L_RUNNER_TELEGRAM_NOT_YET" && is_linked));
        let hints = match (note, &self.telegram_code) {
            (Some(key), _) => vec![(t(key), self.telegram_error)],
            (None, Some(code)) if !is_linked => vec![
                (tr("#L_RUNNER_TELEGRAM_CODE", &[code.as_str()]), false),
                (
                    tr(
                        "#L_RUNNER_DISCORD_UNTIL",
                        &[clock(lang, wall, self.telegram_expires).as_str()],
                    ),
                    false,
                ),
            ],
            _ => vec![(
                t(if is_linked {
                    "#L_RUNNER_TELEGRAM_HINT"
                } else {
                    "#L_RUNNER_TELEGRAM_LINK_HINT"
                }),
                false,
            )],
        };
        TelegramView {
            linked: is_linked,
            label,
            hints,
            busy: self.telegram_busy,
        }
    }

    fn discord_view(&self, lang: Language, wall: &WallClock, tr: &Tr) -> DiscordView {
        let t = |key: &str| tr(key, &[]);
        let link = &self.discord_link;
        let is_linked = linked(link);
        let label = match str_of(prop(link, "username")).filter(|u| !u.is_empty()) {
            _ if !is_linked => t("#L_RUNNER_DISCORD_NOT_LINKED"),
            Some(name) => tr("#L_RUNNER_DISCORD_LINKED", &[name]),
            None => t("#L_RUNNER_DISCORD_LINKED_PLAIN"),
        };
        let booster = prop(link, "boosterSince")
            .filter(|v| truthy(Some(*v)))
            .and_then(Value::as_f64);
        let hint = match (is_linked, booster, &self.discord_code) {
            (true, Some(since), _) => tr(
                "#L_RUNNER_DISCORD_BOOST",
                &[wall
                    .format(lang, DateStyle::DateLong, since * 1000.0)
                    .as_str()],
            ),
            (true, None, _) => String::new(),
            (false, _, Some(_)) => tr(
                "#L_RUNNER_DISCORD_UNTIL",
                &[clock(lang, wall, self.discord_expires).as_str()],
            ),
            (false, _, None) => t("#L_RUNNER_DISCORD_HINT"),
        };
        // В русском тексте обе формы: при английском Discord код не должен уйти обычным
        // сообщением. До выдачи кода в подсказке стоит слово «код».
        let shown = self
            .discord_code
            .clone()
            .unwrap_or_else(|| t("#L_RUNNER_DISCORD_CODE_WORD"));
        let command = format!("/{} {shown}", t("#L_RUNNER_DISCORD_COMMAND"));
        let alt = format!("/link {shown}");
        let command_text = if command == alt {
            alt
        } else {
            format!("{command} ({} {alt})", t("#L_RUNNER_DISCORD_OR"))
        };
        let mut steps = Vec::new();
        if self.discord_code.is_none() {
            steps.push((t("#L_RUNNER_DISCORD_STEP_CODE"), false));
        }
        steps.push((t("#L_RUNNER_DISCORD_STEP_OPEN"), true));
        steps.push((
            tr("#L_RUNNER_DISCORD_STEP_TYPE", &[command_text.as_str()]),
            false,
        ));
        steps.push((t("#L_RUNNER_DISCORD_STEP_CHECK"), false));
        steps.push((t("#L_RUNNER_DISCORD_STEP_BACK"), false));
        DiscordView {
            linked: is_linked,
            label,
            hint,
            code: self.discord_code.is_some(),
            command,
            busy: self.discord_busy,
            help: self.discord_help,
            steps,
            invite: hlsr_delivery::links::url_for(&json!("discord")).unwrap_or_default(),
            message: self.discord_message.map_or(String::new(), t),
        }
    }

    pub fn prepare_delete(&mut self) -> Vec<Effect> {
        self.account_message = None;
        self.request(
            Tag::PrepareDelete,
            "runner-delete-challenge",
            vec![Value::Null],
        )
    }

    pub fn close_delete(&mut self) -> Vec<Effect> {
        self.delete_confirmation = None;
        let was = std::mem::replace(&mut self.countdown_running, false);
        if was {
            vec![Effect::Countdown(false)]
        } else {
            Vec::new()
        }
    }

    pub fn set_delete_agreed(&mut self, agreed: bool) {
        self.delete_agreed = agreed;
    }

    pub fn confirm_delete(&mut self) -> Vec<Effect> {
        let Some(confirmation) = &self.delete_confirmation else {
            return Vec::new();
        };
        let mut body = Map::new();
        if let Some(challenge) = prop(confirmation, "challenge") {
            body.insert("challenge".to_owned(), challenge.clone());
        }
        body.insert("confirmed".to_owned(), Value::Bool(self.delete_agreed));
        self.request(
            Tag::ConfirmDelete,
            "runner-delete-request",
            vec![Value::Object(body)],
        )
    }

    pub fn cancel_delete(&mut self) -> Vec<Effect> {
        self.request(Tag::CancelDelete, "runner-delete-cancel", vec![Value::Null])
    }

    /// `tick()`: секунд до `readyAt`; истина, когда отсчёт кончился (таймер снимается).
    pub fn tick(&mut self, now_ms: i64) -> bool {
        let Some(confirmation) = &self.delete_confirmation else {
            return false;
        };
        let ready = str_of(prop(confirmation, "readyAt"))
            .and_then(js::date_parse)
            .map_or(f64::NAN, |ms| ms as f64);
        let left = ((ready - now_ms as f64) / 1000.0).ceil();
        self.countdown = if left.is_nan() {
            0
        } else {
            left.max(0.0) as i64
        };
        if self.countdown == 0 && self.countdown_running {
            self.countdown_running = false;
            return true;
        }
        false
    }

    /// Ответ на запрос `tag`; `now_ms` — `Date.now()` для первого отсчёта.
    pub fn answer(&mut self, tag: Tag, reply: Result<Value, String>, now_ms: i64) -> Vec<Effect> {
        if matches!(
            tag,
            Tag::Presets
                | Tag::Style
                | Tag::TelegramSync
                | Tag::TelegramCode
                | Tag::TelegramOpen
                | Tag::TelegramUnlink
                | Tag::TelegramRefresh
                | Tag::DiscordCode
                | Tag::DiscordUnlink
                | Tag::DiscordSync
        ) {
            return self.answer_support(&tag, reply, now_ms);
        }
        let mut effects = Vec::new();
        let outcome = self.outcome(reply, &mut effects);
        match tag {
            Tag::Privacy(flag) => {
                if outcome.is_err() {
                    self.privacy_error = Some(flag.clone());
                }
                self.privacy_pending.retain(|(known, _)| *known != flag);
            }
            Tag::Save => match outcome {
                Err(error) => self.form_message = Some((error, false)),
                Ok(_) => {
                    self.dirty = false;
                    self.form_message = Some(("#L_RUNNER_SAVED", true));
                }
            },
            Tag::LinkSpeedrun => {
                if let Err(error) = outcome {
                    self.speedrun_message = Some(
                        if matches!(error, "#L_RUNNER_NETWORK" | "#L_RUNNER_SR_TAKEN") {
                            error
                        } else {
                            "#L_RUNNER_SR_KEY_ERROR"
                        },
                    );
                }
            }
            Tag::UnlinkSpeedrun => {
                self.speedrun_message = outcome.as_ref().err().copied();
                if outcome.is_ok() {
                    self.runs.clear();
                    self.runs_loaded = false;
                }
            }
            Tag::LoadRuns => {
                self.speedrun_message = outcome.as_ref().err().copied();
                if let Ok(result) = outcome {
                    self.runs = prop(&result, "runs")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    self.runs_loaded = true;
                }
            }
            Tag::PinRun | Tag::UnpinRun => {
                self.speedrun_message = outcome.as_ref().err().copied();
            }
            Tag::Logout | Tag::LogoutAll => match outcome {
                Err(error) => self.account_message = Some(error),
                Ok(result) => effects.push(Effect::Updated(result)),
            },
            Tag::PrepareDelete => match outcome {
                Err(error) => self.account_message = Some(error),
                Ok(result) => {
                    self.delete_confirmation = Some(result);
                    self.delete_agreed = false;
                    self.tick(now_ms);
                    // Таймер идёт всегда: нулевой отсчёт снимает его первым же тиком.
                    self.countdown_running = true;
                    effects.push(Effect::Countdown(true));
                }
            },
            Tag::ConfirmDelete => match outcome {
                Err(error) => self.account_message = Some(error),
                Ok(_) => effects.extend(self.close_delete()),
            },
            Tag::CancelDelete => {
                self.account_message = outcome.err();
            }
            _ => {}
        }
        effects
    }

    /// Окно закрыто или раздел снят (`beforeUnmount`): ключ стёрт, таймер остановлен.
    pub fn unmounted(&mut self) -> Vec<Effect> {
        self.speedrun_key.clear();
        let was = std::mem::replace(&mut self.countdown_running, false);
        if was {
            vec![Effect::Countdown(false)]
        } else {
            Vec::new()
        }
    }

    // ---- Показ -----------------------------------------------------------------------------

    pub fn view(&self, lang: Language, wall: &WallClock, tr: &Tr) -> SettingsView {
        let t = |key: &str| tr(key, &[]);
        let locale = t("#L_RUNNER_LOCALE");
        let steam_id = str_of(prop(&self.account, "steamId64"))
            .unwrap_or_default()
            .to_owned();
        let status_length = self.status_length();
        let status_has_link = self.status_has_link();
        let (status_hint, status_error) = match &self.mute {
            Some(mute) => {
                let until = str_of(prop(mute, "expiresAt")).filter(|at| !at.is_empty());
                let mut text = match until {
                    Some(at) => tr("#L_MOD_MUTED_UNTIL", &[mute_until(lang, wall, at).as_str()]),
                    None => t("#L_MOD_MUTED_FOREVER"),
                };
                if let Some(reason) = str_of(prop(mute, "reason")).filter(|r| !r.is_empty()) {
                    text.push(' ');
                    text.push_str(&tr("#L_MOD_REASON", &[reason]));
                }
                (text, true)
            }
            None => (
                if status_has_link {
                    t("#L_RUNNER_STATUS_LINKS")
                } else {
                    tr(
                        "#L_RUNNER_STATUS_HINT",
                        &[status_length.to_string().as_str()],
                    )
                },
                status_length > STATUS_MAX || status_has_link,
            ),
        };
        let pronoun_items: Vec<(String, String)> =
            std::iter::once((String::new(), t("#L_RUNNER_NOT_SET")))
                .chain(
                    pronouns::pronoun_list(&locale)
                        .into_iter()
                        .map(|(id, label)| (id.to_owned(), label.to_owned())),
                )
                .collect();
        let pronouns_label = {
            let label = pronouns::pronoun_label(Some(&self.form.pronouns), &locale);
            if label.is_empty() {
                t("#L_RUNNER_NOT_SET")
            } else {
                label.to_owned()
            }
        };
        let country_items: Vec<(String, String)> =
            std::iter::once((String::new(), t("#L_RUNNER_NOT_SET")))
                .chain(
                    intl::countries(lang)
                        .iter()
                        .map(|country| (country.id.clone(), country.label.clone())),
                )
                .collect();
        let country_label = country_items
            .iter()
            .find(|(id, _)| *id == self.form.country)
            .map_or_else(|| self.form.country.clone(), |(_, label)| label.clone());
        let links = LINKS
            .iter()
            .map(|spec| {
                let invalid = self.link_invalid(spec.id);
                LinkRow {
                    id: spec.id,
                    icon: spec.icon,
                    label: t(spec.label),
                    value: self.form.get(spec.id).to_owned(),
                    max: spec.max,
                    placeholder: spec.placeholder,
                    hint: match (invalid, spec.hint, spec.bad) {
                        (true, _, Some(bad)) => t(bad),
                        (_, Some(hint), _) => t(hint),
                        _ => String::new(),
                    },
                    invalid,
                }
            })
            .collect();
        let pinned = prop(&self.details, "pinnedRun").filter(|run| truthy(Some(*run)));
        let pinned_text = pinned.map_or(String::new(), |run| {
            format!(
                "{} · {} · {}",
                str_of(prop(run, "game")).unwrap_or_default(),
                str_of(prop(run, "category")).unwrap_or_default(),
                super::home::format_run(or_zero(prop(run, "seconds")))
            )
        });
        let speedrun = prop(&self.details, "speedrun").filter(|value| truthy(Some(*value)));
        let pinned_id = pinned.and_then(|run| str_of(prop(run, "id")));
        let runs = self
            .runs
            .iter()
            .map(|run| {
                let place = or_zero(prop(run, "place")) as i64;
                RunRow {
                    id: str_of(prop(run, "id")).unwrap_or_default().to_owned(),
                    game: str_of(prop(run, "game")).unwrap_or_default().to_owned(),
                    category: str_of(prop(run, "category")).unwrap_or_default().to_owned(),
                    place_label: if place > 0 {
                        place_label(place, &locale, tr)
                    } else {
                        String::new()
                    },
                    medal: medal_of(place),
                    time: super::home::format_run(or_zero(prop(run, "seconds"))),
                    pinned: pinned_id.is_some() && pinned_id == str_of(prop(run, "id")),
                }
            })
            .collect();
        let deletion = self.deletion_at.as_deref().map(|at| {
            let ms = js::date_parse(at).map_or(f64::NAN, |ms| ms as f64);
            wall.format(lang, DateStyle::DateTimeDefault, ms)
        });
        SettingsView {
            id_length: if steam_id.is_empty() {
                17
            } else {
                steam_id.chars().count() as i32
            },
            steam_id,
            id_revealed: self.id_revealed,
            show_id: !self.hide_steam_id,
            privacy: PRIVACY
                .iter()
                .map(|(flag, label, hint)| {
                    let error = self.privacy_error.as_deref() == Some(*flag);
                    PrivacyRow {
                        flag,
                        label: t(label),
                        hint: if error {
                            t("#L_RUNNER_FORM_ERROR")
                        } else {
                            t(hint)
                        },
                        error,
                        on: !self.hidden(flag),
                        disabled: self.privacy_pending.iter().any(|(known, _)| known == flag),
                    }
                })
                .collect(),
            status: self.form.status.clone(),
            status_muted: self.mute.is_some(),
            status_hint,
            status_error,
            pronouns: self.form.pronouns.clone(),
            pronouns_label,
            pronouns_items: pronoun_items,
            country: self.form.country.clone(),
            country_label,
            country_items,
            links,
            message: self.form_message.map_or(String::new(), |(key, _)| t(key)),
            message_ok: self.form_message.is_some_and(|(_, ok)| ok),
            busy: self.busy,
            dirty: self.dirty,
            can_save: self.can_save(),
            speedrun_linked: speedrun.is_some(),
            speedrun_name: speedrun
                .and_then(|value| str_of(prop(value, "name")))
                .unwrap_or_default()
                .to_owned(),
            speedrun_hint: if pinned.is_some() {
                tr("#L_RUNNER_SR_PINNED", &[pinned_text.as_str()])
            } else {
                t("#L_RUNNER_SR_LINKED")
            },
            pinned: pinned.is_some(),
            runs,
            runs_loaded: self.runs_loaded,
            speedrun_message: self.speedrun_message.map_or(String::new(), t),
            deletion_scheduled: self.deletion_at.is_some(),
            deletion_hint: deletion.map_or(String::new(), |date| {
                tr("#L_RUNNER_DELETE_PENDING", &[date.as_str()])
            }),
            confirming: self.delete_confirmation.is_some(),
            agreed: self.delete_agreed,
            countdown: self.countdown as i32,
            account_message: self.account_message.map_or(String::new(), t),
            style_group: self.style_group(),
            telegram: self.telegram_view(lang, wall, tr),
            discord: self.discord_view(lang, wall, tr),
        }
    }
}

/// `x.linked` истинно.
fn linked(value: &Value) -> bool {
    truthy(prop(value, "linked"))
}

/// `ipcRenderer.invoke(channel, …args)` без `busy` раздела.
fn send(tag: Tag, channel: &'static str, args: Vec<Value>) -> Vec<Effect> {
    vec![Effect::Request { tag, channel, args }]
}

/// Код и срок ответа: `/^\d{6}$/.test(code) && Number.isSafeInteger(expiresAt)`.
fn code_of(result: &Value) -> Option<(String, f64)> {
    let code = match prop(result, "code")? {
        Value::String(code) => code.clone(),
        Value::Number(code) => code.to_string(),
        _ => return None,
    };
    let expires = prop(result, "expiresAt")?.as_f64()?;
    let safe = expires.fract() == 0.0 && expires.abs() <= 9_007_199_254_740_991.0;
    (code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()) && safe).then_some((code, expires))
}

/// Таймер срока кода: `Math.max(0, expiresAt * 1000 - Date.now())`.
fn timer(code: Code, expires: f64, now_ms: i64) -> Effect {
    Effect::CodeTimer(
        code,
        Some((expires * 1000.0 - now_ms as f64).max(0.0) as u64),
    )
}

/// `discordTime(seconds)`: «14:05» по часам игрока.
fn clock(lang: Language, wall: &WallClock, seconds: f64) -> String {
    wall.format(lang, DateStyle::Time, seconds * 1000.0)
}

impl Default for SettingsState {
    fn default() -> Self {
        SettingsState::new()
    }
}

/// `muteUntil`: `toLocaleString(ru-RU | en-GB, {day, month: "long", hour, minute})`.
fn mute_until(lang: Language, wall: &WallClock, iso: &str) -> String {
    let ms = js::date_parse(iso).map_or(f64::NAN, |ms| ms as f64);
    wall.format(lang, DateStyle::DateTimeNoYear, ms)
}

#[cfg(test)]
mod tests;
