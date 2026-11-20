//! Проверка конфига HL1 по правилам speedrun.com (`GameCfgCheck.vue` 0.4.9) и её значок во
//! вкладке «Проверка» (`checkBadge` `GameConfigEditor.vue`, `ruleCounts` `GameSettingsModal.vue`):
//! итог одной строкой, группы «команда + правило» с числом мест, места с путём alias/bind/exec.
//! Группировка — `hlsr_delivery::hl1_config_groups`; здесь тексты для вида.

use super::game_settings::draft::Tr;
use hlsr_core::js;
use hlsr_delivery::hl1_config_groups::{group_findings, rule_detail, rule_key};
use serde_json::Value;

/// Категории проверки (`CATEGORIES`): радио «Scriptless / Scripted».
pub const CATEGORIES: [(&str, &str); 2] = [("scriptless", "Scriptless"), ("scripted", "Scripted")];

/// Итог проверки для значка (`ruleCounts`): ответ `ok` — его числа; иначе только сбой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleCounts {
    pub errors: u64,
    pub warnings: u64,
    pub failed: bool,
}

fn count(value: &Value) -> u64 {
    js::value::number(value)
        .filter(|n| n.is_finite() && *n > 0.0)
        .map_or(0, |n| n as u64)
}

/// `ruleCounts`: `None` — вкладки «Проверка» нет (не HL1 или проверять нечего).
pub fn rule_counts(check: Option<&Value>, failed: bool) -> Option<RuleCounts> {
    match check.filter(|check| check["status"] == "ok") {
        Some(check) => Some(RuleCounts {
            errors: count(&check["stats"]["errors"]),
            warnings: count(&check["stats"]["warnings"]),
            failed,
        }),
        None => failed.then_some(RuleCounts {
            errors: 0,
            warnings: 0,
            failed: true,
        }),
    }
}

/// Значок во вкладке «Проверка»: уровень, значок, число (0 — без числа) и текст для чтеца.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub level: &'static str,
    pub icon: &'static str,
    pub count: u64,
    pub text: String,
}

/// `checkBadge`.
pub fn badge(counts: Option<RuleCounts>, tr: Tr<'_>) -> Option<Badge> {
    let counts = counts?;
    let badge = |level, icon, count, text| Badge {
        level,
        icon,
        count,
        text,
    };
    Some(if counts.failed {
        badge(
            "warning",
            "fa-exclamation-triangle",
            0,
            tr("#L_HC_FAILED", &[]),
        )
    } else if counts.errors > 0 {
        let n = counts.errors.to_string();
        badge(
            "error",
            "fa-times-circle",
            counts.errors,
            tr("#L_HC_COUNT_ERRORS", &[&n]),
        )
    } else if counts.warnings > 0 {
        let n = counts.warnings.to_string();
        badge(
            "warning",
            "fa-exclamation-triangle",
            counts.warnings,
            tr("#L_HC_COUNT_WARNINGS", &[&n]),
        )
    } else {
        badge("ok", "fa-check-circle", 0, tr("#L_HC_CLEAN", &[]))
    })
}

/// Значок уровня (`ICONS`).
pub fn level_icon(level: &str) -> &'static str {
    match level {
        "error" => "fa-times-circle",
        "warning" => "fa-exclamation-triangle",
        _ => "fa-info-circle",
    }
}

fn level_key(level: &str) -> &'static str {
    match level {
        "error" => "#L_HC_LEVEL_ERROR",
        "warning" => "#L_HC_LEVEL_WARNING",
        _ => "#L_HC_LEVEL_INFO",
    }
}

/// Место находки: `file:line`, команда, путь до неё; `path` — открыть в «Файлах cfg».
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceView {
    pub path: Option<String>,
    pub line: usize,
    /// `file` и `:line`, если строка есть.
    pub place: String,
    pub command: String,
    pub context: String,
    /// `title` кнопки (`#L_HC_PLACE_OPEN`).
    pub title: String,
}

/// Группа «команда + правило».
#[derive(Debug, Clone, PartialEq)]
pub struct GroupView {
    pub key: String,
    pub level: String,
    pub icon: &'static str,
    pub level_text: String,
    pub name: String,
    pub reason: String,
    pub places: Vec<PlaceView>,
}

