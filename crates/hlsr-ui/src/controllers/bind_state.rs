//! Итоговые бинды клавиатуры и «Скриптов» (`renderer/components/Launcher/bind-state.js` 0.4.7):
//! что будет на клавише в игре после запуска, клавиши команды, что было на клавише до лаунчера.
//! Бинды из cfg игры (`keyBinds` ответа `game-settings-get`) — [`key_binds_from_value`].

use hlsr_core::js::object::JsObject;
use hlsr_delivery::bind_presets::command_key;
use hlsr_delivery::game_binds::{
    Bind, BindRead, Overrides, RuntimeBind, RuntimeBinds, alias_map, expand_command,
    sanitize_overrides,
};
use serde_json::Value;

/// Бинд клавиши в игре: команда, файл (`None` — переназначение лаунчера), переназначен ли
/// лаунчером и что было до него.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub command: String,
    pub source: Option<String>,
    pub overridden: bool,
    pub base: Option<String>,
}

/// `effectiveBinds(fileBinds, overrides)`: бинды cfg, поверх них переназначения лаунчера;
/// возврат (`command = base`) — снова бинд игры (`config.cfg`); пустые команды убраны.
pub fn effective_binds(file_binds: &JsObject<Bind>, overrides: &Overrides) -> JsObject<Effective> {
    let mut result = JsObject::new();
    for (key, entry) in file_binds.iter() {
        result.insert(
            key,
            Effective {
                command: entry.command.clone(),
                source: Some(entry.source.clone()),
                overridden: false,
                base: None,
            },
        );
    }
    for (key, entry) in overrides.iter() {
        if entry.command == entry.base {
            if entry.command.is_empty() {
                result.remove(key);
            } else {
                result.insert(
                    key,
                    Effective {
                        command: entry.command.clone(),
                        source: Some("config.cfg".to_owned()),
                        overridden: false,
                        base: None,
                    },
                );
            }
        } else {
            result.insert(
                key,
                Effective {
                    command: entry.command.clone(),
                    source: None,
                    overridden: true,
                    base: Some(entry.base.clone()),
                },
            );
        }
    }
    let empty: Vec<String> = result
        .iter()
        .filter(|(_, entry)| entry.command.is_empty())
        .map(|(key, _)| key.to_owned())
        .collect();
    for key in empty {
        result.remove(&key);
    }
    result
}

/// Команда (ключ [`command_key`]) → клавиши в порядке ключей объекта.
pub type KeysByCommand = JsObject<Vec<String>>;

fn push_key(map: &mut KeysByCommand, command: String, key: &str) {
    match map.get_mut(&command) {
        Some(keys) => keys.push(key.to_owned()),
        None => {
            map.insert(command, vec![key.to_owned()]);
        }
    }
}

/// `keysByCommand(effective)`: команда → клавиши, на которых она стоит.
pub fn keys_by_command(effective: &JsObject<Effective>) -> KeysByCommand {
    let mut map = KeysByCommand::new();
    for (key, entry) in effective.iter() {
        push_key(&mut map, command_key(&entry.command), key);
    }
    map
}

/// `keysByPart(effective, aliases)`: команда → клавиши, где она стоит целиком или частью
/// составного бинда, в том числе внутри перебинда и за цепочкой alias игрока.
pub fn keys_by_part(effective: &JsObject<Effective>, aliases: &JsObject<String>) -> KeysByCommand {
    let mut map = KeysByCommand::new();
    let aliases = alias_map(aliases);
    for (key, entry) in effective.iter() {
        // new Set: ключи в порядке первого появления.
        let mut parts: Vec<String> = vec![command_key(&entry.command)];
        for part in expand_command(&entry.command, &aliases) {
            let part = command_key(&part);
            if !parts.contains(&part) {
                parts.push(part);
            }
        }
        for part in parts {
            push_key(&mut map, part, key);
        }
    }
    map
}

/// `baseOf(key, fileBinds, overrides)`: что было на клавише до лаунчера.
pub fn base_of(key: &str, file_binds: &JsObject<Bind>, overrides: &Overrides) -> String {
    if let Some(entry) = overrides.get(key) {
        return entry.base.clone();
    }
    file_binds
        .get(key)
        .map_or_else(String::new, |bind| bind.command.clone())
}

/// Переназначения лаунчера из черновика (`binds.sanitizeOverrides(this.value)`).
pub fn overrides_of(value: &Value) -> Overrides {
    sanitize_overrides(value)
}

/// `String(value)` для полей cfg: строка — как есть, `null`/нет — пусто.
fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => hlsr_core::js::to_string(other),
    }
}

fn string_map(value: Option<&Value>) -> JsObject<String> {
    let mut map = JsObject::new();
    if let Some(Value::Object(object)) = value {
        for (key, item) in object {
            map.insert(key.clone(), text_of(Some(item)));
        }
    }
    map
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(list)) => list.iter().map(|item| text_of(Some(item))).collect(),
        _ => Vec::new(),
    }
}

/// `runtime` 0.4.9: `{ key: [{ command, file, trigger }] }`; не объект или не список — пусто.
fn runtime_of(value: Option<&Value>) -> RuntimeBinds {
    let mut runtime = RuntimeBinds::new();
    let Some(Value::Object(map)) = value else {
        return runtime;
    };
    for (key, list) in map {
        let Value::Array(list) = list else {
            continue;
        };
        let items: Vec<RuntimeBind> = list
            .iter()
            .filter(|item| item.is_object())
            .map(|item| RuntimeBind {
                command: text_of(item.get("command")),
                file: item
                    .get("file")
                    .filter(|file| !file.is_null())
                    .map(|file| text_of(Some(file))),
                trigger: text_of(item.get("trigger")),
            })
            .collect();
        if !items.is_empty() {
            runtime.insert(key.clone(), items);
        }
    }
    runtime
}

/// `keyBinds` ответа `game-settings-get` (`{binds, aliases, cvars, files, imported, runtime}`);
/// `null`, не объект — `None` (игра не установлена).
pub fn key_binds_from_value(value: Option<&Value>) -> Option<BindRead> {
    let Some(Value::Object(object)) = value else {
        return None;
    };
    let mut binds = JsObject::new();
    if let Some(Value::Object(map)) = object.get("binds") {
        for (key, bind) in map {
            binds.insert(
                key.clone(),
                Bind {
                    command: text_of(bind.get("command")),
                    source: text_of(bind.get("source")),
                },
            );
        }
    }
    Some(BindRead {
        binds,
        aliases: string_map(object.get("aliases")),
        cvars: string_map(object.get("cvars")),
        files: string_list(object.get("files")),
        imported: string_list(object.get("imported")),
        runtime: runtime_of(object.get("runtime")),
    })
}
