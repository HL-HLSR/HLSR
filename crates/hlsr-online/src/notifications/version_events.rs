//! События версий (`notifications/version-events.js`): «однократные» уведомления о новых
//! версиях лаунчера и игр, снятие устаревших приглашений обновиться и квитанция обновления
//! лаунчера. Состояние — `version-state.json` `{schemaVersion: 1, seen: {ключ: мс}, receipt}`
//! (компактный JSON через `<файл>.tmp`, не больше 200 ключей); повреждённый файл уходит в карантин
//! `<файл>.corrupt-<мс>`. Отметки `TM_LU_*` событий самообновления (телеметрия) — тот же `seen`.

use crate::telemetry::launcher_update_events::AnnouncedVersions;
use hlsr_core::js::{self, fs as js_fs};
use hlsr_core::json_formats::{TmpOptions, write_compact_tmp};
use hlsr_core::json_text::{js_number, json_parse};
use hlsr_core::logger::Logger;
use hlsr_core::notifications::{NotificationStore, is_meta_game_id, is_meta_version};
use hlsr_delivery::semver;
use hlsr_delivery::timing::Clock;
use serde_json::{Map, Value, json};
use std::cmp::Ordering;
use std::path::PathBuf;
use std::sync::Arc;

pub const SCHEMA_VERSION: u64 = 1;
pub const MAX_SEEN: usize = 200;

/// `CODES`.
pub mod codes {
    pub const LAUNCHER_UPDATE_AVAILABLE: &str = "LAUNCHER_UPDATE_AVAILABLE";
    pub const LAUNCHER_UPDATED: &str = "LAUNCHER_UPDATED";
    pub const GAME_UPDATE_AVAILABLE: &str = "GAME_UPDATE_AVAILABLE";
    pub const GAME_UPDATED: &str = "GAME_UPDATED";
    pub const GAME_INSTALLED: &str = "GAME_INSTALLED";
}

/// `isVersion(value)`: `^[0-9A-Za-z.\-+]{1,32}$`.
pub fn is_version(value: Option<&str>) -> bool {
    value.is_some_and(is_meta_version)
}

/// `isGameId(value)`: `^[a-z0-9]+([._-][a-z0-9]+)*$`, не длиннее 64.
pub fn is_game_id(value: Option<&str>) -> bool {
    value.is_some_and(is_meta_game_id)
}

/// Квитанция обновления лаунчера: `{from, to}`; `from` — значение из файла (`receipt.from || null`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub from: Value,
    pub to: String,
}

impl Receipt {
    /// `from`, если это строка.
    pub fn from_text(&self) -> Option<&str> {
        self.from.as_str()
    }
}

/// Сведения к событию версии игры: `{hasReleaseNotes, launcherId}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionOptions {
    pub has_release_notes: bool,
    pub launcher_id: Option<String>,
}

pub struct VersionEvents {
    file: PathBuf,
    store: Arc<NotificationStore>,
    logger: Arc<dyn Logger>,
    now: Clock,
}

impl VersionEvents {
    pub fn new(
        file: impl Into<PathBuf>,
        store: Arc<NotificationStore>,
        logger: Arc<dyn Logger>,
        now: Clock,
    ) -> VersionEvents {
        VersionEvents {
            file: file.into(),
            store,
            logger,
            now,
        }
    }

    fn empty_state() -> Map<String, Value> {
        let mut state = Map::new();
        state.insert("schemaVersion".to_owned(), json!(SCHEMA_VERSION));
        state.insert("seen".to_owned(), json!({}));
        state.insert("receipt".to_owned(), Value::Null);
        state
    }

    /// `readState()`: состояние из файла. Не JSON или чужая схема — карантин и пустое состояние.
    pub fn read_state(&self) -> Map<String, Value> {
        let Ok(bytes) = js_fs::read_file(&self.file) else {
            return Self::empty_state();
        };
        let data = match json_parse(&String::from_utf8_lossy(&bytes)) {
            Ok(data) => data,
            Err(_) => {
                self.quarantine("не разбирается как JSON");
                return Self::empty_state();
            }
        };
        let schema_ok = data
            .get("schemaVersion")
            .and_then(Value::as_f64)
            .is_some_and(|version| version == SCHEMA_VERSION as f64);
        let Value::Object(mut data) = data.clone() else {
            // `data && data.schemaVersion`: у примитивов и массивов поля нет.
            let shown = if js::truthy(&data) {
                "undefined".to_owned()
            } else {
                js::to_template_string(Some(&data))
            };
            self.quarantine(&format!("неподдерживаемая схема: {shown}"));
            return Self::empty_state();
        };
        if !schema_ok {
            let shown = js::to_template_string(data.get("schemaVersion"));
            self.quarantine(&format!("неподдерживаемая схема: {shown}"));
            return Self::empty_state();
        }
        if !matches!(data.get("seen"), Some(Value::Object(_))) {
            data.insert("seen".to_owned(), json!({}));
        }
        if data.get("receipt").is_some_and(|receipt| {
            js::truthy(receipt) && !matches!(receipt, Value::Object(_) | Value::Array(_))
        }) {
            data.insert("receipt".to_owned(), Value::Null);
        }
        data
    }

