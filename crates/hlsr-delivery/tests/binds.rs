//! Бинды, заготовки и гайд HL1 против 0.4.7: фикстура `binds.json` из
//! `tools/reference/delivery/binds.js` (модули 0.4.7 в Node 24 на тех же входах).

mod support;

use hlsr_core::js::Platform;
use hlsr_core::js::object::JsObject;
use hlsr_core::json_text::stringify;
use hlsr_delivery::bind_presets::{self, AliasInfo, Catalog, Preset};
use hlsr_delivery::game_binds::{
    self, Bind, BindRead, ImportOptions, Override, Overrides, Rewrite, command_error,
};
use hlsr_delivery::hl1_guide::{self, Script};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("binds.json"));

fn strings(value: &Value) -> Vec<String> {
    items(value).iter().map(|s| str_of(s).to_owned()).collect()
}

// ------------------------------------------------- JSON ⇄ типы Rust

fn bind_object(value: &Value) -> JsObject<Bind> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, bind)| {
                    (
                        key.clone(),
                        Bind {
                            command: str_of(&bind["command"]).to_owned(),
                            source: opt_str(&bind["source"]).unwrap_or_default().to_owned(),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_object(value: &Value) -> JsObject<String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, text)| (key.clone(), str_of(text).to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn pairs(value: &Value) -> Vec<(String, String)> {
    items(value)
        .iter()
        .map(|pair| (str_of(&pair[0]).to_owned(), str_of(&pair[1]).to_owned()))
        .collect()
}

fn bind_read(value: &Value) -> Option<BindRead> {
    if value.is_null() {
        return None;
    }
    Some(BindRead {
        binds: bind_object(&value["binds"]),
        aliases: string_object(&value["aliases"]),
        cvars: string_object(&value["cvars"]),
        files: strings(&value["files"]),
        imported: value
            .get("imported")
            .filter(|v| v.is_array())
            .map(strings)
            .unwrap_or_default(),
        ..BindRead::default()
    })
}

fn entries_of<V>(object: &JsObject<V>, mut one: impl FnMut(&V) -> Value) -> Value {
    Value::Array(
        object
            .iter()
            .map(|(key, value)| json!([key, one(value)]))
            .collect(),
    )
}

fn overrides_entries(overrides: &Overrides) -> Value {
    entries_of(
        overrides,
        |entry| json!({"command": entry.command, "base": entry.base}),
    )
}

fn read_value(read: &BindRead) -> Value {
    json!({
        "binds": entries_of(&read.binds, |bind| json!({"command": bind.command, "source": bind.source})),
        "aliases": entries_of(&read.aliases, |text| json!(text)),
        "cvars": entries_of(&read.cvars, |text| json!(text)),
        "files": read.files,
    })
}

/// Фикстура хранит строку, которую 0.4.7 разворачивает из описания `{ repeat, tail }`.
fn expand(text: &Value) -> String {
    match text {
        Value::String(text) => text.clone(),
        spec => {
            let [piece, count] = [&spec["repeat"][0], &spec["repeat"][1]];
            let tail = opt_str(&spec["tail"]).unwrap_or_default();
            str_of(piece).repeat(count.as_u64().unwrap() as usize) + tail
        }
    }
}

// ------------------------------------------------------------ порядок ключей

#[test]
fn object_key_order_is_that_of_js() {
    let mut mismatches = Mismatches::default();
    for sequence in items(&FIXTURE["objectOrder"]) {
        let mut object: JsObject<i32> = JsObject::new();
        for (step, operation) in items(&sequence["operations"]).iter().enumerate() {
            let key = str_of(&operation[1]);
            match str_of(&operation[0]) {
                "set" => {
                    object.insert(key, 1);
                }
                _ => {
                    object.remove(key);
                }
            }
            let actual: Vec<&str> = object.keys().collect();
            mismatches.check(
                &json!(actual),
                &sequence["states"][step],
                format!("{} после шага {step}", sequence["operations"]),
            );
        }
    }
    mismatches.finish("порядок ключей");
}

// ----------------------------------------------------------------- game-binds

#[test]
fn constants_engines_and_key_lists() {
    let c = &FIXTURE["constants"];
    assert_eq!(game_binds::BINDS_FILE, str_of(&c["BINDS_FILE"]));
    assert_eq!(game_binds::BINDS_EXEC_LINE, str_of(&c["BINDS_EXEC_LINE"]));
    assert_eq!(game_binds::IMPORT_FILE, str_of(&c["IMPORT_FILE"]));
    assert_eq!(game_binds::IMPORT_EXEC_LINE, str_of(&c["IMPORT_EXEC_LINE"]));
    assert_eq!(
        game_binds::MAX_IMPORT_BYTES as u64,
        c["MAX_IMPORT_BYTES"].as_u64().unwrap()
    );
    assert_eq!(
        game_binds::MAX_COMMAND as u64,
        c["MAX_COMMAND"].as_u64().unwrap()
    );
    assert_eq!(json!(game_binds::GOLDSRC_KEYS), c["GOLDSRC_KEYS"]);
    assert_eq!(json!(game_binds::SOURCE_KEYS), c["SOURCE_KEYS"]);

    for (name, expected) in c["engines"].as_object().unwrap() {
        let engine = game_binds::engine_of(name);
        assert_eq!(engine.name, name);
        let keys = match &expected["keys"] {
            Value::String(list) if list == "GOLDSRC_KEYS" => json!(game_binds::GOLDSRC_KEYS),
            Value::String(list) if list == "SOURCE_KEYS" => json!(game_binds::SOURCE_KEYS),
            other => other.clone(),
        };
        assert_eq!(json!(engine.keys), keys, "{name}: keys");
        assert_eq!(
            engine.cfg_dir,
            str_of(&expected["cfgDir"]),
            "{name}: cfgDir"
        );
        assert_eq!(json!(engine.roots), expected["roots"], "{name}: roots");
        assert_eq!(
            engine.exec_file,
            str_of(&expected["execFile"]),
            "{name}: execFile"
        );
        assert_eq!(
            engine.encoding.as_str(),
            str_of(&expected["encoding"]),
            "{name}: encoding"
        );
    }
    for pair in items(&c["engineOf"]) {
        assert_eq!(
            game_binds::engine_of(str_of(&pair[0])).name,
            str_of(&pair[1]),
            "{}",
            pair[0]
        );
    }
}

#[test]
fn normalize_key_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["normalizeKey"]) {
        let actual = game_binds::normalize_key(str_of(&case[0]));
        mismatches.check(
            &json!(actual),
            &case[1],
            format!("normalizeKey({})", case[0]),
        );
    }
    mismatches.finish("normalizeKey");
}

#[test]
fn command_checks_and_cleaning_match() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["command"]) {
        let text = str_of(&case[0]);
        let cleaned = game_binds::clean_command(text);
        mismatches.check(
            &json!(cleaned),
            &case[1],
            format!("cleanCommand({})", case[0]),
        );
        let code = |error: Option<game_binds::CommandError>| json!(error.map(|e| e.code()));
        mismatches.check(
            &code(command_error(text)),
            &case[2],
            format!("commandError({})", case[0]),
        );
        mismatches.check(
            &code(command_error(&cleaned)),
            &case[3],
            format!("commandError(clean({}))", case[0]),
        );
    }
    mismatches.finish("команды");
}

