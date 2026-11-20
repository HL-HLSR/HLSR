//! Раздел «Демо» страницы «Персонализация» (`GameDemos.vue` 0.4.9): список демо с галочками,
//! дата и размер, «скопировать команду» и удаление (HL1), упаковка выбранных в zip с процентом,
//! «Показать», ссылки speedrun.com и Google Drive. Черновик настроек не трогает.
//!
//! Без окна и без каналов: вид зовёт `game-demos-list`, `game-demos-pack`, `game-runs-delete`…
//! и отдаёт ответы сюда (`*_done`), как `await ipcRenderer.invoke(…)` компонента.

use super::boot::{Translate, format_size};
use super::jsv;
use chrono::{DateTime, Datelike, TimeZone, Timelike};
use hlsr_core::js;
use serde_json::{Value, json};
use std::collections::HashMap;

/// Сутки: новые демо моложе суток отмечены сразу.
pub const DAY_MS: f64 = 24.0 * 3600.0 * 1000.0;
/// «Скопировано» на кнопке команды держится 1,5 с (`copiedTimer`).
pub const COPIED_MS: u64 = 1500;

/// Движок игры раздела: у HL1 (`goldsrc`) есть удаление и `playdemo`, у Ghosting нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    GoldSrc,
    Ghosting,
}

impl Engine {
    pub fn can_delete(self) -> bool {
        self == Engine::GoldSrc
    }

    pub fn can_copy(self) -> bool {
        self == Engine::GoldSrc
    }

    /// Ключ ссылки speedrun.com (`open-external-link`).
    pub fn speedrun_key(self) -> &'static str {
        match self {
            Engine::GoldSrc => "speedrun-hl1",
            Engine::Ghosting => "speedrun-hl2",
        }
    }
}

/// Демо из `game-demos-list`: `{ dir, name, size, mtimeMs }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Demo {
    pub dir: String,
    pub name: String,
    pub size: f64,
    pub mtime_ms: f64,
}

impl Demo {
    fn from_value(value: &Value) -> Demo {
        let text = |name: &str| {
            jsv::prop(value, name)
                .map(js::to_string)
                .unwrap_or_default()
        };
        Demo {
            dir: text("dir"),
            name: text("name"),
            size: jsv::or_zero(jsv::prop(value, "size")),
            mtime_ms: jsv::or_zero(jsv::prop(value, "mtimeMs")),
        }
    }

    /// `keyOf(demo)`: «папка|имя».
    pub fn key(&self) -> String {
        format!("{}|{}", self.dir, self.name)
    }

    /// `command(demo)`: «playdemo имя» без `.dem` (регистр не важен).
    pub fn command(&self) -> String {
        let name = self.name.as_str();
        // `get`: последние 4 байта могут резать символ («жa1b») — тогда это не `.dem`.
        let cut = name.len().saturating_sub(4);
        let stem = match name.get(cut..) {
            Some(tail) if tail.len() == 4 && tail.eq_ignore_ascii_case(".dem") => &name[..cut],
            _ => name,
        };
        format!("playdemo {stem}")
    }
}

/// Готовый архив: `{ name, size }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Archive {
    pub name: String,
    pub size: f64,
}

/// Что делать после ответа `game-demos-pack`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackNext {
    Nothing,
    /// Архив готов: показать его в папке (`game-demos-archive-reveal`).
    Reveal(String),
    /// Демо изменились (`DEMO_NOT_FOUND`): перечитать список.
    Reload,
}

/// Состояние раздела (`data()`).
#[derive(Debug, Clone, Default)]
pub struct GameDemos {
    pub demos: Vec<Demo>,
    chosen: HashMap<String, bool>,
    pub loading: bool,
    pub list_error: Option<String>,
    pub packing: bool,
    pub pack_percent: f64,
    pub pack_error: Option<String>,
    pub archive: Option<Archive>,
    pub copied: Option<String>,
    pub pending_delete: Option<String>,
    pub deleting: Option<String>,
    pub delete_error: Option<String>,
}

fn pack_error_key(code: Option<&str>) -> &'static str {
    match code {
        Some("DEMO_NOT_FOUND") => "#L_DEMOS_ERR_CHANGED",
        Some("PACK_BUSY") => "#L_DEMOS_ERR_BUSY",
        Some("NOT_INSTALLED") => "#L_GS_NOT_INSTALLED",
        _ => "#L_DEMOS_ERR_PACK",
    }
}

