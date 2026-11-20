//! Колокольчик и панель уведомлений (`NotificationCenter.vue` 0.4.9): список из
//! `notifications-list`, счётчик, заголовки и подробности по кодам, время, кнопки действий и
//! очередь всплывающих превью (4,8 с, пауза при наведении, «возврат в колокольчик»). Машина без
//! Slint: побочные действия возвращает списком ([`Effect`]); ответы сервисов и таймер превью
//! возвращаются в неё методами. 0.4.9: коды `PRESERVE_FAILED`, `RENDER_ACTIVE`,
//! `SUPPORT_THANKS`, подсказка вместо сетевой ошибки прокси (`shared/proxy-error.js`) и
//! достижение `hoarder` за 50 пунктов в списке.

use super::account::short_name;
use super::bridge::Tr;
use super::jsv::{field, prop, str_of, truthy};
use super::wall_clock::WallClock;
use crate::intl::DateStyle;
use crate::services::Reply;
use hlsr_core::language::Language;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::rc::Rc;

/// Сколько показывается превью (`previewRemaining`).
pub const PREVIEW_MS: f64 = 4800.0;
/// Свежее при первом снимке: `Date.now() - item.ts < 15000`.
const FRESH_MS: f64 = 15_000.0;
/// Сколько превью ставится в очередь за один снимок.
const PREVIEW_BATCH: usize = 5;

