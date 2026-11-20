//! Настройки HL1 и паки против 0.4.7: фикстура `goldsrc-settings.json` из
//! `tools/reference/delivery/goldsrc-settings.js`. Чистые функции: схема (`shared/goldsrc-settings.js`)
//! и нормализация (`sanitizeExtras`, `resetExtras`, `toFlat`, `fromFlat`, `available`),
//! `goldsrc-personalization.js` (`defaultBxt`, `sanitizeBxt`, `toCfgLines`), `hl1-default-userconfig.js`,
//! чистые части `goldsrc-packs.js` (каталог паков, `sanitizePacks`, `mergeCrosshairLines`) и
//! `shared/hl1-pack-preview.js` на настоящем `packs.json` предпросмотра. Порядок ключей объектов
//! сверяется по записи `JSON.stringify`.

mod support;

use hlsr_core::js;
use hlsr_core::json_text::{js_number, stringify};
use hlsr_delivery::goldsrc_packs as packs;
use hlsr_delivery::goldsrc_personalization as personalization;
use hlsr_delivery::goldsrc_settings::{self as gs, Field, FieldDefault, FieldType};
use hlsr_delivery::hl1_default_userconfig as userconfig;
use hlsr_delivery::hl1_pack_preview as preview;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("goldsrc-settings.json"));

/// Запись `JSON.stringify` без отступов: порядок ключей важен.
fn ordered(value: &Value) -> String {
    stringify(value, "")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn default_json(default: &FieldDefault) -> Value {
    match default {
        FieldDefault::Bool(value) => json!(value),
        FieldDefault::Number(value) => js_number(*value),
        FieldDefault::Text(value) => json!(value),
        FieldDefault::Color(_) => panic!("в схеме HL1 цветов нет"),
    }
}

/// Поле схемы в виде объекта 0.4.7 (порядок ключей не важен: объекты сравниваются как есть).
fn field_json(field: &Field) -> Value {
    let mut object = Map::new();
    object.insert("key".into(), json!(field.key));
    match &field.kind {
        FieldType::Boolean => object.insert("type".into(), json!("boolean")),
        FieldType::Number { min, max, step } => {
            object.insert("min".into(), js_number(*min));
            object.insert("max".into(), js_number(*max));
            object.insert("step".into(), js_number(*step));
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
        other => panic!("в схеме HL1 нет поля {other:?}"),
    };
    object.insert("default".into(), default_json(&field.default));
    object.insert("label".into(), json!(field.label));
    if let Some(hint) = field.hint {
        object.insert("hint".into(), json!(hint));
    }
    if let Some(requires) = field.requires {
        object.insert("requires".into(), json!(requires));
    }
    if let Some(value) = field.requires_value {
        object.insert("requiresValue".into(), json!(value));
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
        let mut actual = json!({
            "id": section.id,
            "title": section.title,
            "icon": section.icon,
        });
        if let Some(link) = section.link {
            actual["link"] = json!(link);
        }
        actual["fields"] = Value::Array(section.fields.iter().map(field_json).collect());
        m.check(&actual, wanted, format!("раздел {}", section.id));
    }
    let keys: Vec<&str> = gs::fields().map(|field| field.key).collect();
    let wanted: Vec<&str> = items(&expected["fields"]).iter().map(str_of).collect();
    m.check_eq(keys, wanted, "порядок FIELDS");
    let bxt: Vec<&str> = gs::bxt_keys().collect();
    let wanted: Vec<&str> = items(&expected["bxtKeys"]).iter().map(str_of).collect();
    m.check_eq(bxt, wanted, "BXT_KEYS");
    m.check_eq(
        ordered(&gs::defaults().to_value()),
        ordered(&expected["defaults"]),
        "defaults()",
    );
    let list = |names: &[&str]| -> Value { json!(names) };
    m.check(
        &list(&gs::CHANGER_SHAPES),
        &expected["changerShapes"],
        "CHANGER_SHAPES",
    );
    m.check(
        &list(&gs::CHANGER_COLORS),
        &expected["changerColors"],
        "CHANGER_COLORS",
    );
    m.check(&list(&gs::HUD_PACKS), &expected["hudPacks"], "HUD_PACKS");
    m.check(
        &list(&gs::CROSSHAIR_PACKS),
        &expected["crosshairPacks"],
        "CROSSHAIR_PACKS",
    );
    m.finish("схема HL1");
}

#[test]
fn sanitize_extras_on_probes_of_every_field() {
    let mut m = Mismatches::default();
    let cases = items(&FIXTURE["sanitize"]);
    assert!(cases.len() > 1000, "корпус проб: {}", cases.len());
    for case in cases {
        let key = str_of(&case["input"][0]);
        let input = json!({ key: case["input"][1].clone() });
        let actual = ordered(&gs::sanitize_extras(&input).to_value());
        m.check_eq(
            actual,
            str_of(&case["result"]).to_owned(),
            format!("{input}"),
        );
    }
    m.finish("sanitizeExtras: пробы полей");
}

#[test]
fn sanitize_and_reset_of_objects() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["objects"]) {
        let input = &case["input"];
        let extras = gs::sanitize_extras(input);
        m.check_eq(
            ordered(&extras.to_value()),
            str_of(&case["extras"]).to_owned(),
            format!("sanitizeExtras({input})"),
        );
        m.check_eq(
            ordered(&gs::reset_extras(input).to_value()),
            str_of(&case["reset"]).to_owned(),
            format!("resetExtras({input})"),
        );
        // sanitizeExtras идемпотентна.
        m.check_eq(
            ordered(&gs::sanitize_extras(&extras.to_value()).to_value()),
            ordered(&extras.to_value()),
            format!("sanitizeExtras(sanitizeExtras({input}))"),
        );
    }
    m.finish("sanitizeExtras / resetExtras: объекты");
}

