//! Вкладки «Призраки» (Ghosting) и «Демо» (HL1): список записей `game-runs-*`, удаление с
//! подтверждением в строке, команда демо для консоли; вкладка «Версия»: факты сборки
//! (`runs`, `loadRuns`, `deleteRun`, `versionFacts`, `formatSize`, `formatDate`, `dateText`).

use super::super::jsv;
use super::draft::{Item, Tr};
use chrono::{DateTime, Datelike, TimeZone, Timelike};
use hlsr_core::js;
use serde_json::Value;

/// Запись папки `runs`/демо (`{id, name, size, modifiedAt}`).
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub id: String,
    pub name: String,
    /// `displayName` 0.4.9 (имя из файла записи); пусто — показывается `name`.
    pub display_name: String,
    pub size: f64,
    pub modified_at: String,
}

impl Run {
    fn from_value(value: &Value) -> Run {
        let text = |name: &str| {
            jsv::prop(value, name)
                .map(js::to_string)
                .unwrap_or_default()
        };
        Run {
            id: text("id"),
            name: text("name"),
            display_name: jsv::str_of(jsv::prop(value, "displayName"))
                .unwrap_or_default()
                .to_owned(),
            size: jsv::or_zero(jsv::prop(value, "size")),
            modified_at: text("modifiedAt"),
        }
    }
}

/// Состояние списка записей.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Runs {
    pub items: Vec<Run>,
    pub loading: bool,
    pub error: Option<String>,
    pub truncated: bool,
    /// Строка с вопросом «Вы точно хотите удалить?».
    pub pending_delete: Option<String>,
    /// Идёт удаление этой записи.
    pub deleting: Option<String>,
    pub delete_error: Option<String>,
    /// Демо, команда которого только что скопирована (1,5 с).
    pub copied: Option<String>,
}

/// Что делать после ответа `game-runs-list` (`loadRuns`).
impl Runs {
    /// Применить ответ `game-runs-list`.
    pub fn apply_list(&mut self, result: Option<&Value>, tr: Tr<'_>) {
        let status = result.and_then(|result| jsv::str_of(jsv::prop(result, "status")));
        // `!result` и `not-installed` — игра не установлена.
        match (result.filter(|r| js::truthy(r)), status) {
            (None, _) | (_, Some("not-installed")) => {
                self.items.clear();
                self.error = Some(tr("#L_GS_NOT_INSTALLED", &[]));
            }
            (_, Some("no-directory")) => {
                self.items.clear();
            }
            (Some(result), status) if status != Some("ok") => {
                self.items.clear();
                let code = jsv::prop(result, "code")
                    .filter(|code| js::truthy(code))
                    .map(js::to_string)
                    .unwrap_or_default();
                self.error = Some(tr("#L_GS_RUNS_ERROR", &[&code]));
            }
            (Some(result), _) => {
                self.items = jsv::prop(result, "items")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().map(Run::from_value).collect())
                    .unwrap_or_default();
                self.truncated = jsv::truthy(jsv::prop(result, "truncated"));
            }
        }
    }

    /// `runsAvailable`.
    pub fn available(&self) -> bool {
        self.error.is_none() && !self.loading
    }

    /// `ghostsSummary`.
    pub fn summary(&self, demos: bool, tr: Tr<'_>) -> String {
        if self.loading {
            return tr("#L_GS_LOADING", &[]);
        }
        if let Some(error) = &self.error {
            return error.clone();
        }
        let suffix = if self.truncated { " +" } else { "" };
        let key = if demos {
            "#L_GS_DEMOS_COUNT"
        } else {
            "#L_GS_GHOSTS_COUNT"
        };
        tr(key, &[&format!("{}{suffix}", self.items.len())])
    }

    /// `confirmRunDelete(run)`.
    pub fn confirm_delete(&mut self, id: &str) {
        if self.deleting.is_some() {
            return;
        }
        self.delete_error = None;
        self.pending_delete = Some(id.to_owned());
    }

    /// Esc, пока вопрос открыт (`onKeydown`): закрывает вопрос, если запись не удаляется.
    /// `true` — Esc обработан.
    pub fn escape(&mut self) -> bool {
        if self.pending_delete.is_none() {
            return false;
        }
        if self.deleting.is_none() {
            self.pending_delete = None;
        }
        true
    }

    /// Можно ли начать удаление `id` (`deleteRun`: ничего не удаляется и вопрос задан именно по
    /// этой записи).
    pub fn may_delete(&self, id: &str) -> bool {
        self.deleting.is_none() && self.pending_delete.as_deref() == Some(id)
    }

    /// Ответ `game-runs-delete`; `true` — после удаления нужно перечитать список (усечённый).
    pub fn delete_done(
        &mut self,
        id: &str,
        reply: Result<&Value, ()>,
        demos: bool,
        tr: Tr<'_>,
    ) -> bool {
        self.deleting = None;
        let failed_key = if demos {
            "#L_GS_DEMO_DELETE_FAILED"
        } else {
            "#L_GS_GHOST_DELETE_FAILED"
        };
        let Ok(result) = reply else {
            self.delete_error = Some(tr(failed_key, &[]));
            return false;
        };
        if !jsv::is_str(jsv::prop(result, "status"), "ok") {
            self.delete_error = Some(if jsv::is_str(jsv::prop(result, "code"), "GAME_RUNNING") {
                tr("#L_D_ERR_GAME_RUNNING", &[])
            } else {
                tr(failed_key, &[])
            });
            return false;
        }
        self.items.retain(|item| item.id != id);
        self.pending_delete = None;
        self.truncated
    }
}

