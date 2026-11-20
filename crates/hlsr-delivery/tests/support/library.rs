//! Сценарии транзакций над библиотекой (установка, патчи): нормализация вывода, итог шага,
//! дерево библиотеки с текстом файлов состояния, раскладка с архивами и JSON, копия библиотеки на
//! момент сбоя, лента журнала. Зеркало общих частей `tools/reference/delivery/install.js` и
//! `tools/reference/delivery/patch.js`.

use super::scenario::{self, path_in};
use super::{items, opt_str, str_of};
use hlsr_core::js;
use hlsr_core::json_text::stringify;
use hlsr_delivery::errors::Failure;
use hlsr_delivery::ports::Logger;
use regex::Regex;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------- нормализация

/// Замена в тексте после замены временной папки.
pub enum Rule {
    /// Подстрока → текст.
    Literal(String, String),
    /// Регулярное выражение → текст.
    Pattern(Regex, String),
}

/// То, что в выводе зависит от запуска: временная папка и правила сценария (`n(text)` скрипта).
pub struct Norm {
    root: String,
    escaped: String,
    pid: String,
    rules: Vec<Rule>,
    stderr: Option<Regex>,
}

impl Norm {
    /// `stderr` — текст ошибки 7-Zip → `<stderr>` (node-7z берёт первый кусок stderr, а куски
    /// зависят от времени).
    pub fn new(root: &Path, rules: Vec<Rule>, stderr: bool) -> Norm {
        let root = root.to_string_lossy().into_owned();
        let quoted = serde_json::to_string(&root).unwrap();
        Norm {
            escaped: quoted[1..quoted.len() - 1].to_owned(),
            pid: std::process::id().to_string(),
            rules,
            // До конца строки JSON; в строке значения 7za.exe оставляет «\» в конце.
            stderr: stderr.then(|| Regex::new(r#"Ошибка распаковки: (?:[^"\\]|\\.)*\\?"#).unwrap()),
            root,
        }
    }

    pub fn pid(&self) -> &str {
        &self.pid
    }

    /// Временная папка, как она записана внутри строки JSON.
    pub fn escaped_root(&self) -> &str {
        &self.escaped
    }

    /// `n(text)` скрипта.
    pub fn text(&self, text: &str) -> String {
        let mut out = text
            .replace(&self.escaped, "<root>")
            .replace(&self.root, "<root>");
        for rule in &self.rules {
            out = match rule {
                Rule::Literal(from, to) => out.replace(from, to),
                Rule::Pattern(from, to) => from.replace_all(&out, to.as_str()).into_owned(),
            };
        }
        if let Some(stderr) = &self.stderr {
            out = stderr
                .replace_all(&out, "Ошибка распаковки: <stderr>")
                .into_owned();
        }
        out
    }

    /// Строка значения: pid процесса — `<pid>`; в Windows разделитель путей — `/`, как в
    /// фикстуре из Linux.
    pub fn string(&self, text: &str) -> String {
        if text == self.pid {
            return "<pid>".to_owned();
        }
        let out = self.text(text);
        if cfg!(windows) {
            out.replace('\\', "/")
        } else {
            out
        }
    }

    /// `normalizeValue`: строки значения после нормализации.
    pub fn value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.string(text)),
            Value::Array(list) => Value::Array(list.iter().map(|v| self.value(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| (key.clone(), self.value(item)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// `normalizeBytes`: текст UTF-8 — как UTF-8, прочее — как latin1. В Windows в тексте JSON
    /// разделитель путей записан `\\`.
    pub fn bytes(&self, bytes: &[u8]) -> Vec<u8> {
        match std::str::from_utf8(bytes) {
            Ok(text) => {
                let out = self.text(text);
                let out = if cfg!(windows) {
                    out.replace("\\\\", "/")
                } else {
                    out
                };
                out.into_bytes()
            }
            Err(_) => js::latin1_bytes(&self.text(&js::latin1_string(bytes))),
        }
    }
}

// ---------------------------------------------------------------- итог шага

/// Итог шага: значение, `DeliveryError.toPublic()` или код и текст чужой ошибки.
pub fn outcome(n: &Norm, result: Result<Value, Failure>) -> Value {
    match result {
        Ok(value) => json!({"ok": n.value(&value)}),
        Err(failure) => failure_of(n, &failure),
    }
}

pub fn failure_of(n: &Norm, failure: &Failure) -> Value {
    match failure {
        Failure::Delivery(error) => json!({"error": n.value(&error.to_public())}),
        Failure::Raw(error) => {
            json!({"raw": {"code": error.code, "message": n.string(&error.message)}})
        }
    }
}

/// Дерево папки `rel`: пути, размеры и хеши — после нормализации; у файлов, чей путь подходит
/// под `state_file`, — ещё и текст latin1.
pub fn tree_of(root: &Path, n: &Norm, rel: &str, state_file: &Regex) -> Value {
    let base = path_in(root, rel);
    if !base.exists() {
        return Value::Null;
    }
    let texts: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    let transform = |file: &str, bytes: Vec<u8>| {
        let out = n.bytes(&bytes);
        texts
            .borrow_mut()
            .insert(file.to_owned(), js::latin1_string(&out));
        out
    };
    let mut tree = super::tree::snapshot(&base, Some(&transform));
    let texts = texts.into_inner();
    for entry in tree.as_array_mut().unwrap() {
        let path = str_of(&entry["path"]).to_owned();
        if entry["kind"] == "file"
            && state_file.is_match(&path)
            && entry["size"].as_u64().unwrap() <= scenario::TEXT_LIMIT
        {
            entry["latin1"] = json!(texts[&path]);
        }
        if entry["kind"] == "symlink" {
            entry["target"] = json!(n.text(str_of(&entry["target"])));
        }
        entry["path"] = json!(n.text(&path));
    }
    tree.as_array_mut()
        .unwrap()
        .sort_by(|a, b| js::cmp_utf16(str_of(&a["path"]), str_of(&b["path"])));
    tree
}

// ---------------------------------------------------------------- раскладка

/// `fillValue`: `"<root>/…"` → путь во временной папке, `"<pid>"` → pid процесса.
pub fn fill_value(value: &Value, root: &Path) -> Value {
    match value {
        Value::String(text) if text == "<pid>" => json!(std::process::id()),
        Value::String(text) => match text.strip_prefix("<root>") {
            Some(rest) => json!(format!("{}{rest}", root.display())),
            None => value.clone(),
        },
        Value::Array(list) => Value::Array(list.iter().map(|v| fill_value(v, root)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), fill_value(item, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub fn fill_str(value: &Value, root: &Path) -> Option<String> {
    fill_value(value, root).as_str().map(str::to_owned)
}

/// Разложить `setup`: `{archive, to}` — копия архива-фикстуры (путь даёт `archive_file`),
/// `{json, value, atomic}` — JSON с отступом 2 (и `\n`, если `atomic`), прочее — как в
/// [`scenario::setup`].
pub fn setup(root: &Path, list: &Value, archive_file: &dyn Fn(&str) -> PathBuf) {
    let mut plain = Vec::new();
    for item in items(list) {
        if let Some(name) = opt_str(&item["archive"]) {
            let target = path_in(root, str_of(&item["to"]));
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::copy(archive_file(name), &target).unwrap();
        } else if let Some(rel) = opt_str(&item["json"]) {
            let target = path_in(root, rel);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            let mut text = stringify(&fill_value(&item["value"], root), "  ");
            if item["atomic"] == true {
                text.push('\n');
            }
            std::fs::write(&target, text).unwrap();
        } else {
            plain.push(item.clone());
        }
    }
    scenario::setup(root, &Value::Array(plain));
}

/// Копия дерева (ссылки — как есть): то, что осталось бы на диске после сбоя процесса.
pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        let target = to.join(entry.file_name());
        if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(entry.path()).unwrap(), &target).unwrap();
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Шаг `restoreCrash`: копия библиотеки на момент сбоя — на место библиотеки.
pub fn restore_crash(lib: &Path, crash: &Path) {
    assert!(crash.exists(), "нет копии библиотеки на момент сбоя");
    scenario::open_up(lib);
    std::fs::remove_dir_all(lib).unwrap();
    std::fs::rename(crash, lib).unwrap();
}

// ---------------------------------------------------------------- лента шага

/// События, строки журнала и точки отказа шага в порядке появления.
pub type Timeline = Arc<Mutex<Vec<Value>>>;

pub fn push(timeline: &Timeline, entry: Value) {
    timeline.lock().unwrap().push(entry);
}

/// Забрать ленту шага.
pub fn take(timeline: &Timeline) -> Value {
    Value::Array(std::mem::take(&mut *timeline.lock().unwrap()))
}

/// Журнал сервиса → лента шага (`{log, message}` после нормализации).
pub struct TimelineLogger {
    pub timeline: Timeline,
    pub norm: Arc<Norm>,
}

impl TimelineLogger {
    fn log(&self, level: &str, message: &str) {
        push(
            &self.timeline,
            json!({"log": level, "message": self.norm.string(message)}),
        );
    }
}

impl Logger for TimelineLogger {
    fn info(&self, message: &str) {
        self.log("info", message);
    }
    fn warn(&self, message: &str) {
        self.log("warn", message);
    }
    fn error(&self, message: &str) {
        self.log("error", message);
    }
}