#[test]
fn available_follows_the_chain_of_requires() {
    let mut m = Mismatches::default();
    let cases = items(&FIXTURE["available"]);
    assert!(cases.len() > 100);
    for case in cases {
        let field = gs::field(str_of(&case["field"])).expect("поле схемы");
        m.check_eq(
            gs::available(field, &case["values"]),
            case["result"].as_bool().unwrap(),
            format!("{} при {}", field.key, case["values"]),
        );
    }
    m.finish("available");
}

#[test]
fn flat_form_of_the_window_and_the_two_stored_fields() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["flat"]) {
        m.check_eq(
            ordered(&gs::to_flat(&case["packs"], &case["bxt"]).to_value()),
            str_of(&case["result"]).to_owned(),
            format!("toFlat({}, {})", case["packs"], case["bxt"]),
        );
    }
    for case in items(&FIXTURE["fromFlat"]) {
        m.check_eq(
            ordered(&gs::from_flat(&case["input"]).to_value()),
            str_of(&case["result"]).to_owned(),
            format!("fromFlat({})", case["input"]),
        );
    }
    m.finish("toFlat / fromFlat");
}

#[test]
fn bxt_defaults_and_sanitizing() {
    let mut m = Mismatches::default();
    m.check_eq(
        ordered(&personalization::default_bxt().to_value()),
        str_of(&FIXTURE["bxt"]["defaults"]).to_owned(),
        "defaultBxt()",
    );
    for case in items(&FIXTURE["bxt"]["sanitize"]) {
        m.check_eq(
            ordered(&personalization::sanitize_bxt(&case["input"]).to_value()),
            str_of(&case["result"]).to_owned(),
            format!("sanitizeBxt({})", case["input"]),
        );
    }
    m.finish("BXT");
}

#[test]
fn cfg_lines_are_the_same_bytes() {
    let mut m = Mismatches::default();
    let cases = items(&FIXTURE["cfg"]);
    assert!(cases.len() > 300, "корпус настроек: {}", cases.len());
    for case in cases {
        let text = personalization::to_cfg_lines(&case["input"]);
        let input = &case["input"];
        m.check_eq(
            (text.encode_utf16().count() as u64, sha256(text.as_bytes())),
            (
                case["length"].as_u64().unwrap(),
                str_of(&case["sha256"]).to_owned(),
            ),
            format!("toCfgLines({input})"),
        );
        if let Some(wanted) = case.get("text") {
            m.check_eq(
                text.as_str(),
                str_of(wanted),
                format!("текст toCfgLines({input})"),
            );
        }
    }
    m.finish("goldsrc toCfgLines");
}

#[test]
fn hud_alpha_goes_into_the_colour_for_every_pair() {
    // Сводка sha256 по третьей строке файла (bxt_hud_color…) для всех 65 536 пар (alpha, c).
    let mut lines = Vec::with_capacity(65_536);
    for alpha in 0..=255u32 {
        for c in 0..=255u32 {
            let settings = json!({"personalization": {
                "hudEnabled": true,
                "hudAlpha": alpha,
                "hudColor": {"r": c, "g": 255 - c, "b": (c * 7) % 256},
            }});
            let text = personalization::to_cfg_lines(&settings);
            lines.push(text.split("\r\n").nth(2).unwrap().to_owned());
        }
    }
    let expected = &FIXTURE["hudAlpha"];
    assert_eq!(lines.len() as u64, expected["pairs"].as_u64().unwrap());
    assert_eq!(
        sha256(lines.join("\n").as_bytes()),
        str_of(&expected["sha256"])
    );
}

