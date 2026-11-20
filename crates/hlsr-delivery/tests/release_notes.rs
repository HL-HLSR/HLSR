//! Заметки о выпуске против 0.4.7: фикстура `release-notes.json` из
//! `tools/reference/delivery/release-notes.js`.
//!
//! `to_items` (корпус текстов: пункты списка, разметка, пробелы JS, деление на предложения, пределы
//! 40 × 300 и 2000 знаков, астральные знаки на границах) и `ReleaseNotesStore` (`capture`, `get`,
//! `has`, `list`) на деревьях файлов: `game-release-notes.json` побайтно после серии вызовов, обрезка
//! до 10 версий по времени, порядок версий в списке (`localeCompare`, язык en-US), прежние записи
//! (parser 1), испорченные файлы и записи, ошибки записи. Сценарий — файлы и шаги в данных; их
//! выполняют модуль 0.4.7 в Node 24 и этот модуль на тех же деревьях. Сценарии, помеченные `unix`,
//! зависят от ОС (права доступа): на Windows они пропускаются.

mod support;

use hlsr_core::json_text::{json_parse, stringify};
use hlsr_delivery::errors::RawError;
use hlsr_delivery::ports::{Logger, MemoryLogger};
use hlsr_delivery::release_notes::{self, Notes, ReleaseNotesStore, StoreOptions, to_items};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use support::scenario::{self, common_step, normalize_text, path_in, temp_root};
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("release-notes.json"));

/// `Date.now()` шагов без своего времени.
const NOW: i64 = 1_700_000_000_000;

/// Как сохраняет фикстуру `JSON.stringify` и читает её `JSON.parse`.
fn js_plain(value: &Value) -> Value {
    json_parse(&stringify(value, "")).unwrap()
}