#[test]
fn parse_cfg_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["parseCfg"]) {
        let actual = game_binds::parse_cfg(str_of(&case[0]));
        mismatches.check(&json!(actual), &case[1], format!("parseCfg({})", case[0]));
    }
    mismatches.finish("parseCfg");
}

#[test]
fn read_binds_matches() {
    let mut mismatches = Mismatches::default();
    for scenario in items(&FIXTURE["readBinds"]) {
        let files: HashMap<String, String> = scenario["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, text)| (name.clone(), str_of(text).to_owned()))
            .collect();
        let read_file = |name: &str| files.get(name).cloned();
        let read = if scenario["roots"].is_null() {
            game_binds::read_binds(read_file, game_binds::GOLDSRC.roots)
        } else {
            let roots = strings(&scenario["roots"]);
            let roots: Vec<&str> = roots.iter().map(String::as_str).collect();
            game_binds::read_binds(read_file, &roots)
        };
        mismatches.check(
            &read_value(&read),
            &scenario["result"],
            format!("readBinds «{}»", scenario["name"]),
        );
    }
    mismatches.finish("readBinds");
}

fn override_inputs() -> &'static Vec<Value> {
    items(&FIXTURE["overrides"]["inputs"])
}

fn file_binds() -> Vec<JsObject<Bind>> {
    items(&FIXTURE["overrides"]["fileBinds"])
        .iter()
        .map(bind_object)
        .collect()
}