#[test]
fn panel_anchors_are_percent_over_one_hundred_in_js_notation() {
    for case in items(&FIXTURE["anchors"]) {
        let x = case[0].as_i64().unwrap();
        let text = personalization::to_cfg_lines(
            &json!({"bxt": {"speedometerX": x, "speedometerY": 100 - x, "timerX": x}}),
        );
        let line = |prefix: &str| {
            text.split("\r\n")
                .find(|line| line.starts_with(prefix))
                .map(str::to_owned)
        };
        assert_eq!(
            line("bxt_hud_speedometer_anchor").as_deref(),
            case[1].as_str(),
            "speedometerX = {x}"
        );
        assert_eq!(
            line("bxt_hud_timer_anchor").as_deref(),
            case[2].as_str(),
            "timerX = {x}"
        );
    }
}

#[test]
fn constants_of_the_personalization_files() {
    let expected = &FIXTURE["constants"];
    assert_eq!(
        personalization::PERSONALIZATION_FILE,
        str_of(&expected["PERSONALIZATION_FILE"])
    );
    assert_eq!(personalization::EXEC_LINE, str_of(&expected["EXEC_LINE"]));
    assert_eq!(json!(personalization::PANELS), expected["PANELS"]);
}

#[test]
fn default_userconfig_is_the_same_text_and_the_same_hashes() {
    let expected = &FIXTURE["userconfig"];
    assert_eq!(userconfig::text(), str_of(&expected["text"]));
    assert_eq!(userconfig::stock(), str_of(&expected["stock"]));
    assert_eq!(
        json!(userconfig::REPLACEABLE_SHA256),
        expected["replaceable"]
    );
    assert_eq!(
        sha256(&js::latin1_bytes(userconfig::stock())),
        str_of(&expected["stockSha256"])
    );
}

fn pack_json(pack: &packs::Pack) -> Value {
    json!({
        "kind": pack.kind,
        "title": pack.title,
        "author": pack.author,
        "driveId": pack.drive_id,
        "size": pack.size,
        "sha256": pack.sha256,
        "root": pack.root,
    })
}

#[test]
fn packs_catalogue_constants_and_sanitizing() {
    let expected = &FIXTURE["packs"];
    let mut m = Mismatches::default();
    let constants = &expected["constants"];
    m.check_eq(
        packs::DRIVE_DOWNLOAD,
        str_of(&constants["DRIVE_DOWNLOAD"]),
        "DRIVE_DOWNLOAD",
    );
    m.check_eq(
        packs::DOWNLOAD_TIMEOUT.as_millis() as u64,
        constants["DOWNLOAD_TIMEOUT_MS"].as_u64().unwrap(),
        "DOWNLOAD_TIMEOUT_MS",
    );
    m.check_eq(
        packs::STATE_DIR,
        str_of(&constants["STATE_DIR"]),
        "STATE_DIR",
    );
    m.check_eq(
        packs::SOURCE_PAGE,
        str_of(&constants["SOURCE_PAGE"]),
        "SOURCE_PAGE",
    );
    m.check_eq(
        packs::SOURCE_PAGE,
        str_of(&constants["hl1Resources"]),
        "LINKS.hl1Resources",
    );

    let catalogue = packs::catalogue();
    let actual: Map<String, Value> = catalogue
        .iter()
        .map(|(id, pack)| (id.clone(), pack_json(pack)))
        .collect();
    m.check(&Value::Object(actual), &expected["catalogue"], "PACKS");
    m.check_eq(
        ordered(&packs::default_packs().to_value()),
        ordered(&expected["defaultPacks"]),
        "defaultPacks()",
    );
    m.check(&packs::catalog(), &expected["catalog"], "catalog()");
    for case in items(&expected["sanitize"]) {
        m.check_eq(
            ordered(&packs::sanitize_packs(&case["input"]).to_value()),
            str_of(&case["result"]).to_owned(),
            format!("sanitizePacks({})", case["input"]),
        );
    }
    m.finish("goldsrc-packs: каталог");
}

#[test]
fn the_preview_knows_the_same_pack_hashes() {
    // packs.json предпросмотра закрепляет те же sha256, что и установщик паков.
    let sources = &FIXTURE["preview"]["meta"]["sources"];
    for (id, pack) in packs::catalogue() {
        assert_eq!(sources[id.as_str()], json!(pack.sha256), "{id}");
    }
}

