//! Журнал вызовов окна GPUI ([`super::FixtureServices`]) против `ipc-log.json` Electron: те же записи
//! `{kind, channel, args}` в том же порядке. Летучие поля нормализуются с обеих сторон: строки
//! времени ISO-8601 → `<время>`, числа под ключами времени (`at`, `time`, `timestamp`, `ts`,
//! `now`, `…At`) → `<время>`, значения под ключами токенов (`…token…`, `nonce`) → `<токен>`.
//! Расхождение показывается построчным сравнением (общая подпоследовательность): `-` — было в
//! Electron и нет у GPUI, `+` — лишнее у GPUI, по две строки окружения. Сцены пробников
//! (`journalFrom`) сверяются с хвоста: от последнего вхождения записи-метки — первого вызова
//! самого окна — в обоих журналах ([`compare_from`]).

use serde_json::{Map, Value};

const CONTEXT: usize = 2;

fn is_iso_time(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 16
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
}

fn time_key(key: &str) -> bool {
    matches!(key, "at" | "time" | "timestamp" | "ts" | "now")
        || (key.len() > 2 && key.ends_with("At"))
}

fn token_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.contains("token") || lower == "nonce"
}

/// Значение с нормализованными летучими полями.
pub fn normalize(value: &Value) -> Value {
    match value {
        Value::String(text) if is_iso_time(text) => Value::String("<время>".into()),
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, item) in map {
                let item = if token_key(key) && !item.is_null() {
                    Value::String("<токен>".into())
                } else if time_key(key) && item.is_number() {
                    Value::String("<время>".into())
                } else {
                    normalize(item)
                };
                out.insert(key.clone(), item);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Запись журнала одной строкой: `invoke game-menu-state ["ghosting-renewed"]`.
pub fn entry_line(entry: &Value) -> String {
    let text = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or("?");
    let args = entry
        .get("args")
        .map(|a| serde_json::to_string(a).unwrap_or_default())
        .unwrap_or_default();
    let mut line = format!("{} {} {args}", text("kind"), text("channel"));
    if line.chars().count() > 200 {
        line = line.chars().take(199).collect::<String>() + "…";
    }
    line
}

/// Итог сравнения журналов.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalDiff {
    pub expected: usize,
    pub actual: usize,
    /// Записей Electron, которых нет у GPUI, и лишних у GPUI.
    pub missing: usize,
    pub extra: usize,
    /// Построчное сравнение (пусто, если журналы совпали).
    pub lines: Vec<String>,
    /// Сравнивались хвосты от этой записи ([`compare_from`]).
    pub from: Option<String>,
}

impl JournalDiff {
    pub fn equal(&self) -> bool {
        self.missing == 0 && self.extra == 0
    }

    /// Коротко для таблицы отчёта.
    pub fn summary(&self) -> String {
        let tail = self
            .from
            .as_deref()
            .map(|from| format!(" от `{from}`"))
            .unwrap_or_default();
        if self.equal() {
            format!("совпадает ({}{tail})", self.expected)
        } else {
            format!(
                "−{} +{} (Electron {}, GPUI {}{tail})",
                self.missing, self.extra, self.expected, self.actual
            )
        }
    }
}

/// Сравнить хвосты журналов от последнего вхождения записи `from` в каждом (пробник без
/// оболочки: до первого вызова окна у Electron — вызовы оболочки); журнал без такой записи
/// берётся целиком, и расхождение видно в построчном сравнении. Без `from` — [`compare`].
pub fn compare_from(expected: &[Value], actual: &[Value], from: Option<&Value>) -> JournalDiff {
    let Some(from) = from else {
        return compare(expected, actual);
    };
    let key = |v: &Value| serde_json::to_string(&normalize(v)).unwrap_or_default();
    let marker = key(from);
    let tail = |log: &[Value]| {
        log.iter()
            .rposition(|entry| key(entry) == marker)
            .unwrap_or(0)
    };
    let mut diff = compare(&expected[tail(expected)..], &actual[tail(actual)..]);
    diff.from = Some(entry_line(from));
    diff
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Same(usize, usize),
    Missing(usize),
    Extra(usize),
}

/// Сравнить журнал GPUI `actual` с журналом Electron `expected`.
pub fn compare(expected: &[Value], actual: &[Value]) -> JournalDiff {
    let key = |v: &Value| serde_json::to_string(&normalize(v)).unwrap_or_default();
    let e: Vec<String> = expected.iter().map(key).collect();
    let a: Vec<String> = actual.iter().map(key).collect();
    // Длины общей подпоследовательности суффиксов.
    let (n, m) = (e.len(), a.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if e[i] == a[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && e[i] == a[j] {
            ops.push(Op::Same(i, j));
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            ops.push(Op::Extra(j));
            j += 1;
        } else {
            ops.push(Op::Missing(i));
            i += 1;
        }
    }
    let missing = ops.iter().filter(|o| matches!(o, Op::Missing(_))).count();
    let extra = ops.iter().filter(|o| matches!(o, Op::Extra(_))).count();
    let mut lines = Vec::new();
    if missing + extra > 0 {
        let changed: Vec<usize> = ops
            .iter()
            .enumerate()
            .filter(|(_, o)| !matches!(o, Op::Same(..)))
            .map(|(k, _)| k)
            .collect();
        let near = |k: usize| changed.iter().any(|&c| c.abs_diff(k) <= CONTEXT);
        let mut skipped = false;
        for (k, op) in ops.iter().enumerate() {
            if !near(k) {
                if !skipped {
                    lines.push("  …".to_owned());
                    skipped = true;
                }
                continue;
            }
            skipped = false;
            lines.push(match *op {
                Op::Same(i, _) => format!("  {:>3} {}", i + 1, entry_line(&expected[i])),
                Op::Missing(i) => format!("- {:>3} {}", i + 1, entry_line(&expected[i])),
                Op::Extra(j) => format!("+ {:>3} {}", j + 1, entry_line(&actual[j])),
            });
        }
    }
    JournalDiff {
        expected: n,
        actual: m,
        missing,
        extra,
        lines,
        from: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(kind: &str, channel: &str, args: Value) -> Value {
        json!({"kind": kind, "channel": channel, "args": args})
    }

    #[test]
    fn volatile_fields_are_normalized_on_both_sides() {
        let e = entry(
            "invoke",
            "game-settings-save",
            json!(["218", {"savedAt": 1792152000000i64, "when": "2026-10-16T12:00:00.000Z", "authToken": "abc", "x": 1}]),
        );
        let a = entry(
            "invoke",
            "game-settings-save",
            json!(["218", {"savedAt": 5, "when": "2027-01-01T00:00:00.000Z", "authToken": "zzz", "x": 1}]),
        );
        assert!(compare(std::slice::from_ref(&e), &[a]).equal());
        let other = entry(
            "invoke",
            "game-settings-save",
            json!(["218", {"savedAt": 5, "x": 2}]),
        );
        assert!(!compare(&[e], &[other]).equal());
    }

    #[test]
    fn diff_shows_missing_and_extra_calls_with_context() {
        let expected = vec![
            entry("sendSync", "app-default-library-path", json!([])),
            entry("send", "ready", json!([])),
            entry("invoke", "runner-state", json!([])),
            entry("invoke", "boot-gate-run", json!([{}])),
        ];
        let actual = vec![
            entry("sendSync", "app-default-library-path", json!([])),
            entry("send", "ready", json!([])),
            entry("invoke", "boot-gate-run", json!([{}])),
            entry("invoke", "tool-list", json!([])),
        ];
        let diff = compare(&expected, &actual);
        assert_eq!((diff.missing, diff.extra), (1, 1));
        assert_eq!(
            diff.lines,
            vec![
                "    1 sendSync app-default-library-path []",
                "    2 send ready []",
                "-   3 invoke runner-state []",
                "    4 invoke boot-gate-run [{}]",
                "+   4 invoke tool-list []",
            ]
        );
        assert_eq!(diff.summary(), "−1 +1 (Electron 4, GPUI 4)");
    }

    #[test]
    fn tails_are_compared_from_the_last_marker_on_both_sides() {
        // Electron: оболочка (с её собственным app-version), затем окно; пробник: только окно.
        let marker = entry("sendSync", "app-version", json!([]));
        let expected = vec![
            entry("sendSync", "app-default-library-path", json!([])),
            entry("send", "ready", json!([])),
            entry("sendSync", "app-version", json!([])),
            entry("invoke", "runner-state", json!([])),
            entry("invoke", "tool-list", json!([])),
            entry("sendSync", "app-version", json!([])),
            entry("invoke", "discord-rpc-get", json!([])),
        ];
        let actual = vec![
            entry("sendSync", "app-default-library-path", json!([])),
            entry("send", "ready", json!([])),
            entry("sendSync", "app-version", json!([])),
            entry("invoke", "discord-rpc-get", json!([])),
        ];
        let diff = compare_from(&expected, &actual, Some(&marker));
        assert!(diff.equal(), "{:?}", diff.lines);
        assert_eq!(diff.summary(), "совпадает (2 от `sendSync app-version []`)");
        // Лишний вызов окна виден; без метки — журналы целиком.
        let mut extra = actual.clone();
        extra.push(entry("invoke", "proton-status", json!([])));
        let diff = compare_from(&expected, &extra, Some(&marker));
        assert_eq!((diff.missing, diff.extra), (0, 1));
        assert_eq!(diff.lines.last().unwrap(), "+   3 invoke proton-status []");
        assert_eq!(
            compare_from(&expected, &actual, None),
            compare(&expected, &actual)
        );
        // Метки нет в своём журнале — против хвоста Electron идёт весь журнал.
        let diff = compare_from(&expected, &actual[..2], Some(&marker));
        assert_eq!((diff.missing, diff.extra), (2, 2));
    }

    #[test]
    fn empty_journal_lists_every_expected_call() {
        let expected = vec![
            entry("send", "ready", json!([])),
            entry("invoke", "boot-gate-run", json!([{}])),
        ];
        let diff = compare(&expected, &[]);
        assert_eq!(diff.missing, 2);
        assert_eq!(
            diff.lines,
            vec!["-   1 send ready []", "-   2 invoke boot-gate-run [{}]"]
        );
        assert!(compare(&expected, &expected).lines.is_empty());
    }
}