const TITLE_KEYS: &[(&str, &str)] = &[
    ("STEAM_NOT_INSTALLED", "#L_S_LAUNCH_STEAM_NOT_INSTALLED"),
    ("STEAM_NOT_RUNNING", "#L_S_LAUNCH_STEAM_NOT_RUNNING"),
    ("STEAM_PROCESS_MISMATCH", "#L_S_LAUNCH_STEAM_NOT_RUNNING"),
    ("SOURCE_SDK_NOT_INSTALLED", "#L_S_LAUNCH_SDK_MISSING"),
    ("SOURCE_SDK_INCOMPLETE", "#L_S_LAUNCH_SDK_MISSING"),
    ("LU_UPDATE_CONFIG_MISSING", "#L_N_UPDATE_CONFIG"),
    ("PROFILE_IMPORTED", "#L_N_PROFILE_IMPORTED"),
    ("GAME_CRASHED", "#L_N_GAME_CRASHED"),
    ("LAUNCH_FAILED", "#L_N_LAUNCH_FAILED"),
    ("LAUNCH_CONFIG_INVALID", "#L_N_LAUNCH_FAILED"),
    ("GAME_EXECUTABLE_MISSING", "#L_N_GAME_FILES"),
    ("GAME_DIRECTORY_MISSING", "#L_N_GAME_FILES"),
    ("LIBRARY_PATH_CASE_CONFLICT", "#L_C_CASE_CONFLICT_TITLE"),
    ("STEAM_APP_ID_MISMATCH", "#L_N_GAME_FILES"),
    ("FILES_MANIFEST_INVALID", "#L_N_GAME_FILES"),
    ("SIGNATURE_INVALID", "#L_N_UPDATE_REJECTED"),
    ("CHECKSUM_MISMATCH", "#L_D_ERR_CHECKSUM_MISMATCH"),
    ("ARCHIVE_INVALID", "#L_N_DELIVERY"),
    ("ARCHIVE_UNSUPPORTED", "#L_N_DELIVERY"),
    ("PATCH_INVALID", "#L_N_DELIVERY"),
    ("PATCH_FAILED", "#L_N_DELIVERY"),
    ("PATCH_CHAIN_MISSING", "#L_D_ERR_PATCH_CHAIN_MISSING"),
    ("INSTALL_FAILED", "#L_N_DELIVERY"),
    ("PRESERVE_FAILED", "#L_N_DELIVERY"),
    ("ANTIVIRUS_BLOCKED", "#L_N_ANTIVIRUS"),
    ("UPDATE_FAILED", "#L_N_DELIVERY"),
    ("REPAIR_FAILED", "#L_N_REPAIR"),
    ("REPAIR_SOURCE_MISSING", "#L_D_ERR_REPAIR_SOURCE_MISSING"),
    ("NO_SPACE", "#L_D_ERR_NO_SPACE"),
    ("PATH_NOT_WRITABLE", "#L_N_LIBRARY"),
    ("PATH_INVALID", "#L_N_LIBRARY"),
    ("FILE_LOCKED", "#L_D_ERR_FILE_LOCKED"),
    ("GAME_RUNNING", "#L_D_ERR_GAME_RUNNING"),
    // Операция с игрой во время экспорта клипа из демки (инструмент «Демо»).
    ("RENDER_ACTIVE", "#L_D_ERR_RENDER_ACTIVE"),
    ("MANIFEST_UNAVAILABLE", "#L_N_FEED_OFFLINE"),
    ("NETWORK_ERROR", "#L_N_FEED_OFFLINE"),
    ("FEED_UNAVAILABLE", "#L_N_FEED_OFFLINE"),
    ("LU_FEED_UNAVAILABLE", "#L_N_FEED_OFFLINE"),
    ("LU_FEED_STALE", "#L_N_FEED_STALE"),
    ("UPDATE_CHECK_TIMEOUT", "#L_N_FEED_OFFLINE"),
    ("LU_SIGNATURE_INVALID", "#L_N_UPDATE_REJECTED"),
    ("LU_SCHEMA_INVALID", "#L_N_UPDATE_REJECTED"),
    ("LU_INSTALLER_SHA256", "#L_N_UPDATE_REJECTED"),
    ("LU_INSTALLER_SHA512", "#L_N_UPDATE_REJECTED"),
    ("LU_INSTALLER_SIZE", "#L_N_UPDATE_REJECTED"),
    ("LU_VERSION_MISMATCH", "#L_N_UPDATE_REJECTED"),
    ("LU_VERSION_UNSUPPORTED", "#L_N_UPDATE_REJECTED"),
    ("LU_LATEST_YML_MISMATCH", "#L_N_UPDATE_REJECTED"),
    ("LAUNCHER_UPDATE_ERROR", "#L_N_UPDATE_LAUNCHER"),
    ("STARTUP_ERROR", "#L_N_STARTUP_ERROR"),
    ("LIBRARY_LEGACY_LAYOUT", "#L_N_LEGACY_LIBRARY"),
    ("PROTON_UPDATED", "#L_N_PROTON_UPDATED"),
    ("PROTON_INSTALLING", "#L_N_PROTON_INSTALLING"),
    ("PROTON_INSTALLED", "#L_N_PROTON_INSTALLED"),
    ("PROTON_INSTALL_FAILED", "#L_S_PROTON_ERROR"),
    ("PROTON_UPDATE_AVAILABLE", "#L_N_PROTON_UPDATE_AVAILABLE"),
    ("LAUNCHER_UPDATE_AVAILABLE", "#L_N_V_LAUNCHER_AVAILABLE"),
    ("LAUNCHER_UPDATED", "#L_N_V_LAUNCHER_UPDATED"),
    ("GAME_UPDATE_AVAILABLE", "#L_N_V_GAME_AVAILABLE"),
    ("GAME_INSTALLED", "#L_N_V_GAME_INSTALLED"),
    ("GAME_UPDATED", "#L_N_V_GAME_UPDATED"),
    ("CLIENT_MODIFIED", "#L_N_CLIENT_MODIFIED"),
    ("SUPPORT_THANKS", "#L_N_SUPPORT_THANKS"),
    ("MOD_BAN", "#L_N_MOD_BAN"),
    ("MOD_UNBAN", "#L_N_MOD_UNBAN"),
    ("MOD_MUTE", "#L_N_MOD_MUTE"),
    ("MOD_UNMUTE", "#L_N_MOD_UNMUTE"),
    ("MOD_RESET_STATUS", "#L_N_MOD_RESET_STATUS"),
    ("MOD_RESET_NICK", "#L_N_MOD_RESET_NICK"),
    ("MOD_RESET_AVATAR", "#L_N_MOD_RESET_AVATAR"),
    ("MOD_RESTORE_NICK", "#L_N_MOD_RESTORE_NICK"),
    ("MOD_RESTORE_AVATAR", "#L_N_MOD_RESTORE_AVATAR"),
];

const VERSION_CODES: &[&str] = &[
    "PROTON_UPDATE_AVAILABLE",
    "LAUNCHER_UPDATE_AVAILABLE",
    "LAUNCHER_UPDATED",
    "GAME_UPDATE_AVAILABLE",
    "GAME_INSTALLED",
    "GAME_UPDATED",
];
const VERSIONED_TITLE_CODES: &[&str] = &["LAUNCHER_UPDATED", "GAME_UPDATED"];
const GAME_TITLE_CODES: &[&str] = &[
    "GAME_UPDATE_AVAILABLE",
    "GAME_INSTALLED",
    "GAME_UPDATED",
    "GAME_CRASHED",
];
const DEP_HINTS: &[(&str, &str)] = &[
    ("DEP_ALWAYS_ON", "#L_CR_DEP_ALWAYS_ON"),
    ("DEP_EXEMPTION_FAILED", "#L_CR_DEP_EXEMPTION_FAILED"),
];