/// `formatSize(bytes)`.
pub fn format_size(bytes: f64) -> String {
    if bytes == 0.0 || bytes.is_nan() {
        return "0 KB".to_owned();
    }
    if bytes < 1024.0 {
        return format!("{} B", js::number::number_to_string(bytes));
    }
    if bytes < 1024.0 * 1024.0 {
        return format!("{} KB", js::number::to_fixed(bytes / 1024.0, 1));
    }
    format!("{} MB", js::number::to_fixed(bytes / 1024.0 / 1024.0, 1))
}

fn parse_iso<Tz: TimeZone>(iso: &str, tz: &Tz) -> Option<DateTime<Tz>> {
    DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|moment| moment.with_timezone(tz))
}

/// `formatDate(iso)`: «ДД.ММ.ГГГГ ЧЧ:ММ» по местному времени; неверная дата — пустая строка.
pub fn format_date<Tz: TimeZone>(iso: &str, tz: &Tz) -> String {
    match parse_iso(iso, tz) {
        Some(date) => format!(
            "{:02}.{:02}.{} {:02}:{:02}",
            date.day(),
            date.month(),
            date.year(),
            date.hour(),
            date.minute()
        ),
        None => String::new(),
    }
}

/// `dateText(iso)`: «ДД.ММ.ГГГГ»; нет даты — «Нет данных».
pub fn date_text<Tz: TimeZone>(iso: Option<&Value>, tz: &Tz, tr: Tr<'_>) -> String {
    let none = || tr("#L_NO_DATA", &[]);
    let Some(iso) = iso.filter(|iso| js::truthy(iso)) else {
        return none();
    };
    let Value::String(iso) = iso else {
        return none();
    };
    match parse_iso(iso, tz) {
        Some(date) => format!("{:02}.{:02}.{}", date.day(), date.month(), date.year()),
        None => none(),
    }
}

/// `demoCommand(run)`: демо из папки мода проигрывается по имени.
pub fn demo_command(name: &str) -> String {
    let stem = match name.len().checked_sub(4) {
        Some(at) if name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(".dem") => {
            &name[..at]
        }
        _ => name,
    };
    format!("playdemo {stem}")
}

/// Строка «факт»: подпись и значение (`versionFacts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub label: String,
    pub value: String,
}

/// `versionFacts`: сведения о сборке из `game-delivery-get-status`.
pub fn version_facts<Tz: TimeZone>(delivery: Option<&Value>, tz: &Tz, tr: Tr<'_>) -> Vec<Fact> {
    let delivery = delivery.filter(|d| js::truthy(d));
    let meta = jsv::field(delivery, "buildMetadata").filter(|meta| js::truthy(meta));
    let none = tr("#L_NO_DATA", &[]);
    let shown = |value: Option<&Value>| match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => js::to_string(other),
    };
    let or_none = |value: Option<&Value>| {
        value
            .filter(|value| js::truthy(value))
            .map_or_else(|| none.clone(), |value| shown(Some(value)))
    };
    let trust = {
        let state = jsv::field(delivery, "metadataState")
            .filter(|value| js::truthy(value))
            .map_or_else(|| "unknown".to_owned(), js::to_string);
        tr(
            &format!("#L_GS_TRUST_{}", state.to_uppercase().replace('-', "_")),
            &[],
        )
    };
    let fact = |label: &str, value: String| Fact {
        label: tr(label, &[]),
        value,
    };
    vec![
        fact(
            "#L_GS_V_NAME",
            or_none(meta.and_then(|m| jsv::prop(m, "buildName"))),
        ),
        fact(
            "#L_GS_V_GAME",
            or_none(jsv::field(delivery, "installedVersion")),
        ),
        // `meta ? meta.buildId : none`: пустой buildId показывается пустым.
        fact(
            "#L_GS_V_BUILD",
            match meta {
                Some(meta) => shown(jsv::prop(meta, "buildId")),
                None => none.clone(),
            },
        ),
        fact(
            "#L_GS_V_BUILT",
            date_text(jsv::field(delivery, "buildDate"), tz, tr),
        ),
        fact(
            "#L_GS_V_INSTALLED",
            date_text(jsv::field(delivery, "installedAt"), tz, tr),
        ),
        fact("#L_GS_V_TRUST", trust),
        fact(
            "#L_GS_V_MINLAUNCHER",
            or_none(meta.and_then(|m| jsv::prop(m, "minimumLauncherVersion"))),
        ),
    ]
}