#[test]
fn sanitize_prune_and_reset_match() {
    let o = &FIXTURE["overrides"];
    let mut mismatches = Mismatches::default();
    for case in items(&o["sanitize"]) {
        let input = &override_inputs()[case["input"].as_u64().unwrap() as usize];
        let actual = overrides_entries(&game_binds::sanitize_overrides(input));
        mismatches.check(
            &actual,
            &case["result"],
            format!("sanitizeOverrides({input})"),
        );
    }
    let file_binds = file_binds();
    for case in items(&o["prune"]) {
        let input = &override_inputs()[case["input"].as_u64().unwrap() as usize];
        let file = &file_binds[case["fileBinds"].as_u64().unwrap() as usize];
        let actual = game_binds::prune_overrides(&game_binds::sanitize_overrides(input), file);
        mismatches.check(
            &overrides_entries(&actual),
            &case["result"],
            format!("pruneOverrides({input})"),
        );
    }
    for case in items(&o["reset"]) {
        let input = &override_inputs()[case["input"].as_u64().unwrap() as usize];
        let actual = game_binds::reset_overrides(&game_binds::sanitize_overrides(input));
        mismatches.check(
            &overrides_entries(&actual),
            &case["result"],
            format!("resetOverrides({input})"),
        );
    }
    mismatches.finish("переназначения");
}

#[test]
fn to_cfg_lines_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["overrides"]["toCfg"]) {
        let index = case["input"].as_i64().unwrap();
        let overrides = match usize::try_from(index) {
            Ok(index) => game_binds::sanitize_overrides(&override_inputs()[index]),
            Err(_) => Overrides::new(),
        };
        let aliases = if case["aliases"].is_null() {
            Vec::new()
        } else {
            pairs(&case["aliases"])
        };
        let actual = game_binds::to_cfg_lines(&overrides, opt_str(&case["engine"]), &aliases);
        mismatches.check(
            &json!(actual),
            &case["text"],
            format!("toCfgLines {} {}", case["input"], case["engine"]),
        );
    }
    mismatches.finish("toCfgLines");
}

fn overrides_from_object(value: &Value) -> Overrides {
    game_binds::sanitize_overrides(value)
}

#[test]
fn import_overrides_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["importOverrides"]) {
        let file_binds = bind_object(&case["fileBinds"]);
        let overrides = overrides_from_object(&case["overrides"]);
        let aliases = string_object(&case["aliases"]);
        let known = strings(&case["known"]);
        let known_command = |command: &str| known.iter().any(|k| k == command);
        let options = ImportOptions {
            file_binds: Some(&file_binds),
            overrides: Some(&overrides),
            known_command: Some(&known_command),
            aliases: Some(&aliases),
        };
        let result = game_binds::import_overrides(str_of(&case["text"]), &options);
        let actual = json!({
            "overrides": overrides_entries(&result.overrides),
            "stats": {
                "changed": result.changed, "unbound": result.unbound, "unchanged": result.unchanged,
                "skippedAlias": result.skipped_alias, "skippedInvalid": result.skipped_invalid,
                "skippedKey": result.skipped_key, "found": result.found,
            },
        });
        mismatches.check(
            &actual,
            &case["result"],
            format!("importOverrides({})", case["text"]),
        );
    }
    mismatches.finish("importOverrides");
}

#[test]
fn cfg_text_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["cfgText"]) {
        let actual = game_binds::cfg_text(&expand(&case[0]));
        mismatches.check(&json!(actual), &case[1], format!("cfgText({})", case[0]));
    }
    mismatches.finish("cfgText");
}

#[test]
fn split_line_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["splitLine"]) {
        let (parts, comment) = game_binds::split_line(str_of(&case[0]));
        mismatches.check(
            &json!({"parts": parts, "comment": comment}),
            &case[1],
            format!("splitLine({})", case[0]),
        );
    }
    mismatches.finish("splitLine");
}

fn rewriter(name: &str) -> impl FnMut(&[String], &str) -> Rewrite + use<> {
    let name = name.to_owned();
    move |args: &[String], text: &str| match name.as_str() {
        "none" => Rewrite::Keep,
        "dropExec" if args[0].to_lowercase() == "exec" => Rewrite::Drop(String::new()),
        "dropBind" if args[0] == "bind" => Rewrite::Drop("bind removed".to_owned()),
        "replaceSay" if args[0] == "say" => {
            Rewrite::Replace(format!("echo {}", args[1..].join(" ")))
        }
        "emptyX" if args[0] == "x" => Rewrite::Replace(String::new()),
        "keepText" => Rewrite::Replace(text.to_owned()),
        _ => Rewrite::Keep,
    }
}

