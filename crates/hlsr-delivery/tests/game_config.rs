//! Редактор конфигов во вкладке «Конфиги» против 0.4.7: фикстура `game-config.json` из
//! `tools/reference/delivery/game-config.js`.
//!
//! Ч — `shared/game-config.js`: каталог настроек, `fromCfg`, `toCfg`, `defaultValue`, `lineCommands`,
//! `execute`, `importValues`, `replaceAt`, `appendLine` на корпусе строк и значений.
//! С — `game-config-install.js`: `readConfig` и `writeConfig` на деревьях файлов во временных папках;
//! сравниваются ответ каждого шага и дерево (путь, размер, sha256, текст latin1).
//!
//! Сценарии, помеченные `unix`, зависят от ОС (ссылки, права доступа, регистр имён на диске,
//! ошибки rename): на Windows они пропускаются.

mod support;

use hlsr_core::js::number::number_to_string;
use hlsr_core::json_text::js_number;
use hlsr_delivery::cfg_search::{CfgSearch, SteamLanguage};
use hlsr_delivery::errors::Failure;
use hlsr_delivery::game_config::{self, FieldDefault, FieldType, Label, WindowValue};
use hlsr_delivery::game_config_install::{self, WriteConfigError};
use hlsr_delivery::ports::FakeRegistry;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use support::scenario::{self, common_step, normalize_text, path_in, temp_root};
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("game-config.json"));

// ------------------------------------------------------------- данные в JSON

fn label_json(label: &Label) -> Value {
    json!({"ru": label.ru, "en": label.en})
}

fn num(x: f64) -> Value {
    js_number(x)
}

fn field_json(field: &game_config::Field) -> Value {
    let mut out = json!({
        "cvar": field.cvar,
        "section": field.section,
        "type": match field.kind {
            FieldType::Range => "range",
            FieldType::Percent => "percent",
            FieldType::Toggle => "toggle",
            FieldType::Invert => "invert",
            FieldType::Select => "select",
        },
        "default": match field.default {
            FieldDefault::Bool(b) => json!(b),
            FieldDefault::Num(x) => num(x),
        },
        "engines": field.engines,
        "archived": field.archived,
        "label": label_json(&field.label),
    });
    let map = out.as_object_mut().unwrap();
    if let Some(min) = field.min {
        map.insert("min".into(), num(min));
    }
    if let Some(max) = field.max {
        map.insert("max".into(), num(max));
    }
    if let Some(step) = field.step {
        map.insert("step".into(), num(step));
    }
    if !field.skip.is_empty() {
        map.insert("skip".into(), json!(field.skip));
    }
    if let Some(hint) = &field.hint {
        map.insert("hint".into(), label_json(hint));
    }
    if let Some(scale) = &field.scale {
        map.insert(
            "scale".into(),
            json!([label_json(&scale[0]), label_json(&scale[1])]),
        );
    }
    if !field.options.is_empty() {
        let options: Vec<Value> = field
            .options
            .iter()
            .map(|o| json!({"value": num(o.value), "label": label_json(&o.label)}))
            .collect();
        map.insert("options".into(), Value::Array(options));
    }
    if let Some(requires) = field.requires {
        map.insert("requires".into(), json!(requires));
    }
    out
}

/// `valueText` скрипта: `undefined`, `bool:true` или `num:<String(число)>`.
fn value_text(value: Option<WindowValue>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(WindowValue::Bool(b)) => format!("bool:{b}"),
        Some(WindowValue::Num(x)) => format!("num:{}", number_to_string(x)),
    }
}

fn index_of(field: &game_config::Field) -> usize {
    game_config::FIELDS
        .iter()
        .position(|f| std::ptr::eq(f, field))
        .expect("поле из FIELDS")
}

// ------------------------------------------------------------------ Ч

