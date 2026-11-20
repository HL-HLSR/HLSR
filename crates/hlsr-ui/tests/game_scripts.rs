//! Вкладка «Скрипты» против `GameScripts.vue` 0.4.7 (фикстура `tools/reference/ui/game-scripts.js`):
//! главы и поиск, описание каждого скрипта гайда (перевод или оригинал, адреса видео, строки клавиш,
//! «как в гайде», свои alias, подсказки) и действия шаг за шагом — назначение с вопросом о занятых
//! клавишах, снятие, захват клавиатуры, мыши и колеса по физическим кодам (`keys.rs` → вкладка,
//! как перехват winit в лаунчере), ссылки. Плюс правила webview плеера (решение 12).

use hlsr_core::js::Platform;
use hlsr_ui::controllers::bind_state::key_binds_from_value;
use hlsr_ui::controllers::game_scripts::{
    Guide, Inputs, Outcome, Rect, Scripts, VideoPlacement, VideoScene, video_placement,
};
use hlsr_ui::keyboard_layout::Engine;
use hlsr_ui::keys::{Area, KeyInput, RawInput};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/game-scripts.json")).expect("фикстура")
}

fn t(key: &str, args: &[&str]) -> String {
    if args.is_empty() {
        format!("T({key})")
    } else {
        format!("T({key}|{})", args.join("|"))
    }
}

fn same_json(a: &Value, b: &Value) -> bool {
    serde_json::to_string(a).unwrap() == serde_json::to_string(b).unwrap()
}