/// Пункты вкладки как `Item` (для проб и отчётов).
pub fn run_items(runs: &Runs) -> Vec<Item> {
    runs.items
        .iter()
        .map(|run| Item::new(run.id.clone(), run.name.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Utc};
    use serde_json::json;

    fn tr(key: &str, args: &[&str]) -> String {
        let mut text = key.to_owned();
        for arg in args {
            text = format!("{text}({arg})");
        }
        text
    }

    fn runs(names: &[&str]) -> Runs {
        let mut state = Runs::default();
        let items: Vec<Value> = names
            .iter()
            .map(|name| json!({"id": name, "name": name, "size": 2048, "modifiedAt": "2026-10-01T10:00:00.000Z"}))
            .collect();
        state.apply_list(Some(&json!({"status": "ok", "items": items})), &tr);
        state
    }

    #[test]
    fn list_answers_map_to_rows_and_errors() {
        let state = runs(&["one.run", "two.run"]);
        assert_eq!(state.items.len(), 2);
        assert!(state.available());
        assert_eq!(state.summary(false, &tr), "#L_GS_GHOSTS_COUNT(2)");
        assert_eq!(state.summary(true, &tr), "#L_GS_DEMOS_COUNT(2)");

        let mut state = runs(&["a"]);
        state.apply_list(Some(&json!({"status": "no-directory"})), &tr);
        assert!(state.items.is_empty() && state.error.is_none());
        state.apply_list(Some(&json!({"status": "not-installed"})), &tr);
        assert_eq!(state.error.as_deref(), Some("#L_GS_NOT_INSTALLED"));
        assert!(!state.available());
        state.apply_list(Some(&json!({"status": "error", "code": "EACCES"})), &tr);
        assert_eq!(state.error.as_deref(), Some("#L_GS_RUNS_ERROR(EACCES)"));
        // Пустой ответ — как «игра не установлена».
        state.apply_list(None, &tr);
        assert_eq!(state.error.as_deref(), Some("#L_GS_NOT_INSTALLED"));

        let mut state = Runs {
            loading: true,
            ..Runs::default()
        };
        assert_eq!(state.summary(false, &tr), "#L_GS_LOADING");
        state.loading = false;
        state.truncated = true;
        assert_eq!(state.summary(false, &tr), "#L_GS_GHOSTS_COUNT(0 +)");
    }

    // launcher-ui-selftest.js: «ghost deletion requires inline confirmation and removes only the
    // confirmed row».
    #[test]
    fn ghost_deletion_requires_inline_confirmation_and_removes_only_the_confirmed_row() {
        let mut state = runs(&["one", "two"]);
        assert!(!state.may_delete("one"), "без вопроса удаления нет");
        state.confirm_delete("one");
        assert!(state.may_delete("one"));
        assert!(!state.may_delete("two"));
        state.deleting = Some("one".into());
        assert!(!state.may_delete("one"), "пока идёт удаление — второго нет");
        let refresh = state.delete_done("one", Ok(&json!({"status": "ok"})), false, &tr);
        assert!(!refresh);
        assert_eq!(
            state
                .items
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["two"]
        );
        assert_eq!(state.pending_delete, None);
        assert_eq!(state.deleting, None);
    }

    // «failed ghost deletion keeps the row and confirmation available for retry».
    #[test]
    fn failed_ghost_deletion_keeps_the_row_and_the_question_for_a_retry() {
        let mut state = runs(&["one"]);
        state.confirm_delete("one");
        state.deleting = Some("one".into());
        state.delete_done(
            "one",
            Ok(&json!({"status": "error", "code": "GAME_RUNNING"})),
            false,
            &tr,
        );
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.pending_delete.as_deref(), Some("one"));
        assert_eq!(state.deleting, None);
        assert_eq!(state.delete_error.as_deref(), Some("#L_D_ERR_GAME_RUNNING"));
        // Другая ошибка и сбой канала — текст «не удалось удалить» по виду записей.
        state.deleting = Some("one".into());
        state.delete_done("one", Ok(&json!({"status": "error"})), true, &tr);
        assert_eq!(
            state.delete_error.as_deref(),
            Some("#L_GS_DEMO_DELETE_FAILED")
        );
        state.delete_done("one", Err(()), false, &tr);
        assert_eq!(
            state.delete_error.as_deref(),
            Some("#L_GS_GHOST_DELETE_FAILED")
        );
        // Новый вопрос стирает прежнюю ошибку.
        state.confirm_delete("one");
        assert_eq!(state.delete_error, None);
    }

    #[test]
    fn truncated_list_is_read_again_after_a_delete_and_escape_closes_the_question() {
        let mut state = runs(&["a", "b"]);
        state.truncated = true;
        state.confirm_delete("a");
        assert!(state.delete_done("a", Ok(&json!({"status": "ok"})), false, &tr));
        // Esc: вопрос закрывается, пока ничего не удаляется; во время удаления остаётся.
        state.confirm_delete("b");
        assert!(state.escape());
        assert_eq!(state.pending_delete, None);
        assert!(!state.escape(), "вопроса нет — Esc идёт окну");
        state.confirm_delete("b");
        state.deleting = Some("b".into());
        assert!(state.escape());
        assert_eq!(state.pending_delete.as_deref(), Some("b"));
    }

    #[test]
    fn sizes_and_dates_are_shown_like_the_launcher_does() {
        assert_eq!(format_size(0.0), "0 KB");
        assert_eq!(format_size(512.0), "512 B");
        assert_eq!(format_size(1536.0), "1.5 KB");
        assert_eq!(format_size(1024.0 * 1024.0 * 2.25), "2.3 MB");
        let moscow = FixedOffset::east_opt(3 * 3600).unwrap();
        assert_eq!(
            format_date("2026-10-01T10:05:00.000Z", &moscow),
            "01.10.2026 13:05"
        );
        assert_eq!(format_date("не дата", &moscow), "");
        assert_eq!(
            date_text(Some(&json!("2026-10-01T22:00:00.000Z")), &moscow, &tr),
            "02.10.2026"
        );
        assert_eq!(date_text(None, &Utc, &tr), "#L_NO_DATA");
        assert_eq!(date_text(Some(&json!("")), &Utc, &tr), "#L_NO_DATA");
        assert_eq!(date_text(Some(&json!("мусор")), &Utc, &tr), "#L_NO_DATA");
        assert_eq!(demo_command("speedrun.dem"), "playdemo speedrun");
        assert_eq!(demo_command("Run.DEM"), "playdemo Run");
        assert_eq!(demo_command("x.demo"), "playdemo x.demo");
    }

    #[test]
    fn version_facts_follow_the_delivery_status() {
        let delivery = json!({
            "installedVersion": "1.0.9",
            "installedAt": "2026-10-01T09:30:00.000Z",
            "buildDate": "2026-09-28T00:00:00.000Z",
            "metadataState": "repair-required",
            "buildMetadata": {"buildName": "Ghosting Renewed", "buildId": "b-42", "minimumLauncherVersion": "0.4.7"},
        });
        let facts = version_facts(Some(&delivery), &Utc, &tr);
        let labels: Vec<&str> = facts.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "#L_GS_V_NAME",
                "#L_GS_V_GAME",
                "#L_GS_V_BUILD",
                "#L_GS_V_BUILT",
                "#L_GS_V_INSTALLED",
                "#L_GS_V_TRUST",
                "#L_GS_V_MINLAUNCHER"
            ]
        );
        let values: Vec<&str> = facts.iter().map(|f| f.value.as_str()).collect();
        assert_eq!(
            values,
            [
                "Ghosting Renewed",
                "1.0.9",
                "b-42",
                "28.09.2026",
                "01.10.2026",
                "#L_GS_TRUST_REPAIR_REQUIRED",
                "0.4.7"
            ]
        );
        // Нет статуса: везде «Нет данных», доверие — «нет данных».
        let empty = version_facts(None, &Utc, &tr);
        assert!(
            empty
                .iter()
                .filter(|f| f.label != "#L_GS_V_TRUST")
                .all(|f| f.value == "#L_NO_DATA")
        );
        assert_eq!(empty[5].value, "#L_GS_TRUST_UNKNOWN");
        // Метаданные без buildId — пустое значение, не «Нет данных».
        let partial = json!({"buildMetadata": {"buildName": "X"}, "metadataState": "legacy"});
        let facts = version_facts(Some(&partial), &Utc, &tr);
        assert_eq!(facts[2].value, "");
        assert_eq!(facts[5].value, "#L_GS_TRUST_LEGACY");
    }
}