/// Временные файлы атомарной записи называются `.<имя>.<pid>.<время36>.tmp`: pid и время
/// заменяются метками (то же выражение, что в скрипте Node).
fn mask_temp(text: &str) -> String {
    static TEMP: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(/\.[^/'\s]+)\.[0-9]+\.[0-9a-z]+\.tmp").unwrap());
    TEMP.replace_all(text, "$1.<pid>.<time>.tmp").into_owned()
}

/// Текст ошибки или журнала: пути временной папки — метки, временные файлы без pid и времени.
fn mask(root: &std::path::Path, text: &str) -> String {
    mask_temp(&normalize_text(root, text))
}

fn failure(root: &std::path::Path, error: &RawError) -> Value {
    match &error.code {
        Some(code) => json!({"error": {
            "kind": "raw",
            "code": code,
            "message": mask(root, &error.message),
        }}),
        None => json!({"error": {
            "kind": "type",
            "type": "TypeError",
            "message": mask(root, &error.message),
        }}),
    }
}

fn notes_value(notes: &Notes) -> Value {
    json!({"gameId": notes.game_id, "version": notes.version, "items": notes.items})
}

struct Runner {
    root: PathBuf,
    dir: Option<String>,
    _temp: tempfile::TempDir,
}

impl Runner {
    fn new(dir: Option<String>) -> Runner {
        let (temp, root) = temp_root();
        Runner {
            root,
            dir,
            _temp: temp,
        }
    }

    fn store_dir(&self) -> PathBuf {
        match &self.dir {
            Some(dir) => path_in(&self.root, dir),
            None => self.root.clone(),
        }
    }

    fn step(&self, step: &Value) -> Value {
        if let Some(common) = common_step(&self.root, step) {
            return common;
        }
        let root = self.root.as_path();
        match str_of(&step["op"]) {
            "toItems" => match opt_str(&step["text"]) {
                Some(text) => json!({"ok": to_items(text)}),
                // Не строка — `toItems` не вызывается: тип проверяет `capture` (шаги `capture`).
                None => json!({"ok": []}),
            },
            "newEmpty" => match ReleaseNotesStore::new(StoreOptions::default()) {
                Ok(_) => json!({"ok": null}),
                Err(error) => {
                    json!({"error": {"kind": "type", "type": "Error", "message": error.message}})
                }
            },
            "file" => {
                let file = self.store_dir().join(release_notes::FILE_NAME);
                match std::fs::read(&file) {
                    Ok(bytes) => json!({"ok": hlsr_core::js::latin1_string(&bytes)}),
                    Err(_) => json!({"ok": null}),
                }
            }
            op => {
                let logger = Arc::new(MemoryLogger::default());
                let now = step["now"].as_i64().unwrap_or(NOW);
                let store = ReleaseNotesStore::new(StoreOptions {
                    dir: self.store_dir(),
                    logger: Some(logger.clone() as Arc<dyn Logger>),
                    clock: Some(Arc::new(move || now)),
                })
                .expect("каталог задан");
                // Идентификатор или версия не строкой (шаг без поля): тип проверяет сам Rust, а у 0.4.7
                // это `typeof` в `isGameId`/`isVersion`, и ответ — отказ.
                let ids = opt_str(&step["gameId"]).zip(opt_str(&step["version"]));
                let result = match op {
                    "capture" => match ids {
                        Some((game_id, version)) => store
                            .capture(game_id, version, step.get("notes").unwrap_or(&Value::Null))
                            .map(|saved| json!(saved)),
                        None => Ok(json!(false)),
                    },
                    "get" => Ok(ids
                        .and_then(|(game_id, version)| store.get(game_id, version))
                        .map_or(Value::Null, |notes| notes_value(&notes))),
                    "has" => {
                        Ok(json!(ids.is_some_and(|(game_id, version)| {
                            store.has(game_id, version)
                        })))
                    }
                    "list" => Ok(Value::Array(
                        store
                            .list_in("en-US")
                            .iter()
                            .map(|game| {
                                json!({
                                    "gameId": game.game_id,
                                    "versions": game.versions.iter().map(|v| json!({
                                        "version": v.version,
                                        "items": v.items,
                                        "ts": v.ts,
                                    })).collect::<Vec<_>>(),
                                })
                            })
                            .collect(),
                    )),
                    "readAll" => Ok(store.read_all()),
                    other => panic!("неизвестный шаг {other}"),
                };
                let mut out = match result {
                    Ok(value) => json!({"ok": value}),
                    Err(error) => failure(root, &error),
                };
                let lines = logger.take();
                if !lines.is_empty() {
                    out["logs"] = Value::Array(
                        lines
                            .iter()
                            .map(|(level, text)| json!([level, mask(root, text)]))
                            .collect(),
                    );
                }
                out
            }
        }
    }
}

/// Результат в виде, каким его видит JS (числа, `Infinity` → `null`).
fn comparable(value: &Value) -> Value {
    js_plain(value)
}

fn run_scenarios(filter: impl Fn(&str) -> bool, what: &str) {
    let mut mismatches = Mismatches::default();
    let mut ran = 0;
    for scenario in items(&FIXTURE["scenarios"]) {
        let name = str_of(&scenario["name"]);
        if !filter(name) {
            continue;
        }
        if scenario["unix"].as_bool().unwrap_or(false) && cfg!(windows) {
            continue;
        }
        // Права доступа не действуют на root.
        if scenario["permissions"].as_bool().unwrap_or(false) && scenario::is_root_user() {
            continue;
        }
        ran += 1;
        let runner = Runner::new(opt_str(&scenario["dir"]).map(str::to_owned));
        scenario::setup(&runner.root, &scenario["setup"]);
        assert_eq!(
            items(&scenario["steps"]).len(),
            items(&scenario["results"]).len(),
            "шагов и результатов поровну"
        );
        for (index, step) in items(&scenario["steps"]).iter().enumerate() {
            let actual = comparable(&runner.step(step));
            let expected = comparable(&scenario["results"][index]);
            let context = format!("{name}: шаг {index} {}", short(step));
            mismatches.check(&actual, &expected, &context);
            // Равенство значений не видит порядка ключей, а в файле он виден.
            if actual == expected {
                let (actual, expected) = (stringify(&actual, ""), stringify(&expected, ""));
                mismatches.check_eq(actual, expected, format!("{context}: порядок ключей"));
            }
        }
        scenario::open_up(&runner.root);
    }
    assert!(ran > 0, "{what}: ни одного сценария");
    mismatches.finish(what);
}

/// Шаг для сообщения: крупные поля не печатаются целиком.
fn short(step: &Value) -> String {
    let text = step.to_string();
    if text.chars().count() > 300 {
        format!("{}…", text.chars().take(300).collect::<String>())
    } else {
        text
    }
}

#[test]
fn constants_are_those_of_0_4_7() {
    let constants = &FIXTURE["constants"];
    assert_eq!(release_notes::FILE_NAME, str_of(&constants["FILE_NAME"]));
    assert_eq!(
        release_notes::MAX_ITEMS,
        constants["MAX_ITEMS"].as_u64().unwrap() as usize
    );
    assert_eq!(
        release_notes::MAX_ITEM_LENGTH,
        constants["MAX_ITEM_LENGTH"].as_u64().unwrap() as usize
    );
}

#[test]
fn to_items_cuts_notes_into_items_like_0_4_7() {
    run_scenarios(|name| name.starts_with("toItems"), "release-notes: toItems");
}

#[test]
fn store_writes_the_same_bytes_after_a_series_of_calls() {
    run_scenarios(|name| name.starts_with("store:"), "release-notes: store");
}
