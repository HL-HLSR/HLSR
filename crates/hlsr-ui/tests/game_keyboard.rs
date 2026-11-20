//! Клавиатура окна настроек игры против `GameKeyboard.vue` и `bind-state.js` 0.4.7 (фикстура
//! `tools/reference/ui/game-keyboard.js`): схема, легенда, клавиши вне схемы, категории, группы
//! заготовок и сведения о клавише при тех же данных, черновик биндов после «назначить, вернуть,
//! снять», своей команды и её ошибок — шаг за шагом, как в 0.4.7.

use hlsr_core::js::Platform;
use hlsr_ui::controllers::bind_state::{self, key_binds_from_value};
use hlsr_ui::controllers::game_keyboard::{Board, CapView, Inputs, Keyboard, RunMode, Shown};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/game-keyboard.json")).expect("фикстура")
}

/// Переводчик-метка, как `$t` скрипта съёма: T(ключ|аргументы).
fn t(key: &str, args: &[&str]) -> String {
    if args.is_empty() {
        format!("T({key})")
    } else {
        format!("T({key}|{})", args.join("|"))
    }
}

fn cap_json(cap: &CapView) -> Value {
    json!({
        "id": cap.id,
        "key": cap.key,
        "text": cap.text,
        "bind": cap.bind,
        "script": cap.script,
        "aria": cap.aria,
        "classes": cap.classes(),
    })
}

fn shown_json(shown: Option<Shown>) -> Value {
    match shown {
        None => Value::Null,
        Some(s) => json!({
            "key": s.key,
            "name": s.name,
            "command": s.command,
            "label": s.label,
            "tone": s.tone,
            "note": s.note,
            "scriptWarning": s.script_warning,
            "practice": s.practice,
            "source": s.source,
            "overridden": s.overridden,
            "baseLabel": s.base_label,
            "alias": s.alias,
            "customAlias": s.custom_alias,
            "otherKeys": s.other_keys,
        }),
    }
}

/// Порядок ключей объекта важен (байты JSON настроек): сравнение строкой.
fn same_json(a: &Value, b: &Value) -> bool {
    serde_json::to_string(a).unwrap() == serde_json::to_string(b).unwrap()
}

struct Taken {
    out: Value,
    caps: Vec<Value>,
}

