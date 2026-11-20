//! Раскладка, подписи и разбор нажатий против `keyboard-layout.js` 0.4.7
//! (фикстура `tools/reference/ui/keyboard-layout.js`).

use hlsr_ui::keyboard_layout::{
    BOARD_HEIGHT, BOARD_WIDTH, Engine, InputEvent, key_from_event, key_name, layout_for,
};
use serde_json::Value;

fn fixture() -> Value {
    let text = include_str!("fixtures/keyboard-layout.json");
    serde_json::from_str(text).expect("фикстура разбирается")
}

fn engine(value: &Value) -> Engine {
    Engine::from_name(value.as_str().expect("engine — строка"))
}

fn expected(value: &Value) -> Option<&str> {
    value.as_str()
}

#[test]
fn board_size_matches() {
    let fixture = fixture();
    assert_eq!(fixture["board"]["width"].as_f64(), Some(BOARD_WIDTH));
    assert_eq!(fixture["board"]["height"].as_f64(), Some(BOARD_HEIGHT));
}

#[test]
fn layouts_match_for_both_engines() {
    let fixture = fixture();
    for name in ["goldsrc", "source"] {
        let want = &fixture["layouts"][name];
        let got = layout_for(Engine::from_name(name));
        let keys = want["keys"].as_array().unwrap();
        assert_eq!(got.keys.len(), keys.len(), "{name}: число клавиш");
        for (cap, want) in got.keys.iter().zip(keys) {
            assert_eq!(
                cap.label,
                want["label"].as_str().unwrap(),
                "{name}: {}",
                cap.id
            );
            assert_eq!(cap.key, want["key"].as_str(), "{name}: {}", cap.id);
            assert_eq!(cap.id, want["id"].as_str().unwrap(), "{name}");
            for (field, value) in [("x", cap.x), ("y", cap.y), ("w", cap.w), ("h", cap.h)] {
                assert_eq!(
                    Some(value),
                    want[field].as_f64(),
                    "{name}: {} {field}",
                    cap.id
                );
            }
        }
        let board: Vec<&str> = want["boardKeys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap())
            .collect();
        assert_eq!(got.board_keys, board, "{name}: boardKeys");
        assert!(board.iter().all(|key| got.has(key)));
        assert!(!got.has("NOPE"));
    }
}

#[test]
fn key_names_match() {
    let fixture = fixture();
    let t = |id: &str| format!("T({id})");
    for entry in fixture["keyNames"].as_array().unwrap() {
        let key = entry["key"].as_str().unwrap();
        assert_eq!(
            key_name(key, t),
            entry["name"].as_str().unwrap(),
            "keyName({key})"
        );
    }
}

#[test]
fn key_codes_match() {
    let fixture = fixture();
    let cases = fixture["keys"].as_array().unwrap();
    assert!(cases.len() > 300, "корпус кодов");
    for case in cases {
        let code = case["code"].as_str().unwrap_or("");
        let got = key_from_event(&InputEvent::Key { code }, engine(&case["engine"]));
        assert_eq!(got, expected(&case["result"]), "keyFromEvent({case})");
    }
}

#[test]
fn mouse_buttons_match() {
    let fixture = fixture();
    for case in fixture["mouse"].as_array().unwrap() {
        let button = case["button"].as_i64().unwrap() as i32;
        let got = key_from_event(&InputEvent::MouseButton { button }, Engine::GoldSrc);
        assert_eq!(got, expected(&case["result"]), "keyFromEvent({case})");
    }
}

#[test]
fn wheel_matches() {
    let fixture = fixture();
    for case in fixture["wheel"].as_array().unwrap() {
        let delta_y = match &case["deltaY"] {
            Value::String(text) if text == "-0" => -0.0,
            value => value.as_f64().unwrap(),
        };
        let got = key_from_event(&InputEvent::Wheel { delta_y }, Engine::GoldSrc);
        assert_eq!(got, expected(&case["result"]), "keyFromEvent({case})");
    }
}
