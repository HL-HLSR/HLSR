//! Чистые модули конфига 0.4.9 (задача A′) против эталона: фикстуры `cfg-import-flatten.json`,
//! `hl1-config-check.json` и `runtime-binds.json` из `tools/reference/delivery/<имя>.js`
//! (модули release-0.4.9 в Node на тех же входах). JSON сравнивается строкой — с порядком ключей.

mod support;

use hlsr_core::js::object::JsObject;
use hlsr_delivery::cfg_import::{self, ImportFile, ImportFiles, Root};
use hlsr_delivery::game_binds::{self, runtime_to_value};
use hlsr_delivery::hl1_config_check::{self, Category};
use hlsr_delivery::hl1_config_groups::{self, RULE_KEYS};
use serde_json::{Value, json};
use std::collections::HashMap;
use support::{Mismatches, fixture, items, opt_str, sha256_hex, str_of};

/// Сравнение байтов JSON: одинаковые значения с другим порядком ключей — тоже расхождение.
fn check_bytes(mismatches: &mut Mismatches, actual: &Value, expected: &Value, context: String) {
    let (a, e) = (actual.to_string(), expected.to_string());
    if a == e {
        return;
    }
    if actual == expected {
        mismatches.check_eq(a, e, format!("{context}: порядок ключей"));
    } else {
        mismatches.check(actual, expected, context);
    }
}

fn strings(value: &Value) -> Vec<String> {
    items(value).iter().map(|s| str_of(s).to_owned()).collect()
}

// ---------------------------------------------------------------- flattenImport

fn import_files(value: &Value) -> ImportFiles {
    value
        .as_object()
        .expect("files — объект")
        .iter()
        .map(|(key, file)| {
            let file = ImportFile {
                name: opt_str(&file["name"]).unwrap_or_default().to_owned(),
                text: str_of(&file["text"]).to_owned(),
                target: opt_str(&file["target"]).map(str::to_owned),
            };
            (key.clone(), file)
        })
        .collect()
}

/// Длинный текст фикстура хранит длиной и sha256 (как `short` в скрипте).
fn short(text: &str, big: usize) -> Value {
    let length = text.encode_utf16().count();
    if length > big {
        json!({"length": length, "sha256": sha256_hex(text.as_bytes())})
    } else {
        json!(text)
    }
}

#[test]
fn flatten_import_matches_0_4_9() {
    let fixture = fixture("cfg-import-flatten.json");
    let big = fixture["big"].as_u64().unwrap() as usize;
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["cases"]) {
        let root = Root {
            name: str_of(&case["root"]["name"]).to_owned(),
            text: str_of(&case["root"]["text"]).to_owned(),
            virtual_root: case["root"]["virtual"].as_bool().unwrap_or(false),
        };
        let flat = cfg_import::flatten_import(
            str_of(&case["engine"]),
            &root,
            &import_files(&case["files"]),
            &strings(&case["missing"]),
        );
        let mut actual = flat.to_value();
        actual["text"] = short(&flat.text, big);
        for (item, file) in flat
            .runtime_files
            .iter()
            .zip(actual["runtimeFiles"].as_array_mut().unwrap().iter_mut())
        {
            file["text"] = short(&item.text, big);
        }
        check_bytes(
            &mut mismatches,
            &actual,
            &case["flat"],
            format!("flattenImport «{}»", str_of(&case["name"])),
        );
    }
    mismatches.finish("flattenImport");
}

// ---------------------------------------------------------------- checkConfig, groupFindings

fn groups_value(findings: &[Value]) -> Value {
    Value::Array(
        hl1_config_groups::group_findings(findings)
            .iter()
            .map(|group| json!({"group": group.to_value(), "detail": hl1_config_groups::rule_detail(group)}))
            .collect(),
    )
}

#[test]
fn hl1_config_check_matches_0_4_9() {
    let fixture = fixture("hl1-config-check.json");
    let mut mismatches = Mismatches::default();
    mismatches.check_eq(
        sha256_hex(hl1_config_check::WHITELIST_JSON.as_bytes()),
        str_of(&fixture["whitelistSha256"]).to_owned(),
        "таблица разрешённых команд — байты 0.4.9",
    );
    let keys: Value = RULE_KEYS
        .iter()
        .map(|(rule, key)| ((*rule).to_owned(), json!(key)))
        .collect::<serde_json::Map<_, _>>()
        .into();
    check_bytes(
        &mut mismatches,
        &keys,
        &fixture["ruleKeys"],
        "RULE_KEYS".into(),
    );

    for case in items(&fixture["cases"]) {
        let files: JsObject<String> = case["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, text)| (name.clone(), str_of(text).to_owned()))
            .collect();
        let category = Category::parse(opt_str(&case["category"]).unwrap_or_default());
        let result = hl1_config_check::check_config(&files, str_of(&case["root"]), category);
        let name = str_of(&case["name"]);
        let actual = result.to_value();
        check_bytes(
            &mut mismatches,
            &actual,
            &case["result"],
            format!("checkConfig «{name}»"),
        );
        let findings = items(&actual["findings"]);
        check_bytes(
            &mut mismatches,
            &groups_value(findings),
            &case["groups"],
            format!("groupFindings «{name}»"),
        );
    }

    let groups = &fixture["groups"];
    check_bytes(
        &mut mismatches,
        &groups_value(items(&groups["findings"])),
        &groups["groups"],
        "groupFindings с путями".into(),
    );
    mismatches.finish("hl1-config-check");
}

// ---------------------------------------------------------------- runtimeBinds

fn read_of(case: &Value) -> game_binds::BindRead {
    let files: HashMap<String, String> = case["files"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, text)| (name.clone(), str_of(text).to_owned()))
        .collect();
    let read_file = |name: &str| files.get(name).cloned();
    if case["roots"].is_null() {
        game_binds::read_binds(read_file, game_binds::GOLDSRC.roots)
    } else {
        let roots = strings(&case["roots"]);
        let roots: Vec<&str> = roots.iter().map(String::as_str).collect();
        game_binds::read_binds(read_file, &roots)
    }
}

