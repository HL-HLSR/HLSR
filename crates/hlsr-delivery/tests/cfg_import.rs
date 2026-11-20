//! Импорт cfg против 0.4.7: фикстура `cfg-import.json` из
//! `tools/reference/delivery/cfg-import.js` (`shared/cfg-import.js` и
//! `shared/goldsrc-import-adopt.js` в Node 24 на тех же входах).

mod support;

use hlsr_core::js::object::JsObject;
use hlsr_delivery::cfg_import::{
    self, Built, ImportFile, ImportFiles, Picked, Root, Wanted, companions_of, exec_target,
    execs_of,
};
use hlsr_delivery::goldsrc_import_adopt;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("cfg-import.json"));

// ------------------------------------------------- JSON ⇄ типы Rust

fn import_files(value: &Value) -> ImportFiles {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, file)| {
                    (
                        key.clone(),
                        ImportFile {
                            name: opt_str(&file["name"]).unwrap_or_default().to_owned(),
                            text: str_of(&file["text"]).to_owned(),
                            target: opt_str(&file["target"]).map(str::to_owned),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn root_of(value: &Value) -> Root {
    Root {
        name: str_of(&value["name"]).to_owned(),
        text: str_of(&value["text"]).to_owned(),
        virtual_root: value["virtual"].as_bool().unwrap_or(false),
    }
}

fn strings(value: &Value) -> Vec<String> {
    items(value).iter().map(|s| str_of(s).to_owned()).collect()
}

fn picked_of(value: &Value) -> Vec<Picked> {
    items(value)
        .iter()
        .map(|file| Picked {
            name: str_of(&file["name"]).to_owned(),
            text: str_of(&file["text"]).to_owned(),
        })
        .collect()
}

fn files_entries(files: &ImportFiles) -> Value {
    Value::Array(
        files
            .iter()
            .map(|(key, file)| {
                json!([key, {"name": file.name, "target": file.target, "text": file.text}])
            })
            .collect(),
    )
}

fn target_value(target: Option<cfg_import::ExecTarget>) -> Value {
    match target {
        Some(t) => json!({"key": t.key, "name": t.name, "system": t.system, "auto": t.auto}),
        None => Value::Null,
    }
}

fn wanted_value(wanted: Option<Wanted>) -> Value {
    match wanted {
        Some(w) => json!({"key": w.key, "name": w.name, "from": w.from}),
        None => Value::Null,
    }
}

fn pairs(list: &[(String, String)]) -> Value {
    Value::Array(list.iter().map(|(a, b)| json!([a, b])).collect())
}

fn built_value(built: &Built, hashed_flat: bool) -> Value {
    let outputs: Vec<Value> = built
        .outputs
        .iter()
        .map(|item| {
            if item.root {
                json!({"file": item.file, "text": item.text, "root": true})
            } else {
                json!({"file": item.file, "text": item.text})
            }
        })
        .collect();
    let duplicates: Vec<Value> = built
        .report
        .duplicates
        .iter()
        .map(|d| json!({"kind": d.kind, "name": d.name, "files": d.files, "value": d.value}))
        .collect();
    let flat = if hashed_flat {
        let units = built.flat_text.encode_utf16().count();
        let sha: String = Sha256::digest(built.flat_text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        json!({"length": units, "sha256": sha})
    } else {
        json!(built.flat_text)
    };
    json!({
        "outputs": outputs,
        "execs": built.execs,
        "flatText": flat,
        "bytes": built.bytes,
        "report": {
            "files": built.report.files,
            "missing": built.report.missing,
            "cycles": pairs(&built.report.cycles),
            "duplicateFiles": pairs(&built.report.duplicate_files),
            "duplicates": duplicates,
        },
    })
}

// ---------------------------------------------------------------- проверки

#[test]
fn constants_are_those_of_0_4_7() {
    assert_eq!(
        cfg_import::MAX_FILES as u64,
        FIXTURE["constants"]["MAX_FILES"].as_u64().unwrap()
    );
}

#[test]
fn exec_target_on_corpus() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["execTarget"]) {
        let (arg, engine) = (str_of(&case[0]), str_of(&case[1]));
        mismatches.check(
            &target_value(exec_target(arg, engine)),
            &case[2],
            format!("execTarget({arg:?}, {engine:?})"),
        );
    }
    mismatches.finish("cfg-import.js: execTarget");
}

#[test]
fn execs_of_on_corpus() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["execsOf"]) {
        let (text, engine) = (str_of(&case[0]), str_of(&case[1]));
        let actual: Vec<Value> = execs_of(text, engine)
            .into_iter()
            .map(|exec| {
                json!({
                    "runtime": exec.runtime,
                    "key": exec.target.key,
                    "name": exec.target.name,
                    "system": exec.target.system,
                    "auto": exec.target.auto,
                })
            })
            .collect();
        mismatches.check(
            &Value::Array(actual),
            &case[2],
            format!("execsOf({text:?}, {engine:?})"),
        );
    }
    mismatches.finish("cfg-import.js: execsOf");
}

#[test]
fn safe_names_and_engine_run_on_corpus() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["isSafeImportName"]) {
        mismatches.check_eq(
            cfg_import::is_safe_import_name(str_of(&case[0])),
            case[1].as_bool().unwrap(),
            format!("isSafeImportName({})", case[0]),
        );
    }
    for case in items(&FIXTURE["engineRun"]) {
        mismatches.check_eq(
            cfg_import::is_engine_run(str_of(&case[0])),
            case[1].as_bool().unwrap(),
            format!("ENGINE_RUN.test({})", case[0]),
        );
    }
    mismatches.finish("cfg-import.js: имена");
}

