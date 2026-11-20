//! Размер панели у разделителя (`pane-size.js` 0.4.9, `PaneSplitter`): пределы, шаг клавиш и
//! запоминание. В 0.4.9 размер лежал в localStorage `hlsr.pane.<key>`, здесь — в объекте `pane`
//! файла `launcher-prefs.json` (`{ "<key>": 412 }`, туда же переносит миграция LevelDB).

use hlsr_core::js::math_round;
use hlsr_core::js::value::to_number;
use serde_json::{Map, Value, json};

/// Шаг стрелок, px CSS.
pub const STEP: f64 = 16.;

/// `clampSize`: окно меньше двух минимумов (`max < min`) — побеждает `min`, как у CSS `clamp()`.
pub fn clamp_size(size: f64, min: f64, max: f64) -> f64 {
    math_round(min.max(max.min(size)))
}

/// `readSize`: `None` — размер не запоминали (или в `pane` мусор, или `pane` не объект).
pub fn read_size(pane: Option<&Value>, key: &str) -> Option<f64> {
    let value = to_number(pane?.as_object()?.get(key));
    (value.is_finite() && value > 0.).then_some(value)
}

/// `writeSize`: новый объект `pane` для prefs; `None` — забыть (панель вернётся к размеру по
/// умолчанию). Прежний `pane` не объект — начинается заново.
pub fn write_size(pane: Option<&Value>, key: &str, size: Option<f64>) -> Value {
    let mut map = match pane {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };
    match size {
        Some(size) => map.insert(key.to_owned(), json!(math_round(size) as i64)),
        None => map.remove(key),
    };
    Value::Object(map)
}

/// `keySize`: новый размер по клавише (имена `KeyboardEvent.key`); `None` — клавиша не меняет
/// размер.
pub fn key_size(size: f64, key: &str, min: f64, max: f64) -> Option<f64> {
    match key {
        "ArrowUp" | "ArrowLeft" => Some(clamp_size(size - STEP, min, max)),
        "ArrowDown" | "ArrowRight" => Some(clamp_size(size + STEP, min, max)),
        "Home" => Some(min),
        "End" => Some(clamp_size(max, min, max)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Порт scripts/pane-size-selftest.js 0.4.9.

    #[test]
    fn size_is_clamped_and_min_wins_in_a_tight_window() {
        assert_eq!(clamp_size(250.4, 130., 400.), 250.);
        assert_eq!(clamp_size(90., 130., 400.), 130.);
        assert_eq!(clamp_size(900., 130., 400.), 400.);
        assert_eq!(clamp_size(300., 130., 100.), 130.);
    }

    #[test]
    fn arrows_move_by_16_home_and_end_go_to_the_edges() {
        assert_eq!(STEP, 16.);
        assert_eq!(key_size(200., "ArrowDown", 130., 400.), Some(216.));
        assert_eq!(key_size(200., "ArrowRight", 130., 400.), Some(216.));
        assert_eq!(key_size(200., "ArrowUp", 130., 400.), Some(184.));
        assert_eq!(key_size(200., "ArrowLeft", 130., 400.), Some(184.));
        assert_eq!(key_size(395., "ArrowDown", 130., 400.), Some(400.));
        assert_eq!(key_size(135., "ArrowUp", 130., 400.), Some(130.));
        assert_eq!(key_size(200., "Home", 130., 400.), Some(130.));
        assert_eq!(key_size(200., "End", 130., 400.6), Some(401.));
        assert_eq!(key_size(200., "Enter", 130., 400.), None);
        assert_eq!(key_size(200., "constructor", 130., 400.), None);
    }

    #[test]
    fn size_is_remembered_by_key_reset_forgets_junk_reads_as_none() {
        let pane = write_size(None, "keyboard:hl1-won", Some(311.6));
        assert_eq!(pane, json!({"keyboard:hl1-won": 312}));
        assert_eq!(read_size(Some(&pane), "keyboard:hl1-won"), Some(312.));
        assert_eq!(read_size(Some(&pane), "keyboard:ghosting-renewed"), None);
        let pane = write_size(Some(&pane), "pz-fields:ghosting-renewed", Some(380.));
        let pane = write_size(Some(&pane), "keyboard:hl1-won", None);
        assert_eq!(read_size(Some(&pane), "keyboard:hl1-won"), None);
        assert_eq!(pane, json!({"pz-fields:ghosting-renewed": 380}));

        // Мусор: localStorage 0.4.9 хранил строки — те же правила Number().
        for junk in [
            json!(""),
            json!("abc"),
            json!("-40"),
            json!("0"),
            json!("Infinity"),
            json!(-40),
            json!(0),
            json!(null),
            json!({}),
        ] {
            let pane = json!({ "x": junk });
            assert_eq!(read_size(Some(&pane), "x"), None, "{junk}");
        }
        assert_eq!(read_size(Some(&json!({"x": " 400 "})), "x"), Some(400.));

        // Хранилища нет или оно не объект — размер по умолчанию, запись начинает заново.
        assert_eq!(read_size(None, "x"), None);
        assert_eq!(read_size(Some(&json!([1, 2])), "x"), None);
        assert_eq!(
            write_size(Some(&json!("x")), "x", Some(200.)),
            json!({"x": 200})
        );
    }
}