fn detail_json(guide: &Guide<'_>, scripts: &Scripts) -> Value {
    let current = guide.current();
    let rows = guide.action_rows();
    let (notes, usage) = guide.description();
    json!({
        "id": current.id,
        "title": current.title,
        "section": current.section,
        "credit": current.credit,
        "chapterTitle": guide.chapter_title(),
        "save": current.save,
        "guideUrl": guide.guide_url(),
        "video": guide.video().map(|v| json!({"poster": v.poster, "embed": v.embed, "watch": v.watch})),
        "translation": guide.translation().is_some(),
        "showOriginal": scripts.show_original,
        "notes": notes,
        "usage": usage,
        "actionRows": rows.iter().map(|row| json!({
            "command": row.command,
            "guideKey": row.guide_key,
            "guideKeyName": row.guide_key_name,
            "keys": row.keys.iter().map(|k| json!({
                "key": k.key,
                "name": k.name,
                "whole": k.whole,
                "fromConfig": k.from_config,
                "title": k.title,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "guideKeysMissing": Guide::guide_keys_missing(&rows),
        "needsAutoAlias": current.needs_auto_alias,
        "ownAliases": guide.own_aliases(),
        "usesSensitivity": guide.uses_sensitivity(),
        "sensitivity": guide.sensitivity(),
        "scripted": rows.iter().any(|row| !row.keys.is_empty()),
    })
}

fn state_json(scripts: &Scripts, value: &Value) -> Value {
    json!({
        "currentId": scripts.current_id,
        "playing": scripts.playing,
        "capturing": scripts.capturing,
        "conflict": scripts.conflict.as_ref().map(|c| json!({
            "assignments": Value::Object(c.assignments.iter().map(|(k, v)| (k.to_owned(), json!(v))).collect()),
            "items": c.items.iter().map(|i| json!({"key": i.key, "name": i.name, "label": i.label})).collect::<Vec<_>>(),
        })),
        "value": value,
    })
}

fn check(errors: &mut Vec<String>, label: &str, want: &Value, got: &Value) {
    if !same_json(want, got) {
        errors.push(format!(
            "{label}\n  ждали {}\n  вышло {}",
            serde_json::to_string(want).unwrap(),
            serde_json::to_string(got).unwrap()
        ));
    }
}

fn key_binds(scenario: &Value, fixture: &Value) -> Option<hlsr_delivery::game_binds::BindRead> {
    match &scenario["keyBinds"] {
        Value::String(name) if name == "default" => {
            key_binds_from_value(Some(&fixture["keyBinds"]))
        }
        other => key_binds_from_value(Some(other)),
    }
}

#[test]
fn chapters_and_every_script_match_game_scripts_vue() {
    let fixture = fixture();
    let read = key_binds_from_value(Some(&fixture["keyBinds"]));
    let value = json!({});
    let mut errors = Vec::new();
    for catalog in fixture["catalogs"].as_array().unwrap() {
        let lang = catalog["lang"].as_str().unwrap();
        let inputs = Inputs {
            value: &value,
            key_binds: read.as_ref(),
            lang,
            platform: Platform::Win32,
            t: &t,
        };
        let mut scripts = Scripts::default();
        for list in catalog["lists"].as_array().unwrap() {
            scripts.query = list["query"].as_str().unwrap().to_owned();
            let guide = Guide::new(&scripts, &inputs);
            let got: Vec<Value> = guide
                .chapters()
                .iter()
                .map(|chapter| {
                    json!({
                        "id": chapter.id,
                        "title": chapter.title,
                        "scripts": chapter.scripts.iter().map(|s| json!([s.id, s.title, s.mark, s.keys])).collect::<Vec<_>>(),
                    })
                })
                .collect();
            check(
                &mut errors,
                &format!("{lang}: главы «{}»", scripts.query),
                &list["chapters"],
                &Value::Array(got),
            );
        }
        scripts.query.clear();
        for detail in catalog["details"].as_array().unwrap() {
            scripts.current_id = detail["id"].as_str().unwrap().to_owned();
            scripts.show_original = false;
            let mut got = detail_json(&Guide::new(&scripts, &inputs), &scripts);
            if detail.get("original").is_some() {
                scripts.show_original = true;
                let original = detail_json(&Guide::new(&scripts, &inputs), &scripts);
                got["original"] = json!({"notes": original["notes"], "usage": original["usage"]});
            }
            check(
                &mut errors,
                &format!("{lang}: {}", scripts.current_id),
                detail,
                &got,
            );
        }
    }
    assert!(
        errors.is_empty(),
        "{}\n\n(всего {})",
        errors
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
        errors.len()
    );
}

/// Область кнопки «Назначить»: мышь и колесо в ней — клавиши.
const BUTTON: Area = Area {
    x: 100.0,
    y: 100.0,
    width: 120.0,
    height: 30.0,
};

#[test]
fn actions_match_game_scripts_vue_step_by_step() {
    let fixture = fixture();
    let mut errors = Vec::new();
    for scenario in fixture["scenarios"].as_array().unwrap() {
        let name = scenario["name"].as_str().unwrap();
        let lang = scenario["lang"].as_str().unwrap();
        let read = key_binds(scenario, &fixture);
        let mut value = scenario["value"].clone();
        let mut scripts = Scripts::default();
        let mut keys = KeyInput::new();
        {
            let inputs = Inputs {
                value: &value,
                key_binds: read.as_ref(),
                lang,
                platform: Platform::Win32,
                t: &t,
            };
            check(
                &mut errors,
                &format!("{name}: начало"),
                &scenario["start"]["detail"],
                &detail_json(&Guide::new(&scripts, &inputs), &scripts),
            );
        }
        for step in scenario["steps"].as_array().unwrap() {
            let label = format!("{name}: {}", step["step"].as_str().unwrap());
            let text = step["step"].as_str().unwrap();
            let (action, rest) = text.split_once(' ').unwrap_or((text, ""));
            let mut outcome = Outcome::default();
            {
                let inputs = Inputs {
                    value: &value,
                    key_binds: read.as_ref(),
                    lang,
                    platform: Platform::Win32,
                    t: &t,
                };
                // Событие перехвата → итог ожидания → вкладка.
                let mut feed = |scripts: &mut Scripts, input: RawInput, outcome: &mut Outcome| {
                    let handled = keys.handle(&input, false);
                    if let Some(end) = handled.capture {
                        *outcome = scripts.capture_end(&inputs, &end.owner, end.key);
                    }
                };
                match action {
                    "open" => outcome = scripts.open(rest),
                    "set" => {
                        let (field, raw) = rest.split_once(' ').unwrap();
                        match field {
                            "conflict" => scripts.cancel_conflict(),
                            "playing" => scripts.playing = raw == "true",
                            "showOriginal" => scripts.show_original = raw == "true",
                            other => panic!("поле {other}"),
                        }
                    }
                    "assignGuideKeys" => outcome = scripts.assign_guide_keys(&inputs),
                    "confirmConflict" => outcome = scripts.confirm_conflict(&inputs),
                    "unassign" => outcome = scripts.unassign(&inputs, rest),
                    "capture" => {
                        outcome = scripts.start_capture(rest);
                        if outcome.capture_started {
                            keys.start_capture(rest, Engine::GoldSrc, BUTTON);
                        }
                    }
                    "key" => feed(
                        &mut scripts,
                        RawInput::KeyDown {
                            code: rest.to_owned(),
                            repeat: false,
                        },
                        &mut outcome,
                    ),
                    "mouse" => {
                        let (_, button) = rest.rsplit_once(' ').unwrap();
                        feed(
                            &mut scripts,
                            RawInput::MouseDown {
                                button: button.parse().unwrap(),
                                x: 110.0,
                                y: 110.0,
                            },
                            &mut outcome,
                        );
                    }
                    "wheel" => {
                        let (_, delta) = rest.rsplit_once(' ').unwrap();
                        feed(
                            &mut scripts,
                            RawInput::Wheel {
                                delta_y: delta.parse().unwrap(),
                                x: 110.0,
                                y: 110.0,
                            },
                            &mut outcome,
                        );
                    }
                    // Нажатие на самой кнопке, что ждёт клавишу, — её разбирает «mouse».
                    "outside" if rest == "true" => {}
                    "outside" => feed(
                        &mut scripts,
                        RawInput::MouseDown {
                            button: 0,
                            x: 5.0,
                            y: 5.0,
                        },
                        &mut outcome,
                    ),
                    "deactivate" => {
                        outcome = scripts.deactivate();
                        keys.stop_capture();
                    }
                    "openLink" => {
                        outcome = Outcome {
                            open_link: Some(rest.to_owned()),
                            ..Outcome::default()
                        }
                    }
                    "link" => {
                        let target: Value = serde_json::from_str(rest).unwrap();
                        outcome = if let Some(id) = target["script"].as_str() {
                            scripts.link("script", id)
                        } else if let Some(href) = target["href"].as_str() {
                            scripts.link("href", href)
                        } else {
                            Outcome::default()
                        };
                    }
                    other => panic!("действие {other}"),
                }
            }
            if outcome.capture_stopped && scripts.capturing.is_none() {
                keys.stop_capture();
            }
            if let Some(next) = &outcome.emitted {
                value = next.clone();
            }
            check(
                &mut errors,
                &format!("{label}: состояние"),
                &step["state"],
                &state_json(&scripts, &value),
            );
            check(
                &mut errors,
                &format!("{label}: черновик"),
                &step["emitted"],
                &outcome.emitted.clone().unwrap_or(Value::Null),
            );
            let invoked: Vec<Value> = outcome
                .open_link
                .iter()
                .map(|url| json!(["hl1-guide-open-link", url]))
                .collect();
            check(
                &mut errors,
                &format!("{label}: вызовы"),
                &step["invoked"],
                &Value::Array(invoked),
            );
            let listeners = step["listeners"].as_array().unwrap();
            let added = listeners.iter().any(|l| l[0] == "add");
            let removed = listeners.iter().any(|l| l[0] == "remove");
            check(
                &mut errors,
                &format!("{label}: подписка"),
                &json!([added, removed]),
                &json!([outcome.capture_started, outcome.capture_stopped]),
            );
            if let Some(detail) = step.get("detail").filter(|d| !d.is_null()) {
                let inputs = Inputs {
                    value: &value,
                    key_binds: read.as_ref(),
                    lang,
                    platform: Platform::Win32,
                    t: &t,
                };
                check(
                    &mut errors,
                    &format!("{label}: деталь"),
                    detail,
                    &detail_json(&Guide::new(&scripts, &inputs), &scripts),
                );
            }
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

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[test]
fn video_webview_follows_layer_and_scroll_rules() {
    let frame = rect(400.0, 300.0, 640.0, 360.0);
    let viewport = rect(380.0, 200.0, 800.0, 500.0);
    let scene = VideoScene {
        playing: true,
        frame,
        viewport,
        covered: false,
        moving: false,
    };
    // Видно целиком: рамка и обрезка совпадают.
    assert_eq!(
        video_placement(&scene),
        VideoPlacement::Shown { frame, clip: frame }
    );
    // Не играет — плеера нет.
    assert_eq!(
        video_placement(&VideoScene {
            playing: false,
            ..scene
        }),
        VideoPlacement::Closed
    );
    // Поверх меню, подсказка, тост или окно выше — скрыт.
    assert_eq!(
        video_placement(&VideoScene {
            covered: true,
            ..scene
        }),
        VideoPlacement::Hidden
    );
    // Переход (появление окна, свайп страниц) — скрыт.
    assert_eq!(
        video_placement(&VideoScene {
            moving: true,
            ..scene
        }),
        VideoPlacement::Hidden
    );
    // Прокручено: верх рамки ушёл за край — рамка на месте, обрезка по видимой части.
    let scrolled = rect(400.0, 120.0, 640.0, 360.0);
    assert_eq!(
        video_placement(&VideoScene {
            frame: scrolled,
            ..scene
        }),
        VideoPlacement::Shown {
            frame: scrolled,
            clip: rect(400.0, 200.0, 640.0, 280.0)
        }
    );
    // Целиком за краем — скрыт.
    assert_eq!(
        video_placement(&VideoScene {
            frame: rect(400.0, -400.0, 640.0, 360.0),
            ..scene
        }),
        VideoPlacement::Hidden
    );
    // Окно уже рамки (узкое окно): обрезка и справа.
    assert_eq!(
        video_placement(&VideoScene {
            viewport: rect(380.0, 200.0, 500.0, 500.0),
            ..scene
        }),
        VideoPlacement::Shown {
            frame,
            clip: rect(400.0, 300.0, 480.0, 360.0)
        }
    );
}
