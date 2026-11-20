//! Логика «Персонализации» (задачи 30–32) против 0.4.7: фикстура
//! `fixtures/personalization-logic.json` снята `tools/reference/export-personalization-logic.js`
//! с настоящих скриптов `PersonalizationPanel.vue`, `GhostingSettingsExtras.vue`,
//! `GhostingPersonalization.vue` и `LiveSplitOverlayEditor.vue` на корпусе входов: черновики и
//! их снимки, значение редактора HL1, сброс, возврат, импорт кода и сохранение; разделы,
//! resolved, неверные поля, счёт изменений, ползунки (детент, шаг, заливка), колонки, клавиши
//! списка; HSV-круг (битмап и цвет по точке), клавиши круга, пресеты, HEX и каналы; источник
//! оверлея LiveSplit, его ползунки, изменения и правки.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hlsr_ui::controllers::hud_editor::{self, HudEditor};
use hlsr_ui::controllers::livesplit_editor::{self, LivesplitEditor, Tone};
use hlsr_ui::controllers::personalization_panel::{self as panel, PersonalizationPanel};
use hlsr_ui::controllers::settings_extras::{self as extras, ExtrasEditor, Schema};
use hlsr_ui::render::hsv;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn fixture() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/personalization-logic.json");
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn str_of(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// `"45%"` → 45.0.
fn percent(v: &Value) -> f64 {
    str_of(v).trim_end_matches('%').parse().unwrap()
}

/// Равны с допуском; `NaN` — только `NaN` (`"NaN%"` у ползунка с неверным значением).
fn close(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || (a - b).abs() < 1e-9
}

/// Ответ `game-settings-get` из базы сцен (та же, что у скрипта фикстуры).
fn base_settings(id: &str) -> Value {
    let base: Value = serde_json::from_str(
        &std::fs::read_to_string(hlsr_ui::fixtures::screens_dir().join("_base.json")).unwrap(),
    )
    .unwrap();
    base["invoke"]["game-settings-get"]["$args"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case[0][0] == id)
        .map(|case| case[1].clone())
        .unwrap()
}

fn loaded_panel(id: &str) -> PersonalizationPanel {
    let mut p = PersonalizationPanel::new("hlsr");
    assert_eq!(p.set_active_tab(id), Some(id.to_owned()));
    p.loaded(id, Ok(base_settings(id)));
    p
}

// ---- PersonalizationPanel ----

#[test]
fn drafts_match_the_renderer_byte_for_byte() {
    let f = fixture();
    for load in f["panel"]["loads"].as_array().unwrap() {
        let id = str_of(&load["id"]);
        let p = loaded_panel(id);
        let draft = p.draft().unwrap();
        assert_eq!(draft.look, load["draft"]["look"], "{id}: look");
        assert_eq!(draft.saved, str_of(&load["draft"]["saved"]), "{id}: снимок");
        assert_eq!(p.is_dirty(id), load["dirty"].as_bool().unwrap());
        assert_eq!(p.is_goldsrc(), load["isGoldSrc"].as_bool().unwrap());
        assert_eq!(
            p.editor_value().unwrap(),
            load["editorValue"],
            "{id}: editorValue"
        );
        let tabs: Vec<&str> = p.tabs().iter().map(|t| t.id).collect();
        let expected: Vec<&str> = load["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| str_of(&t["id"]))
            .collect();
        assert_eq!(tabs, expected);
    }
}

#[test]
fn editor_input_splits_hl1_packs_and_bxt_like_the_renderer() {
    let f = fixture();
    let cases = f["panel"]["editor"].as_array().unwrap();
    // Ghosting: всё значение — в personalization.
    let mut p = loaded_panel("218");
    let mut value = p.editor_value().unwrap();
    value["hudEnabled"] = json!(true);
    value["hudColor"] = json!({"r": 1, "g": 2, "b": 3});
    value["extra"]["gh_hud_bg"] = json!(false);
    p.on_editor_input(value);
    assert_eq!(p.draft().unwrap().look, cases[0]["look"]);
    assert!(p.is_dirty("218"));
    // HL1: extra раскладывается в packs и bxt; personalization.extra остаётся прежним.
    let mut p = loaded_panel("70");
    let mut value = p.editor_value().unwrap();
    value["crossEnabled"] = json!(true);
    value["extra"]["packHud"] = json!("ag-hud");
    value["extra"]["timer"] = json!(true);
    value["extra"]["timerX"] = json!(25);
    value["extra"]["changerShape"] = json!("3px");
    p.on_editor_input(value);
    assert_eq!(p.draft().unwrap().look, cases[1]["look"]);
    assert_eq!(p.editor_value().unwrap(), cases[1]["editorValue"]);
    p.on_overlay_input(json!({"enabled": true, "x": 10, "y": 20}));
    assert_eq!(p.draft().unwrap().look, cases[2]["look"]);
    assert!(p.is_dirty("70"));
}

#[test]
fn reset_revert_import_and_save_follow_the_renderer() {
    let f = fixture();
    for (reset, revert) in f["panel"]["reset"]
        .as_array()
        .unwrap()
        .iter()
        .zip(f["panel"]["revert"].as_array().unwrap())
    {
        let id = str_of(&reset["id"]);
        let mut p = loaded_panel(id);
        p.on_overlay_input(json!({"enabled": true, "x": 5, "y": 5, "key": "color"}));
        if id == "70" {
            let mut value = p.editor_value().unwrap();
            value["extra"]["packHud"] = json!("ag-hud");
            p.on_editor_input(value);
        }
        p.reset_game();
        assert_eq!(p.draft().unwrap().look, reset["look"], "{id}: сброс");
        assert_eq!(p.is_dirty(id), reset["dirty"].as_bool().unwrap());
        p.revert_game();
        assert_eq!(p.draft().unwrap().look, revert["look"], "{id}: возврат");
        assert_eq!(p.is_dirty(id), revert["dirty"].as_bool().unwrap());
    }
    let import = f["panel"]["import"].as_array().unwrap();
    let mut p = loaded_panel("218");
    let mut personalization = p.draft().unwrap().look["personalization"].clone();
    personalization["hudEnabled"] = json!(true);
    assert!(p.import_code_look("218", &json!({"personalization": personalization})));
    assert_eq!(p.draft().unwrap().look, import[0]["look"]);
    assert_eq!(p.is_dirty("218"), import[0]["dirty"].as_bool().unwrap());
    assert_eq!(p.code_imported, str_of(&import[0]["codeImported"]));
    assert!(!p.import_code_look("70", &json!({"personalization": {}})));
    assert_eq!(p.draft().unwrap().look, import[1]["look"]);

    let save = &f["panel"]["save"][0];
    let mut p = loaded_panel("218");
    let mut value = p.editor_value().unwrap();
    value["hudEnabled"] = json!(true);
    p.on_editor_input(value);
    let (id, payload) = p.begin_save().unwrap();
    let call = &save["calls"][0];
    assert_eq!(call["channel"], "game-settings-save");
    assert_eq!(call["args"][0], id);
    assert_eq!(call["args"][1], payload);
    assert_eq!(call["args"][2], "personalization");
    let mut settings = base_settings("218")["settings"].clone();
    for (key, value) in payload.as_object().unwrap() {
        settings[key] = value.clone();
    }
    assert!(p.saved(
        &id,
        payload,
        Ok(json!({"status": "ok", "settings": settings}))
    ));
    let draft = p.draft().unwrap();
    assert_eq!(draft.look, save["draft"]["look"]);
    assert_eq!(draft.saved, str_of(&save["draft"]["saved"]));
    assert_eq!(draft.settings, save["draft"]["settings"]);
    assert_eq!(p.is_dirty("218"), save["dirty"].as_bool().unwrap());
    assert_eq!(p.just_saved, str_of(&save["justSaved"]));
}

#[test]
fn normalize_and_pick_look_handle_partial_settings_like_the_renderer() {
    // Частичные настройки: умолчания под значениями, цвета поканально, лишние ключи остаются.
    let partial = json!({
        "personalization": {"hudEnabled": true, "hudColor": {"r": 10}, "crossPreset": "dot", "extra": {"gh_hud_bg": false, "stray": 1}},
        "liveSplitOverlay": {"enabled": true, "x": 200, "key": "luma", "crop": {"top": "4"}}
    });
    let look = panel::pick_look(Some(&partial));
    assert_eq!(
        look["personalization"]["hudColor"],
        json!({"r": 10, "g": 220, "b": 0})
    );
    assert_eq!(look["personalization"]["crossPreset"], "dot");
    assert_eq!(look["personalization"]["extra"]["stray"], 1);
    assert_eq!(look["packs"], Value::Null);
    assert_eq!(look["liveSplitOverlay"]["x"], 100);
    assert_eq!(look["liveSplitOverlay"]["key"], "luma");
    assert_eq!(look["liveSplitOverlay"]["crop"]["top"], 4);
    assert_eq!(
        panel::pick_look(None)["personalization"],
        hlsr_delivery::ghosting_personalization::default_personalization().to_value()
    );
    // Порядок ключей снимка — как у JS: умолчания, затем новые ключи.
    let keys: Vec<&str> = look["personalization"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys[0], "hudEnabled");
    assert_eq!(*keys.last().unwrap(), "extra");
}

// ---- GhostingSettingsExtras ----

fn extras_editor(schema: Schema, value: Value) -> ExtrasEditor {
    let mut e = ExtrasEditor::new(
        schema,
        if schema == Schema::Goldsrc {
            "70"
        } else {
            "218"
        },
        value,
        Some(json!({"enabled": false})),
    );
    e.has_code_slot = false;
    e
}

fn check_fields(e: &mut ExtrasEditor, expected: &Value, name: &str) {
    for (section_id, section) in expected["fields"].as_object().unwrap() {
        e.select_section(section_id);
        // 0.4.9: у «Призраков» своя сцена превью.
        assert_eq!(
            e.has_preview(),
            section["hasPreview"].as_bool().unwrap() || section_id == "ghosts",
            "{name}/{section_id}: превью"
        );
        let columns: Vec<Vec<&str>> = e
            .field_columns()
            .iter()
            .map(|c| c.iter().map(|f| f.key).collect())
            .collect();
        let mut expected_columns: Vec<Vec<&str>> = section["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_array().unwrap().iter().map(str_of).collect())
            .collect();
        // С превью — одной колонкой.
        if section_id == "ghosts" {
            expected_columns = vec![expected_columns.concat()];
        }
        assert_eq!(columns, expected_columns, "{name}/{section_id}: колонки");
        for row in section["rows"].as_array().unwrap() {
            let key = str_of(&row["key"]);
            let field = e.schema.field(key).unwrap();
            assert_eq!(
                e.is_changed(&field),
                row["changed"].as_bool().unwrap(),
                "{name}/{key}: changed"
            );
            assert_eq!(
                e.is_available(&field),
                row["available"].as_bool().unwrap(),
                "{name}/{key}: available"
            );
            if let Some(requirement) = row["requirement"].as_str() {
                assert_eq!(
                    e.requirement_label(&field),
                    requirement,
                    "{name}/{key}: зависимость"
                );
            }
            if let Some(has_range) = row["hasRange"].as_bool() {
                assert_eq!(
                    ExtrasEditor::has_range(&field),
                    has_range,
                    "{name}/{key}: hasRange"
                );
            }
            if row["sliderStyle"].is_object() {
                let style = e.slider_style(&field);
                let s = &row["sliderStyle"];
                assert!(
                    close(style.value, percent(&s["--value"])),
                    "{name}/{key}: value"
                );
                assert!(
                    close(style.default, percent(&s["--default"])),
                    "{name}/{key}: default"
                );
                assert!(
                    close(style.from, percent(&s["--from"])),
                    "{name}/{key}: from"
                );
                assert!(close(style.to, percent(&s["--to"])), "{name}/{key}: to");
            }
            if let Some(kind) = row["assetKind"].as_str() {
                // 0.4.9: своя модель призрака выбирается файлом.
                let kind = if key == "gh_model" { "model" } else { kind };
                assert_eq!(ExtrasEditor::asset_kind(&field), kind, "{name}/{key}: файл");
            }
            if let Some(label) = row["selectLabel"].as_str() {
                assert_eq!(
                    e.select_label(&field),
                    label,
                    "{name}/{key}: подпись варианта"
                );
            }
            if let Some(hex) = row["hex"].as_str() {
                assert_eq!(
                    ExtrasEditor::to_hex(&e.resolved_value(key)),
                    hex,
                    "{name}/{key}: hex"
                );
            }
        }
    }
}

#[test]
fn ghosting_sections_resolved_values_and_changes_match_the_renderer() {
    let f = fixture();
    let defaults = hlsr_delivery::ghosting_personalization::default_personalization().to_value();
    for case in f["extras"]["ghosting"].as_array().unwrap() {
        let name = str_of(&case["name"]);
        let mut value = defaults.clone();
        value["extra"] = case["resolved"].clone();
        // resolved в фикстуре — defaults + extra; extra восстанавливается из входа корпуса.
        let extra: Value = match name {
            "defaults" => json!({}),
            "changed" => {
                json!({"gh_hud_bg": false, "gh_hud_fontscale": 1.5, "gh_health_align": "3", "gh_spray_file": "C:\\spray.png", "gh_hud_font": "Arial"})
            }
            "invalid" => {
                json!({"gh_hud_fontscale": "abc", "gh_hud_font": "bad\"quote", "gh_health_x": 9999})
            }
            "off-chain" => json!({"gh_hud_font": "", "gh_hud_fontscale": 2}),
            other => panic!("случай {other}"),
        };
        value["extra"] = extra;
        let mut e = extras_editor(Schema::Ghosting, value);
        assert_eq!(e.resolved(), case["resolved"], "{name}: resolved");
        let invalid: BTreeSet<String> = case["invalidFields"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(e.invalid_fields(), invalid, "{name}: неверные поля");
        assert_eq!(e.valid(), case["valid"].as_bool().unwrap());
        // Список разделов: как в 0.4.7; «Моды» и подпись первого — 0.4.9.
        let items = without_mods(e.nav_items());
        let expected: Vec<&Value> = case["navItems"].as_array().unwrap().iter().collect();
        assert_eq!(items.len(), expected.len(), "{name}: разделы");
        for (item, want) in items.iter().zip(expected) {
            assert_eq!(item.id, str_of(&want["id"]));
            let title = match str_of(&want["title"]) {
                "#L_GS_S_PERSONAL" => "#L_GS_S_LOOK",
                other => other,
            };
            assert_eq!(item.title, title);
            assert_eq!(item.icon, str_of(&want["icon"]), "{}: значок", item.id);
            assert_eq!(
                item.changes,
                want["changes"].as_u64().unwrap() as usize,
                "{}: изменения",
                item.id
            );
        }
        check_fields(&mut e, case, name);
        // Сброс раздела hud.
        e.select_section("hud");
        let reset = e.reset_section().unwrap();
        assert_eq!(reset, case["resetHud"][0]["payload"], "{name}: сброс hud");
    }
}

#[test]
fn hl1_sections_match_the_renderer() {
    let f = fixture();
    let defaults = hlsr_delivery::ghosting_personalization::default_personalization().to_value();
    for case in f["extras"]["goldsrc"].as_array().unwrap() {
        let name = str_of(&case["name"]);
        let (packs, bxt) = match name {
            "defaults" => (Value::Null, Value::Null),
            "changer" => (
                json!({"hud": "ag-hud", "crosshair": "crosshair-changer", "changerShape": "3px", "changerColor": "red"}),
                json!({"timer": true, "timerX": 25, "fov": 110, "precision": 3}),
            ),
            other => panic!("случай {other}"),
        };
        let mut value = defaults.clone();
        value["extra"] = hlsr_delivery::goldsrc_settings::to_flat(&packs, &bxt).to_value();
        let mut e = extras_editor(Schema::Goldsrc, value);
        assert_eq!(e.resolved(), case["resolved"], "{name}: resolved");
        assert!(e.invalid_fields().is_empty());
        let items = without_mods(e.nav_items());
        let expected = case["navItems"].as_array().unwrap();
        assert_eq!(items.len(), expected.len());
        for (item, want) in items.iter().zip(expected) {
            assert_eq!(item.id, str_of(&want["id"]));
            assert_eq!(item.icon, str_of(&want["icon"]));
            assert_eq!(
                item.changes,
                want["changes"].as_u64().unwrap() as usize,
                "{}: изменения",
                item.id
            );
            let link = e.active_section_link(item.id);
            assert_eq!(link, want["link"].as_str(), "{}: ссылка", item.id);
        }
        check_fields(&mut e, case, name);
    }
}

/// Список разделов без «Модов» (их нет в фикстуре 0.4.7).
fn without_mods(items: Vec<extras::NavItem>) -> Vec<extras::NavItem> {
    items.into_iter().filter(|i| i.id != extras::MODS).collect()
}

trait SectionLink {
    fn active_section_link(&self, id: &str) -> Option<&'static str>;
}

impl SectionLink for ExtrasEditor {
    fn active_section_link(&self, id: &str) -> Option<&'static str> {
        self.schema
            .sections()
            .into_iter()
            .find(|s| s.id == id)
            .and_then(|s| s.link)
    }
}

#[test]
fn sliders_detent_snap_and_number_inputs_match_the_renderer() {
    let f = fixture();
    let defaults = hlsr_delivery::ghosting_personalization::default_personalization().to_value();
    for case in f["extras"]["sliders"].as_array().unwrap() {
        // Каждый случай корпуса сняли с редактора на умолчаниях (значение между случаями
        // скрипт назад не подставлял).
        let mut e = extras_editor(Schema::Ghosting, defaults.clone());
        let key = str_of(&case["key"]);
        if key == "color" {
            assert_eq!(
                ExtrasEditor::to_hex(&json!({"r": 10, "g": 11, "b": 12})),
                str_of(&case["hex"])
            );
            assert_eq!(ExtrasEditor::to_hex(&Value::Null), str_of(&case["hexBad"]));
            assert_eq!(
                ExtrasEditor::to_hex(&json!({"r": 300, "g": -5, "b": 12.6})),
                str_of(&case["hexClamp"])
            );
            if let Some(field_key) = case["field"].as_str() {
                let field = e.schema.field(field_key).unwrap();
                let next = e.set_color(&field, "#0A0b0C").unwrap();
                assert_eq!(next, case["emitted"][0]["payload"]);
                assert_eq!(e.set_color(&field, "#12"), None);
                assert_eq!(case["emitted"].as_array().unwrap().len(), 1);
            }
            continue;
        }
        let field = e.schema.field(key).unwrap();
        if let Some(raw) = case["raw"].as_f64() {
            let width = case["width"].as_f64().unwrap();
            let detent = ExtrasEditor::detent(&field, raw, width);
            assert!(
                close(detent, case["detent"].as_f64().unwrap()),
                "{key} {raw}: детент"
            );
            assert!(
                close(
                    ExtrasEditor::snap(&field, detent),
                    case["snap"].as_f64().unwrap()
                ),
                "{key} {raw}: шаг"
            );
            assert!(close(
                ExtrasEditor::snap(&field, raw),
                case["snapRaw"].as_f64().unwrap()
            ));
        } else {
            let text = str_of(&case["text"]);
            let (next, shown) = e.set_number(&field, Value::String(text.to_owned()));
            assert_eq!(
                shown,
                str_of(&case["inputAfter"]),
                "{key} «{text}»: поле после"
            );
            let emitted = case["emitted"].as_array().unwrap();
            match next {
                Some(next) => assert_eq!(next, emitted[0]["payload"], "{key} «{text}»"),
                None => assert!(emitted.is_empty(), "{key} «{text}»: не должно меняться"),
            }
            // Как в 0.4.7, окно получает значение обратно (prop value).
            if let Some(payload) = emitted.first().map(|c| c["payload"].clone()) {
                e.set_value(payload);
            }
        }
    }
    // Клавиши списка разделов (со слотом кода, как в панели).
    let nav = &f["extras"]["nav"][0];
    let mut e = extras_editor(
        Schema::Ghosting,
        hlsr_delivery::ghosting_personalization::default_personalization().to_value(),
    );
    e.has_code_slot = true;
    let ids = e.nav_ids();
    // «Моды» 0.4.9 стоят перед оверлеем; клавиши ходят по списку с ними.
    let without: Vec<&str> = ids
        .iter()
        .copied()
        .filter(|id| *id != extras::MODS)
        .collect();
    let expected: Vec<&str> = nav["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(str_of)
        .collect();
    assert_eq!(without, expected);
    for key in ["ArrowDown", "ArrowUp", "End", "Home", "ArrowLeft"] {
        let index = e.nav_key(key).unwrap();
        e.select_section(ids[index]);
        assert_eq!(e.active_id, str_of(&nav[key]), "{key}");
    }
}

// ---- GhostingPersonalization ----

#[test]
fn the_wheel_bitmap_is_the_canvas_of_the_renderer() {
    let f = fixture();
    let wheel = &f["hud"]["wheel"];
    let data = hsv::wheel_bitmap();
    assert_eq!(data.len(), 168 * 168 * 4);
    use sha2::Digest as _;
    let digest: String = sha2::Sha256::digest(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(digest, str_of(&wheel["sha256"]), "битмап круга 168 × 168");
    for sample in wheel["samples"].as_array().unwrap() {
        let (x, y) = (
            sample["x"].as_u64().unwrap() as usize,
            sample["y"].as_u64().unwrap() as usize,
        );
        let at = (y * 168 + x) * 4;
        let rgba: Vec<u8> = sample["rgba"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        assert_eq!(&data[at..at + 4], rgba.as_slice(), "пиксель {x},{y}");
    }
}

fn rgb_of(v: &Value) -> hud_editor::Rgb {
    hud_editor::Rgb::from_value(Some(v))
}

#[test]
fn dragging_the_wheel_gives_the_colours_of_the_renderer() {
    let f = fixture();
    let defaults = hlsr_delivery::ghosting_personalization::default_personalization().to_value();
    for point in f["hud"]["points"].as_array().unwrap() {
        let mut value = defaults.clone();
        value["hudColor"] = point["start"].clone();
        let mut hud = HudEditor::new(value, false);
        let next = hud.wheel_to_color(
            point["x"].as_f64().unwrap(),
            point["y"].as_f64().unwrap(),
            108.0,
            108.0,
        );
        assert!(
            close(hud.wheel.h, point["wheel"]["h"].as_f64().unwrap()),
            "{point}: h"
        );
        assert!(
            close(hud.wheel.s, point["wheel"]["s"].as_f64().unwrap()),
            "{point}: s"
        );
        assert_eq!(next, point["emitted"][0]["payload"], "{point}: цвет");
        let (left, top) = hud.wheel_cursor();
        assert!(
            close(left * 100.0, percent(&point["cursor"]["left"])),
            "{point}: курсор x"
        );
        assert!(
            close(top * 100.0, percent(&point["cursor"]["top"])),
            "{point}: курсор y"
        );
    }
    for case in f["hud"]["keys"].as_array().unwrap() {
        let mut value = defaults.clone();
        value["hudColor"] = case["start"].clone();
        let mut hud = HudEditor::new(value, false);
        let keys: Vec<&str> = str_of(&case["key"]).split(',').collect();
        let mut emitted = Vec::new();
        for key in &keys {
            // Корпус снят без возврата значения в prop value: круг остаётся на
            // посчитанном оттенке, а не на оттенке округлённого цвета.
            if let Some(next) = hud.on_wheel_key(key) {
                emitted.push(next);
            }
        }
        if let Some(prevented) = case["prevented"].as_bool() {
            assert_eq!(!emitted.is_empty(), prevented, "{}", case["key"]);
        }
        assert!(
            close(hud.wheel.h, case["wheel"]["h"].as_f64().unwrap()),
            "{}: h",
            case["key"]
        );
        assert!(
            close(hud.wheel.s, case["wheel"]["s"].as_f64().unwrap()),
            "{}: s",
            case["key"]
        );
        let expected: Vec<Value> = case["emitted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["payload"].clone())
            .collect();
        assert_eq!(emitted, expected, "{}", case["key"]);
    }
}

#[test]
fn hud_editor_methods_match_the_renderer() {
    let f = fixture();
    let defaults = hlsr_delivery::ghosting_personalization::default_personalization().to_value();
    let swatches = hud_editor::swatches();
    for case in f["hud"]["methods"].as_array().unwrap() {
        let name = str_of(&case["name"]);
        let mut value = defaults.clone();
        for (key, v) in case["patch"].as_object().unwrap() {
            value[key] = v.clone();
        }
        let hud = HudEditor::new(value, case["goldsrc"].as_bool().unwrap());
        let emitted = case["emitted"].as_array().unwrap();
        let first = emitted.first().map(|e| e["payload"].clone());
        let result = &case["result"];
        match name {
            "initial" => {
                assert_eq!(hud.hud_hex(), str_of(&result["hudHex"]));
                assert_eq!(hud.cross_hex(), str_of(&result["crossHex"]));
                assert_eq!(hud.dot_hex(), str_of(&result["dotHex"]));
                assert_eq!(hud.hud_alpha_label(), None);
                let (left, top) = hud.wheel_cursor();
                assert!(close(left * 100.0, percent(&result["cursor"]["left"])));
                assert!(close(top * 100.0, percent(&result["cursor"]["top"])));
                for (swatch, want) in swatches.iter().zip(result["presets"].as_array().unwrap()) {
                    assert_eq!(swatch.id, str_of(&want["id"]));
                    assert_eq!(swatch.label, str_of(&want["label"]));
                    assert_eq!(hud.is_hud_preset(swatch), want["active"].as_bool().unwrap());
                    assert_eq!(
                        hud_editor::rgb_to_hex(swatch.color),
                        str_of(&want["css"]).to_lowercase()
                    );
                }
            }
            "initial-black" => {
                assert_eq!(hud.hud_hex(), str_of(&result["hudHex"]));
                assert_eq!(
                    hud.hud_alpha_label().as_deref(),
                    Some(str_of(&result["hudAlphaLabel"]))
                );
                let (left, top) = hud.wheel_cursor();
                assert!(close(left * 100.0, percent(&result["cursor"]["left"])));
                assert!(close(top * 100.0, percent(&result["cursor"]["top"])));
            }
            "alpha-goldsrc-0" | "alpha-ghosting-0" | "alpha-goldsrc-5" => {
                let label = hud
                    .hud_alpha_label()
                    .unwrap_or_else(|| "#L_GS_P_AUTO".to_owned());
                assert_eq!(label, str_of(result), "{name}");
            }
            "affects" => {
                assert_eq!(hud.cross_hex(), str_of(&result["crossHex"]));
                assert_eq!(hud.dot_hex(), str_of(&result["dotHex"]));
            }
            "dot-color" => assert_eq!(hud.dot_hex(), str_of(result)),
            "onHudHex-ok" => assert_eq!(hud.on_hud_hex(" #0A0b0C "), first),
            "onHudHex-bad" => assert_eq!(hud.on_hud_hex("#12"), first),
            "onHudHex-nohash" => assert_eq!(hud.on_hud_hex("abcdef"), first),
            "onHudChannel" => assert_eq!(Some(hud.on_hud_channel("g", "300")), first),
            "onHudChannel-nan" => assert_eq!(Some(hud.on_hud_channel("b", "x")), first),
            "onHudChannel-frac" => assert_eq!(Some(hud.on_hud_channel("r", "12.5")), first),
            "setHudPreset" => assert_eq!(Some(hud.set_hud_preset(&swatches[3])), first),
            "setCrossColor" => assert_eq!(Some(hud.set_cross_color(&swatches[4])), first),
            "onCrossHex" => assert_eq!(hud.on_cross_hex("#ff00ff"), first),
            "set-size-disabled" | "set-size-enabled" => {
                assert_eq!(Some(hud.set("crossSize", json!(12))), first)
            }
            "set-crossEnabled" => assert_eq!(Some(hud.set("crossEnabled", json!(false))), first),
            "set-hud" => assert_eq!(Some(hud.set("hudEnabled", json!(true))), first),
            "toggleCross-on-standard" | "toggleCross-on-custom" => {
                assert_eq!(Some(hud.toggle_cross_enabled(true)), first, "{name}");
            }
            "toggleCross-off" => assert_eq!(Some(hud.toggle_cross_enabled(false)), first),
            "hexToColor" => {
                assert_eq!(hud_editor::hex_to_rgb("#010203"), Some(rgb_of(&result[0])));
                assert_eq!(hud_editor::hex_to_rgb("zz"), None);
                assert!(result[1].is_null());
            }
            "crossPresets" => {
                let ids: Vec<&str> = result
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| str_of(&p["id"]))
                    .collect();
                let titles: Vec<&str> = result
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| str_of(&p["title"]))
                    .collect();
                assert_eq!(
                    hud_editor::CROSS_PRESET_IDS
                        .iter()
                        .map(|(id, _)| *id)
                        .collect::<Vec<_>>(),
                    ids
                );
                assert_eq!(
                    hud_editor::CROSS_PRESET_IDS
                        .iter()
                        .map(|(_, t)| *t)
                        .collect::<Vec<_>>(),
                    titles
                );
            }
            "preset-active" | "preset-active-standard" => {
                let active: Vec<bool> = hud_editor::CROSS_PRESET_IDS
                    .iter()
                    .map(|(id, _)| hud.cross_preset_active(id))
                    .collect();
                let expected: Vec<bool> = result
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_bool().unwrap())
                    .collect();
                assert_eq!(active, expected, "{name}");
            }
            other if other.starts_with("applyCrossPreset-") => {
                let id = other.trim_start_matches("applyCrossPreset-");
                assert_eq!(hud.apply_cross_preset(id), first, "{name}");
            }
            other => panic!("неизвестный случай {other}"),
        }
    }
}

// ---- LiveSplitOverlayEditor ----

#[test]
fn the_livesplit_source_card_matches_the_renderer() {
    let f = fixture();
    for case in f["livesplit"]["sources"].as_array().unwrap() {
        let name = str_of(&case["name"]);
        let mut e = LivesplitEditor::new(None, "218", false);
        let result = case_result(name);
        if let Some(result) = result {
            e.capture_reply(false, Ok(result));
        }
        if name == "capturing-pending" {
            e.begin_capture(false);
        }
        assert_eq!(e.status(), str_of(&case["status"]), "{name}: статус");
        assert_eq!(
            e.other_prefix(),
            case["otherPrefix"].as_bool().unwrap(),
            "{name}"
        );
        let tone = match e.source_tone() {
            Tone::Idle => "idle",
            Tone::Ok => "ok",
            Tone::Warn => "warn",
        };
        assert_eq!(tone, str_of(&case["tone"]), "{name}: тон");
        assert_eq!(e.source_icon(), str_of(&case["icon"]), "{name}: значок");
        assert_eq!(
            e.source_title(),
            str_of(&case["title"]),
            "{name}: заголовок"
        );
        let (hint, arg) = e.source_hint();
        let expected_hint = str_of(&case["hint"]);
        match arg {
            Some(arg) => assert_eq!(format!("{hint}({arg})"), expected_hint, "{name}: подсказка"),
            None => assert_eq!(hint, expected_hint, "{name}: подсказка"),
        }
        let background = e
            .background_color()
            .map(|c| json!({"r": c.r, "g": c.g, "b": c.b}));
        assert_eq!(
            background,
            (!case["background"].is_null()).then(|| case["background"].clone()),
            "{name}: фон"
        );
    }
}

/// Ответ снимка по имени случая (пиксели — как в скрипте: B, G, R, X одного цвета).
fn case_result(name: &str) -> Option<Value> {
    let pixels = |w: usize, h: usize, rgb: [u8; 3]| -> Value {
        let mut data = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            data.extend([rgb[2], rgb[1], rgb[0], 255]);
        }
        json!(STANDARD.encode(data))
    };
    Some(match name {
        "pending" | "capturing-pending" => return None,
        "ok" => {
            json!({"status": "ok", "width": 320, "height": 200, "pixels": pixels(320, 200, [1, 2, 3])})
        }
        "ok-other" => {
            json!({"status": "ok", "width": 320, "height": 200, "otherGame": true, "pixels": pixels(320, 200, [1, 2, 3])})
        }
        "not-running" => json!({"status": "not-running"}),
        "no-window" => json!({"status": "no-window"}),
        "error" => json!({"status": "error"}),
        "weird" => json!({"status": "weird"}),
        other => panic!("случай {other}"),
    })
}

#[test]
fn livesplit_sliders_changes_and_edits_match_the_renderer() {
    let f = fixture();
    for case in f["livesplit"]["sliders"].as_array().unwrap() {
        let value = case["value"].clone();
        let e = LivesplitEditor::new((!value.is_null()).then(|| value.clone()), "218", false);
        assert_eq!(e.overlay().to_value(), case["overlay"]);
        assert_eq!(e.changes(), case["changes"].as_u64().unwrap() as usize);
        for (key, style) in case["styles"].as_object().unwrap() {
            let range = livesplit_editor::SIZE_FIELDS
                .iter()
                .chain(&livesplit_editor::COLOR_FIELDS)
                .chain(&livesplit_editor::LUMA_FIELDS)
                .find(|f| f.key == key)
                .map(|f| (f.min, f.max));
            let s = e.slider_style(key, range);
            assert!(close(s.value, percent(&style["--value"])), "{key}: value");
            assert!(
                close(s.default, percent(&style["--default"])),
                "{key}: default"
            );
            assert!(close(s.from, percent(&style["--from"])), "{key}: from");
            assert!(close(s.to, percent(&style["--to"])), "{key}: to");
        }
        for (key, changed) in case["changed"].as_object().unwrap() {
            assert_eq!(e.changed(key), changed.as_bool().unwrap(), "{key}: changed");
        }
    }
    let anchors: Vec<(&str, f64, f64)> = f["livesplit"]["anchors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                str_of(&a["id"]),
                a["x"].as_f64().unwrap(),
                a["y"].as_f64().unwrap(),
            )
        })
        .collect();
    let ours: Vec<(&str, f64, f64)> = livesplit_editor::ANCHORS
        .iter()
        .map(|a| (a.id, a.x, a.y))
        .collect();
    assert_eq!(ours, anchors);
    let fields = &f["livesplit"]["fields"];
    let keys =
        |list: &[livesplit_editor::NumberField]| list.iter().map(|f| f.key).collect::<Vec<_>>();
    let want = |v: &Value| {
        v.as_array()
            .unwrap()
            .iter()
            .map(|f| str_of(&f["key"]).to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&livesplit_editor::SIZE_FIELDS), want(&fields["size"]));
    assert_eq!(
        keys(&livesplit_editor::COLOR_FIELDS),
        want(&fields["color"])
    );
    assert_eq!(keys(&livesplit_editor::LUMA_FIELDS), want(&fields["luma"]));
    for (ours, want) in livesplit_editor::SIZE_FIELDS
        .iter()
        .chain(&livesplit_editor::COLOR_FIELDS)
        .zip(
            fields["size"]
                .as_array()
                .unwrap()
                .iter()
                .chain(fields["color"].as_array().unwrap()),
        )
    {
        assert_eq!(ours.min, want["min"].as_f64().unwrap());
        assert_eq!(ours.max, want["max"].as_f64().unwrap());
        assert_eq!(ours.step, want["step"].as_f64().unwrap());
        assert_eq!(ours.label, str_of(&want["label"]));
    }
    assert_eq!(
        livesplit_editor::CROP_SIDES.to_vec(),
        fields["crop"]
            .as_array()
            .unwrap()
            .iter()
            .map(str_of)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        f["livesplit"]["constants"]["AUTO_REFRESH_MS"],
        livesplit_editor::AUTO_REFRESH_MS
    );
    assert_eq!(
        f["livesplit"]["constants"]["MIN_SPIN_MS"],
        livesplit_editor::MIN_SPIN_MS
    );

    let e = LivesplitEditor::new(None, "218", false);
    for case in f["livesplit"]["set"].as_array().unwrap() {
        let name = str_of(&case["name"]);
        let emitted = case["emitted"].as_array().unwrap();
        let first = emitted.first().map(|e| e["payload"].clone());
        let patch = |entries: Value| {
            let mut map = serde_json::Map::new();
            for (k, v) in entries.as_object().unwrap() {
                map.insert(k.clone(), v.clone());
            }
            e.set(map)
        };
        match name {
            "set-anchor" => assert_eq!(Some(patch(json!({"x": 0, "y": 100}))), first),
            "set-key" => assert_eq!(Some(patch(json!({"key": "luma"}))), first),
            "set-bad" => assert_eq!(Some(patch(json!({"x": 500, "opacity": "abc"}))), first),
            "setNumber-ok" => {
                let (next, shown) = e.set_number("height", "77");
                assert_eq!(Some(next), first);
                assert_eq!(shown, str_of(&case["extra"]));
            }
            "setNumber-bad" => {
                let (next, shown) = e.set_number("opacity", "zzz");
                assert_eq!(Some(next), first);
                assert_eq!(shown, str_of(&case["extra"]));
            }
            "setCrop" => {
                let (next, shown) = e.set_crop("top", "4000");
                assert_eq!(Some(next), first);
                assert_eq!(shown, str_of(&case["extra"]));
            }
            "setColor" => assert_eq!(e.set_color("#0102ff"), first),
            "setColor-bad" => assert_eq!(e.set_color("#12"), first),
            "hex" => assert_eq!(LivesplitEditor::hex(0, 255, 16), str_of(&case["extra"])),
            "reset-keeps-enabled" => {
                let e = LivesplitEditor::new(Some(json!({"enabled": true, "x": 1})), "218", false);
                assert_eq!(Some(e.reset()), first);
            }
            other => panic!("случай {other}"),
        }
    }
}

#[test]
fn personalization_changes_counts_like_the_renderer() {
    let defaults = hlsr_delivery::ghosting_personalization::default_personalization().to_value();
    assert_eq!(extras::personalization_changes(Some(&defaults)), 0);
    assert_eq!(extras::personalization_changes(None), 0);
    let mut changed = defaults.clone();
    changed["hudEnabled"] = json!(true);
    changed["crossPreset"] = json!("dot");
    changed["extra"] = json!({"gh_hud_bg": false});
    assert_eq!(extras::personalization_changes(Some(&changed)), 1);
    changed["hudColor"] = json!({"r": 255, "g": 220, "b": 1});
    assert_eq!(extras::personalization_changes(Some(&changed)), 2);
    // Нет ключа — считается изменённым (JSON.stringify(undefined)).
    let mut missing = defaults;
    missing.as_object_mut().unwrap().remove("crossAlpha");
    assert_eq!(extras::personalization_changes(Some(&missing)), 1);
}