    fn quarantine(&self, reason: &str) {
        if !self.file.exists() {
            return;
        }
        let mut target = self.file.as_os_str().to_owned();
        target.push(format!(".corrupt-{}", (self.now)()));
        let target = PathBuf::from(target);
        match std::fs::rename(&self.file, &target) {
            Ok(()) => self.logger.warn(&format!(
                "[versions] состояние повреждено ({reason}), карантин: {}",
                target
                    .file_name()
                    .map(|name| name.to_string_lossy())
                    .unwrap_or_default()
            )),
            Err(error) => self.logger.warn(&format!(
                "[versions] карантин не выполнен: {}",
                js::errno::fs_message(
                    &error,
                    "rename",
                    &self.file.to_string_lossy(),
                    Some(&target.to_string_lossy())
                )
            )),
        }
    }

    /// `writeState(state)`: сбой — строка в журнале и уборка `.tmp`.
    fn write_state(&self, state: &Map<String, Value>) {
        if let Err(error) = write_compact_tmp(
            &self.file,
            &Value::Object(state.clone()),
            TmpOptions::default(),
        ) {
            self.logger.warn(&format!(
                "[versions] состояние не сохранено: {}",
                error.message()
            ));
            if let Some(temp) = &error.temp {
                let _ = std::fs::remove_file(temp);
            }
        }
    }

    /// `key(code, version, gameId)`: `CODE:gameId:версия` или `CODE:версия`.
    pub fn key(code: &str, version: &str, game_id: Option<&str>) -> String {
        match game_id.filter(|id| is_meta_game_id(id)) {
            Some(game_id) => format!("{code}:{game_id}:{version}"),
            None => format!("{code}:{version}"),
        }
    }

    fn seen(state: &Map<String, Value>) -> Option<&Map<String, Value>> {
        state.get("seen").and_then(Value::as_object)
    }

    /// `alreadyAnnounced(code, version, state, gameId)`: ключ с игрой — или общий ключ без игры.
    pub fn already_announced_in(
        state: &Map<String, Value>,
        code: &str,
        version: &str,
        game_id: Option<&str>,
    ) -> bool {
        let Some(seen) = Self::seen(state) else {
            return false;
        };
        if seen.contains_key(&Self::key(code, version, game_id)) {
            return true;
        }
        is_game_id(game_id) && seen.contains_key(&Self::key(code, version, None))
    }

    /// `markAnnounced(code, version, state, gameId)`: отметить и сохранить; старше 200-го — вон.
    pub fn mark_announced_in(
        &self,
        state: &mut Map<String, Value>,
        code: &str,
        version: &str,
        game_id: Option<&str>,
    ) {
        let now = js_number((self.now)() as f64);
        if let Some(Value::Object(seen)) = state.get_mut("seen") {
            seen.insert(Self::key(code, version, game_id), now);
            if seen.len() > MAX_SEEN {
                // `keys.sort((a, b) => seen[a] - seen[b])`: самые старые отметки — первыми.
                let mut stamps: Vec<(String, f64)> = seen
                    .iter()
                    .map(|(key, value)| (key.clone(), value.as_f64().unwrap_or(f64::NAN)))
                    .collect();
                js::sort::sort_by_comparator(&mut stamps, |a, b| a.1 - b.1);
                let excess = stamps.len() - MAX_SEEN;
                for (key, _) in &stamps[..excess] {
                    seen.shift_remove(key);
                }
            }
        }
        self.write_state(state);
    }

