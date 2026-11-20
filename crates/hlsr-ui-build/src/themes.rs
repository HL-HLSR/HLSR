//! Токены тем: разбор и проверка `themes.json` (`tools/reference/export-themes.js`) —
//! значения по темам и ступени сетки по размеру окна.

use crate::{BuildError, invalid};
use serde_json::Value as Json;
use std::fs;
use std::path::Path;

/// Значение переменной темы.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `0xRRGGBBAA`.
    Color(u32),
    Gradient(Gradient),
    /// Пиксели CSS.
    Length(f64),
    Number(f64),
    /// Миллисекунды.
    Duration(f64),
    /// `cubic-bezier(x1, y1, x2, y2)`.
    Easing([f64; 4]),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    /// Градусы, как в CSS: 180 — сверху вниз.
    pub angle: f64,
    pub stops: Vec<Stop>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stop {
    pub color: u32,
    /// 0…1.
    pub pos: f64,
}

/// Условие `@media`: `(min-height: 708px)` и т. п.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Condition {
    pub feature: Feature,
    pub px: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    MinWidth,
    MaxWidth,
    MinHeight,
    MaxHeight,
}

impl Condition {
    /// Выполняется ли условие для окна в CSS-пикселях.
    pub fn matches(&self, width: f64, height: f64) -> bool {
        match self.feature {
            Feature::MinWidth => width >= self.px,
            Feature::MaxWidth => width <= self.px,
            Feature::MinHeight => height >= self.px,
            Feature::MaxHeight => height <= self.px,
        }
    }
}

/// Значения сетки при одном наборе выполненных условий.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutCase {
    pub matches: Vec<bool>,
    /// В порядке `ThemeTable::layout_keys`.
    pub values: Vec<f64>,
}

/// Таблица `themes.json`.
#[derive(Debug, Clone)]
pub struct ThemeTable {
    order: Vec<String>,
    keys: Vec<String>,
    /// `values[тема][ключ]` в порядке `order` и `keys`.
    values: Vec<Vec<Value>>,
    unsupported: Vec<(String, String)>,
    conditions: Vec<Condition>,
    layout_keys: Vec<String>,
    cases: Vec<LayoutCase>,
    /// Индекс случая, где не выполнено ни одно условие.
    base_case: Option<usize>,
}