#[test]
fn import_names_come_out_of_root_of_picked_like_in_0_4_7() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["rootOfPicked"])
        .iter()
        .chain(items(&FIXTURE["singlePicked"]))
    {
        let picked = picked_of(&case["picked"]);
        let result = cfg_import::root_of_picked(&picked);
        let mut root = json!({"name": result.root.name, "text": result.root.text});
        if result.root.virtual_root {
            root["virtual"] = json!(true);
        }
        mismatches.check(&root, &case["root"], format!("root {}", case["picked"]));
        let expected: Vec<Value> = items(&case["files"])
            .iter()
            .map(|entry| {
                let file = &entry[1];
                // В фикстуре у выбранного одного файла нет target, а JSON.stringify
                // выбрасывает undefined: нормализуем отсутствие к null.
                json!([entry[0], {
                    "name": file["name"],
                    "target": file.get("target").cloned().unwrap_or(Value::Null),
                    "text": file["text"],
                }])
            })
            .collect();
        mismatches.check(
            &files_entries(&result.files),
            &Value::Array(expected),
            format!("files {}", case["picked"]),
        );
    }
    mismatches.finish("cfg-import.js: rootOfPicked");
}

#[test]
fn companions_on_corpus() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["companionsOf"]) {
        let names = strings(&case[0]);
        let actual: Vec<Value> = companions_of(&names)
            .into_iter()
            .map(|c| json!({"name": c.name, "from": c.from, "before": c.before}))
            .collect();
        mismatches.check(
            &Value::Array(actual),
            &case[1],
            format!("companionsOf({})", case[0]),
        );
    }
    mismatches.finish("cfg-import.js: companionsOf");
}

#[test]
fn builds_on_hand_written_scenarios() {
    let mut mismatches = Mismatches::default();
    for scenario in items(&FIXTURE["builds"]) {
        let built = cfg_import::build_import(
            str_of(&scenario["engine"]),
            &root_of(&scenario["root"]),
            &import_files(&scenario["files"]),
            &strings(&scenario["missing"]),
        );
        let hashed = scenario["built"]["flatText"].is_object();
        mismatches.check(
            &built_value(&built, hashed),
            &scenario["built"],
            str_of(&scenario["name"]),
        );
    }
    mismatches.finish("cfg-import.js: buildImport");
}

/// Что делает окно импорта: спрашивает файл, который просит `next_wanted`, и добавляет
/// его из «диска игрока» (`world`) или отмечает отказом.
#[test]
fn chains_as_the_import_window_walks_them() {
    let mut mismatches = Mismatches::default();
    for scenario in items(&FIXTURE["chains"]) {
        let engine = str_of(&scenario["engine"]);
        let top_level_only = scenario["topLevelOnly"].as_bool().unwrap();
        let (root, mut files) = match scenario.get("picked") {
            Some(picked) => {
                let result = cfg_import::root_of_picked(&picked_of(picked));
                (result.root, result.files)
            }
            None => (root_of(&scenario["root"]), JsObject::new()),
        };
        let mut missing: Vec<String> = Vec::new();
        let mut steps: Vec<Value> = Vec::new();
        for _ in 0..40 {
            let want = cfg_import::next_wanted(engine, &root, &files, &missing, top_level_only);
            steps.push(wanted_value(want.as_ref().map(|w| Wanted {
                key: w.key.clone(),
                name: w.name.clone(),
                from: w.from.clone(),
            })));
            let Some(want) = want else { break };
            match scenario["world"].get(&want.key) {
                Some(found) => {
                    files.insert(
                        want.key.clone(),
                        ImportFile {
                            name: str_of(&found["name"]).to_owned(),
                            text: str_of(&found["text"]).to_owned(),
                            target: None,
                        },
                    );
                }
                None => missing.push(want.key),
            }
        }
        let name = str_of(&scenario["name"]);
        mismatches.check(
            &Value::Array(steps),
            &scenario["steps"],
            format!("{name}: что спрашивает окно"),
        );
        let built = cfg_import::build_import(engine, &root, &files, &missing);
        mismatches.check(
            &built_value(&built, false),
            &scenario["built"],
            format!("{name}: buildImport"),
        );
    }
    mismatches.finish("cfg-import.js: цепочки");
}

fn object_of(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

#[test]
fn adopting_imported_values_on_corpus() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["adopt"]) {
        let text = str_of(&case["text"]);
        let options = &case["options"];
        let result = goldsrc_import_adopt::adopt_imported(
            text,
            &object_of(&options["personalization"]),
            &object_of(&options["bxt"]),
        );
        mismatches.check(
            &result.to_value(),
            &case["result"],
            format!("adoptImported({text:?}, {options})"),
        );
    }
    mismatches.finish("goldsrc-import-adopt.js: adoptImported");
}

#[test]
fn cvars_of_keep_the_order_of_object_keys() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["cvarsOf"]) {
        let text = str_of(&case[0]);
        let actual: Vec<Value> = goldsrc_import_adopt::cvars_of(text)
            .iter()
            .map(|(key, value)| json!([key, value]))
            .collect();
        mismatches.check(
            &Value::Array(actual),
            &case[1],
            format!("cvarsOf({text:?})"),
        );
    }
    mismatches.finish("goldsrc-import-adopt.js: cvarsOf");
}