fn mapper(name: &str) -> fn(&str) -> String {
    match name {
        "trim" => |part| hlsr_core::js::trim(part).to_owned(),
        "upper" => |part| part.to_uppercase(),
        _ => |part| part.to_owned(),
    }
}

#[test]
fn rewrite_top_level_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["rewriteTopLevel"]) {
        let actual = game_binds::rewrite_top_level(
            str_of(&case[0]),
            rewriter(str_of(&case[1])),
            mapper(str_of(&case[2])),
        );
        mismatches.check(
            &json!(actual),
            &case[3],
            format!("rewriteTopLevel({}, {}, {})", case[0], case[1], case[2]),
        );
    }
    mismatches.finish("rewriteTopLevel");
}

#[test]
fn import_file_text_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["importFileText"]) {
        let actual = game_binds::import_file_text(str_of(&case[0]), case[1].as_bool().unwrap());
        mismatches.check(
            &json!(actual),
            &case[2],
            format!("importFileText({}, {})", case[0], case[1]),
        );
    }
    mismatches.finish("importFileText");
}

#[test]
fn import_sources_and_preview_match() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["isImportSource"]) {
        let imported = if case[1].is_null() {
            Vec::new()
        } else {
            strings(&case[1])
        };
        let actual = game_binds::is_import_source(opt_str(&case[0]).unwrap_or_default(), &imported);
        mismatches.check(
            &json!(actual),
            &case[2],
            format!("isImportSource({}, {})", case[0], case[1]),
        );
    }
    for case in items(&FIXTURE["withImport"]) {
        let read = bind_read(&case["read"]);
        let actual = game_binds::with_import(read.as_ref(), str_of(&case["text"]));
        mismatches.check(
            &read_value(&actual),
            &case["result"],
            format!("withImport({}, {})", case["read"], case["text"]),
        );
    }
    for case in items(&FIXTURE["importTouchedKeys"]) {
        let actual = game_binds::import_touched_keys(str_of(&case[0]));
        mismatches.check(
            &json!(actual),
            &case[1],
            format!("importTouchedKeys({})", case[0]),
        );
    }
    mismatches.finish("импорт");
}

#[test]
fn alias_expansion_matches() {
    let a = &FIXTURE["aliases"];
    let sets: Vec<JsObject<String>> = items(&a["aliasSets"]).iter().map(string_object).collect();
    let maps: Vec<_> = sets.iter().map(game_binds::alias_map).collect();
    let mut mismatches = Mismatches::default();
    for case in items(&a["commandsOf"]) {
        let actual = game_binds::commands_of(str_of(&case[0]));
        mismatches.check(&json!(actual), &case[1], format!("commandsOf({})", case[0]));
    }
    for case in items(&a["expandCommand"]) {
        let set = case[0].as_u64().unwrap() as usize;
        let actual = game_binds::expand_command(str_of(&case[1]), &maps[set]);
        mismatches.check(
            &json!(actual),
            &case[2],
            format!("expandCommand({}, набор {set})", case[1]),
        );
    }
    for (set, expected) in items(&a["reassigned"]).iter().enumerate() {
        let mut names: Vec<String> = game_binds::reassigned_aliases(&maps[set])
            .into_iter()
            .collect();
        names.sort();
        mismatches.check(
            &json!(names),
            expected,
            format!("reassignedAliases(набор {set})"),
        );
    }
    for (set, expected) in items(&a["aliasMap"]).iter().enumerate() {
        if expected.is_null() {
            continue;
        }
        let mut entries: Vec<(&String, &String)> = maps[set].iter().collect();
        entries.sort();
        let mut want: Vec<(String, String)> = pairs(expected);
        want.sort();
        let actual: Vec<(String, String)> = entries
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        mismatches.check_eq(actual, want, format!("aliasMap(набор {set})"));
    }
    mismatches.finish("alias");
}

// -------------------------------------------------------------- bind-presets

fn platform_of(name: &str) -> Platform {
    match name {
        "win32" => Platform::Win32,
        "linux" => Platform::Linux,
        other => panic!("платформа {other}"),
    }
}

fn loc_value(loc: &bind_presets::Loc) -> Value {
    json!({"ru": loc.ru, "en": loc.en})
}

