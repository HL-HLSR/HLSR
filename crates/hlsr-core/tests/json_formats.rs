//! Форматы JSON-файлов 0.4.7 (Compact, Pretty2, JSONL) против Node 24: фикстура
//! `node-semantics-json-formats.json` из `tools/reference/node-semantics-json-formats.js`.
//! Тексты — на корпусе значений; сценарии — те же последовательности вызовов fs, что в модулях
//! 0.4.7, на деревьях файлов: итог шага, код и текст ошибки, содержимое и права файлов.
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use hlsr_core::json_formats::{
    TmpOptions, WriteError, append_jsonl_line, compact, jsonl_line, pretty2, rewrite_jsonl,
    write_compact_tmp, write_pretty2,
};
use hlsr_core::json_text::parse_js_json;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

fn fixture() -> Value {
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/node-semantics-json-formats.json"
    );
    parse_js_json(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn str_of(value: &Value) -> &str {
    value.as_str().expect("строка")
}

#[test]
fn texts_match_node() {
    let fixture = fixture();
    let texts = fixture["texts"].as_array().unwrap();
    assert!(texts.len() >= 20, "корпус слишком мал: {}", texts.len());
    for case in texts {
        let text = str_of(&case["text"]);
        let value = parse_js_json(text).unwrap();
        assert_eq!(compact(&value), str_of(&case["compact"]), "compact {text}");
        assert_eq!(pretty2(&value), str_of(&case["pretty2"]), "pretty2 {text}");
        assert_eq!(
            jsonl_line(&value),
            str_of(&case["jsonlLine"]),
            "jsonl {text}"
        );
    }
}

fn path_in(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part))
}

fn setup(root: &Path, items: &Value) {
    for item in items.as_array().into_iter().flatten() {
        if let Some(dir) = item["dir"].as_str() {
            std::fs::create_dir_all(path_in(root, dir)).unwrap();
        } else {
            let file = path_in(root, str_of(&item["file"]));
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, str_of(&item["utf8"])).unwrap();
        }
    }
}

/// Текст ошибки: корень — `<root>`, pid во временном файле — `<pid>`, прямые косые.
fn normalize(root: &Path, text: &str) -> String {
    let text = text
        .replace(root.to_str().unwrap(), "<root>")
        .replace('\\', "/");
    let pid = format!(".{}.tmp", std::process::id());
    text.replace(&pid, ".<pid>.tmp")
}

fn outcome(root: &Path, result: Result<(), WriteError>) -> Value {
    match result {
        Ok(()) => json!({"ok": null}),
        Err(error) => json!({"error": {
            "code": error.code().map(|code| code.into_owned()),
            "message": normalize(root, &error.message()),
        }}),
    }
}

fn step(root: &Path, step: &Value) -> Value {
    let file = path_in(root, str_of(&step["file"]));
    let result = match str_of(&step["op"]) {
        "pretty2" => write_pretty2(&file, &step["value"]),
        "compactTmp" => write_compact_tmp(
            &file,
            &step["value"],
            TmpOptions {
                private: step["private"].as_bool().unwrap_or(false),
                clear_stale: step["clearStale"].as_bool().unwrap_or(false),
            },
        ),
        "appendJsonl" => append_jsonl_line(&file, &compact(&step["value"])),
        "rewriteJsonl" => {
            let lines: Vec<String> = step["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|line| str_of(line).to_owned())
                .collect();
            rewrite_jsonl(&file, &lines)
        }
        other => panic!("неизвестная операция {other}"),
    };
    outcome(root, result)
}

/// Файлы дерева: содержимое latin1 и права (0600 — число, как у нового файла — `"default"`).
#[cfg(unix)]
fn files(root: &Path) -> Value {
    use std::os::unix::fs::PermissionsExt;
    let baseline = root.join(".mode-baseline");
    std::fs::write(&baseline, "").unwrap();
    let default_mode = std::fs::metadata(&baseline).unwrap().permissions().mode() & 0o777;
    std::fs::remove_file(&baseline).unwrap();
    let mut out = Vec::new();
    fn walk(dir: &Path, prefix: &str, default_mode: u32, out: &mut Vec<(String, Value)>) {
        for item in std::fs::read_dir(dir).unwrap() {
            let item = item.unwrap();
            let name = item.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if item.file_type().unwrap().is_dir() {
                out.push((rel.clone(), json!("dir")));
                walk(&item.path(), &rel, default_mode, out);
            } else {
                let mode = std::fs::metadata(item.path()).unwrap().permissions().mode() & 0o777;
                let bytes = std::fs::read(item.path()).unwrap();
                out.push((
                    rel,
                    json!({
                        "latin1": hlsr_core::js::latin1_string(&bytes),
                        "mode": if mode == default_mode { json!("default") } else { json!(mode) },
                    }),
                ));
            }
        }
    }
    walk(root, "", default_mode, &mut out);
    out.sort_by(|a, b| hlsr_core::js::cmp_utf16(&a.0, &b.0));
    Value::Object(out.into_iter().collect::<Map<_, _>>())
}

/// Тексты ошибок и права сняты в Linux; в Windows (Wine) сверяются только тексты форматов.
#[cfg(unix)]
#[test]
fn scenarios_match_node() {
    let fixture = fixture();
    let scenarios = fixture["scenarios"].as_array().unwrap();
    assert!(scenarios.len() >= 15, "сценариев мало: {}", scenarios.len());
    let mut mismatches = Vec::new();
    for scenario in scenarios {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        setup(&root, &scenario["setup"]);
        let results: Vec<Value> = scenario["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| step(&root, s))
            .collect();
        let actual = json!({"results": results, "files": files(&root)});
        let expected = json!({"results": scenario["results"], "files": scenario["files"]});
        if actual != expected {
            mismatches.push(format!(
                "{}\n- {expected}\n+ {actual}",
                str_of(&scenario["name"])
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "расхождения с 0.4.7:\n\n{}",
        mismatches.join("\n\n")
    );
}
