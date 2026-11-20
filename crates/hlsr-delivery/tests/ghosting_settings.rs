//! Настройки Ghosting против 0.4.7: фикстура `ghosting-settings.json` из
//! `tools/reference/delivery/ghosting-settings.js`. Чистые функции: схема (`shared/ghosting-settings.js`),
//! `sanitizeExtras` / `serializeExtras` / `importExtras` / `available` / `resetExtras`,
//! `sanitizePersonalization` / `applyPreset` / `toCfgLines`, стандартный autoexec и разбор
//! разрешения экрана Source (`source-screen.js`). Порядок ключей сверяется по записи `JSON.stringify`.

mod support;

use hlsr_core::js::Platform;
use hlsr_core::json_text::{js_number, stringify};
use hlsr_delivery::ghosting_default_autoexec::default_autoexec;
use hlsr_delivery::ghosting_personalization as gp;
use hlsr_delivery::ghosting_settings::{self as gs, Field, FieldDefault, FieldType};
use hlsr_delivery::source_screen;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("ghosting-settings.json"));

/// Запись `JSON.stringify` без отступов: порядок ключей важен.
fn ordered(value: &Value) -> String {
    stringify(value, "")
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn number(n: f64) -> Value {
    js_number(n)
}

fn color_json(color: gs::Color) -> Value {
    json!({"r": color.r, "g": color.g, "b": color.b})
}

fn default_json(default: &FieldDefault) -> Value {
    match default {
        FieldDefault::Bool(value) => json!(value),
        FieldDefault::Number(value) => number(*value),
        FieldDefault::Text(value) => json!(value),
        FieldDefault::Color(color) => color_json(*color),
    }
}

/// Поле схемы в виде объекта 0.4.7 (порядок ключей не важен: объекты сравниваются как есть).
fn field_json(field: &Field) -> Value {
    let mut object = Map::new();
    object.insert("key".into(), json!(field.key));
    match &field.kind {
        FieldType::Boolean => object.insert("type".into(), json!("boolean")),
        FieldType::Color => object.insert("type".into(), json!("color")),
        FieldType::Text { max_length } => {
            object.insert("maxLength".into(), json!(max_length));
            object.insert("type".into(), json!("text"))
        }
        FieldType::Number { min, max, step } => {
            object.insert("min".into(), number(*min));
            object.insert("max".into(), number(*max));
            object.insert("step".into(), number(*step));
            object.insert("type".into(), json!("number"))
        }
        FieldType::Select { options } => {
            let list: Vec<Value> = options
                .iter()
                .map(|option| json!({"value": option.value, "label": option.label}))
                .collect();
            object.insert("options".into(), Value::Array(list));
            object.insert("type".into(), json!("select"))
        }
    };
    object.insert("default".into(), default_json(&field.default));
    object.insert("label".into(), json!(field.label));
    if let Some(hint) = field.hint {
        object.insert("hint".into(), json!(hint));
    }
    if let Some(unit) = field.unit {
        object.insert("unit".into(), json!(unit));
    }
    if let Some([low, high]) = field.scale {
        object.insert("scale".into(), json!([low, high]));
    }
    if field.command {
        object.insert("command".into(), json!(true));
    }
    if let Some(requires) = field.requires {
        object.insert("requires".into(), json!(requires));
    }
    Value::Object(object)
}

#[test]
fn schema_is_the_same_as_in_0_4_7() {
    let expected = &FIXTURE["schema"];
    let mut m = Mismatches::default();
    let sections = items(&expected["sections"]);
    m.check_eq(gs::SECTIONS.len(), sections.len(), "число разделов");
    for (section, wanted) in gs::SECTIONS.iter().zip(sections) {
        let actual = json!({
            "id": section.id,
            "icon": section.icon,
            "title": section.title,
            "fields": section.fields.iter().map(field_json).collect::<Vec<_>>(),
        });
        m.check(&actual, wanted, format!("раздел {}", section.id));
    }
    let keys: Vec<&str> = gs::fields().map(|field| field.key).collect();
    let wanted_keys: Vec<&str> = items(&expected["fields"]).iter().map(str_of).collect();
    m.check_eq(keys, wanted_keys, "порядок FIELDS");
    m.check(
        &ordered_value(&gs::defaults().to_value()),
        &ordered_value(&expected["defaults"]),
        "defaults()",
    );
    m.finish("схема Ghosting");
}

/// Для сравнения порядка ключей через `Mismatches::check`: значение — строка записи.
fn ordered_value(value: &Value) -> Value {
    Value::String(ordered(value))
}

#[test]
fn sanitize_value_matches_node_on_probes_of_every_type() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["sanitizeValue"]) {
        let key = str_of(&case[0]);
        let input = json!({ key: case[1].clone() });
        let actual = gs::sanitize_extras(&input).to_value();
        // 0.4.9 (S-5): спрей по сети (`//…`) и с `..` не принимается — 0.4.7 его брала.
        let spray = case[1].as_str().map(|text| text.trim().replace('\\', "/"));
        let expected = match spray {
            Some(text)
                if key == "gh_spray_file"
                    && (text.starts_with("//") || text.split('/').any(|part| part == "..")) =>
            {
                json!({})
            }
            _ => case[2].clone(),
        };
        m.check(
            &ordered_value(&actual),
            &ordered_value(&expected),
            format!("sanitizeExtras({{{key}: {}}})", case[1]),
        );
    }
    m.finish("sanitizeExtras на пробах");
}