fn digest(value: &Value) -> String {
    let hash = Sha256::digest(stringify(value, "").as_bytes());
    hash.iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Заготовка в виде JSON из `dumpPreset` скрипта сверки.
fn preset_value(preset: &Preset) -> Value {
    let mut map = Map::new();
    map.insert("category".into(), json!(preset.category));
    map.insert("command".into(), json!(preset.command));
    map.insert("label".into(), loc_value(&preset.label));
    map.insert("scripted".into(), json!(preset.scripted));
    map.insert("practice".into(), json!(preset.practice));
    map.insert("listed".into(), json!(preset.listed));
    let aliases = json!(
        preset
            .aliases
            .iter()
            .map(|(n, v)| json!([n, v]))
            .collect::<Vec<_>>()
    );
    if preset.category == "guide" {
        let guide_note = &FIXTURE["presets"]["guideNote"];
        let note = preset.note.as_ref().map(loc_value);
        assert_eq!(
            note.as_ref(),
            Some(guide_note),
            "{}: пояснение заготовки гайда",
            preset.command
        );
        map.insert("note".into(), json!("guide-note"));
        map.insert(
            "aliases".into(),
            json!({"count": preset.aliases.len(), "sha": digest(&aliases)}),
        );
    } else {
        map.insert("aliases".into(), aliases);
        if let Some(note) = &preset.note {
            map.insert("note".into(), loc_value(note));
        }
    }
    if let Some(guide) = &preset.guide {
        map.insert("guide".into(), json!(guide));
    }
    if let Some(auto) = preset.auto_alias {
        map.insert("autoAlias".into(), json!(auto));
    }
    Value::Object(map)
}

fn catalog_value(catalog: &Catalog) -> Value {
    let keys: Vec<&str> = catalog.by_command.keys().collect();
    let first: Vec<usize> = catalog.by_command.values().copied().collect();
    json!({
        "gameId": catalog.game_id,
        "engine": catalog.engine,
        "runModes": catalog.run_modes,
        "categories": catalog.categories.iter().map(|c| json!({"id": c.id, "tone": c.tone, "title": loc_value(&c.title)})).collect::<Vec<_>>(),
        "presets": catalog.presets.iter().map(preset_value).collect::<Vec<_>>(),
        "firstOfKey": first,
        "keys": keys,
    })
}

/// То же, что `against` скрипта сверки: отличия каталога от базового.
fn catalog_diff(catalog: &Catalog, base: &Catalog) -> Value {
    let a = catalog_value(catalog);
    let b = catalog_value(base);
    let (left, right) = (
        a["presets"].as_array().unwrap(),
        b["presets"].as_array().unwrap(),
    );
    assert_eq!(
        left.len(),
        right.len(),
        "{}: число заготовок",
        catalog.game_id
    );
    assert_eq!(
        a["categories"], b["categories"],
        "{}: категории",
        catalog.game_id
    );
    let diff: Vec<Value> = left
        .iter()
        .zip(right)
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .map(|(index, (x, _))| json!([index, x]))
        .collect();
    json!({"engine": a["engine"], "presetDiff": diff, "firstOfKey": a["firstOfKey"], "keys": a["keys"]})
}

fn catalog(game: &str, platform: Platform) -> &'static Catalog {
    bind_presets::catalog_for(game, platform).unwrap_or_else(|| panic!("нет каталога {game}"))
}

#[test]
fn preset_constants_and_engines() {
    let c = &FIXTURE["presets"]["consts"];
    for pair in items(&c["GAME_ENGINES"]) {
        assert_eq!(
            bind_presets::engine_for(str_of(&pair[0])),
            Some(str_of(&pair[1])),
            "{}",
            pair[0]
        );
    }
    assert_eq!(bind_presets::HL1_RUN_ALIAS, str_of(&c["HL1_RUN_ALIAS"]));
    assert_eq!(
        bind_presets::pre_steampipe_save_deletion(),
        str_of(&c["PRE_STEAMPIPE_SAVE_DELETION"])
    );
    for platform in ["linux", "win32"] {
        let p = platform_of(platform);
        assert_eq!(
            bind_presets::ghosting_save_deletion(p),
            str_of(&c["GHOSTING_SAVE_DELETION"][platform]),
            "{platform}"
        );
        assert_eq!(
            bind_presets::steampipe_save_deletion(p),
            str_of(&c["STEAMPIPE_SAVE_DELETION"][platform]),
            "{platform}"
        );
    }
    for pair in items(&FIXTURE["presets"]["engineFor"]) {
        assert_eq!(
            bind_presets::engine_for(str_of(&pair[0])),
            opt_str(&pair[1]),
            "engineFor({})",
            pair[0]
        );
    }
    for pair in items(&FIXTURE["presets"]["catalogFor"]) {
        let found = bind_presets::catalog_for(str_of(&pair[0]), Platform::Linux)
            .map(|c| c.game_id.as_str());
        assert_eq!(found, opt_str(&pair[1]), "catalogFor({})", pair[0]);
    }
    assert_eq!(
        json!(bind_presets::GAME_IDS),
        json!([
            "hl1-won",
            "ghosting-renewed",
            "hl2-4260",
            "hl2-5288",
            "hl2-2707"
        ])
    );
}