fn snapshot(kb: &Keyboard, inputs: &Inputs<'_>) -> Taken {
    let board = Board::new(kb, inputs);
    let shown_selected = board.shown_selected();
    let groups = board.groups();
    let mut active = Vec::new();
    for group in &groups {
        for action in &group.actions {
            if board.chip_active(shown_selected.as_ref(), &action.command) {
                active.push(action.command.clone());
            }
        }
    }
    let caps: Vec<Value> = board.caps().iter().map(cap_json).collect();
    let out = json!({
        "hovered": kb.hovered,
        "selected": kb.selected,
        "runMode": kb.run_mode.id(),
        "query": kb.query,
        "category": kb.category,
        "custom": kb.custom,
        "customError": kb.custom_error,
        "value": inputs.value,
        "shown": shown_json(board.shown()),
        "isSelectedShown": board.is_selected_shown(),
        "selectedName": board.selected_name(),
        "legend": board.legend().iter().map(|l| json!({"id": l.id, "tone": l.tone, "title": l.title})).collect::<Vec<_>>(),
        "offBoard": board.off_board().iter().map(|o| json!({"key": o.key, "name": o.name, "label": o.label})).collect::<Vec<_>>(),
        "listedCategories": board.listed_categories().iter().map(|c| json!({"id": c.id, "tone": c.tone, "title": c.title})).collect::<Vec<_>>(),
        "groupIds": groups.iter().map(|g| json!([g.id, g.actions.len()])).collect::<Vec<_>>(),
        "active": active,
        "groups": groups.iter().map(|g| json!({
            "id": g.id,
            "tone": g.tone,
            "title": g.title,
            "actions": g.actions.iter().map(|a| json!({
                "command": a.command,
                "label": a.label,
                "scripted": a.scripted,
                "practice": a.practice,
                "keys": a.keys,
                "title": a.title,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    Taken { out, caps }
}

fn check_field(errors: &mut Vec<String>, label: &str, field: &str, want: &Value, got: &Value) {
    if !same_json(want, got) {
        errors.push(format!(
            "{label}: {field}\n  ждали {}\n  вышло {}",
            serde_json::to_string(want).unwrap(),
            serde_json::to_string(got).unwrap()
        ));
    }
}

#[test]
fn keyboard_follows_game_keyboard_vue_step_by_step() {
    let fixture = fixture();
    let mut errors = Vec::new();
    for scenario in fixture["scenarios"].as_array().unwrap() {
        let name = scenario["name"].as_str().unwrap();
        let game_id = scenario["gameId"].as_str().unwrap();
        let lang = scenario["lang"].as_str().unwrap();
        let key_binds = key_binds_from_value(Some(&scenario["keyBinds"]));
        let mut value = scenario["value"].clone();
        let mut kb = Keyboard::new(RunMode::from_stored(scenario["runMode"].as_str()));
        let mut stored: Option<String> = scenario["runMode"].as_str().map(str::to_owned);
        let mut previous: Option<(Vec<Value>, Value)> = None;
        for snap in scenario["snapshots"].as_array().unwrap() {
            let label = format!("{name}: {}", snap["label"].as_str().unwrap());
            let step = snap["label"].as_str().unwrap();
            let mut emitted: Option<Value> = None;
            if step != "start" {
                let inputs = Inputs {
                    game_id,
                    value: &value,
                    key_binds: key_binds.as_ref(),
                    lang,
                    platform: Platform::Win32,
                    t: &t,
                    full: true,
                    saved_value: None,
                    saved_key_binds: None,
                };
                let (action, rest) = step.split_once(' ').unwrap_or((step, ""));
                match action {
                    "set" => {
                        let (field, raw) = rest.split_once(' ').unwrap_or((rest, ""));
                        let parsed: Value =
                            serde_json::from_str(raw).unwrap_or(Value::String(raw.to_owned()));
                        let text = parsed.as_str().map(str::to_owned);
                        match field {
                            "hovered" => kb.hovered = text,
                            "query" => kb.query = text.unwrap_or_default(),
                            "category" => kb.category = text,
                            "custom" => kb.custom = text.unwrap_or_default(),
                            other => panic!("поле {other}"),
                        }
                    }
                    "select" => {
                        let key: Value =
                            serde_json::from_str(rest).unwrap_or(Value::String(rest.to_owned()));
                        kb.select(key.as_str());
                    }
                    "assign" => emitted = kb.assign(&inputs, rest),
                    "revert" => emitted = kb.revert(&inputs),
                    "assignCustom" => emitted = kb.assign_custom(&inputs),
                    "setRunMode" => {
                        kb.run_mode = RunMode::from_stored(Some(rest));
                        stored = Some(rest.to_owned());
                    }
                    other => panic!("действие {other}"),
                }
                if let Some(next) = &emitted {
                    value = next.clone();
                }
            }
            let inputs = Inputs {
                game_id,
                value: &value,
                key_binds: key_binds.as_ref(),
                lang,
                platform: Platform::Win32,
                t: &t,
                full: true,
                saved_value: None,
                saved_key_binds: None,
            };
            let taken = snapshot(&kb, &inputs);
            for field in [
                "hovered",
                "selected",
                "runMode",
                "query",
                "category",
                "custom",
                "customError",
                "value",
                "shown",
                "isSelectedShown",
                "selectedName",
                "groupIds",
                "active",
            ] {
                check_field(&mut errors, &label, field, &snap[field], &taken.out[field]);
            }
            for field in ["legend", "offBoard", "listedCategories"] {
                if snap[field] != "same" {
                    check_field(&mut errors, &label, field, &snap[field], &taken.out[field]);
                } else if let Some((_, prev)) = &previous {
                    check_field(&mut errors, &label, field, &prev[field], &taken.out[field]);
                }
            }
            if step != "start" {
                check_field(
                    &mut errors,
                    &label,
                    "emitted",
                    &snap["emitted"],
                    &emitted.clone().unwrap_or(Value::Null),
                );
                check_field(
                    &mut errors,
                    &label,
                    "stored",
                    &snap["stored"],
                    &json!(stored),
                );
            }
            if let Some(caps) = snap.get("caps") {
                check_field(
                    &mut errors,
                    &label,
                    "caps",
                    caps,
                    &Value::Array(taken.caps.clone()),
                );
                check_field(
                    &mut errors,
                    &label,
                    "groups",
                    &snap["groups"],
                    &taken.out["groups"],
                );
            } else {
                let (prev_caps, _) = previous.as_ref().expect("прежний снимок");
                let changed: Vec<Value> = taken
                    .caps
                    .iter()
                    .zip(prev_caps)
                    .filter(|(now, before)| !same_json(now, before))
                    .map(|(now, _)| now.clone())
                    .collect();
                check_field(
                    &mut errors,
                    &label,
                    "changed",
                    &snap["changed"],
                    &Value::Array(changed),
                );
            }
            previous = Some((taken.caps, taken.out));
        }
    }
    assert!(
        errors.is_empty(),
        "{}\n\n(всего {})",
        errors
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
        errors.len()
    );
}

#[test]
fn bind_state_matches_0_4_7() {
    let fixture = fixture();
    for case in fixture["bindState"].as_array().unwrap() {
        let input = &case["input"];
        let read = key_binds_from_value(Some(&json!({
            "binds": input["fileBinds"],
            "aliases": input["aliases"],
        })))
        .unwrap();
        let overrides = bind_state::overrides_of(&input["overrides"]);
        let effective = bind_state::effective_binds(&read.binds, &overrides);
        let effective_json = Value::Object(
            effective
                .iter()
                .map(|(key, entry)| {
                    let mut item = json!({
                        "command": entry.command,
                        "source": entry.source,
                        "overridden": entry.overridden,
                    });
                    if let Some(base) = &entry.base {
                        item["base"] = json!(base);
                    }
                    (key.to_owned(), item)
                })
                .collect(),
        );
        assert!(
            same_json(&case["effective"], &effective_json),
            "effective\n{}\n{}",
            case["effective"],
            effective_json
        );
        let by_command = bind_state::keys_by_command(&effective);
        let by_command_json = Value::Object(
            by_command
                .iter()
                .map(|(command, keys)| (command.to_owned(), json!(keys)))
                .collect(),
        );
        assert!(
            same_json(&case["keysByCommand"], &by_command_json),
            "keysByCommand {by_command_json}"
        );
        let by_part = bind_state::keys_by_part(&effective, &read.aliases);
        let by_part_json = Value::Object(
            by_part
                .iter()
                .map(|(command, keys)| (command.to_owned(), json!(keys)))
                .collect(),
        );
        assert!(
            same_json(&case["keysByPart"], &by_part_json),
            "keysByPart {by_part_json}"
        );
        for pair in case["baseOf"].as_array().unwrap() {
            let key = pair[0].as_str().unwrap();
            assert_eq!(
                bind_state::base_of(key, &read.binds, &overrides),
                pair[1].as_str().unwrap(),
                "baseOf {key}"
            );
        }
    }
}

/// 0.4.9: основной блок (15 u), бинд по нажатию другой клавиши своим цветом, «Пианино».
#[test]
fn keyboard_049_main_block_runtime_and_piano() {
    use hlsr_delivery::game_binds::{Bind, BindRead, RuntimeBind};
    use hlsr_ui::keyboard_layout::{Engine, layout_for};
    let bind = |command: &str| Bind {
        command: command.to_owned(),
        source: "config.cfg".to_owned(),
    };
    let mut read = BindRead::default();
    read.binds.insert("q".to_owned(), bind("exec tc.cfg"));
    read.runtime.insert(
        "e".to_owned(),
        vec![RuntimeBind {
            command: "+use".to_owned(),
            file: Some("tc.cfg".to_owned()),
            trigger: "q".to_owned(),
        }],
    );
    let value = json!({});
    let kb = Keyboard::default();
    let main = Inputs {
        game_id: "hl1-won",
        value: &value,
        key_binds: Some(&read),
        lang: "ru",
        platform: Platform::Win32,
        t: &t,
        full: false,
        saved_value: None,
        saved_key_binds: None,
    };
    let board = Board::new(&kb, &main);
    assert_eq!(board.cols(), 15.0);
    let caps = board.caps();
    assert!(caps.iter().all(|cap| cap.x + cap.w <= 1.0 + 1e-9));
    assert!(caps.iter().any(|cap| cap.key == Some("SPACE")));
    assert!(
        caps.iter()
            .all(|cap| !matches!(cap.key, Some("UPARROW" | "MOUSE1" | "KP_ENTER")))
    );
    let e = caps.iter().find(|cap| cap.key == Some("e")).expect("E");
    assert_eq!(e.tone.as_deref(), Some("runtime"));
    assert_eq!(e.classes(), ["tone-runtime", "is-runtime"]);
    assert_eq!(e.bind, board.label_of("+use"));
    assert!(e.aria.ends_with(&board.label_of("+use")));
    assert!(board.legend().iter().any(|item| item.id == "runtime"));
    let shown = board.describe("e");
    assert_eq!(shown.tone, "runtime");
    assert_eq!(shown.label, board.label_of("+use"));
    assert_eq!(shown.runtime.len(), 1);
    assert_eq!(shown.runtime[0].file.as_deref(), Some("tc.cfg"));
    assert!(board.describe("q").runtime.is_empty());
    // Схема целиком шире: 26 u, бинды вне схемы считаются по всей схеме.
    let full = Inputs { full: true, ..main };
    assert_eq!(Board::new(&kb, &full).cols(), 26.0);

    // «Пианино»: каждая клавиша всей схемы в сохранённом, какой бы вид ни был на экране.
    let mut all = BindRead::default();
    for key in &layout_for(Engine::GoldSrc).board_keys {
        all.binds.insert((*key).to_owned(), bind("+jump"));
    }
    let saved = json!({});
    let piano = Inputs {
        game_id: "hl1-won",
        value: &value,
        key_binds: Some(&all),
        lang: "ru",
        platform: Platform::Win32,
        t: &t,
        full: false,
        saved_value: Some(&saved),
        saved_key_binds: Some(&all),
    };
    assert!(Board::new(&kb, &piano).every_key_bound());
    let unbound = json!({"SPACE": {"command": "", "base": "+jump"}});
    let one_off = Inputs {
        saved_value: Some(&unbound),
        ..piano
    };
    assert!(!Board::new(&kb, &one_off).every_key_bound());
    let unknown = Inputs {
        saved_value: None,
        ..piano
    };
    assert!(!Board::new(&kb, &unknown).every_key_bound());
}

/// `keyBinds.runtime` ответа окна разбирается в бинды по нажатию.
#[test]
fn key_binds_read_runtime() {
    let read = key_binds_from_value(Some(&json!({
        "binds": {},
        "runtime": {"e": [{"command": "+use", "file": null, "trigger": "q"}], "x": "мусор"}
    })))
    .expect("объект");
    let list = read.runtime.get("e").expect("e");
    assert_eq!(list[0].command, "+use");
    assert_eq!(list[0].file, None);
    assert_eq!(list[0].trigger, "q");
    assert!(read.runtime.get("x").is_none());
}
