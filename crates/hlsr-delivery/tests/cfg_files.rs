//! Редактор cfg-файлов (список, чтение, запись) против 0.4.7: фикстура `cfg-files.json` из
//! `tools/reference/delivery/cfg-files.js`. Сценарий — файлы и шаги в данных; их выполняют
//! модуль 0.4.7 в Node 24 и этот модуль на тех же деревьях во временных папках; сравниваются
//! ответ каждого шага и дерево файлов (путь, размер, sha256, текст latin1).
//!
//! Порядок списка — `localeCompare` (ICU): язык шага (`en-US` по умолчанию, `ru-RU`, `de-DE`).
//! Время записи — `mtimeMs` (миллисекунды с дробью); у только что записанного файла в фикстуре
//! `"now"` (или `mtimeRecent`), а не число.
//!
//! Сценарии, помеченные `unix`, зависят от ОС (ссылки, права доступа, регистр имён на диске,
//! ошибки rename): на Windows они пропускаются.

mod support;

use hlsr_core::json_text::{js_number, stringify};
use hlsr_delivery::cfg_files::{self, CfgFilesError, WriteOptions};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};
use support::scenario::{self, common_step, expand, path_in, temp_root};
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("cfg-files.json"));

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64
}

/// Время записи только что писавшегося файла — «сейчас» (признак вместо числа).
fn nowish(mtime: f64) -> Value {
    if (now_ms() - mtime).abs() < 120_000.0 {
        json!("now")
    } else {
        js_number(mtime)
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Больше 40 записей — сводка: число, три первые, три последние и sha256 JSON всего списка.
fn summarize(list: Vec<Value>) -> Value {
    if list.len() <= 40 {
        return Value::Array(list);
    }
    let text = stringify(&Value::Array(list.clone()), "");
    json!({
        "count": list.len(),
        "head": list[..3],
        "tail": list[list.len() - 3..],
        "sha256": sha256_hex(text.as_bytes()),
    })
}

fn failure(error: &CfgFilesError) -> Value {
    match error {
        CfgFilesError::Cfg(error) => json!({"error": {"kind": "cfg", "code": error.code.as_str()}}),
        CfgFilesError::Raw(error) => json!({"error": {"kind": "raw", "code": error.code}}),
    }
}

struct Runner {
    root: PathBuf,
    _temp: tempfile::TempDir,
}

impl Runner {
    fn new() -> Runner {
        let (temp, root) = temp_root();
        Runner { root, _temp: temp }
    }

    fn install(&self, step: &Value) -> PathBuf {
        match opt_str(&step["install"]) {
            Some("empty") => PathBuf::new(),
            Some("missing") => path_in(&self.root, "no-such-install"),
            _ => match opt_str(&step["installDir"]) {
                Some(dir) => path_in(&self.root, dir),
                None => self.root.clone(),
            },
        }
    }

    fn step(&self, step: &Value) -> Value {
        if let Some(common) = common_step(&self.root, step) {
            return common;
        }
        let install = self.install(step);
        match str_of(&step["op"]) {
            "cfgList" => {
                let listing =
                    cfg_files::list_in(&install, opt_str(&step["locale"]).unwrap_or("en-US"));
                let files = listing
                    .files
                    .iter()
                    .map(|f| json!({"path": f.path, "size": f.size, "mtime": nowish(f.mtime)}))
                    .collect();
                json!({"files": summarize(files), "truncated": listing.truncated})
            }
            "cfgRead" => match cfg_files::read(&install, str_of(&step["relative"])) {
                Err(error) => failure(&error),
                Ok(read) => {
                    let mut ok = json!({
                        "path": read.path,
                        "encoding": read.encoding.as_str(),
                        "eol": read.eol.as_str(),
                        "mtime": nowish(read.mtime),
                    });
                    let units = read.content.encode_utf16().count();
                    if units <= 4000 {
                        ok["content"] = json!(read.content);
                    } else {
                        ok["contentLength"] = json!(units);
                        ok["contentSha256"] = json!(sha256_hex(read.content.as_bytes()));
                    }
                    json!({"ok": ok})
                }
            },
            "cfgWrite" => {
                // Как index.js: meta из рендерера — объект {encoding, eol, mtime}.
                let options = WriteOptions::from_meta(&step["meta"]);
                let content = match step.get("big") {
                    Some(big) => expand(big),
                    None => cfg_files::content_text(&step["content"]),
                };
                match cfg_files::write(&install, str_of(&step["relative"]), &content, &options) {
                    Err(error) => failure(&error),
                    Ok(written) => json!({"ok": {
                        "path": written.path,
                        "mtimeRecent": (now_ms() - written.mtime).abs() < 120_000.0,
                    }}),
                }
            }
            other => panic!("неизвестный шаг {other}"),
        }
    }
}

fn run_scenarios(filter: impl Fn(&Value) -> bool, what: &str) {
    let mut mismatches = Mismatches::default();
    for scenario in items(&FIXTURE["scenarios"]) {
        let name = str_of(&scenario["name"]);
        if !filter(scenario) {
            continue;
        }
        if scenario["unix"].as_bool().unwrap_or(false) && cfg!(windows) {
            continue;
        }
        // Права доступа не действуют на root.
        if scenario["permissions"].as_bool().unwrap_or(false) && scenario::is_root_user() {
            continue;
        }
        let runner = Runner::new();
        scenario::setup(&runner.root, &scenario["setup"]);
        assert_eq!(
            items(&scenario["steps"]).len(),
            items(&scenario["results"]).len(),
            "шагов и результатов поровну"
        );
        for (index, step) in items(&scenario["steps"]).iter().enumerate() {
            let actual = runner.step(step);
            mismatches.check(
                &actual,
                &scenario["results"][index],
                format!("{name}: шаг {index} {step}"),
            );
        }
        scenario::open_up(&runner.root);
    }
    mismatches.finish(what);
}

#[test]
fn limit_is_that_of_0_4_7() {
    assert_eq!(
        cfg_files::MAX_FILE_BYTES,
        FIXTURE["constants"]["MAX_FILE_BYTES"].as_u64().unwrap()
    );
}

#[test]
fn list_orders_names_like_locale_compare_and_stops_at_the_limits() {
    run_scenarios(
        |scenario| str_of(&scenario["name"]).starts_with("список"),
        "cfg-files: список",
    );
}

#[test]
fn read_and_write_keep_encoding_eol_and_check_the_mtime() {
    run_scenarios(
        |scenario| !str_of(&scenario["name"]).starts_with("список"),
        "cfg-files: чтение и запись",
    );
}

#[test]
fn meta_of_the_renderer_maps_to_write_options_like_index_js() {
    use hlsr_delivery::game_binds::Encoding;
    let options = |meta: Value| WriteOptions::from_meta(&meta);
    let defaults = options(json!({}));
    assert_eq!(defaults.encoding, Encoding::Utf8);
    assert!(!defaults.crlf);
    assert_eq!(defaults.expected_mtime, None);
    for meta in [
        json!(null),
        json!("text"),
        json!([1]),
        json!(5),
        json!({"encoding": 5}),
    ] {
        let parsed = options(meta.clone());
        assert_eq!(parsed.encoding, Encoding::Utf8, "{meta}");
        assert!(!parsed.crlf, "{meta}");
        assert_eq!(parsed.expected_mtime, None, "{meta}");
    }
    let full = options(json!({"encoding": "latin1", "eol": "crlf", "mtime": "1700000000000"}));
    assert_eq!(full.encoding, Encoding::Latin1);
    assert!(full.crlf);
    assert_eq!(full.expected_mtime, Some(1_700_000_000_000.0));
    // Строгое равенство: регистр и пробелы важны.
    let loose = options(json!({"encoding": "LATIN1", "eol": "CRLF"}));
    assert_eq!(loose.encoding, Encoding::Utf8);
    assert!(!loose.crlf);
    // Number(значение): булево, массив, пустая строка, шестнадцатеричная запись; объект — NaN.
    let mtime = |value: Value| options(json!({"mtime": value})).expected_mtime;
    assert_eq!(mtime(json!(true)), Some(1.0));
    assert_eq!(mtime(json!([5])), Some(5.0));
    assert_eq!(mtime(json!("")), Some(0.0));
    assert_eq!(mtime(json!("0x10")), Some(16.0));
    assert!(mtime(json!({})).unwrap().is_nan());
    assert!(mtime(json!("abc")).unwrap().is_nan());
    assert_eq!(mtime(json!(null)), None);
    // String(content == null ? "" : content).
    for (content, text) in [
        (json!(null), ""),
        (json!(5), "5"),
        (json!(1e21), "1e+21"),
        (json!([1, null, "a"]), "1,,a"),
        (json!({"a": 1}), "[object Object]"),
        (json!(true), "true"),
        (json!("строка"), "строка"),
    ] {
        assert_eq!(cfg_files::content_text(&content), text, "{content}");
    }
}