fn status_ok(result: &Value) -> bool {
    jsv::is_str(jsv::prop(result, "status"), "ok")
}

fn code_of(result: &Value) -> Option<&str> {
    jsv::str_of(jsv::prop(result, "code"))
}

impl GameDemos {
    pub fn is_chosen(&self, key: &str) -> bool {
        self.chosen.get(key).copied().unwrap_or(false)
    }

    /// `chosenCount`.
    pub fn chosen_count(&self) -> usize {
        self.demos
            .iter()
            .filter(|demo| self.is_chosen(&demo.key()))
            .count()
    }

    /// `isUnique(demo)`: удаление идёт по имени (на Windows без учёта регистра) — при
    /// одинаковых именах в двух папках кнопки нет.
    pub fn is_unique(&self, demo: &Demo) -> bool {
        let name = demo.name.to_lowercase();
        self.demos
            .iter()
            .filter(|item| item.name.to_lowercase() == name)
            .count()
            == 1
    }

    /// `summary`: «Загрузка…» или «Демо: N».
    pub fn summary(&self, t: Translate) -> String {
        if self.loading {
            t("#L_GS_LOADING", &[])
        } else {
            t("#L_GS_DEMOS_COUNT", &[&self.demos.len().to_string()])
        }
    }

    /// `toggle(demo)`.
    pub fn toggle(&mut self, key: &str) {
        let on = !self.is_chosen(key);
        self.chosen.insert(key.to_owned(), on);
    }

    /// `setAll(value)`: только демо из списка.
    pub fn set_all(&mut self, on: bool) {
        self.chosen = self.demos.iter().map(|demo| (demo.key(), on)).collect();
    }

    /// Начало `load()`; `false` — список уже читается.
    pub fn begin_load(&mut self) -> bool {
        if self.loading {
            return false;
        }
        self.loading = true;
        self.list_error = None;
        self.pending_delete = None;
        true
    }