/// Побочное действие машины.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// `ipcRenderer.invoke(channel, ...args)`; ответ вернуть методом по `tag`.
    Invoke {
        tag: Tag,
        channel: &'static str,
        args: Vec<Value>,
    },
    /// `setTimeout(dismissPreview, ms)`.
    StartPreviewTimer(f64),
    StopPreviewTimer,
    /// `uiState.openDropdowns++` / `--`.
    OpenMenus(i32),
    /// Фокус на колокольчик.
    FocusToggle,
    /// `$nextTick(nextPreview)`: вызвать [`NotifMachine::tick_after_dismiss`] со следующего такта.
    NextTick,
    /// Событие оболочке.
    Emit(Event),
    /// `navigator.clipboard.writeText(text)`.
    Copy(String),
}

/// Чему ответ.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    /// `notifications-list`, номер запроса.
    List(u64),
    /// Остальные каналы: после ответа — `refresh()`.
    Then,
    /// Ответ не нужен (`.catch(() => {})`).
    Ignore,
}

/// Сетевая ошибка Chromium, когда не отвечает прокси из настроек системы (`isProxyError`
/// 0.4.9): слово `ERR_PROXY_*`, `ERR_TUNNEL_CONNECTION_FAILED` или
/// `ERR_MANDATORY_PROXY_CONFIGURATION_FAILED` (границы `\b` — по ASCII-словам, как в JS).
fn is_proxy_error(text: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|word| {
            matches!(
                word,
                "ERR_TUNNEL_CONNECTION_FAILED" | "ERR_MANDATORY_PROXY_CONFIGURATION_FAILED"
            ) || word.strip_prefix("ERR_PROXY_").is_some_and(|rest| {
                !rest.is_empty()
                    && rest
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            })
        })
}

/// События оболочке (`$emit`).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// `open-dump` (игра, если есть).
    OpenDump(Option<String>),
    ExportDiagnostics,
    OpenDiscord,
    /// `whats-new` / `run-update`: цель (`launcher`, `game`, `proton`), игра, версия.
    WhatsNew(Target),
    RunUpdate(Target),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Target {
    pub target: Option<String>,
    pub game_id: Option<String>,
    pub version: Option<String>,
}

/// Строка списка или превью, готовая к показу.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NotifRow {
    /// Идентификатор уведомления в JSON (для вызовов).
    pub id: String,
    /// `info`, `warning`, `error`.
    pub level: &'static str,
    pub icon: &'static str,
    pub read: bool,
    pub title: String,
    pub detail: String,
    pub time: String,
    pub code: String,
    /// «×N», если повторов больше одного.
    pub count: String,
    /// Кнопки версий: «Что нового» и «Обновить».
    pub whats_new: bool,
    pub update: bool,
    /// Кнопки сбоя игры.
    pub crash: bool,
    pub crash_dump: bool,
}

/// Что показывает колокольчик.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NotifView {
    pub open: bool,
    pub unread: i32,
    pub badge: String,
    pub bell_label: String,
    /// Колокольчик «принимает» превью: `notif-receive`.
    pub returning: bool,
    /// Превью показано сейчас (`preview && !open && !suspended`).
    pub peek_shown: bool,
    /// Превью есть (может быть скрыто, пока колокольчик открыт).
    pub announcing: bool,
    /// Список не пуст; сами строки — [`NotifMachine::rows`], только пока панель видна.
    pub has_items: bool,
    pub peek: NotifRow,
    pub peek_update: bool,
}

pub struct NotifMachine {
    wall: WallClock,
    /// Монотонные часы, мс (`Date.now()` в паузе и возобновлении превью).
    mono: Rc<dyn Fn() -> f64>,
    items: Vec<Value>,
    unread: i64,
    open: bool,
    preview: Option<Value>,
    queue: Vec<Value>,
    seen: HashMap<String, f64>,
    hydrated: bool,
    sequence: u64,
    disposed: bool,
    suspended: bool,
    timer_running: bool,
    remaining: f64,
    started_at: f64,
    returning: bool,
}