#[test]
fn constants_and_catalog_data_are_those_of_0_4_7_with_0_4_9_delta() {
    let constants = &FIXTURE["constants"];
    assert_eq!(game_config::CONFIG_FILE, str_of(&constants["CONFIG_FILE"]));
    // 0.4.9: CONFIG_EXEC_LINE убран, файлы лаунчера правятся на месте — не правится только WON.cfg.
    assert_eq!(json!(game_config::GENERATED_FILES), json!(["won.cfg"]));
    for engine in ["goldsrc", "source", "source-old"] {
        assert_eq!(
            json!(game_config::engine_roots(engine)),
            constants["ENGINE_ROOTS"][engine],
            "ENGINE_ROOTS.{engine}"
        );
        assert_eq!(
            json!(game_config::exec_file(engine)),
            constants["EXEC_FILES"][engine],
            "EXEC_FILES.{engine}"
        );
    }
    assert_eq!(game_config::engine_roots("bogus"), None);
    assert_eq!(game_config::exec_file("bogus"), None);
    let sections: Vec<Value> = game_config::SECTIONS
        .iter()
        .map(|s| json!({"id": s.id, "icon": s.icon, "title": label_json(&s.title)}))
        .collect();
    assert_eq!(Value::Array(sections), constants["SECTIONS"]);

    let fields: Vec<Value> = game_config::FIELDS.iter().map(field_json).collect();
    let expected = items(&FIXTURE["fields"]);
    assert_eq!(fields.len(), expected.len());
    for (index, (got, want)) in fields.iter().zip(expected).enumerate() {
        let mut want = want.clone();
        // 0.4.9: fps_max у Source не выше 999.
        if want["cvar"] == "fps_max" && want["max"] == 1000 {
            want["max"] = json!(999);
        }
        support::assert_json_eq(got, &want, &format!("FIELDS[{index}] {}", want["cvar"]));
    }
}

#[test]
fn catalog_for_filters_by_engine_and_game() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["catalogs"]) {
        let game_id = str_of(&case["gameId"]);
        let engine = str_of(&case["engine"]);
        let actual = match game_config::catalog_for(game_id, engine) {
            None => Value::Null,
            Some(catalog) => {
                let by_cvar: Vec<Value> = {
                    // Порядок Map JS — порядок полей каталога.
                    catalog
                        .fields
                        .iter()
                        .map(|f| json!([f.cvar.to_lowercase(), index_of(f)]))
                        .collect()
                };
                json!({
                    "engine": catalog.engine,
                    "sections": catalog.sections.iter().map(|section| json!({
                        "id": section.section.id,
                        "icon": section.section.icon,
                        "title": label_json(&section.section.title),
                        "fields": section.fields.iter().map(|f| index_of(f)).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                    "fields": catalog.fields.iter().map(|f| index_of(f)).collect::<Vec<_>>(),
                    "byCvar": by_cvar,
                })
            }
        };
        mismatches.check(
            &actual,
            &case["catalog"],
            format!("{game_id:?} / {engine:?}"),
        );
        if let Some(catalog) = game_config::catalog_for(game_id, engine) {
            for field in &catalog.fields {
                let found = catalog.by_cvar(&field.cvar.to_lowercase());
                mismatches.check_eq(found.map(index_of), Some(index_of(field)), field.cvar);
            }
            mismatches.check_eq(catalog.by_cvar("nothing").is_none(), true, "by_cvar");
        }
    }
    mismatches.finish("game-config: catalogFor");
}

#[test]
fn default_value_from_cfg_and_to_cfg_on_the_corpus() {
    let values = &FIXTURE["values"];
    let mut mismatches = Mismatches::default();

    for (index, field) in game_config::FIELDS.iter().enumerate() {
        mismatches.check_eq(
            value_text(Some(game_config::default_value(field))),
            str_of(&values["defaults"][index]).to_owned(),
            format!("defaultValue {}", field.cvar),
        );
    }

    let raws = items(&values["fromCfg"]["raws"]);
    for (index, field) in game_config::FIELDS.iter().enumerate() {
        for (n, raw) in raws.iter().enumerate() {
            mismatches.check_eq(
                value_text(game_config::from_cfg(field, opt_str(raw))),
                str_of(&values["fromCfg"]["results"][index][n]).to_owned(),
                format!("fromCfg {} {raw}", field.cvar),
            );
        }
    }

    let to_values = items(&values["toCfg"]["values"]);
    for (index, field) in game_config::FIELDS.iter().enumerate() {
        for (n, value) in to_values.iter().enumerate() {
            let got = game_config::to_cfg(field, value, None);
            let mut want = opt_str(&values["toCfg"]["results"][index][n]);
            // 0.4.9: fps_max у Source не выше 999.
            if field.max == Some(999.0) && want == Some("1000") {
                want = None;
            }
            mismatches.check_eq(
                got.as_deref(),
                want,
                format!("toCfg {} {value}", field.cvar),
            );
        }
    }

    let m_pitch = game_config::FIELDS
        .iter()
        .find(|f| f.cvar == "m_pitch")
        .unwrap();
    let previous = items(&values["toCfgPrevious"]["previous"]);
    let probes = items(&values["toCfgPrevious"]["values"]);
    for (p, previous) in previous.iter().enumerate() {
        for (n, value) in probes.iter().enumerate() {
            mismatches.check_eq(
                game_config::to_cfg(m_pitch, value, opt_str(previous)).as_deref(),
                opt_str(&values["toCfgPrevious"]["results"][p][n]),
                format!("toCfg m_pitch {value} previous {previous}"),
            );
        }
    }
    mismatches.finish("game-config: значения");
}

/// Смещение в строке JS — единицы UTF-16; у Rust — байты.
fn utf16_at(line: &str, byte: usize) -> usize {
    line[..byte].encode_utf16().count()
}

#[test]
fn line_commands_split_like_the_engine() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["lineCommands"]) {
        let line = str_of(&case[0]);
        let actual: Vec<Value> = game_config::line_commands(line)
            .iter()
            .map(|command| {
                json!({
                    "start": utf16_at(line, command.start),
                    "end": utf16_at(line, command.end),
                    "args": command.args.iter().map(|a| json!({"value": a.value, "quoted": a.quoted})).collect::<Vec<_>>(),
                })
            })
            .collect();
        mismatches.check(&Value::Array(actual), &case[1], format!("{line:?}"));
    }
    mismatches.finish("game-config: lineCommands");
}