    /// `publishOnce(code, version, level, detail, meta)`: уведомление, если для версии (и игры)
    /// его ещё не было. Ошибка — сбой записи `notifications.json`.
    fn publish_once(
        &self,
        code: &str,
        version: Option<&str>,
        level: &str,
        meta: Value,
    ) -> Result<Option<Value>, String> {
        let Some(version) = version.filter(|version| is_meta_version(version)) else {
            self.logger.warn(&format!(
                "[versions] отклонена некорректная версия для {code}"
            ));
            return Ok(None);
        };
        let game_id = meta
            .get("gameId")
            .and_then(Value::as_str)
            .filter(|id| is_meta_game_id(id))
            .map(str::to_owned);
        let mut state = self.read_state();
        if Self::already_announced_in(&state, code, version, game_id.as_deref()) {
            return Ok(None);
        }
        let item = self
            .store
            .add(level, code, Some(version), Some(&meta))
            .map_err(|error| error.to_string())?;
        self.mark_announced_in(&mut state, code, version, game_id.as_deref());
        Ok(item)
    }

    /// `resolveUpdateOffers(target, installedVersion, gameId)`: снять приглашения обновиться до
    /// версии не новее установленной; число снятых.
    pub fn resolve_update_offers(
        &self,
        target: &str,
        installed_version: Option<&str>,
        game_id: Option<&str>,
    ) -> usize {
        let Some(installed) = installed_version.filter(|v| is_meta_version(v)) else {
            return 0;
        };
        let code = if target == "launcher" {
            codes::LAUNCHER_UPDATE_AVAILABLE
        } else {
            codes::GAME_UPDATE_AVAILABLE
        };
        let id = game_id.filter(|id| is_meta_game_id(id));
        let removed = self.store.remove_where(|item| {
            if item.get("code").and_then(Value::as_str) != Some(code) {
                return false;
            }
            let meta = item.get("meta").filter(|meta| js::truthy(meta));
            let field = |key: &str| meta.and_then(|meta| meta.get(key));
            if target == "game"
                && let Some(id) = id
                && let Some(meta_game) = field("gameId").filter(|value| js::truthy(value))
                && meta_game.as_str() != Some(id)
            {
                return false;
            }
            let Some(new_version) = field("newVersion")
                .and_then(Value::as_str)
                .filter(|v| is_meta_version(v))
            else {
                return false;
            };
            semver::compare(new_version, installed) != Ordering::Greater
        });
        let removed = match removed {
            Ok(removed) => removed,
            Err(error) => {
                self.logger.warn(&format!(
                    "[versions] устаревшие приглашения не сняты: {error}"
                ));
                return 0;
            }
        };
        if removed > 0 {
            self.logger.info(&format!(
                "[versions] снято приглашений обновиться: {removed} (установлено {installed})"
            ));
        }
        removed
    }

    fn version_meta(
        target: &str,
        game_id: Option<&str>,
        launcher_id: Option<&str>,
        old_version: Option<&str>,
        new_version: Option<&str>,
        has_release_notes: bool,
    ) -> Value {
        let mut meta = Map::new();
        meta.insert("target".to_owned(), json!(target));
        if let Some(game_id) = game_id.filter(|id| is_meta_game_id(id)) {
            meta.insert("gameId".to_owned(), json!(game_id));
        }
        if let Some(launcher_id) = launcher_id {
            meta.insert("launcherId".to_owned(), json!(launcher_id));
        }
        if let Some(old) = old_version.filter(|v| is_meta_version(v)) {
            meta.insert("oldVersion".to_owned(), json!(old));
        }
        if let Some(new) = new_version {
            meta.insert("newVersion".to_owned(), json!(new));
        }
        meta.insert("hasReleaseNotes".to_owned(), json!(has_release_notes));
        Value::Object(meta)
    }

    /// `launcherUpdateAvailable(version, currentVersion, {hasReleaseNotes})`.
    pub fn launcher_update_available(
        &self,
        version: Option<&str>,
        current_version: Option<&str>,
        has_release_notes: bool,
    ) -> Result<Option<Value>, String> {
        let meta = Self::version_meta(
            "launcher",
            None,
            None,
            current_version,
            version,
            has_release_notes,
        );
        self.publish_once(codes::LAUNCHER_UPDATE_AVAILABLE, version, "info", meta)
    }

    /// `launcherUpdated(fromVersion, toVersion)`.
    pub fn launcher_updated(
        &self,
        from_version: Option<&str>,
        to_version: Option<&str>,
    ) -> Result<Option<Value>, String> {
        let meta = Self::version_meta("launcher", None, None, from_version, to_version, true);
        self.publish_once(codes::LAUNCHER_UPDATED, to_version, "info", meta)
    }