#[test]
fn random_extras_sanitize_serialize_and_import_like_node() {
    let mut m = Mismatches::default();
    for (at, case) in items(&FIXTURE["extras"]).iter().enumerate() {
        let input = &case["input"];
        let sanitized = gs::sanitize_extras(input);
        m.check(
            &ordered_value(&sanitized.to_value()),
            &ordered_value(&case["sanitized"]),
            format!("случай {at}: sanitizeExtras({input})"),
        );
        let cfg = gs::serialize_extras(input);
        m.check_eq(
            cfg.as_str(),
            str_of(&case["cfg"]),
            format!("случай {at}: serializeExtras({input})"),
        );
        // Разбор записи 0.4.7 над записью Rust и наоборот: чтение и запись замкнуты.
        let back = gs::import_extras(&cfg).to_value();
        let wanted = case.get("back").unwrap_or(&case["sanitized"]);
        m.check(
            &ordered_value(&back),
            &ordered_value(wanted),
            format!("случай {at}: importExtras(serializeExtras)"),
        );
    }
    m.finish("случайные extras");
}

#[test]
fn every_field_alone_serializes_and_reads_back_like_node() {
    let mut m = Mismatches::default();
    let defaults = gs::defaults().to_value();
    for (case, field) in items(&FIXTURE["single"]).iter().zip(gs::fields()) {
        let value = match &field.kind {
            FieldType::Number { max, .. } => number(*max),
            FieldType::Select { options } => json!(options[options.len() - 1].value),
            _ => defaults[field.key].clone(),
        };
        let input = json!({ field.key: value });
        m.check_eq(
            gs::serialize_extras(&input),
            str_of(&case["cfg"]).to_owned(),
            format!("serializeExtras({})", field.key),
        );
        m.check(
            &ordered_value(&gs::import_extras(&gs::serialize_extras(&input)).to_value()),
            &ordered_value(&case["back"]),
            format!("importExtras({})", field.key),
        );
    }
    m.finish("одиночные поля");
}

#[test]
fn special_serialization_cases_match_node() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["serializeSpecial"]) {
        let input = &case["input"];
        m.check_eq(
            gs::serialize_extras(input),
            str_of(&case["cfg"]).to_owned(),
            format!("serializeExtras({input})"),
        );
        m.check(
            &ordered_value(&gs::sanitize_extras(input).to_value()),
            &ordered_value(&case["sanitized"]),
            format!("sanitizeExtras({input})"),
        );
    }
    m.finish("особые случаи записи");
}

#[test]
fn import_extras_matches_node_on_texts() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["import"]) {
        let text = str_of(&case[0]);
        m.check(
            &ordered_value(&gs::import_extras(text).to_value()),
            &ordered_value(&case[1]),
            format!("importExtras({text:?})"),
        );
    }
    m.finish("importExtras на текстах");
}

#[test]
fn available_follows_the_chain_of_requirements_like_node() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["available"]) {
        let key = str_of(&case[0]);
        let field = gs::field(key).unwrap_or_else(|| panic!("нет поля {key}"));
        m.check_eq(
            gs::available(field, &case[1]),
            case[2].as_bool().unwrap(),
            format!("available({key}, {})", case[1]),
        );
    }
    m.finish("available");
}

#[test]
fn reset_extras_resets_only_managed_fields_like_node() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["reset"]) {
        m.check(
            &ordered_value(&gs::reset_extras(&case["input"]).to_value()),
            &ordered_value(&case["reset"]),
            format!("resetExtras({})", case["input"]),
        );
    }
    m.finish("resetExtras");
}

// ------------------------------------------------------------------ персонализация