#[test]
fn execute_runs_the_chain_like_the_engine() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["execute"]) {
        let name = str_of(&case["name"]);
        let files: HashMap<String, String> = case["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.to_lowercase(), str_of(v).to_owned()))
            .collect();
        let roots = scenario::strings(&case["roots"]);
        let roots: Vec<&str> = roots.iter().map(String::as_str).collect();
        let cvars = scenario::strings(&case["cvars"]);
        let cvars: Vec<&str> = cvars.iter().map(String::as_str).collect();
        let executed = game_config::execute(
            &mut |name: &str| files.get(&name.to_lowercase()).cloned(),
            &roots,
            &cvars,
        );
        let mut found: Vec<(&String, &game_config::Hit)> = executed.found.iter().collect();
        found.sort_by(|a, b| a.0.cmp(b.0));
        let actual = json!({
            "found": found.iter().map(|(verb, hit)| json!([verb, {
                "value": hit.value, "file": hit.file, "line": hit.line, "index": hit.index, "generated": hit.generated,
            }])).collect::<Vec<_>>(),
            "files": executed.files,
        });
        // Ключи `found` в 0.4.7 идут в порядке вставки; сравнение — по именам.
        let mut expected = case["result"].clone();
        let found = expected["found"].as_array_mut().unwrap();
        found.sort_by(|a, b| str_of(&a[0]).cmp(str_of(&b[0])));
        // 0.4.9: GENERATED_FILES — только won.cfg (файлы лаунчера больше не пишутся).
        for hit in found.iter_mut() {
            let won = str_of(&hit[1]["file"]).eq_ignore_ascii_case("won.cfg");
            hit[1]["generated"] = json!(won);
        }
        mismatches.check(&actual, &expected, name);
    }
    mismatches.finish("game-config: execute");
}

#[test]
fn import_values_take_the_last_value_and_clamp_to_the_window() {
    let section = &FIXTURE["importValues"];
    let texts = items(&section["texts"]);
    let catalogs = [
        ("hl1-won", "goldsrc"),
        ("hl2-4260", "source"),
        ("ghosting-renewed", "source"),
        ("hl2-2707", "source-old"),
    ];
    let mut mismatches = Mismatches::default();
    for (c, (game_id, engine)) in catalogs.iter().enumerate() {
        let catalog = game_config::catalog_for(game_id, engine).unwrap();
        for (t, text) in texts.iter().enumerate() {
            let imported = game_config::import_values(&catalog, str_of(text));
            let actual = json!({
                "values": imported.values.iter().map(|(cvar, v)| json!([cvar, value_text(Some(*v))])).collect::<Vec<_>>(),
                "skipped": imported.skipped,
            });
            mismatches.check(
                &actual,
                &section["results"][c][t],
                format!("{game_id} {text}"),
            );
        }
    }
    mismatches.finish("game-config: importValues");
}

#[test]
fn replace_at_keeps_the_line_the_command_and_the_comment() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["replaceAt"]) {
        let actual = game_config::replace_at(
            str_of(&case[0]),
            case[1].as_u64().unwrap() as usize,
            case[2].as_u64().unwrap() as usize,
            str_of(&case[3]),
        );
        mismatches.check_eq(
            actual.as_deref(),
            opt_str(&case[4]),
            format!("{} / {} / {} / {}", case[0], case[1], case[2], case[3]),
        );
    }
    mismatches.finish("game-config: replaceAt");
}