#[test]
fn catalogs_match() {
    let presets = &FIXTURE["presets"];
    let mut mismatches = Mismatches::default();
    for (id, expected) in presets["linux"].as_object().unwrap() {
        mismatches.check(
            &catalog_value(catalog(id, Platform::Linux)),
            expected,
            format!("каталог {id} (linux)"),
        );
    }
    let base = catalog("hl2-4260", Platform::Linux);
    for (id, expected) in presets["sameAsHl2"].as_object().unwrap() {
        mismatches.check(
            &catalog_diff(catalog(id, Platform::Linux), base),
            expected,
            format!("каталог {id} против hl2-4260"),
        );
    }
    for (id, expected) in presets["win32"].as_object().unwrap() {
        mismatches.check(
            &catalog_diff(catalog(id, Platform::Win32), catalog(id, Platform::Linux)),
            expected,
            format!("каталог {id}: win32 против linux"),
        );
    }
    mismatches.finish("каталоги");
}

#[test]
fn command_key_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["presets"]["commandKey"]) {
        mismatches.check(
            &json!(bind_presets::command_key(str_of(&case[0]))),
            &case[1],
            format!("commandKey({})", case[0]),
        );
    }
    mismatches.finish("commandKey");
}

#[test]
fn preset_lookup_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["presets"]["presetOf"]) {
        let platform = platform_of(str_of(&case["platform"]));
        let catalog = catalog(str_of(&case["gameId"]), platform);
        assert_eq!(
            items(&case["commands"]).len(),
            items(&case["results"]).len(),
            "шагов и результатов поровну"
        );
        for (command, expected) in items(&case["commands"]).iter().zip(items(&case["results"])) {
            let index = catalog
                .preset_of(str_of(command))
                .and_then(|found| catalog.presets.iter().position(|p| std::ptr::eq(p, found)))
                .map_or(-1, |index| index as i64);
            mismatches.check(
                &json!(index),
                expected,
                format!(
                    "presetOf {} {}: {command}",
                    case["platform"], case["gameId"]
                ),
            );
        }
    }
    mismatches.finish("presetOf");
}

#[test]
fn script_aliases_and_auto_alias_match() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["presets"]["aliasesFor"]) {
        let platform = platform_of(str_of(&case["platform"]));
        let game = str_of(&case["gameId"]);
        let catalog = catalog(game, platform);
        let expected_flags = str_of(&case["autoAlias"]).as_bytes();
        for (index, set) in items(&case["sets"]).iter().enumerate() {
            let mut overrides = Overrides::new();
            for (position, item) in items(set).iter().enumerate() {
                let preset = item[0].as_i64().unwrap();
                let entry = if preset < 0 {
                    Override {
                        command: String::new(),
                        base: "x".into(),
                    }
                } else {
                    let command = &catalog.presets[preset as usize].command;
                    let upper = item[1].as_i64() == Some(1);
                    Override {
                        command: if upper {
                            command.to_uppercase()
                        } else {
                            command.clone()
                        },
                        base: String::new(),
                    }
                };
                overrides.insert(format!("k{position}"), entry);
            }
            let aliases = bind_presets::aliases_for(game, platform, &overrides);
            let value = json!(
                aliases
                    .iter()
                    .map(|(n, v)| json!([n, v]))
                    .collect::<Vec<_>>()
            );
            let context = format!(
                "aliasesFor {} {} набор {index}",
                case["platform"], case["gameId"]
            );
            mismatches.check(
                &json!([aliases.len(), digest(&value)]),
                &case["aliases"][index],
                &context,
            );
            if index < 4 {
                mismatches.check(&value, &case["first"][index], &context);
            }
            let auto = bind_presets::needs_auto_alias(game, platform, &overrides);
            mismatches.check_eq(
                auto,
                expected_flags[index] == b'1',
                format!("needsAutoAlias {}", context),
            );
        }
    }
    for case in items(&FIXTURE["presets"]["nullCases"]) {
        // Неизвестная игра и пустой набор: пусто. Обращение к набору с null в Rust невозможно.
        let overrides = Overrides::new();
        let game = str_of(&case[0]);
        let aliases = bind_presets::aliases_for(game, Platform::Linux, &overrides);
        assert!(aliases.is_empty(), "{game}");
        assert_eq!(json!(case[2]), json!([]), "{game}: 0.4.7 тоже пусто");
        assert!(!bind_presets::needs_auto_alias(
            game,
            Platform::Linux,
            &overrides
        ));
    }
    mismatches.finish("alias заготовок");
}