/// Числа итога (`stats`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub files: u64,
    pub commands: u64,
    pub errors: u64,
    pub warnings: u64,
    pub infos: u64,
}

/// Что показывает «Проверка».
#[derive(Debug, Clone, PartialEq)]
pub struct CheckView {
    /// Есть ответ `ok` (`ready`); нет — «Проверка идёт…» или «Проверить не получилось».
    pub ready: bool,
    pub loading: bool,
    /// Перепроверка не удалась: показан прежний итог.
    pub failed: bool,
    pub stats: Stats,
    pub groups: Vec<GroupView>,
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => js::to_string(other),
    }
}

/// `trailText`: alias и bind, из которых запущена команда; exec — файл, зовущийся из них.
pub fn trail_text(trail: &Value, tr: Tr<'_>) -> String {
    trail
        .as_array()
        .into_iter()
        .flatten()
        .map(|step| match step["kind"].as_str() {
            Some("exec") => tr("#L_HC_CTX_EXEC", &[&trail_text(&step["via"], tr)]),
            Some("bind") => tr("#L_HC_CTX_BIND", &[&text(&step["name"])]),
            _ => tr("#L_HC_CTX_ALIAS", &[&text(&step["name"])]),
        })
        .collect::<Vec<_>>()
        .join(" › ")
}