impl ThemeTable {
    /// Прочитать и проверить `themes.json`.
    pub fn load(path: &Path) -> Result<ThemeTable, BuildError> {
        let text = fs::read_to_string(path).map_err(|source| BuildError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let json: Json = serde_json::from_str(&text).map_err(|source| BuildError::Json {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json(path, &json)
    }

    fn from_json(path: &Path, json: &Json) -> Result<ThemeTable, BuildError> {
        let err = |message: String| invalid(path, message);

        let order: Vec<String> = json["order"]
            .as_array()
            .ok_or_else(|| err("нет order".into()))?
            .iter()
            .map(|id| {
                id.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| err(format!("order: {id}")))
            })
            .collect::<Result<_, _>>()?;
        if order.is_empty() {
            return Err(err("order пуст".into()));
        }
        let themes = json["themes"]
            .as_object()
            .ok_or_else(|| err("нет themes".into()))?;
        if themes.len() != order.len() || order.iter().any(|id| !themes.contains_key(id)) {
            return Err(err(format!(
                "темы {:?} не совпадают с order",
                themes.keys().collect::<Vec<_>>()
            )));
        }

        let first = themes[&order[0]]
            .as_object()
            .ok_or_else(|| err(format!("{}: не объект", order[0])))?;
        let keys: Vec<String> = first.keys().cloned().collect();
        let mut values = Vec::with_capacity(order.len());
        for id in &order {
            let theme = themes[id]
                .as_object()
                .ok_or_else(|| err(format!("{id}: не объект")))?;
            if theme.keys().ne(keys.iter()) {
                return Err(err(format!("{id}: ключи не совпадают с {}", order[0])));
            }
            let parsed = theme
                .iter()
                .map(|(key, value)| {
                    parse_value(value).map_err(|message| err(format!("{id}.{key}: {message}")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            values.push(parsed);
        }

        let table_keys = &keys;
        for (index, key) in table_keys.iter().enumerate() {
            let base = &values[0][index];
            for (theme, id) in values.iter().zip(&order) {
                let value = &theme[index];
                if std::mem::discriminant(value) != std::mem::discriminant(base) {
                    return Err(err(format!("{key}: в {id} другой тип")));
                }
                match (base, value) {
                    (Value::Gradient(a), Value::Gradient(b)) if a != b => {
                        let same_shape = a.angle == b.angle
                            && a.stops.len() == b.stops.len()
                            && a.stops.iter().zip(&b.stops).all(|(x, y)| x.pos == y.pos);
                        if !same_shape {
                            return Err(err(format!("{key}: в {id} градиент другой формы")));
                        }
                    }
                    (Value::Easing(a), Value::Easing(b)) if a != b => {
                        return Err(err(format!("{key}: кривая меняется с темой")));
                    }
                    _ => {}
                }
            }
        }

        let unsupported = match &json["unsupported"] {
            Json::Object(map) => map
                .iter()
                .map(|(key, value)| {
                    value
                        .as_str()
                        .map(|text| (key.clone(), text.to_owned()))
                        .ok_or_else(|| err(format!("unsupported.{key}: не строка")))
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err(err("нет unsupported".into())),
        };
        if !json["scoped"].is_object() {
            return Err(err("нет scoped".into()));
        }

        let mut table = ThemeTable {
            order,
            keys,
            values,
            unsupported,
            conditions: Vec::new(),
            layout_keys: Vec::new(),
            cases: Vec::new(),
            base_case: None,
        };
        table.load_layout(path, &json["layout"])?;
        Ok(table)
    }

    fn load_layout(&mut self, path: &Path, layout: &Json) -> Result<(), BuildError> {
        let err = |message: String| invalid(path, message);
        let conditions = layout["conditions"]
            .as_array()
            .ok_or_else(|| err("нет layout.conditions".into()))?;
        for condition in conditions {
            let feature = match condition["feature"].as_str() {
                Some("min-width") => Feature::MinWidth,
                Some("max-width") => Feature::MaxWidth,
                Some("min-height") => Feature::MinHeight,
                Some("max-height") => Feature::MaxHeight,
                _ => return Err(err(format!("layout: условие {condition}"))),
            };
            let px = condition["px"]
                .as_f64()
                .ok_or_else(|| err(format!("layout: условие {condition}")))?;
            self.conditions.push(Condition { feature, px });
        }

        let cases = layout["cases"]
            .as_array()
            .ok_or_else(|| err("нет layout.cases".into()))?;
        for (number, case) in cases.iter().enumerate() {
            let matches: Vec<bool> = case["matches"]
                .as_array()
                .ok_or_else(|| err(format!("layout.cases[{number}]: нет matches")))?
                .iter()
                .map(|value| {
                    value
                        .as_bool()
                        .ok_or_else(|| err(format!("layout.cases[{number}]: matches")))
                })
                .collect::<Result<_, _>>()?;
            if matches.len() != self.conditions.len() {
                return Err(err(format!(
                    "layout.cases[{number}]: {} условий вместо {}",
                    matches.len(),
                    self.conditions.len()
                )));
            }
            if self.cases.iter().any(|known| known.matches == matches) {
                return Err(err(format!("layout.cases[{number}]: повтор {matches:?}")));
            }
            let values = case["values"]
                .as_object()
                .ok_or_else(|| err(format!("layout.cases[{number}]: нет values")))?;
            if number == 0 {
                self.layout_keys = values.keys().cloned().collect();
            } else if values.keys().ne(self.layout_keys.iter()) {
                return Err(err(format!("layout.cases[{number}]: другие ключи")));
            }
            let mut lengths = Vec::with_capacity(values.len());
            for (key, value) in values {
                match parse_value(value) {
                    Ok(Value::Length(px)) => lengths.push(px),
                    _ => return Err(err(format!("layout.cases[{number}].{key}: не длина"))),
                }
            }
            if matches.iter().all(|matched| !matched) {
                self.base_case = Some(self.cases.len());
            }
            self.cases.push(LayoutCase {
                matches,
                values: lengths,
            });
        }

        let varying = self.varying_keys();
        for key in &self.layout_keys {
            let Some(index) = self.index(key) else {
                return Err(err(format!("layout: {key} нет в темах")));
            };
            if varying.contains(&key.as_str()) {
                return Err(err(format!("layout: {key} меняется и с темой")));
            }
            if !matches!(self.values[0][index], Value::Length(_)) {
                return Err(err(format!("layout: {key} в темах не длина")));
            }
        }
        if !self.cases.is_empty() && self.base_case.is_none() {
            return Err(err("layout: нет случая без условий".into()));
        }
        Ok(())
    }

    /// Темы в порядке `themes.json`.
    pub fn order(&self) -> &[String] {
        &self.order
    }

    /// Ключи темы `id` (имена переменных без `--`); у всех тем одинаковые.
    pub fn keys(&self, id: &str) -> Vec<&str> {
        match self.theme(id) {
            Some(_) => self.keys.iter().map(String::as_str).collect(),
            None => Vec::new(),
        }
    }

    pub fn value(&self, id: &str, key: &str) -> Option<&Value> {
        Some(&self.values[self.theme(id)?][self.index(key)?])
    }

    /// Цвет как `#RRGGBBAA`.
    pub fn color(&self, id: &str, key: &str) -> Option<String> {
        match self.value(id, key)? {
            Value::Color(rgba) => Some(hex(*rgba)),
            _ => None,
        }
    }

    pub fn length(&self, id: &str, key: &str) -> Option<f64> {
        match self.value(id, key)? {
            Value::Length(px) => Some(*px),
            _ => None,
        }
    }

    /// Переменные, которые Slint не выразить одним значением: тени, несколько
    /// градиентов, длины от размера окна. Значение — строка темы `hlsr`.
    pub fn unsupported(&self) -> &[(String, String)] {
        &self.unsupported
    }

    /// Ключи, значение которых различается между темами.
    pub fn varying_keys(&self) -> Vec<&str> {
        self.keys
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                self.values
                    .iter()
                    .any(|theme| theme[*index] != self.values[0][*index])
            })
            .map(|(_, key)| key.as_str())
            .collect()
    }

    /// Ключи, значение которых меняется со ступенью окна.
    pub fn layout_keys(&self) -> Vec<&str> {
        self.layout_keys.iter().map(String::as_str).collect()
    }

    /// Значения сетки для окна `width`×`height` в CSS-пикселях.
    pub fn layout_for(&self, width: f32, height: f32) -> Vec<(&str, f64)> {
        let Some(case) = self.case_for(width as f64, height as f64) else {
            return Vec::new();
        };
        self.layout_keys
            .iter()
            .map(String::as_str)
            .zip(self.cases[case].values.iter().copied())
            .collect()
    }

    fn case_for(&self, width: f64, height: f64) -> Option<usize> {
        let matches: Vec<bool> = self
            .conditions
            .iter()
            .map(|condition| condition.matches(width, height))
            .collect();
        self.cases
            .iter()
            .position(|case| case.matches == matches)
            .or(self.base_case)
    }

    fn theme(&self, id: &str) -> Option<usize> {
        self.order.iter().position(|known| known == id)
    }

    fn index(&self, key: &str) -> Option<usize> {
        self.keys.iter().position(|known| known == key)
    }
}

fn parse_value(json: &Json) -> Result<Value, String> {
    let object = json
        .as_object()
        .filter(|map| map.len() == 1)
        .ok_or("ожидался объект с одним полем")?;
    let (kind, value) = object.iter().next().expect("одно поле");
    let number = |value: &Json| value.as_f64().ok_or_else(|| format!("{kind}: не число"));
    match kind.as_str() {
        "color" => parse_color(value).map(Value::Color),
        "gradient" => {
            let angle = number(&value["angle"])?;
            let stops = value["stops"]
                .as_array()
                .filter(|stops| stops.len() >= 2)
                .ok_or("gradient: меньше двух точек")?
                .iter()
                .map(|stop| {
                    Ok(Stop {
                        color: parse_color(&stop["color"])?,
                        pos: number(&stop["pos"])?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(Value::Gradient(Gradient { angle, stops }))
        }
        "length" => number(value).map(Value::Length),
        "number" => number(value).map(Value::Number),
        "duration" => number(value).map(Value::Duration),
        "easing" => {
            let list = value
                .as_array()
                .filter(|list| list.len() == 4)
                .ok_or("easing: не четыре числа")?;
            let mut points = [0.0; 4];
            for (slot, item) in points.iter_mut().zip(list) {
                *slot = number(item)?;
            }
            Ok(Value::Easing(points))
        }
        other => Err(format!("неизвестный тип {other}")),
    }
}

fn parse_color(json: &Json) -> Result<u32, String> {
    let text = json.as_str().ok_or("цвет не строка")?;
    match text.strip_prefix('#') {
        Some(digits) if digits.len() == 8 && digits.bytes().all(|b| b.is_ascii_hexdigit()) => {
            u32::from_str_radix(digits, 16).map_err(|error| error.to_string())
        }
        _ => Err(format!("цвет {text} не #RRGGBBAA")),
    }
}

fn hex(rgba: u32) -> String {
    format!("#{rgba:08X}")
}