#[test]
fn is_scripted_matches() {
    let sets: Vec<JsObject<String>> = items(&FIXTURE["aliases"]["aliasSets"])
        .iter()
        .map(string_object)
        .collect();
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["presets"]["scripted"]) {
        let platform = platform_of(str_of(&case["platform"]));
        let game = str_of(&case["gameId"]);
        let catalog = bind_presets::catalog_for(game, platform);
        let commands = strings(
            &FIXTURE["presets"]["scriptedCommands"]
                [format!("{}/{game}", case["platform"].as_str().unwrap())],
        );
        for (index, expected) in items(&case["sets"]).iter().enumerate() {
            let info = sets
                .get(index)
                .map_or_else(AliasInfo::default, AliasInfo::new);
            let actual: String = commands
                .iter()
                .map(|command| {
                    if bind_presets::is_scripted(catalog, command, &info) {
                        '1'
                    } else {
                        '0'
                    }
                })
                .collect();
            if actual != str_of(expected) {
                let at = actual
                    .bytes()
                    .zip(str_of(expected).bytes())
                    .position(|(a, b)| a != b);
                mismatches.check_eq(
                    at.map(|at| commands[at].clone()),
                    None,
                    format!(
                        "isScripted {} {} набор {index}: первая разница",
                        case["platform"], case["gameId"]
                    ),
                );
            }
        }
    }
    mismatches.finish("isScripted");
}

// ------------------------------------------------------------------ hl1-guide

fn script_variant(script: &Script, variant: &str) -> Script {
    let mut changed = script.clone();
    match variant {
        "usage" => changed.usage = "new text".into(),
        "moreNotes" => changed.notes.push("extra".into()),
        "noNotes" => changed.notes.clear(),
        "otherId" => changed.id = "unknown-script".into(),
        _ => {}
    }
    changed
}

fn translation_value(translation: Option<hl1_guide::Translation>) -> Value {
    translation.map_or(Value::Null, |t| json!({"notes": t.notes, "usage": t.usage}))
}

#[test]
fn guide_data_is_byte_identical_to_0_4_7() {
    let data = &FIXTURE["guide"]["data"];
    for (name, text) in [
        ("hl1-guide-data.json", hl1_guide::DATA_JSON),
        ("hl1-guide-ru.json", hl1_guide::RU_JSON),
    ] {
        assert_eq!(
            text.len() as u64,
            data[name]["bytes"].as_u64().unwrap(),
            "{name}: размер"
        );
        let hash: String = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hash, str_of(&data[name]["sha256"]), "{name}: sha256");
    }
}

#[test]
fn guide_constants_and_structure() {
    let g = &FIXTURE["guide"];
    let c = &g["constants"];
    assert_eq!(
        hl1_guide::SENSITIVITY_TOKEN,
        str_of(&c["SENSITIVITY_TOKEN"])
    );
    assert_eq!(
        hl1_guide::DEFAULT_SENSITIVITY,
        str_of(&c["DEFAULT_SENSITIVITY"])
    );
    assert_eq!(hl1_guide::M_PITCH_TOKEN, str_of(&c["M_PITCH_TOKEN"]));
    assert_eq!(hl1_guide::DEFAULT_M_PITCH, str_of(&c["DEFAULT_M_PITCH"]));
    assert_eq!(hl1_guide::AUTO_ALIAS, str_of(&c["AUTO_ALIAS"]));
    assert_eq!(hl1_guide::GAME_CFG, str_of(&c["GAME_CFG"]));
    assert_eq!(hl1_guide::source().repo, str_of(&g["source"]["repo"]));
    assert_eq!(hl1_guide::source().commit, str_of(&g["source"]["commit"]));
    let chapters: Vec<&str> = hl1_guide::chapters()
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(json!(chapters), g["chapters"]);
    assert_eq!(
        hl1_guide::scripts().len() as u64,
        g["scriptCount"].as_u64().unwrap()
    );
    for script in hl1_guide::scripts() {
        assert!(
            chapters.contains(&script.chapter.as_str()),
            "{}: глава",
            script.id
        );
    }
}

