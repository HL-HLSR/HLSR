//! Общее для сценариев сверки канала: фикстура со ссылками, подстановка адресов серверов,
//! нормализация перед сравнением, исходы вызовов, запуск серверов сценария.
//! Скрипт Node — `tools/reference/delivery/channel.js` (тот же формат).

use super::support::http::{RequestLog, Server, dead_port, start_server};
use super::support::{Mismatches, fixture, items, str_of};
use hlsr_core::json_text::{parse_js_json, stringify};
use hlsr_delivery::errors::{DeliveryError, Failure, RawError};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::LazyLock;

pub static FIXTURE: LazyLock<Value> = LazyLock::new(|| expand_refs(fixture("channel.json")));

// ---------------------------------------------------------------- ссылки фикстуры

/// Фикстура сжата повторами (`tools/reference/delivery/lib/intern.js`): объект
/// `{"$ref": "dN"}` — значение из `$defs`. Разворачивается целиком при чтении.
fn expand_refs(mut fixture: Value) -> Value {
    let defs = fixture
        .as_object_mut()
        .and_then(|object| object.remove("$defs"))
        .unwrap_or(Value::Null);
    expand(&fixture, &defs)
}

fn expand(value: &Value, defs: &Value) -> Value {
    match value {
        Value::Array(list) => Value::Array(list.iter().map(|item| expand(item, defs)).collect()),
        Value::Object(object) => {
            if let (1, Some(Value::String(id))) = (object.len(), object.get("$ref")) {
                return expand(&defs[id], defs);
            }
            Value::Object(
                object
                    .iter()
                    .map(|(key, item)| (key.clone(), expand(item, defs)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------- подстановки

/// Подстановка адресов: `{имя}` → `127.0.0.1:<порт>` во всех строках и ключах.
pub fn fill(value: &Value, map: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(text) => Value::String(fill_text(text, map)),
        Value::Array(list) => Value::Array(list.iter().map(|item| fill(item, map)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| (fill_text(key, map), fill(item, map)))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub fn fill_text(text: &str, map: &BTreeMap<String, String>) -> String {
    let mut out = text.to_owned();
    for (name, authority) in map {
        out = out.replace(&format!("{{{name}}}"), authority);
    }
    out
}

/// Обратная замена: адрес → `{имя}`; длинные адреса первыми. В строках с `{root}` разделитель
/// путей Windows (`\`) становится `/`: фикстура снята на Linux.
pub fn unfill(value: &Value, map: &BTreeMap<String, String>) -> Value {
    let mut pairs: Vec<(&String, &String)> = map.iter().collect();
    pairs.sort_by_key(|(_, authority)| std::cmp::Reverse(authority.len()));
    let text = |t: &str| {
        let out = pairs.iter().fold(t.to_owned(), |t, (name, authority)| {
            t.replace(*authority, &format!("{{{name}}}"))
        });
        if cfg!(windows) && out.contains("{root}") {
            out.replace('\\', "/")
        } else {
            out
        }
    };
    map_strings(value, &text)
}

fn map_strings(value: &Value, convert: &dyn Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(convert(text)),
        Value::Array(list) => Value::Array(list.iter().map(|v| map_strings(v, convert)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| (convert(key), map_strings(item, convert)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Временные файлы атомарной записи называются `.<имя>.<pid>.<время36>.tmp`: pid и время
/// заменяются метками (то же выражение, что в скрипте Node).
fn mask_temp(text: &str) -> String {
    static TEMP: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(/\.[^/'\s]+)\.[0-9]+\.[0-9a-z]+\.tmp").unwrap());
    TEMP.replace_all(text, "$1.<pid>.<time>.tmp").into_owned()
}

/// Текст разбора JSON у V8 и у serde_json разный; совпадать обязан код ошибки.
fn mask_parse_message(text: &str) -> String {
    for marker in ["не является JSON: ", "не разбирается как JSON: "] {
        if let Some(at) = text.find(marker) {
            return format!("{}<сообщение разбора>", &text[..at + marker.len()]);
        }
    }
    text.to_owned()
}

/// Числа так, как их видит JS после `JSON.parse`; тексты ошибок разбора и имена временных
/// файлов заменены метками.
pub fn normalize(value: &Value) -> Value {
    match value {
        Value::Array(list) => Value::Array(list.iter().map(normalize).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| (mask_text(key), normalize(item)))
                .collect(),
        ),
        Value::Number(_) => parse_js_json(&stringify(value, "")).unwrap(),
        Value::String(text) => Value::String(mask_text(text)),
        other => other.clone(),
    }
}

fn mask_text(text: &str) -> String {
    mask_temp(&mask_parse_message(text))
}

/// Сравнить с ожиданием 0.4.7: значения, а затем порядок ключей (он виден в записанных файлах
/// и в объектах, которые уходят в окно).
pub fn check(
    m: &mut Mismatches,
    actual: &Value,
    expected: &Value,
    context: impl std::fmt::Display,
) {
    let (actual, expected) = (normalize(actual), normalize(expected));
    if actual != expected {
        m.check(&actual, &expected, context);
    } else if stringify(&actual, "") != stringify(&expected, "") {
        m.check_eq(
            stringify(&actual, "  "),
            stringify(&expected, "  "),
            format!("{context}: порядок ключей"),
        );
    }
}

/// Шаг сценария — объект полей (`result`, `logs`, `tree`…): поля сравниваются по одному, их
/// порядок в записи шага не важен.
pub fn check_entry(
    m: &mut Mismatches,
    actual: &Value,
    expected: &Value,
    context: impl std::fmt::Display,
) {
    let mut keys: Vec<&String> = Vec::new();
    for source in [expected, actual] {
        for key in source.as_object().into_iter().flat_map(Map::keys) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    for key in keys {
        let pick = |entry: &Value| entry.get(key).cloned().unwrap_or(Value::Null);
        check(
            m,
            &pick(actual),
            &pick(expected),
            format!("{context}, поле {key}"),
        );
    }
}

/// Журнал запросов Rust: reqwest всегда шлёт `Accept: */*`, Node — нет.
pub fn without_default_accept(log: &[Value]) -> Vec<Value> {
    log.iter()
        .map(|entry| {
            let mut entry = entry.clone();
            let headers = entry["headers"].as_object_mut().unwrap();
            if headers.get("accept") == Some(&json!("*/*")) {
                headers.shift_remove("accept");
            }
            entry
        })
        .collect()
}

// ---------------------------------------------------------------- исходы

pub fn delivery_json(error: &DeliveryError) -> Value {
    json!({ "error": error.to_public() })
}

pub fn raw_json(error: &RawError) -> Value {
    json!({ "raw": { "code": error.code, "message": error.message } })
}

pub fn failure_json(failure: &Failure) -> Value {
    match failure {
        Failure::Delivery(error) => delivery_json(error),
        Failure::Raw(error) => raw_json(error),
    }
}

pub fn ok(value: Value) -> Value {
    json!({ "ok": value })
}

// ---------------------------------------------------------------- серверы сценария

pub struct World {
    pub map: BTreeMap<String, String>,
    pub log: RequestLog,
    _servers: Vec<Server>,
}

/// Запустить серверы сценария (`servers`: имя → маршруты) и адрес без слушателя `{dead}`.
pub async fn start_world(scenario: &Value) -> World {
    let log = RequestLog::default();
    let mut map = BTreeMap::new();
    map.insert(
        "dead".to_owned(),
        format!("127.0.0.1:{}", dead_port().await),
    );
    let mut servers: Vec<Server> = Vec::new();
    if let Some(sources) = scenario["servers"].as_object() {
        for name in sources.keys() {
            let server = start_server(name, log.clone()).await;
            map.insert(name.clone(), server.authority.clone());
            servers.push(server);
        }
        for server in &servers {
            let routes = fill(&sources[&server.name], &map);
            *server.routes.lock().unwrap() = routes.as_object().unwrap().clone();
        }
    }
    World {
        map,
        log,
        _servers: servers,
    }
}

impl World {
    /// Запросы, записанные с позиции `mark`, и новая позиция.
    pub fn requests_since(&self, mark: usize) -> (Vec<Value>, usize) {
        let log = self.log.lock().unwrap();
        (without_default_accept(&log[mark..]), log.len())
    }
}

/// Все будущие разом, итоги — в исходном порядке; опрос по кругу, начиная с первого.
pub async fn join_all<F: Future<Output = Value>>(futures: impl Iterator<Item = F>) -> Vec<Value> {
    let mut tasks: Vec<_> = futures.map(|future| Some(Box::pin(future))).collect();
    let mut results: Vec<Option<Value>> = vec![None; tasks.len()];
    std::future::poll_fn(|cx| {
        for (task, result) in tasks.iter_mut().zip(results.iter_mut()) {
            if let Some(future) = task
                && let std::task::Poll::Ready(value) = future.as_mut().poll(cx)
            {
                *result = Some(value);
                *task = None;
            }
        }
        if tasks.iter().all(Option::is_none) {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    results.into_iter().map(Option::unwrap).collect()
}

// ---------------------------------------------------------------- прочее

/// Сценарий фикстуры по имени.
pub fn scenario<'a>(section: &'a Value, name: &str) -> &'a Value {
    items(section)
        .iter()
        .find(|item| item["name"] == name)
        .unwrap_or_else(|| panic!("нет сценария {name}"))
}

/// Имена сценариев раздела — для проверки, что тесты покрывают все.
pub fn names(section: &Value) -> Vec<&str> {
    items(section)
        .iter()
        .map(|item| str_of(&item["name"]))
        .collect()
}