    /// Ответ `game-demos-list` (`Err` — исключение канала). `now_ms` — `Date.now()`.
    pub fn list_done(&mut self, reply: Result<&Value, ()>, now_ms: f64, t: Translate) {
        match reply {
            Ok(result) if status_ok(result) => {
                let list: Vec<Demo> = jsv::prop(result, "demos")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().map(Demo::from_value).collect())
                    .unwrap_or_default();
                let since = now_ms - DAY_MS;
                // Выбор пользователя переживает обновление; новые демо отмечены, если им нет суток.
                let next = list
                    .iter()
                    .map(|demo| {
                        let key = demo.key();
                        let on = self
                            .chosen
                            .get(&key)
                            .copied()
                            .unwrap_or(demo.mtime_ms >= since);
                        (key, on)
                    })
                    .collect();
                self.demos = list;
                self.chosen = next;
            }
            Ok(result) => {
                self.demos.clear();
                self.chosen.clear();
                let code = code_of(result);
                self.list_error = Some(if code == Some("NOT_INSTALLED") {
                    t("#L_GS_NOT_INSTALLED", &[])
                } else {
                    t("#L_GS_RUNS_ERROR", &[code.unwrap_or("")])
                });
            }
            Err(()) => {
                self.demos.clear();
                self.list_error = Some(t("#L_GS_RUNS_ERROR", &[""]));
            }
        }
        self.loading = false;
    }

    /// Начало `pack()`: что упаковать (`[{dir, name}]`); `None` — нечего или уже идёт.
    pub fn begin_pack(&mut self) -> Option<Value> {
        if self.packing || self.chosen_count() == 0 {
            return None;
        }
        self.packing = true;
        self.pack_percent = 0.0;
        self.pack_error = None;
        self.archive = None;
        let items: Vec<Value> = self
            .demos
            .iter()
            .filter(|demo| self.is_chosen(&demo.key()))
            .map(|demo| json!({"dir": demo.dir, "name": demo.name}))
            .collect();
        Some(Value::Array(items))
    }

    /// Событие `game-demos-pack-progress` `{ gameId, percent }`: только своей игры.
    pub fn progress(&mut self, game_id: &str, payload: &Value) {
        if jsv::is_str(jsv::prop(payload, "gameId"), game_id) {
            self.pack_percent = jsv::num(payload.get("percent")).unwrap_or(0.0);
        }
    }

    /// Ответ `game-demos-pack`.
    pub fn pack_done(&mut self, reply: Result<&Value, ()>, t: Translate) -> PackNext {
        self.packing = false;
        let result = match reply {
            Ok(result) => result,
            Err(()) => {
                self.pack_error = Some(t("#L_DEMOS_ERR_PACK", &[]));
                return PackNext::Nothing;
            }
        };
        if status_ok(result)
            && let Some(archive) = jsv::prop(result, "archive").filter(|a| js::truthy(a))
        {
            let name = jsv::prop(archive, "name")
                .map(js::to_string)
                .unwrap_or_default();
            self.archive = Some(Archive {
                name: name.clone(),
                size: jsv::or_zero(jsv::prop(archive, "size")),
            });
            return PackNext::Reveal(name);
        }
        let code = code_of(result);
        self.pack_error = Some(t(pack_error_key(code), &[]));
        if code == Some("DEMO_NOT_FOUND") {
            PackNext::Reload
        } else {
            PackNext::Nothing
        }
    }

    /// Кнопка «Удалить демо»: вопрос в строке.
    pub fn ask_delete(&mut self, key: &str) {
        if self.deleting.is_none() {
            self.pending_delete = Some(key.to_owned());
        }
    }

    pub fn cancel_delete(&mut self) {
        if self.deleting.is_none() {
            self.pending_delete = None;
        }
    }

    /// Начало `remove(demo)`: имя для `game-runs-delete`; `None` — нельзя.
    pub fn begin_delete(&mut self, key: &str) -> Option<String> {
        if self.deleting.is_some() || self.pending_delete.as_deref() != Some(key) {
            return None;
        }
        let name = self
            .demos
            .iter()
            .find(|demo| demo.key() == key)?
            .name
            .clone();
        self.deleting = Some(key.to_owned());
        self.delete_error = None;
        Some(name)
    }

    /// Ответ `game-runs-delete`.
    pub fn delete_done(&mut self, key: &str, reply: Result<&Value, ()>, t: Translate) {
        match reply {
            Ok(result) if status_ok(result) => {
                self.demos.retain(|demo| demo.key() != key);
                self.pending_delete = None;
            }
            Ok(result) => {
                self.delete_error = Some(if code_of(result) == Some("GAME_RUNNING") {
                    t("#L_D_ERR_GAME_RUNNING", &[])
                } else {
                    t("#L_GS_DEMO_DELETE_FAILED", &[])
                });
            }
            Err(()) => self.delete_error = Some(t("#L_GS_DEMO_DELETE_FAILED", &[])),
        }
        self.deleting = None;
    }

    /// `copy(demo)`: команда в буфер, на кнопке галочка до [`COPIED_MS`].
    pub fn copy(&mut self, key: &str) -> Option<String> {
        let command = self.demos.iter().find(|demo| demo.key() == key)?.command();
        self.copied = Some(key.to_owned());
        Some(command)
    }

    /// Срок «скопировано» вышел (только если с тех пор не копировали другое).
    pub fn copy_expired(&mut self, key: &str) {
        if self.copied.as_deref() == Some(key) {
            self.copied = None;
        }
    }

    /// «Упаковать в архив (N)» или «Упаковываю… 42%».
    pub fn pack_label(&self, t: Translate) -> String {
        if !self.packing {
            return t("#L_DEMOS_PACK", &[&self.chosen_count().to_string()]);
        }
        let mut text = t("#L_DEMOS_PACKING", &[]);
        if self.pack_percent > 0.0 {
            text.push_str(&format!(
                " {}%",
                js::number::number_to_string(self.pack_percent)
            ));
        }
        text
    }

    /// «Архив имя, размер».
    pub fn archive_label(&self, t: Translate) -> Option<String> {
        let archive = self.archive.as_ref()?;
        Some(t(
            "#L_DEMOS_ARCHIVE",
            &[&archive.name, &format_size(archive.size, t)],
        ))
    }
}

/// `formatDate(mtimeMs)` в местном поясе ОС.
pub fn format_date_local(ms: f64) -> String {
    format_date_ms(ms, &chrono::Local)
}