fn id_key(item: &Value) -> String {
    prop(item, "id").map(Value::to_string).unwrap_or_default()
}

fn count_of(item: &Value) -> f64 {
    // `item.count || 1`.
    match prop(item, "count").and_then(hlsr_core::js::value::number) {
        Some(count) if count != 0.0 && !count.is_nan() => count,
        _ => 1.0,
    }
}

fn code_of(item: &Value) -> &str {
    str_of(prop(item, "code")).unwrap_or_default()
}

fn meta_str<'a>(item: &'a Value, key: &str) -> Option<&'a str> {
    str_of(field(prop(item, "meta"), key)).filter(|text| !text.is_empty())
}

fn is_update_available(item: &Value) -> bool {
    matches!(
        code_of(item),
        "LAUNCHER_UPDATE_AVAILABLE" | "GAME_UPDATE_AVAILABLE" | "PROTON_UPDATE_AVAILABLE"
    )
}

impl NotifMachine {
    pub fn new(wall: WallClock, mono: Rc<dyn Fn() -> f64>) -> NotifMachine {
        NotifMachine {
            wall,
            mono,
            items: Vec::new(),
            unread: 0,
            open: false,
            preview: None,
            queue: Vec::new(),
            seen: HashMap::new(),
            hydrated: false,
            sequence: 0,
            disposed: false,
            suspended: false,
            timer_running: false,
            remaining: PREVIEW_MS,
            started_at: 0.0,
            returning: false,
        }
    }

    /// `mounted`: список уведомлений.
    pub fn mounted(&mut self) -> Vec<Effect> {
        self.refresh()
    }