#[test]
fn runtime_binds_match_0_4_9() {
    let fixture = fixture("runtime-binds.json");
    let mut mismatches = Mismatches::default();
    let mut reads: HashMap<String, game_binds::BindRead> = HashMap::new();
    for case in items(&fixture["readBinds"]) {
        let read = read_of(case);
        let name = str_of(&case["name"]);
        check_bytes(
            &mut mismatches,
            &runtime_to_value(&read.runtime),
            &case["runtime"],
            format!("readBinds(…).runtime «{name}»"),
        );
        reads.insert(name.to_owned(), read);
    }
    for case in items(&fixture["withImport"]) {
        let name = str_of(&case["name"]);
        let mut read = reads[name].clone();
        if !case["imported"].is_null() {
            read.imported = strings(&case["imported"]);
        }
        let result = game_binds::with_import(Some(&read), str_of(&case["text"]));
        check_bytes(
            &mut mismatches,
            &runtime_to_value(&result.runtime),
            &case["runtime"],
            format!(
                "withImport(…).runtime «{name}», {:?}",
                str_of(&case["text"])
            ),
        );
    }
    mismatches.finish("runtimeBinds");
}

// ---------------------------------------------------------------- пределы (у 0.4.9 их нет)

#[test]
fn hl1_config_check_survives_deep_alias_line() {
    // Без предела глубины — рекурсия на каждые два слова и abort по стеку.
    let files: JsObject<String> = [("a.cfg".to_owned(), "alias x ".repeat(20_000))]
        .into_iter()
        .collect();
    let result = hl1_config_check::check_config(&files, "a.cfg", Category::parse("scriptless"));
    assert_eq!(result.stats.files, 1);
}

#[test]
fn flatten_import_stops_exec_diamond() {
    // f0 … f29: каждый зовёт следующий дважды — без бюджета 2^30 копий текста.
    let files: ImportFiles = (0..30)
        .map(|i| {
            let text = format!("sensitivity 1\nexec f{0}.cfg\nexec f{0}.cfg\n", i + 1);
            let file = ImportFile {
                name: format!("f{i}.cfg"),
                text,
                target: None,
            };
            (format!("f{i}.cfg"), file)
        })
        .collect();
    let root = Root {
        name: "main.cfg".to_owned(),
        text: "exec f0.cfg\n".to_owned(),
        virtual_root: false,
    };
    let flat = cfg_import::flatten_import("goldsrc", &root, &files, &[]);
    assert!(flat.bytes > game_binds::MAX_IMPORT_BYTES, "{}", flat.bytes);
}

#[test]
fn flatten_import_budget_is_shared_by_runtime_files() {
    // f0 … f19 — ромб, r0 … r11 зовутся из bind и каждый раскрывает f0: бюджет один на всех.
    let file = |name: String, text: String| {
        let key = name.clone();
        let target = None;
        (key, ImportFile { name, text, target })
    };
    let files: ImportFiles = (0..20)
        .map(|i| {
            let text = format!("sensitivity 1\nexec f{0}.cfg\nexec f{0}.cfg\n", i + 1);
            file(format!("f{i}.cfg"), text)
        })
        .chain((0..12).map(|i| file(format!("r{i}.cfg"), "exec f0.cfg\n".to_owned())))
        .collect();
    let root = Root {
        name: "main.cfg".to_owned(),
        text: (0..12)
            .map(|i| format!("bind k{i} \"exec r{i}.cfg\"\n"))
            .collect(),
        virtual_root: false,
    };
    let flat = cfg_import::flatten_import("goldsrc", &root, &files, &[]);
    assert_eq!(flat.runtime_files.len(), 12);
    assert!(flat.bytes > game_binds::MAX_IMPORT_BYTES, "{}", flat.bytes);
    assert!(
        flat.bytes < 32 * game_binds::MAX_IMPORT_BYTES,
        "{}",
        flat.bytes
    );
}