    /// `gameUpdateAvailable(version, currentVersion, gameId, options)`.
    pub fn game_update_available(
        &self,
        version: Option<&str>,
        current_version: Option<&str>,
        game_id: Option<&str>,
        options: &VersionOptions,
    ) -> Result<Option<Value>, String> {
        let meta = Self::version_meta(
            "game",
            game_id,
            options.launcher_id.as_deref(),
            current_version,
            version,
            options.has_release_notes,
        );
        self.publish_once(codes::GAME_UPDATE_AVAILABLE, version, "info", meta)
    }

    /// `gameUpdated(fromVersion, toVersion, gameId, options)`.
    pub fn game_updated(
        &self,
        from_version: Option<&str>,
        to_version: Option<&str>,
        game_id: Option<&str>,
        options: &VersionOptions,
    ) -> Result<Option<Value>, String> {
        let meta = Self::version_meta(
            "game",
            game_id,
            options.launcher_id.as_deref(),
            from_version,
            to_version,
            options.has_release_notes,
        );
        self.publish_once(codes::GAME_UPDATED, to_version, "info", meta)
    }

    /// `gameInstalled(version, gameId, options)`.
    pub fn game_installed(
        &self,
        version: Option<&str>,
        game_id: Option<&str>,
        options: &VersionOptions,
    ) -> Result<Option<Value>, String> {
        let meta = Self::version_meta(
            "game",
            game_id,
            options.launcher_id.as_deref(),
            None,
            version,
            options.has_release_notes,
        );
        self.publish_once(codes::GAME_INSTALLED, version, "info", meta)
    }

    /// `writePendingReceipt(fromVersion, toVersion)`: квитанция перед перезапуском в новую версию.
    pub fn write_pending_receipt(
        &self,
        from_version: Option<&str>,
        to_version: Option<&str>,
    ) -> Option<Value> {
        let to = to_version.filter(|v| is_meta_version(v))?;
        let mut state = self.read_state();
        let receipt = json!({
            "from": from_version.filter(|v| is_meta_version(v)),
            "to": to,
            "ts": js_number((self.now)() as f64),
        });
        state.insert("receipt".to_owned(), receipt.clone());
        self.write_state(&state);
        Some(receipt)
    }

    fn receipt_of(state: &Map<String, Value>) -> Option<Receipt> {
        let receipt = state.get("receipt").filter(|receipt| js::truthy(receipt))?;
        let to = receipt
            .get("to")
            .and_then(Value::as_str)
            .filter(|v| is_meta_version(v))?;
        let from = receipt
            .get("from")
            .filter(|from| js::truthy(from))
            .cloned()
            .unwrap_or(Value::Null);
        Some(Receipt {
            from,
            to: to.to_owned(),
        })
    }

    /// `peekReceipt()`.
    pub fn peek_receipt(&self) -> Option<Receipt> {
        Self::receipt_of(&self.read_state())
    }

    /// `consumeReceipt(currentVersion)`: квитанция на эту версию — снять и сообщить
    /// `LAUNCHER_UPDATED`.
    pub fn consume_receipt(&self, current_version: &str) -> Result<Option<Value>, String> {
        let mut state = self.read_state();
        let Some(receipt) = Self::receipt_of(&state) else {
            return Ok(None);
        };
        if receipt.to != current_version {
            return Ok(None);
        }
        state.insert("receipt".to_owned(), Value::Null);
        self.write_state(&state);
        // `receipt.from` у 0.4.7 — что лежит в файле: не строка-версия даёт meta без oldVersion.
        self.launcher_updated(receipt.from_text(), Some(&receipt.to))
    }

    /// `clearReceipt()`.
    pub fn clear_receipt(&self) -> bool {
        let mut state = self.read_state();
        if !state.get("receipt").is_some_and(js::truthy) {
            return false;
        }
        state.insert("receipt".to_owned(), Value::Null);
        self.write_state(&state);
        true
    }
}

impl AnnouncedVersions for VersionEvents {
    fn already_announced(&self, stage: &str, version: &str) -> Result<bool, String> {
        Ok(Self::already_announced_in(
            &self.read_state(),
            stage,
            version,
            None,
        ))
    }

    fn mark_announced(&self, stage: &str, version: &str) -> Result<(), String> {
        let mut state = self.read_state();
        self.mark_announced_in(&mut state, stage, version, None);
        Ok(())
    }
}
