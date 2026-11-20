//! Цепочка патчей (`patch-chain.js`): кратчайший путь от установленной версии к целевой по
//! объявленным патчам или полная сборка, если пути нет, версия понижается или патчи
//! не легче сборки.

use crate::js_value::{self as jv, prop};
use crate::semver;
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet, VecDeque};

/// Вид плана.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    Full,
    UpToDate,
    Patches,
}

impl PlanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanKind::Full => "full",
            PlanKind::UpToDate => "up-to-date",
            PlanKind::Patches => "patches",
        }
    }
}

/// Параметры `planUpdate`: патчи — объекты с `fromVersion`, `toVersion`, `archive.size`.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions<'a> {
    pub patches: &'a [Value],
    /// Установленная версия; пусто — не установлена.
    pub from_version: Option<&'a str>,
    pub to_version: Option<&'a str>,
    /// Размер полной сборки; ложное значение JS (`0`, `NaN`) — `0`.
    pub full_size: Option<f64>,
}

/// Итог `planUpdate`.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdatePlan {
    pub kind: PlanKind,
    /// `not-installed`, `downgrade`, `no-chain`, `patches-larger` у полной сборки.
    pub reason: Option<&'static str>,
    pub steps: Vec<Value>,
    pub total_size: f64,
    /// Есть у планов `patches-larger` и `patches`.
    pub full_size: Option<f64>,
    pub message: Option<String>,
}

impl UpdatePlan {
    /// Объект плана как у 0.4.7: набор и порядок ключей зависят от вида.
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("kind".to_owned(), json!(self.kind.as_str()));
        if let Some(reason) = self.reason {
            object.insert("reason".to_owned(), json!(reason));
        }
        object.insert("steps".to_owned(), Value::Array(self.steps.clone()));
        object.insert("totalSize".to_owned(), js_number(self.total_size));
        if let Some(full_size) = self.full_size {
            object.insert("fullSize".to_owned(), js_number(full_size));
        }
        if let Some(message) = &self.message {
            object.insert("message".to_owned(), json!(message));
        }
        Value::Object(object)
    }
}

fn plan(kind: PlanKind, reason: Option<&'static str>, total_size: f64) -> UpdatePlan {
    UpdatePlan {
        kind,
        reason,
        steps: Vec::new(),
        total_size,
        full_size: None,
        message: None,
    }
}

/// `planUpdate(options)`.
pub fn plan_update(options: &PlanOptions) -> UpdatePlan {
    let full_size = options
        .full_size
        .filter(|size| *size != 0.0 && !size.is_nan())
        .unwrap_or(0.0);
    let Some(from) = options.from_version.filter(|from| !from.is_empty()) else {
        return plan(PlanKind::Full, Some("not-installed"), full_size);
    };
    let to = options.to_version;

    if Some(from) == to {
        return plan(PlanKind::UpToDate, None, 0.0);
    }
    if semver::compare(from, to.unwrap_or_default()).is_gt() {
        return plan(PlanKind::Full, Some("downgrade"), full_size);
    }

    // Патчи по исходной версии, в порядке объявления.
    let mut by_from: HashMap<&str, Vec<&Value>> = HashMap::new();
    for patch in options.patches {
        if let Some(version) = prop(patch, "fromVersion").and_then(Value::as_str) {
            by_from.entry(version).or_default().push(patch);
        }
    }

    let mut queue: VecDeque<(&str, Vec<&Value>)> = VecDeque::from([(from, Vec::new())]);
    let mut visited: HashSet<&str> = HashSet::from([from]);
    let mut found: Option<Vec<&Value>> = None;

    'search: while let Some((version, steps)) = queue.pop_front() {
        for patch in by_from.get(version).into_iter().flatten() {
            let Some(next) = prop(patch, "toVersion").and_then(Value::as_str) else {
                continue;
            };
            if visited.contains(next) {
                continue;
            }
            let mut path = steps.clone();
            path.push(patch);
            if Some(next) == to {
                found = Some(path);
                break 'search;
            }
            visited.insert(next);
            queue.push_back((next, path));
        }
    }

    let Some(steps) = found else {
        return UpdatePlan {
            message: Some(format!(
                "Нет цепочки патчей от {from} к {}",
                to.unwrap_or("undefined")
            )),
            ..plan(PlanKind::Full, Some("no-chain"), full_size)
        };
    };

    let total_size: f64 = steps
        .iter()
        .map(
            |patch| match prop(patch, "archive").filter(|archive| js::truthy(archive)) {
                Some(archive) => prop(archive, "size")
                    .and_then(jv::number)
                    .unwrap_or(f64::NAN),
                None => 0.0,
            },
        )
        .sum();
    let steps: Vec<Value> = steps.into_iter().cloned().collect();

    if full_size != 0.0 && total_size >= full_size {
        return UpdatePlan {
            kind: PlanKind::Full,
            reason: Some("patches-larger"),
            steps,
            total_size,
            full_size: Some(full_size),
            message: Some(format!(
                "Патчи весят {} байт против {} у полной сборки",
                js::to_string(&js_number(total_size)),
                js::to_string(&js_number(full_size))
            )),
        };
    }

    UpdatePlan {
        kind: PlanKind::Patches,
        reason: None,
        steps,
        total_size,
        full_size: Some(full_size),
        message: None,
    }
}