/// Вид «Проверки» по ответу `player-config-check` (`status: "ok"` или `None`).
pub fn check_view(check: Option<&Value>, loading: bool, failed: bool, tr: Tr<'_>) -> CheckView {
    let Some(check) = check.filter(|check| check["status"] == "ok") else {
        return CheckView {
            ready: false,
            loading,
            failed,
            stats: Stats::default(),
            groups: Vec::new(),
        };
    };
    let stats = &check["stats"];
    let findings = check["findings"].as_array().cloned().unwrap_or_default();
    let groups = group_findings(&findings)
        .into_iter()
        .map(|group| {
            let reason = match rule_key(&group.rule) {
                Some(key) => tr(key, &[rule_detail(&group)]),
                None => group.note.clone(),
            };
            let places = group
                .places
                .iter()
                .map(|place| {
                    let file = text(&place.file);
                    let line = js::value::number(&place.line)
                        .filter(|n| *n > 0.0)
                        .map_or(0, |n| n as usize);
                    let line_text = text(&place.line);
                    let has_trail = place
                        .trail
                        .as_array()
                        .is_some_and(|steps| !steps.is_empty());
                    PlaceView {
                        path: place.path.clone(),
                        line,
                        place: if js::truthy(&place.line) {
                            format!("{file}:{line_text}")
                        } else {
                            file.clone()
                        },
                        command: text(&place.command),
                        context: if has_trail {
                            trail_text(&place.trail, tr)
                        } else {
                            tr("#L_HC_TOP_LEVEL", &[])
                        },
                        title: tr("#L_HC_PLACE_OPEN", &[&file, &line_text]),
                    }
                })
                .collect();
            GroupView {
                icon: level_icon(&group.level),
                level_text: tr(level_key(&group.level), &[]),
                key: group.key,
                level: group.level,
                name: group.name,
                reason,
                places,
            }
        })
        .collect();
    CheckView {
        ready: true,
        loading,
        failed,
        stats: Stats {
            files: count(&stats["files"]),
            commands: count(&stats["commands"]),
            errors: count(&stats["errors"]),
            warnings: count(&stats["warnings"]),
            infos: count(&stats["infos"]),
        },
        groups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tr(key: &str, args: &[&str]) -> String {
        if args.is_empty() {
            key.to_owned()
        } else {
            format!("{key}({})", args.join("|"))
        }
    }

    fn check() -> Value {
        json!({
            "status": "ok",
            "stats": {"files": 3, "commands": 41, "errors": 2, "warnings": 1, "infos": 0},
            "findings": [
                {"path": "valve/userconfig.cfg", "file": "userconfig.cfg", "line": 7, "command": "cl_forwardspeed 999",
                 "rule": "values", "level": "warning", "note": "Allowed values: 400"},
                {"path": "valve/userconfig.cfg", "file": "userconfig.cfg", "line": 12, "command": "+jumpbug",
                 "rule": "banned", "level": "error", "note": "",
                 "trail": [{"kind": "alias", "name": "jb"}, {"kind": "exec", "via": [{"kind": "bind", "name": "MOUSE4"}]}]},
                {"path": "", "file": "autoexec.cfg", "line": 0, "command": "+JumpBug x",
                 "rule": "banned", "level": "error", "note": ""}
            ]
        })
    }

    // GameCfgCheck.vue: группы по команде и правилу, ошибки выше; причина — строка правила с
    // хвостом пояснения; место — файл:строка, путь alias › exec из bind.
    #[test]
    fn findings_become_groups_with_places_and_reasons() {
        let view = check_view(Some(&check()), false, false, &tr);
        assert!(view.ready);
        assert_eq!(view.stats.files, 3);
        assert_eq!(view.stats.commands, 41);
        assert_eq!(view.groups.len(), 2);
        let banned = &view.groups[0];
        assert_eq!(banned.key, "+jumpbug banned");
        assert_eq!(banned.icon, "fa-times-circle");
        assert_eq!(banned.level_text, "#L_HC_LEVEL_ERROR");
        assert_eq!(banned.reason, "#L_HC_RULE_BANNED()");
        assert_eq!(banned.places.len(), 2);
        let first = &banned.places[0];
        assert_eq!(first.place, "userconfig.cfg:12");
        assert_eq!(first.line, 12);
        assert_eq!(first.path.as_deref(), Some("valve/userconfig.cfg"));
        assert_eq!(
            first.context,
            "#L_HC_CTX_ALIAS(jb) › #L_HC_CTX_EXEC(#L_HC_CTX_BIND(MOUSE4))"
        );
        assert_eq!(first.title, "#L_HC_PLACE_OPEN(userconfig.cfg|12)");
        // Без строки и пути: только файл, кнопка не открывает.
        let second = &banned.places[1];
        assert_eq!(second.place, "autoexec.cfg");
        assert_eq!(second.path, None);
        assert_eq!(second.context, "#L_HC_TOP_LEVEL");
        let values = &view.groups[1];
        assert_eq!(values.reason, "#L_HC_RULE_VALUES(400)");
        assert_eq!(values.icon, "fa-exclamation-triangle");
    }

    #[test]
    fn without_an_ok_reply_there_is_only_the_state_line() {
        let view = check_view(Some(&json!({"status": "nothing"})), true, false, &tr);
        assert!(!view.ready && view.loading && view.groups.is_empty());
        let view = check_view(None, false, true, &tr);
        assert!(!view.ready && view.failed);
    }

    // GameSettingsModal.ruleCounts + GameConfigEditor.checkBadge.
    #[test]
    fn the_badge_shows_the_worst_level_and_its_count() {
        let counts = rule_counts(Some(&check()), false);
        assert_eq!(
            counts,
            Some(RuleCounts {
                errors: 2,
                warnings: 1,
                failed: false
            })
        );
        let badge = badge(counts, &tr).unwrap();
        assert_eq!((badge.level, badge.count), ("error", 2));
        assert_eq!(badge.text, "#L_HC_COUNT_ERRORS(2)");
        let warn = super::badge(
            Some(RuleCounts {
                errors: 0,
                warnings: 3,
                failed: false,
            }),
            &tr,
        )
        .unwrap();
        assert_eq!(
            (warn.level, warn.icon, warn.count),
            ("warning", "fa-exclamation-triangle", 3)
        );
        let clean = super::badge(
            Some(RuleCounts {
                errors: 0,
                warnings: 0,
                failed: false,
            }),
            &tr,
        )
        .unwrap();
        assert_eq!(
            (clean.level, clean.count, clean.text.as_str()),
            ("ok", 0, "#L_HC_CLEAN")
        );
        // Сбой перепроверки — предупреждение без числа, даже при прежних ошибках.
        let failed = super::badge(rule_counts(Some(&check()), true), &tr).unwrap();
        assert_eq!((failed.level, failed.count), ("warning", 0));
        assert_eq!(
            rule_counts(None, true),
            Some(RuleCounts {
                errors: 0,
                warnings: 0,
                failed: true
            })
        );
        assert_eq!(
            rule_counts(Some(&json!({"status": "unsupported"})), false),
            None
        );
        assert!(super::badge(None, &tr).is_none());
    }
}