#[test]
fn guide_text_hash_and_translation_match() {
    let g = &FIXTURE["guide"];
    let mut mismatches = Mismatches::default();
    for (script, case) in hl1_guide::scripts().iter().zip(items(&g["hashes"])) {
        mismatches.check_eq(script.id.as_str(), str_of(&case[0]), "порядок скриптов");
        mismatches.check(
            &json!(hl1_guide::text_hash(script)),
            &case[1],
            format!("textHash({})", script.id),
        );
        let ru = hl1_guide::translation_of(script, "ru");
        let sha = ru.map(|t| {
            let value = json!({"notes": t.notes, "usage": t.usage});
            let hash = Sha256::digest(stringify(&value, "").as_bytes());
            hash.iter().map(|b| format!("{b:02x}")).collect::<String>()
        });
        mismatches.check(
            &json!(sha),
            &case[2],
            format!("translationOf({}, ru)", script.id),
        );
    }
    for case in items(&g["translation"]) {
        let script = hl1_guide::script_by_id(str_of(&case["id"])).expect("скрипт");
        let changed = script_variant(script, str_of(&case["variant"]));
        let context = format!("{} {} {}", case["id"], case["variant"], case["lang"]);
        mismatches.check(
            &json!(hl1_guide::text_hash(&changed)),
            &case["hash"],
            format!("textHash {context}"),
        );
        let actual = translation_value(hl1_guide::translation_of(&changed, str_of(&case["lang"])));
        mismatches.check(&actual, &case["result"], format!("translationOf {context}"));
    }
    for case in items(&g["scriptById"]) {
        let found = hl1_guide::script_by_id(str_of(&case[0]))
            .and_then(|s| hl1_guide::scripts().iter().position(|x| std::ptr::eq(x, s)))
            .map_or(-1, |at| at as i64);
        mismatches.check(&json!(found), &case[1], format!("scriptById({})", case[0]));
    }
    mismatches.finish("гайд: хеши и переводы");
}

#[test]
fn guide_player_values_match() {
    let g = &FIXTURE["guide"];
    let mut mismatches = Mismatches::default();
    for case in items(&g["cleanSensitivity"]) {
        let actual = hl1_guide::clean_sensitivity(Some(str_of(&case[0])));
        mismatches.check(
            &json!(actual),
            &case[1],
            format!("cleanSensitivity({})", case[0]),
        );
    }
    for case in items(&g["cleanPitch"]) {
        let actual = hl1_guide::clean_pitch(Some(str_of(&case[0])));
        mismatches.check(&json!(actual), &case[1], format!("cleanPitch({})", case[0]));
    }
    for case in items(&g["cleanNonString"]) {
        mismatches.check(
            &json!(hl1_guide::clean_sensitivity(None)),
            &case[1],
            format!("cleanSensitivity({})", case[0]),
        );
        mismatches.check(
            &json!(hl1_guide::clean_pitch(None)),
            &case[2],
            format!("cleanPitch({})", case[0]),
        );
    }
    for case in items(&g["withSensitivity"]) {
        let actual = hl1_guide::with_sensitivity(str_of(&case[0]), Some(str_of(&case[1])));
        mismatches.check(
            &json!(actual),
            &case[2],
            format!("withSensitivity({}, {})", case[0], case[1]),
        );
    }
    for case in items(&g["withPlayerCvars"]) {
        let cvars = string_object(&case[1]);
        let actual = hl1_guide::with_player_cvars(str_of(&case[0]), &cvars);
        mismatches.check(
            &json!(actual),
            &case[2],
            format!("withPlayerCvars({}, {})", case[0], case[1]),
        );
    }
    for case in items(&g["hasPlayerToken"]) {
        mismatches.check(
            &json!(hl1_guide::has_player_token(str_of(&case[0]))),
            &case[1],
            format!("hasPlayerToken({})", case[0]),
        );
    }
    mismatches.finish("гайд: значения игрока");
}

#[test]
fn guide_links_match() {
    let corpus = items(&FIXTURE["guide"]["isGuideLink"][0]);
    let expected = str_of(&FIXTURE["guide"]["isGuideLink"][1]).as_bytes();
    assert_eq!(corpus.len(), expected.len());
    let mut mismatches = Mismatches::default();
    for (url, flag) in corpus.iter().zip(expected) {
        mismatches.check_eq(
            hl1_guide::is_guide_link(str_of(url)),
            *flag == b'1',
            format!("isGuideLink({url})"),
        );
    }
    mismatches.finish("гайд: ссылки");
}