#[test]
fn personalization_constants_and_defaults_match_node() {
    let expected = &FIXTURE["personalization"];
    let constants = &expected["constants"];
    assert_eq!(gp::PERSONALIZATION_FILE, str_of(&constants["file"]));
    assert_eq!(gp::GAME_DIR, str_of(&constants["gameDir"]));
    assert_eq!(gp::CFG_DIR, str_of(&constants["cfgDir"]));
    assert_eq!(gp::EXTRAS_MARKER, str_of(&constants["marker"]));
    let hud: Map<String, Value> = gp::HUD_PRESETS
        .iter()
        .map(|(id, color)| ((*id).to_owned(), color_json(*color)))
        .collect();
    assert_eq!(
        ordered(&Value::Object(hud)),
        ordered(&constants["hudPresets"])
    );
    let cross: Map<String, Value> = gp::CROSS_PRESETS
        .iter()
        .map(|(id, preset)| {
            let value = preset.as_ref().map_or(Value::Null, |preset| {
                json!({
                    "crossEnabled": preset.cross_enabled,
                    "crossColor": color_json(preset.cross_color),
                    "crossAlpha": preset.cross_alpha,
                    "thickness": preset.thickness,
                    "size": preset.size,
                    "gap": preset.gap,
                    "outline": preset.outline,
                    "outlineThickness": preset.outline_thickness,
                    "circleRadius": preset.circle_radius,
                    "dotColor": preset.dot_color.map_or(Value::Null, color_json),
                    "dotSize": preset.dot_size,
                    "topLine": preset.top_line,
                    "bottomLine": preset.bottom_line,
                    "leftLine": preset.left_line,
                    "rightLine": preset.right_line,
                    "hideDefault": preset.hide_default,
                })
            });
            ((*id).to_owned(), value)
        })
        .collect();
    assert_eq!(
        ordered(&Value::Object(cross)),
        ordered(&constants["crossPresets"])
    );
    assert_eq!(
        ordered(&gp::default_personalization().to_value()),
        ordered(&expected["default"])
    );
}

#[test]
fn sanitize_personalization_and_cfg_match_node() {
    let mut m = Mismatches::default();
    for (at, case) in items(&FIXTURE["personalization"]["sanitize"])
        .iter()
        .enumerate()
    {
        let input = &case["input"];
        m.check(
            &ordered_value(&gp::sanitize_personalization(input).to_value()),
            &ordered_value(&case["sanitized"]),
            format!("случай {at}: sanitizePersonalization({input})"),
        );
        m.check_eq(
            sha256(&gp::to_cfg_lines(input)),
            str_of(&case["cfgSha256"]).to_owned(),
            format!("случай {at}: toCfgLines({input})"),
        );
    }
    m.finish("sanitizePersonalization");
}

#[test]
fn single_personalization_fields_are_normalized_like_node() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["personalization"]["probes"]) {
        let input = match case[0].as_str() {
            Some(key) => json!({ key: case[1].clone() }),
            None => json!({}),
        };
        let clean = gp::sanitize_personalization(&input).to_value();
        let actual = match case[0].as_str() {
            Some(key) => clean[key].clone(),
            None => clean,
        };
        m.check(
            &ordered_value(&actual),
            &ordered_value(&case[2]),
            format!("sanitizePersonalization({input})"),
        );
        m.check_eq(
            sha256(&gp::to_cfg_lines(&input)),
            str_of(&case[3]).to_owned(),
            format!("toCfgLines({input})"),
        );
    }
    m.finish("поля персонализации по одному");
}

#[test]
fn cfg_text_matches_node_byte_for_byte() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["personalization"]["cfg"]) {
        m.check_eq(
            gp::to_cfg_lines(&case["input"]),
            str_of(&case["cfg"]).to_owned(),
            format!("toCfgLines({})", case["input"]),
        );
    }
    m.finish("toCfgLines");
}

#[test]
fn presets_match_node_including_inherited_names() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["personalization"]["preset"]) {
        let id = str_of(&case["id"]);
        m.check(
            &ordered_value(&gp::apply_preset(&case["base"], id).to_value()),
            &ordered_value(&case["result"]),
            format!("applyPreset({}, {id:?})", case["base"]),
        );
    }
    m.finish("applyPreset");
}

#[test]
fn default_autoexec_matches_node_on_both_platforms() {
    let expected = &FIXTURE["autoexec"];
    let linux = default_autoexec(Platform::Linux);
    assert_eq!(linux, str_of(&expected["linux"]));
    assert_eq!(sha256(&linux), str_of(&expected["linuxSha256"]));
    let windows = default_autoexec(Platform::Win32);
    assert_eq!(windows.encode_utf16().count(), expected["win32"]["length"]);
    assert_eq!(sha256(&windows), str_of(&expected["win32"]["sha256"]));
    let line = windows
        .split("\r\n")
        .find(|line| line.starts_with("bind h "))
        .unwrap();
    assert_eq!(line, str_of(&expected["win32"]["line"]));
}

// -------------------------------------------------------------------- source-screen

fn resolution_json(found: Option<source_screen::Resolution>) -> Value {
    found.map_or(
        Value::Null,
        |found| json!({"width": found.width, "height": found.height}),
    )
}

#[test]
fn user_reg_and_reg_query_are_read_like_node() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["screen"]["userReg"]) {
        let (text, game_mod) = (str_of(&case[0]), str_of(&case[1]));
        m.check(
            &resolution_json(source_screen::from_user_reg(text, game_mod)),
            &case[2],
            format!("fromUserReg({text:?}, {game_mod:?})"),
        );
    }
    for case in items(&FIXTURE["screen"]["regQuery"]) {
        let text = str_of(&case[0]);
        m.check(
            &resolution_json(source_screen::from_reg_query(text)),
            &case[1],
            format!("fromRegQuery({text:?})"),
        );
    }
    m.finish("разрешение экрана Source");
}