#[test]
fn crosshair_lines_of_a_pack_replace_only_the_crosshair_lines() {
    let mut m = Mismatches::default();
    let cases = items(&FIXTURE["packs"]["merge"]);
    assert!(cases.len() > 100);
    for case in cases {
        let merged = packs::merge_crosshair_lines(
            &js::latin1_bytes(str_of(&case["base"])),
            &js::latin1_bytes(str_of(&case["pack"])),
        );
        m.check_eq(
            js::latin1_string(&merged),
            str_of(&case["result"]).to_owned(),
            format!("{} + {}", case["base"], case["pack"]),
        );
    }
    m.finish("mergeCrosshairLines");
}

// ----------------------------------------------------------------- предпросмотр

fn rects_of(value: &Value) -> preview::Rects {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, rect)| {
            let number = |at: usize| rect[at].as_f64().unwrap();
            (name.clone(), [number(0), number(1), number(2), number(3)])
        })
        .collect::<HashMap<_, _>>()
}

fn rect_json(rect: &[f64; 4]) -> Value {
    Value::Array(rect.iter().map(|n| js_number(*n)).collect())
}

fn command_json(command: &preview::Command) -> Value {
    match command {
        preview::Command::Sprite { name, rect, at } => {
            let mut object = Map::new();
            object.insert("name".into(), json!(name));
            if let Some(rect) = rect {
                object.insert("box".into(), rect_json(rect));
            }
            object.insert(
                "at".into(),
                Value::Array(vec![js_number(at[0]), js_number(at[1])]),
            );
            Value::Object(object)
        }
        preview::Command::Fill { at, size } => json!({
            "at": [js_number(at[0]), js_number(at[1])],
            "fill": [js_number(size[0]), js_number(size[1])],
        }),
    }
}

#[test]
fn preview_hud_commands_for_every_size_and_rectangles() {
    let expected = &FIXTURE["preview"];
    let variants = items(&expected["variants"]);
    let mut m = Mismatches::default();
    for case in items(&expected["hud"]) {
        let rects = rects_of(&variants[case["variant"].as_u64().unwrap() as usize]);
        let (width, height) = match (case["width"].as_f64(), case["height"].as_f64()) {
            (Some(width), Some(height)) => (width, height),
            _ => (preview::DEFAULT_WIDTH, preview::DEFAULT_HEIGHT),
        };
        let actual = preview::hud_commands(&rects, width, height).expect("сцена строится");
        let actual = Value::Array(actual.iter().map(command_json).collect());
        m.check(
            &actual,
            &case["commands"],
            format!("вариант {} при {width}x{height}", case["variant"]),
        );
    }
    m.finish("hudCommands");
}

#[test]
fn preview_hud_without_a_required_rectangle_fails_like_node() {
    let expected = &FIXTURE["preview"];
    let meta = rects_of(&expected["meta"]["hud"]);
    for case in items(&expected["missing"]) {
        let name = str_of(&case["without"]);
        let mut rects = meta.clone();
        rects.remove(name);
        let result = preview::hud_commands(&rects, preview::DEFAULT_WIDTH, preview::DEFAULT_HEIGHT);
        match case.get("error") {
            Some(error) => {
                assert_eq!(str_of(&error["type"]), "TypeError", "{name}");
                assert_eq!(
                    result.unwrap_err().message,
                    str_of(&error["message"]),
                    "без {name}"
                );
            }
            None => {
                let actual = Value::Array(result.expect(name).iter().map(command_json).collect());
                assert_eq!(actual, case["commands"], "без {name}");
            }
        }
    }
}

fn meta_of(value: &Value) -> preview::PackMeta {
    let strings = |list: &Value| -> Vec<String> {
        items(list).iter().map(|s| str_of(s).to_owned()).collect()
    };
    preview::PackMeta {
        shapes: strings(&value["shapes"]),
        colors: strings(&value["colors"]),
        changer_cell: value["changerCell"].as_f64().unwrap(),
        changer_inset: value["changerInset"].as_f64().unwrap(),
        changer_size: value["changerSize"].as_f64().unwrap(),
    }
}

#[test]
fn preview_crosshair_layer_for_every_shape_and_colour() {
    let expected = &FIXTURE["preview"];
    let meta = meta_of(&expected["meta"]);
    let mut m = Mismatches::default();
    for case in items(&expected["layers"]) {
        let used = case.get("metadata").map_or_else(|| meta.clone(), meta_of);
        let actual = match preview::crosshair_layer(&case["values"], &used) {
            Some(layer) => json!({"image": layer.image, "box": rect_json(&layer.rect)}),
            None => Value::Null,
        };
        m.check(&actual, &case["layer"], format!("{}", case["values"]));
    }
    m.finish("crosshairLayer");
}