    /// `beforeUnmount`.
    pub fn unmounted(&mut self) -> Vec<Effect> {
        self.disposed = true;
        let mut effects = vec![Effect::StopPreviewTimer];
        self.timer_running = false;
        if self.open {
            effects.extend(self.close(false));
        }
        effects
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Превью уходит в колокольчик: по концу ухода — [`NotifMachine::after_preview_leave`].
    pub fn returning(&self) -> bool {
        self.returning
    }

    /// Превью на экране: оно есть, колокольчик закрыт и окон поверх нет.
    fn peek_rendered(&self) -> bool {
        self.preview.is_some() && !self.open && !self.suspended
    }

    /// Событие `notifications-changed`.
    pub fn changed(&mut self) -> Vec<Effect> {
        self.refresh()
    }

    fn refresh(&mut self) -> Vec<Effect> {
        self.sequence += 1;
        vec![Effect::Invoke {
            tag: Tag::List(self.sequence),
            channel: "notifications-list",
            args: vec![],
        }]
    }

    /// Ответ `notifications-list`: отказ и пустой ответ оставляют показанное.
    pub fn list_reply(&mut self, sequence: u64, reply: Reply) -> Vec<Effect> {
        let Ok(result) = reply else { return vec![] };
        if !truthy(Some(&result)) || self.disposed || sequence != self.sequence {
            return vec![];
        }
        self.apply_snapshot(&result)
    }

    fn apply_snapshot(&mut self, result: &Value) -> Vec<Effect> {
        let items: Vec<Value> = prop(result, "items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let wall = self.wall.now_ms() as f64;
        let fresh: Vec<Value> = items
            .iter()
            .filter(|item| {
                if truthy(prop(item, "read")) {
                    return false;
                }
                if self.hydrated {
                    match self.seen.get(&id_key(item)) {
                        Some(seen) => count_of(item) > *seen,
                        None => true,
                    }
                } else {
                    let ts = prop(item, "ts")
                        .and_then(hlsr_core::js::value::number)
                        .unwrap_or(f64::NAN);
                    is_update_available(item) || wall - ts < FRESH_MS
                }
            })
            .cloned()
            .collect();
        self.unread = prop(result, "unread")
            .and_then(hlsr_core::js::value::number)
            .filter(|unread| *unread != 0.0)
            .unwrap_or(0.0) as i64;
        let mut effects = vec![];
        // «Синдром Плюшкина»: в списке накопилось 50 уведомлений.
        if items.len() >= 50 {
            effects.push(Effect::Invoke {
                tag: Tag::Ignore,
                channel: "runner-secret-unlock",
                args: vec![json!("hoarder")],
            });
        }
        // id уникальны и не возвращаются: ушедшие из списка помнить незачем.
        self.seen = items
            .iter()
            .map(|item| (id_key(item), count_of(item)))
            .collect();
        self.items = items;
        self.hydrated = true;
        let unread_ids: Vec<String> = self
            .items
            .iter()
            .filter(|item| !truthy(prop(item, "read")))
            .map(id_key)
            .collect();
        self.queue
            .retain(|queued| unread_ids.contains(&id_key(queued)));
        if let Some(preview) = &self.preview
            && !unread_ids.contains(&id_key(preview))
        {
            effects.extend(self.dismiss_preview());
        }
        if !self.open {
            for item in fresh.iter().take(PREVIEW_BATCH).rev() {
                if self
                    .preview
                    .as_ref()
                    .is_some_and(|preview| id_key(preview) == id_key(item))
                {
                    self.preview = Some(item.clone());
                    continue;
                }
                match self
                    .queue
                    .iter()
                    .position(|entry| id_key(entry) == id_key(item))
                {
                    Some(at) => self.queue[at] = item.clone(),
                    None => self.queue.push(item.clone()),
                }
            }
        }
        effects.extend(self.next_preview());
        effects
    }

    /// `nextPreview()`.
    pub fn next_preview(&mut self) -> Vec<Effect> {
        if self.preview.is_some()
            || self.returning
            || self.open
            || self.suspended
            || self.disposed
            || self.queue.is_empty()
        {
            return vec![];
        }
        self.preview = Some(self.queue.remove(0));
        self.remaining = PREVIEW_MS;
        self.resume_preview(false)
    }

    /// `pausePreview()` (наведение или фокус на превью).
    pub fn pause_preview(&mut self) -> Vec<Effect> {
        if !self.timer_running {
            return vec![];
        }
        self.remaining = (self.remaining - ((self.mono)() - self.started_at)).max(0.0);
        self.timer_running = false;
        vec![Effect::StopPreviewTimer]
    }

    /// `resumePreview()`: `held` — указатель или фокус на превью.
    pub fn resume_preview(&mut self, held: bool) -> Vec<Effect> {
        if self.preview.is_none() || self.timer_running || self.disposed {
            return vec![];
        }
        if held && self.peek_rendered() {
            return vec![];
        }
        self.started_at = (self.mono)();
        self.timer_running = true;
        vec![Effect::StartPreviewTimer(self.remaining)]
    }

    /// Сработал таймер превью (или крестик превью): `dismissPreview()`.
    pub fn dismiss_preview(&mut self) -> Vec<Effect> {
        let mut effects = vec![];
        if self.timer_running {
            effects.push(Effect::StopPreviewTimer);
        }
        self.timer_running = false;
        if self.preview.is_none() {
            return effects;
        }
        self.returning = self.peek_rendered();
        self.preview = None;
        if !self.returning {
            effects.push(Effect::NextTick);
        }
        effects
    }

    /// Превью ушло с экрана (`after-leave`).
    pub fn after_preview_leave(&mut self) -> Vec<Effect> {
        self.returning = false;
        self.next_preview()
    }

    /// `$nextTick` после `dismissPreview`, когда превью не на экране.
    pub fn tick_after_dismiss(&mut self) -> Vec<Effect> {
        if self.returning {
            vec![]
        } else {
            self.next_preview()
        }
    }

    /// `watch suspended`: окна поверх прячут превью, возвращают в очередь.
    pub fn set_suspended(&mut self, suspended: bool) -> Vec<Effect> {
        if self.suspended == suspended {
            return vec![];
        }
        if suspended {
            if let Some(preview) = self.preview.clone() {
                self.queue.insert(0, preview);
                let effects = self.dismiss_preview();
                self.suspended = true;
                return effects;
            }
            self.suspended = true;
            vec![]
        } else {
            self.suspended = false;
            self.next_preview()
        }
    }

    pub fn toggle(&mut self) -> Vec<Effect> {
        if self.open {
            self.close(false)
        } else {
            self.show()
        }
    }

    /// `show()`: очередь превью сброшена, панель открыта, список обновлён.
    pub fn show(&mut self) -> Vec<Effect> {
        if self.open {
            return vec![];
        }
        self.queue.clear();
        let mut effects = self.dismiss_preview();
        self.open = true;
        effects.push(Effect::OpenMenus(1));
        effects.extend(self.refresh());
        effects
    }

    /// `close(focusToggle)`.
    pub fn close(&mut self, focus_toggle: bool) -> Vec<Effect> {
        if !self.open {
            return vec![];
        }
        self.open = false;
        let mut effects = vec![Effect::OpenMenus(-1)];
        if focus_toggle {
            effects.push(Effect::FocusToggle);
        }
        effects.extend(self.next_preview());
        effects
    }

    fn find(&self, id: &str) -> Option<Value> {
        self.items
            .iter()
            .chain(self.preview.iter())
            .find(|item| id_key(item) == id)
            .cloned()
    }

    /// `markRead(item)`.
    pub fn mark_read(&mut self, id: &str) -> Vec<Effect> {
        let Some(item) = self.find(id) else {
            return vec![];
        };
        if truthy(prop(&item, "read")) {
            return vec![];
        }
        vec![Effect::Invoke {
            tag: Tag::Then,
            channel: "notifications-mark-read",
            args: vec![prop(&item, "id").cloned().unwrap_or(Value::Null)],
        }]
    }

    pub fn mark_all_read(&mut self) -> Vec<Effect> {
        vec![Effect::Invoke {
            tag: Tag::Then,
            channel: "notifications-mark-all-read",
            args: vec![],
        }]
    }

    pub fn clear_all(&mut self) -> Vec<Effect> {
        self.queue.clear();
        let mut effects = self.dismiss_preview();
        effects.push(Effect::Invoke {
            tag: Tag::Then,
            channel: "notifications-clear",
            args: vec![],
        });
        effects
    }

    /// Ответ вызова с `Tag::Then`: `this.refresh()`.
    pub fn then_refresh(&mut self) -> Vec<Effect> {
        self.refresh()
    }

    fn target_of(item: &Value) -> Target {
        Target {
            target: meta_str(item, "target").map(str::to_owned),
            game_id: meta_str(item, "gameId").map(str::to_owned),
            version: meta_str(item, "newVersion").map(str::to_owned),
        }
    }

    /// Кнопка «Что нового».
    pub fn whats_new(&mut self, id: &str) -> Vec<Effect> {
        let Some(item) = self.find(id) else {
            return vec![];
        };
        let mut effects = self.mark_read(id);
        effects.push(Effect::Emit(Event::WhatsNew(Self::target_of(&item))));
        effects.extend(self.close(false));
        effects
    }

    /// Кнопка «Обновить».
    pub fn run_update(&mut self, id: &str) -> Vec<Effect> {
        let Some(item) = self.find(id) else {
            return vec![];
        };
        let mut effects = self.dismiss_preview();
        effects.extend(self.mark_read(id));
        effects.push(Effect::Emit(Event::RunUpdate(Self::target_of(&item))));
        effects.extend(self.close(false));
        effects
    }

    /// Кнопка «Копировать код» сбоя игры.
    pub fn copy_code(&self, id: &str) -> Vec<Effect> {
        let Some(item) = self.find(id) else {
            return vec![];
        };
        let exit = match field(prop(&item, "meta"), "exitCode") {
            Some(value) if !value.is_null() => hlsr_core::js::to_string(value),
            _ => "?".to_owned(),
        };
        let mut code = format!("GAME_CRASHED exit={exit}");
        if let Some(hint) = meta_str(&item, "hint") {
            code.push(' ');
            code.push_str(hint);
        }
        vec![Effect::Copy(code)]
    }

    pub fn open_dump(&self, id: &str) -> Vec<Effect> {
        let item = self.find(id);
        vec![Effect::Emit(Event::OpenDump(
            item.as_ref()
                .and_then(|item| meta_str(item, "gameId"))
                .map(str::to_owned),
        ))]
    }

    // ---- Тексты -------------------------------------------------------------------------

    fn game_name(item: &Value, tr: &Tr) -> String {
        meta_str(item, "launcherId")
            .and_then(short_name)
            .map(str::to_owned)
            .unwrap_or_else(|| tr("#L_N_V_GAME_FALLBACK", &[]))
    }

    fn title_of(item: &Value, tr: &Tr) -> String {
        let code = code_of(item);
        if let Some((_, key)) = TITLE_KEYS.iter().find(|(known, _)| *known == code) {
            let mut args: Vec<String> = vec![];
            if GAME_TITLE_CODES.contains(&code) {
                args.push(Self::game_name(item, tr));
            }
            if VERSIONED_TITLE_CODES.contains(&code)
                && let Some(version) = meta_str(item, "newVersion")
            {
                args.push(version.to_owned());
            }
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let text = tr(key, &refs);
            if !text.is_empty() && text != *key {
                return text;
            }
        }
        tr("#L_N_GENERIC", &[])
    }

    fn preview_title(item: &Value, tr: &Tr) -> String {
        match code_of(item) {
            "PROTON_UPDATE_AVAILABLE" => tr("#L_N_UPDATE_FOR", &["Proton Wineland"]),
            "LAUNCHER_UPDATE_AVAILABLE" => tr("#L_N_UPDATE_FOR", &["HLSR"]),
            "GAME_UPDATE_AVAILABLE" => tr("#L_N_UPDATE_FOR", &[Self::game_name(item, tr).as_str()]),
            _ => Self::title_of(item, tr),
        }
    }

    fn moderation_detail(&self, item: &Value, lang: Language, tr: &Tr) -> Option<String> {
        let mut parts: Vec<String> = vec![];
        if let Some(until) = meta_str(item, "until") {
            let ms = chrono::DateTime::parse_from_rfc3339(until)
                .map(|at| at.timestamp_millis() as f64)
                .unwrap_or(f64::NAN);
            let when = self.wall.format(lang, DateStyle::DateTimeLong, ms);
            parts.push(tr("#L_N_MOD_UNTIL", &[when.as_str()]));
        } else if truthy(field(prop(item, "meta"), "forever")) {
            parts.push(tr("#L_N_MOD_FOREVER", &[]));
        }
        if let Some(detail) = str_of(prop(item, "detail")).filter(|detail| !detail.is_empty()) {
            parts.push(tr("#L_MOD_REASON", &[detail]));
        }
        match code_of(item) {
            "MOD_RESET_NICK" => parts.push(tr("#L_N_MOD_NICK_HINT", &[])),
            "MOD_RESET_AVATAR" => parts.push(tr("#L_N_MOD_AVATAR_HINT", &[])),
            _ => {}
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }

    fn detail_of(&self, item: &Value, lang: Language, tr: &Tr) -> Option<String> {
        let code = code_of(item);
        if code.starts_with("MOD_") {
            return self.moderation_detail(item, lang, tr);
        }
        if code == "SUPPORT_THANKS" {
            return Some(tr("#L_THANKS_TEXT", &[]));
        }
        if code == "CLIENT_MODIFIED" {
            return Some(tr("#L_N_CLIENT_MODIFIED_TEXT", &[]));
        }
        if code == "PROTON_UPDATE_AVAILABLE"
            && let Some(version) = meta_str(item, "newVersion")
        {
            let short = version.strip_prefix("wineland-").unwrap_or(version);
            return Some(tr("#L_N_PROTON_UPDATE_TOAST", &[short]));
        }
        if code == "GAME_CRASHED"
            && let Some(hint) = meta_str(item, "hint")
            && let Some((_, key)) = DEP_HINTS.iter().find(|(known, _)| *known == hint)
        {
            return Some(tr(key, &[]));
        }
        if code == "GAME_CRASHED" {
            return Some(if truthy(field(prop(item, "meta"), "hasDump")) {
                tr("#L_CR_TEXT_DUMP", &[])
            } else {
                tr("#L_CR_TEXT_NO_DUMP", &[])
            });
        }
        if VERSION_CODES.contains(&code) && truthy(prop(item, "meta")) {
            let from = meta_str(item, "oldVersion");
            let to = meta_str(item, "newVersion");
            if let (Some(from), Some(to)) = (from, to)
                && from != to
            {
                return Some(format!("{from} → {to}"));
            }
            if let Some(to) = to {
                return Some(tr("#L_N_VERSION", &[to]));
            }
        }
        let detail = str_of(prop(item, "detail")).filter(|detail| !detail.is_empty())?;
        // Голый код прокси человеку ничего не говорит; путь к настройке есть только у Windows.
        if is_proxy_error(detail) {
            let key = if cfg!(windows) {
                "#L_N_PROXY_HINT"
            } else {
                "#L_N_PROXY_HINT_SYSTEM"
            };
            return Some(tr(key, &[]));
        }
        if lang != Language::En {
            return Some(detail.to_owned());
        }
        // В английском интерфейсе русская причина не показывается.
        if detail.chars().any(|c| matches!(c, 'А'..='я' | 'Ё' | 'ё')) {
            None
        } else {
            Some(detail.to_owned())
        }
    }

    fn preview_detail(&self, item: &Value, lang: Language, tr: &Tr) -> Option<String> {
        if let Some(new) = meta_str(item, "newVersion") {
            let short = |version: &str| {
                version
                    .strip_prefix("wineland-")
                    .unwrap_or(version)
                    .to_owned()
            };
            return Some(match meta_str(item, "oldVersion") {
                Some(old) if old != new => format!("{} → {}", short(old), short(new)),
                _ => short(new),
            });
        }
        self.detail_of(item, lang, tr)
    }

    /// `timeOf(ts)`: чч:мм, а не сегодняшнее — «дд.мм чч:мм».
    fn time_of(&self, ts: f64) -> String {
        let Some(date) = self.wall.local(ts as i64) else {
            return "NaN:NaN".to_owned();
        };
        let hm = format!("{:02}:{:02}", date.hour, date.minute);
        let today = self.wall.local(self.wall.now_ms());
        match today {
            Some(now) if (now.year, now.month, now.day) == (date.year, date.month, date.day) => hm,
            _ => format!("{:02}.{:02} {hm}", date.day, date.month),
        }
    }

    fn row(&self, item: &Value, preview: bool, lang: Language, tr: &Tr) -> NotifRow {
        let level = match str_of(prop(item, "level")) {
            Some("error") => "error",
            Some("warning") => "warning",
            _ => "info",
        };
        let count = count_of(item);
        let ts = prop(item, "ts")
            .and_then(hlsr_core::js::value::number)
            .unwrap_or(f64::NAN);
        let version_code = VERSION_CODES.contains(&code_of(item));
        let meta_true = |key: &str| truthy(field(prop(item, "meta"), key));
        NotifRow {
            id: prop(item, "id").map(Value::to_string).unwrap_or_default(),
            level,
            icon: match level {
                "error" => "fas fa-times-circle",
                "warning" => "fas fa-exclamation-triangle",
                _ => "fas fa-info-circle",
            },
            read: truthy(prop(item, "read")),
            title: if preview {
                Self::preview_title(item, tr)
            } else {
                Self::title_of(item, tr)
            },
            detail: if preview {
                self.preview_detail(item, lang, tr)
            } else {
                self.detail_of(item, lang, tr)
            }
            .unwrap_or_default(),
            time: self.time_of(ts),
            code: code_of(item).to_owned(),
            count: if count > 1.0 {
                format!("×{}", hlsr_core::js::number::number_to_string(count))
            } else {
                String::new()
            },
            whats_new: version_code && meta_true("hasReleaseNotes"),
            update: version_code && is_update_available(item),
            crash: code_of(item) == "GAME_CRASHED",
            crash_dump: meta_true("hasDump"),
        }
    }

    pub fn view(&self, lang: Language, tr: &Tr) -> NotifView {
        let title = tr("#L_N_TITLE", &[]);
        NotifView {
            open: self.open,
            unread: self.unread.clamp(0, i32::MAX as i64) as i32,
            badge: if self.unread > 99 {
                "99+".to_owned()
            } else {
                self.unread.to_string()
            },
            bell_label: if self.unread > 0 {
                format!("{title} ({})", self.unread)
            } else {
                title
            },
            returning: self.returning,
            peek_shown: self.peek_rendered(),
            announcing: self.preview.is_some(),
            has_items: !self.items.is_empty(),
            peek: self
                .preview
                .as_ref()
                .map(|item| self.row(item, true, lang, tr))
                .unwrap_or_default(),
            peek_update: self.preview.as_ref().is_some_and(is_update_available),
        }
    }

    /// Строки панели.
    pub fn rows(&self, lang: Language, tr: &Tr) -> Vec<NotifRow> {
        self.items
            .iter()
            .map(|item| self.row(item, false, lang, tr))
            .collect()
    }

    /// Номера в очереди превью (тесты).
    pub fn queued(&self) -> Vec<String> {
        self.queue.iter().map(id_key).collect()
    }

    /// Превью сейчас (тесты).
    pub fn previewing(&self) -> Option<String> {
        self.preview.as_ref().map(id_key)
    }

    /// Оставшееся время превью, мс (тесты).
    pub fn remaining(&self) -> f64 {
        self.remaining
    }
}

/// Снимок для тестов и сцен: `{items, unread}`.
pub fn snapshot(items: Vec<Value>) -> Value {
    let unread = items
        .iter()
        .filter(|item| !truthy(prop(item, "read")))
        .count();
    json!({"items": items, "unread": unread})
}

#[cfg(test)]
mod tests;
