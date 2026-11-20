//! Реакции на посты ленты (`shared/news-contract.js`): пять видов, номер поста, счётчики и
//! «мои» реакции после проверки. Одно и то же знают клиент ленты, IPC и окно.

use crate::js_value;
use serde_json::{Map, Value};

/// `REACTIONS`: вид → смайлик, в порядке 0.4.7.
pub const REACTIONS: [(&str, &str); 5] = [
    ("like", "👍"),
    ("love", "❤️"),
    ("fire", "🔥"),
    ("wow", "😮"),
    ("celebrate", "🎉"),
];

/// Предел счётчика: `n <= 1000000000`.
const COUNT_MAX: f64 = 1_000_000_000.0;

/// `validPostId(id)`: число без ведущего нуля, до 19 знаков (`/^[1-9][0-9]{0,18}$/`).
pub fn valid_post_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=19).contains(&bytes.len())
        && (b'1'..=b'9').contains(&bytes[0])
        && bytes.iter().all(u8::is_ascii_digit)
}

/// Вид реакции из каталога: `Object.hasOwn(REACTIONS, key)`.
pub fn reaction_key(key: &str) -> Option<&'static str> {
    REACTIONS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(name, _)| *name)
}

/// Счётчики по видам (`counts`): целые от 0 до 10⁹, остальное — 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reactions {
    pub like: u32,
    pub love: u32,
    pub fire: u32,
    pub wow: u32,
    pub celebrate: u32,
}

impl Reactions {
    /// Счётчик вида `key`.
    pub fn get(&self, key: &str) -> u32 {
        match key {
            "like" => self.like,
            "love" => self.love,
            "fire" => self.fire,
            "wow" => self.wow,
            "celebrate" => self.celebrate,
            _ => 0,
        }
    }

    /// Объект `{like, love, fire, wow, celebrate}` в порядке 0.4.7.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        for (key, _) in REACTIONS {
            map.insert(key.to_owned(), Value::from(self.get(key)));
        }
        Value::Object(map)
    }
}

/// `counts(raw)`: `raw` — значение из ответа сервера (`None` — поля нет).
pub fn counts(raw: Option<&Value>) -> Reactions {
    let one = |key: &str| {
        let n = js_value::field(raw, key);
        match js_value::safe_integer(n) {
            Some(n) if (0..=COUNT_MAX as i64).contains(&n) => n as u32,
            _ => 0,
        }
    };
    Reactions {
        like: one("like"),
        love: one("love"),
        fire: one("fire"),
        wow: one("wow"),
        celebrate: one("celebrate"),
    }
}

/// `mine(raw)`: виды из массива без повторов и чужих значений, в порядке первого появления.
pub fn mine(raw: Option<&Value>) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for item in raw.and_then(Value::as_array).into_iter().flatten() {
        if let Some(key) = item.as_str().and_then(reaction_key)
            && !out.contains(&key)
        {
            out.push(key);
        }
    }
    out
}