/// `formatDate(mtimeMs)` в поясе `tz`: «ДД.ММ.ГГГГ ЧЧ:ММ»; нет даты — пусто.
pub fn format_date_ms<Tz: TimeZone>(ms: f64, tz: &Tz) -> String {
    if !ms.is_finite() {
        return String::new();
    }
    match DateTime::from_timestamp_millis(ms as i64) {
        Some(date) => {
            let date = date.with_timezone(tz);
            format!(
                "{:02}.{:02}.{} {:02}:{:02}",
                date.day(),
                date.month(),
                date.year(),
                date.hour(),
                date.minute()
            )
        }
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(key: &str, args: &[&str]) -> String {
        if key == "#L_DECIMAL" {
            ".".to_owned()
        } else if args.is_empty() {
            key.to_owned()
        } else {
            format!("{key}({})", args.join(","))
        }
    }

    const NOW: f64 = 1_763_380_800_000.0;

    fn list() -> Value {
        json!({"status": "ok", "demos": [
            {"dir": "valve_WON", "name": "c1a0.dem", "size": 4096, "mtimeMs": NOW - 1000.0},
            {"dir": "valve_WON", "name": "old.dem", "size": 2048, "mtimeMs": NOW - DAY_MS - 1.0},
            {"dir": "valve", "name": "OLD.DEM", "size": 10, "mtimeMs": NOW - 2.0 * DAY_MS},
        ]})
    }

    fn loaded() -> GameDemos {
        let mut demos = GameDemos::default();
        assert!(demos.begin_load());
        assert!(
            !demos.begin_load(),
            "второй load, пока идёт первый, — ничего"
        );
        demos.list_done(Ok(&list()), NOW, &t);
        demos
    }

    #[test]
    fn fresh_demos_are_chosen_and_the_choice_survives_a_reload() {
        let mut demos = loaded();
        assert!(!demos.loading);
        assert_eq!(demos.summary(&t), "#L_GS_DEMOS_COUNT(3)");
        assert!(demos.is_chosen("valve_WON|c1a0.dem"));
        assert!(!demos.is_chosen("valve_WON|old.dem"));
        assert_eq!(demos.chosen_count(), 1);
        demos.toggle("valve_WON|old.dem");
        demos.toggle("valve_WON|c1a0.dem");
        assert!(demos.begin_load());
        demos.list_done(Ok(&list()), NOW, &t);
        assert!(demos.is_chosen("valve_WON|old.dem"));
        assert!(!demos.is_chosen("valve_WON|c1a0.dem"));
        demos.set_all(true);
        assert_eq!(demos.chosen_count(), 3);
        demos.set_all(false);
        assert_eq!(demos.pack_label(&t), "#L_DEMOS_PACK(0)");
        assert_eq!(demos.begin_pack(), None, "ничего не выбрано");
    }

    #[test]
    fn list_errors_clear_the_list_and_say_why() {
        let mut demos = loaded();
        demos.begin_load();
        demos.list_done(
            Ok(&json!({"status": "error", "code": "NOT_INSTALLED"})),
            NOW,
            &t,
        );
        assert!(demos.demos.is_empty());
        assert_eq!(demos.list_error.as_deref(), Some("#L_GS_NOT_INSTALLED"));
        demos.begin_load();
        assert_eq!(demos.list_error, None);
        demos.list_done(Ok(&json!({"status": "error", "code": "EACCES"})), NOW, &t);
        assert_eq!(
            demos.list_error.as_deref(),
            Some("#L_GS_RUNS_ERROR(EACCES)")
        );
        demos.begin_load();
        demos.list_done(Err(()), NOW, &t);
        assert_eq!(demos.list_error.as_deref(), Some("#L_GS_RUNS_ERROR()"));
    }

    #[test]
    fn packing_sends_the_chosen_demos_shows_progress_and_reveals_the_archive() {
        let mut demos = loaded();
        let items = demos.begin_pack().unwrap();
        assert_eq!(items, json!([{"dir": "valve_WON", "name": "c1a0.dem"}]));
        assert!(demos.begin_pack().is_none(), "упаковка уже идёт");
        demos.progress("other", &json!({"gameId": "70", "percent": 40}));
        assert_eq!(demos.pack_label(&t), "#L_DEMOS_PACKING");
        demos.progress("70", &json!({"gameId": "70", "percent": 42}));
        assert_eq!(demos.pack_label(&t), "#L_DEMOS_PACKING 42%");
        let next = demos.pack_done(
            Ok(&json!({"status": "ok", "archive": {"name": "demos.zip", "size": 2048}})),
            &t,
        );
        assert_eq!(next, PackNext::Reveal("demos.zip".into()));
        assert!(!demos.packing);
        assert_eq!(
            demos.archive_label(&t).as_deref(),
            Some("#L_DEMOS_ARCHIVE(demos.zip,2.0\u{a0}#L_KB)")
        );
    }

    #[test]
    fn pack_errors_have_their_own_texts_and_a_changed_demo_reloads_the_list() {
        let mut demos = loaded();
        for (code, key, next) in [
            ("DEMO_NOT_FOUND", "#L_DEMOS_ERR_CHANGED", PackNext::Reload),
            ("PACK_BUSY", "#L_DEMOS_ERR_BUSY", PackNext::Nothing),
            ("NOT_INSTALLED", "#L_GS_NOT_INSTALLED", PackNext::Nothing),
            ("EIO", "#L_DEMOS_ERR_PACK", PackNext::Nothing),
        ] {
            demos.begin_pack().unwrap();
            let reply = json!({"status": "error", "code": code});
            assert_eq!(demos.pack_done(Ok(&reply), &t), next, "{code}");
            assert_eq!(demos.pack_error.as_deref(), Some(key));
        }
        demos.begin_pack().unwrap();
        assert_eq!(demos.pack_done(Err(()), &t), PackNext::Nothing);
        assert_eq!(demos.pack_error.as_deref(), Some("#L_DEMOS_ERR_PACK"));
    }

    #[test]
    fn delete_asks_first_and_names_with_twins_have_no_button() {
        let mut demos = loaded();
        let old = demos.demos[1].clone();
        assert!(
            !demos.is_unique(&old),
            "old.dem и OLD.DEM — одно имя на Windows"
        );
        assert!(demos.is_unique(&demos.demos[0].clone()));
        let key = "valve_WON|c1a0.dem";
        assert_eq!(demos.begin_delete(key), None, "без вопроса не удаляется");
        demos.ask_delete(key);
        assert_eq!(demos.begin_delete(key).as_deref(), Some("c1a0.dem"));
        demos.cancel_delete();
        assert_eq!(
            demos.pending_delete.as_deref(),
            Some(key),
            "во время удаления отмены нет"
        );
        demos.delete_done(
            key,
            Ok(&json!({"status": "error", "code": "GAME_RUNNING"})),
            &t,
        );
        assert_eq!(demos.delete_error.as_deref(), Some("#L_D_ERR_GAME_RUNNING"));
        assert_eq!(demos.demos.len(), 3);
        assert_eq!(demos.begin_delete(key).as_deref(), Some("c1a0.dem"));
        assert_eq!(demos.delete_error, None);
        demos.delete_done(key, Ok(&json!({"status": "ok"})), &t);
        assert_eq!(demos.demos.len(), 2);
        assert_eq!(demos.pending_delete, None);
        assert_eq!(demos.deleting, None);
    }

    #[test]
    fn the_console_command_drops_the_extension_and_the_check_mark_expires() {
        let mut demos = loaded();
        assert_eq!(demos.copy("valve|OLD.DEM").as_deref(), Some("playdemo OLD"));
        assert_eq!(
            demos.copy("valve_WON|c1a0.dem").as_deref(),
            Some("playdemo c1a0")
        );
        demos.copy_expired("valve|OLD.DEM");
        assert_eq!(demos.copied.as_deref(), Some("valve_WON|c1a0.dem"));
        demos.copy_expired("valve_WON|c1a0.dem");
        assert_eq!(demos.copied, None);
        let odd = |name: &str| Demo {
            dir: "valve".into(),
            name: name.into(),
            size: 0.0,
            mtime_ms: 0.0,
        };
        assert_eq!(odd("жa1b").command(), "playdemo жa1b");
        assert_eq!(odd("дем.DEM").command(), "playdemo дем");
        assert_eq!(odd("dem").command(), "playdemo dem");
        assert!(Engine::GoldSrc.can_delete() && !Engine::Ghosting.can_copy());
        assert_eq!(Engine::Ghosting.speedrun_key(), "speedrun-hl2");
    }

    #[test]
    fn dates_are_day_month_year_and_time_in_the_zone() {
        let utc = chrono::Utc;
        assert_eq!(format_date_ms(NOW, &utc), "17.11.2025 12:00");
        assert_eq!(format_date_ms(f64::NAN, &utc), "");
    }
}