#[test]
fn append_line_keeps_the_eol_of_the_file() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["appendLine"]) {
        let actual = game_config::append_line(
            opt_str(&case[0]).unwrap_or_default(),
            "volume 0.5",
            opt_str(&case[1]),
        );
        mismatches.check_eq(
            actual.as_str(),
            str_of(&case[2]),
            format!("{} / {}", case[0], case[1]),
        );
    }
    mismatches.finish("game-config: appendLine");
}

// ------------------------------------------------------------------ С

struct Runner {
    root: PathBuf,
    _temp: tempfile::TempDir,
    search: CfgSearch,
}

impl Runner {
    fn new() -> Runner {
        let (temp, root) = temp_root();
        // Реестра нет: язык Steam не читается, как в Linux.
        let search = CfgSearch::new(SteamLanguage::new(Arc::new(FakeRegistry::default())));
        Runner {
            root,
            _temp: temp,
            search,
        }
    }

    /// Ошибка шага так, как её описывает скрипт.
    fn failure(&self, failure: &Failure) -> Value {
        match failure {
            Failure::Delivery(error) => json!({"error": {
                "kind": "delivery",
                "code": error.code.as_str(),
                "message": normalize_text(&self.root, &error.message),
            }}),
            Failure::Raw(error) => json!({"error": {"kind": "raw", "code": error.code}}),
        }
    }

    fn step(&self, step: &Value) -> Value {
        if let Some(common) = common_step(&self.root, step) {
            return common;
        }
        let install: PathBuf = if step["install"] == "empty" {
            PathBuf::new()
        } else if let Some(dir) = opt_str(&step["installDir"]) {
            path_in(&self.root, dir)
        } else {
            self.root.clone()
        };
        // launchConfig: null и {} — «папки игры нет», как пустая папка.
        let game_directory = match opt_str(&step["launchConfig"]) {
            Some("null" | "empty") => "",
            _ => opt_str(&step["gameDirectory"]).unwrap_or_default(),
        };
        let game_id = str_of(&step["gameId"]);
        let engine = str_of(&step["engine"]);
        match str_of(&step["op"]) {
            "readConfig" => {
                match game_config_install::read_config(
                    &self.search,
                    &install,
                    game_directory,
                    game_id,
                    engine,
                ) {
                    Ok(None) => json!({"ok": null}),
                    Ok(Some(result)) => json!({"ok": {
                        "values": result.values.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                        "sources": result.sources.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                        "files": result.files,
                    }}),
                    Err(hlsr_delivery::player_config::Error::Failure(failure)) => {
                        self.failure(failure.as_ref())
                    }
                    Err(error) => json!({"error": {"kind": "player", "code": error.code()}}),
                }
            }
            "writeConfig" => {
                let parsed;
                let changes = match opt_str(&step["parse"]) {
                    Some(text) => {
                        parsed = hlsr_core::json_text::parse_js_json(text).unwrap();
                        &parsed
                    }
                    None => &step["changes"],
                };
                match game_config_install::write_config(
                    &self.search,
                    &install,
                    game_directory,
                    game_id,
                    engine,
                    changes,
                    None,
                ) {
                    Ok(result) => json!({"ok": {"written": result.written}}),
                    Err(WriteConfigError::Config(error)) => json!({"error": {
                        "kind": "config", "code": error.code.as_str(), "cvar": error.cvar,
                    }}),
                    Err(WriteConfigError::Other(failure)) => self.failure(&failure),
                    Err(WriteConfigError::Player(error)) => {
                        json!({"error": {"kind": "player", "code": error.code()}})
                    }
                }
            }
            other => panic!("неизвестный шаг {other}"),
        }
    }
}

#[test]
#[ignore = "0.4.7: правки шли в config.cfg и hlsr-config.cfg; 0.4.9 пишет в файл игрока (config_selftests.rs)"]
fn read_and_write_config_on_scenarios() {
    let mut mismatches = Mismatches::default();
    for scenario in items(&FIXTURE["install"]) {
        let name = str_of(&scenario["name"]);
        if scenario["unix"].as_bool().unwrap_or(false) && cfg!(windows) {
            continue;
        }
        // Намеренное отступление от 0.4.7 (потеря данных в 0.4.7): поведение Rust проверяют
        // `unreadable_or_oversized_target_*` в config_selftests.rs; причина — в поле `deviation`.
        if scenario.get("deviation").is_some() {
            assert!(
                !str_of(&scenario["deviation"]).is_empty(),
                "{name}: у отступления нет причины"
            );
            continue;
        }
        if name.contains("прав") && scenario::is_root_user() {
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
    mismatches.finish("game-config-install: сценарии");
}
