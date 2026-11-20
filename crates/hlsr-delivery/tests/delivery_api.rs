//! Фасад доставки против 0.4.7: фикстура `delivery-api.json` из
//! `tools/reference/delivery/delivery-api.js`.
//!
//! `DeliveryApi` с готовой службой доставки (встроенные манифесты, канал и сеть выключены, у
//! сценария «экспорт и импорт сборки» — свой манифест ghosting-renewed 1.0.0), настоящим 7za и
//! фейками окна: диалоги, оболочка ОС, уведомления, телеметрия, достижения, presence, оверлей
//! LiveSplit, процессы игр. Каналы вызываются по сценарию скрипта; сравниваются ответ, события
//! окна (у статусов — без полей, зависящих от времени; подряд одинаковые — одно), вызовы фейков,
//! журнал и деревья файлов.
//!
//! Нормализация — та же, что в скрипте: временная папка → `<root>`, pid и время в имени
//! временного файла атомарной записи, случайные имена папок экспорта и импорта, токен
//! подтверждения импорта, время изменения файлов, размер архива `.hlsrpack`.

mod support;

use hlsr_core::json_text::stringify;
use hlsr_delivery::api::DeliveryEvent;
use hlsr_delivery::game_actions::LauncherIdentity;
use hlsr_delivery::game_delivery_service::Registry;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use support::api::{Setup, harness};
use support::library::{Norm, Rule, tree_of};
use support::scenario::{path_in, set_mtime, temp_root};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("delivery-api.json"));

/// Файлы, у которых в дереве записан текст.
static STATE_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.(json|cfg|desktop|hlsrbuild|ini)$").unwrap());

/// Чем запущен лаунчер (как в скрипте): пробел и `%` проверяют экранирование `Exec=`.
const LAUNCHER_EXEC: &str = "/opt/HLSR 100%/hlsr";
const LAUNCHER_APP: &str = "/opt/HLSR 100%/resources/app";

/// Время файлов игры (`MTIME_MS`), мс.
const MTIME_MS: i64 = 1_704_164_645_000;

/// Поля статуса, зависящие от времени и кусков вывода 7-Zip: в событиях не сравниваются.
const VOLATILE: [&str; 6] = [
    "percent",
    "bytesDownloaded",
    "speedBps",
    "etaSeconds",
    "checked",
    "totalChecked",
];

fn now() -> i64 {
    FIXTURE["now"].as_i64().unwrap()
}

fn iso(ms: i64) -> String {
    hlsr_delivery::timing::iso_from_ms(ms)
}

// ---------------------------------------------------------------- нормализация

/// `withoutTimes`: время изменения файлов — от часов ОС.
fn without_times(value: &Value) -> Value {
    match value {
        Value::Array(list) => Value::Array(list.iter().map(without_times).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| {
                    let time = (key == "mtime" || key == "modifiedAt") && !item.is_null();
                    let item = if time {
                        json!("<mtime>")
                    } else {
                        without_times(item)
                    };
                    (key.clone(), item)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Что добавил 0.4.9 (задача T0), а в фикстуре 0.4.7 этого нет: `vulkan` в настройках игры
/// (рядом с `startTimer`; строка в `game-settings.json`, на те же байты длиннее архив сборки),
/// `displayName` у записей игры. Из ответа это убирается, текст файла пересчитывается без строки
/// `vulkan`, размер архива при импорте (`totalBytes` статусов операции `import`) — метка с обеих сторон.
/// Задача B: `fileRepair` в статусе; в `game-menu-state` проверка и ремонт включены по списку игр
/// (в 0.4.7 — «в разработке» у всех, кроме сборок Steam), «Настройки» без установки выключены.
fn as_0_4_7(value: &Value) -> Value {
    match value {
        Value::Array(list) => Value::Array(list.iter().map(as_0_4_7).collect()),
        Value::Object(map) => {
            let settings = map.contains_key("startTimer");
            let importing = map.get("operation").and_then(Value::as_str) == Some("import");
            let mut out: Map<String, Value> = map
                .iter()
                .filter(|(key, _)| {
                    !matches!(key.as_str(), "displayName" | "fileRepair")
                        && !(settings && key.as_str() == "vulkan")
                })
                .map(|(key, item)| (key.clone(), as_0_4_7(item)))
                .collect();
            if let (Some(Value::Object(items)), Some(busy)) =
                (out.get_mut("items"), map.get("busy"))
            {
                let in_development = json!({"enabled": false, "reason": "in-development"});
                for key in ["verify", "repair"] {
                    if items
                        .get(key)
                        .is_some_and(|item| item["reason"] != "steam-managed")
                    {
                        items.insert(key.into(), in_development.clone());
                    }
                }
                if items
                    .get("settings")
                    .is_some_and(|item| item["reason"] == "not-installed")
                {
                    let enabled = *busy != true;
                    let reason = if enabled { Value::Null } else { json!("busy") };
                    items.insert(
                        "settings".into(),
                        json!({"enabled": enabled, "reason": reason}),
                    );
                }
            }
            if importing && out.contains_key("totalBytes") {
                out.insert("totalBytes".into(), json!("<size>"));
            }
            if let Some(text) = out.get("latin1").and_then(Value::as_str)
                && text.contains("\"vulkan\": ")
            {
                let text: String = text
                    .split_inclusive('\n')
                    .filter(|line| !line.trim_start().starts_with("\"vulkan\": "))
                    .collect();
                let bytes: Vec<u8> = text.chars().map(|c| c as u8).collect();
                out.insert("size".into(), json!(bytes.len()));
                out.insert("sha256".into(), json!(support::sha256_hex(&bytes)));
                out.insert("latin1".into(), json!(text));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Подряд одинаковые элементы — один.
fn collapse(list: Vec<Value>) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for item in list {
        if out
            .last()
            .is_none_or(|last| stringify(last, "") != stringify(&item, ""))
        {
            out.push(item);
        }
    }
    Value::Array(out)
}

fn project_event(event: &DeliveryEvent) -> Value {
    let mut payload = event.payload.clone();
    if event.channel == "game-delivery-status"
        && let Some(map) = payload.as_object_mut()
    {
        for key in VOLATILE {
            map.shift_remove(key);
        }
    }
    json!({"channel": event.channel, "payload": payload})
}

/// `withoutStack`: стек JS в строке журнала не сравнивается.
fn without_stack(message: &str) -> String {
    static ERROR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n[A-Za-z]*Error: ").unwrap());
    match ERROR.find(message) {
        Some(found) if message.contains("\n    at ") => message[..found.start()].to_owned(),
        _ => message.to_owned(),
    }
}

fn norm(root: &Path) -> Norm {
    let pid = std::process::id();
    Norm::new(
        root,
        vec![
            Rule::Pattern(
                Regex::new(&format!(r"\.{pid}\.[0-9a-z]+\.tmp")).unwrap(),
                ".<pid>.<ms>.tmp".to_owned(),
            ),
            Rule::Pattern(
                Regex::new(r"-(export|import)-[0-9a-f]{8}").unwrap(),
                "-$1-<hex>".to_owned(),
            ),
            Rule::Literal(format!("-origin-{pid}"), "-origin-<pid>".to_owned()),
        ],
        false,
    )
}

// ---------------------------------------------------------------- прогон

fn registry_of(kind: &str) -> Option<Registry> {
    if kind != "pack" {
        return None;
    }
    let mut registry = Registry::bundled();
    let manifest = FIXTURE["packManifest"].clone();
    match registry
        .manifests
        .iter_mut()
        .find(|(id, _)| id == "ghosting-renewed")
    {
        Some(entry) => entry.1 = manifest,
        None => registry
            .manifests
            .push(("ghosting-renewed".to_owned(), manifest)),
    }
    let files = vec![("1.0.0".to_owned(), FIXTURE["packFilesManifest"].clone())];
    match registry
        .files
        .iter_mut()
        .find(|(id, _)| id == "ghosting-renewed")
    {
        Some(entry) => entry.1 = files,
        None => registry.files.push(("ghosting-renewed".to_owned(), files)),
    }
    Some(registry)
}

/// `fill`: `<root>` (внутри строки JSON), `<token>` и `"<prev:ключ>"` в аргументах шага.
fn fill_args(args: &Value, root: &Path, token: Option<&str>, last_ok: &Value) -> Vec<Value> {
    let escaped = serde_json::to_string(&root.to_string_lossy()).unwrap();
    let escaped = &escaped[1..escaped.len() - 1];
    let text = stringify(args, "")
        .replace("<root>", escaped)
        .replace("<token>", token.unwrap_or(""));
    static PREV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""<prev:(\w+)>""#).unwrap());
    let text = PREV.replace_all(&text, |caps: &regex::Captures<'_>| {
        stringify(last_ok.get(&caps[1]).unwrap_or(&Value::Null), "")
    });
    serde_json::from_str::<Value>(&text)
        .unwrap()
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Шаг `install`: файлы игры (время — своё у каждого) и `installed.json`.
fn install(root: &Path, step: &Value) {
    let game = str_of(&step["game"]);
    let install_path = root.join("lib").join("Games").join(game);
    if let Some(files) = step["files"].as_object() {
        for (index, (rel, text)) in files.iter().enumerate() {
            let full = path_in(&install_path, rel);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, str_of(text)).unwrap();
            set_mtime(&full, (MTIME_MS + index as i64 * 1000) as f64 / 1000.0);
        }
    }
    std::fs::create_dir_all(&install_path).unwrap();
    let mut installed = Map::new();
    installed.insert("schemaVersion".into(), json!(1));
    installed.insert("gameId".into(), json!(game));
    installed.insert("version".into(), step["version"].clone());
    installed.insert("state".into(), json!("ready"));
    installed.insert("installPath".into(), json!(install_path.to_string_lossy()));
    installed.insert("installedAt".into(), json!(iso(now())));
    if let Some(extra) = step["extra"].as_object() {
        for (key, value) in extra {
            installed.insert(key.clone(), value.clone());
        }
    }
    std::fs::write(
        install_path.join("installed.json"),
        stringify(&Value::Object(installed), "  "),
    )
    .unwrap();
}

/// Папка сценария той же длины (в записи JSON), что в скрипте: путь установки лежит в
/// `installed.json`, и от его длины зависят размеры, которые считает удаление.
fn padded_root(dir: &Path) -> PathBuf {
    let base = format!("{}{}", dir.to_string_lossy(), std::path::MAIN_SEPARATOR);
    let quoted = serde_json::to_string(&base).unwrap();
    let length = quoted.len() - 2;
    let wanted = FIXTURE["rootLength"].as_u64().unwrap() as usize;
    assert!(length < wanted, "временная папка длиннее {wanted}: {base}");
    let root = PathBuf::from(format!("{base}{}", "p".repeat(wanted - length)));
    std::fs::create_dir(&root).unwrap();
    root
}

/// Discord `selected` 0.4.9 (задача F2l): `game-delivery-selected` шлёт id сборки (`gameId || id`), а
/// не id из окна; `game-select-build` шлёт `selected` новой сборки. В запись 0.4.7 это приводится
/// обратно: id из аргумента, без нового сигнала. Сам 0.4.9 проверяет `selected_presence_like_0_4_9`.
fn selected_as_0_4_7(channel: &str, args: &[Value], value: &Value) -> Value {
    let mut out = value.clone();
    let selected = |effect: &Value| effect["presence"]["type"] == "selected";
    if let Some(effects) = out["effects"].as_array_mut() {
        match channel {
            "game-select-build" => effects.retain(|effect| !selected(effect)),
            "game-delivery-selected" => {
                for effect in effects.iter_mut().filter(|effect| selected(effect)) {
                    effect["presence"]["gameId"] = args.first().cloned().unwrap_or(Value::Null);
                }
            }
            _ => {}
        }
    }
    out
}

/// Код персонализации сравнивается видом, который из него читается: сжатие у Rust (miniz_oxide)
/// и у zlib Node даёт разные байты (отчёт задачи 16), а `decode` читает оба.
fn decoded_code(channel: &str, args: &[Value], value: &Value) -> Value {
    let mut out = value.clone();
    if channel == "game-personalization-code-export"
        && let Some(code) = value["ok"]["code"].as_str()
    {
        let game = match str_of(&args[0]) {
            "218" => "ghosting-renewed",
            "70" => "hl1-won",
            other => panic!("код для {other}"),
        };
        let look = hlsr_delivery::personalization_code::decode(game, &json!(code))
            .expect("код читается")
            .to_value();
        out["ok"]["code"] = json!({"decoded": look});
    }
    out
}

async fn run_scenario(spec: &Value) -> Mismatches {
    let name = str_of(&spec["name"]);
    let (_temp, temp) = temp_root();
    let root = padded_root(&temp);
    let mut h = harness(Setup {
        root: root.clone(),
        registry: registry_of(str_of(&spec["registry"])),
        now: now(),
        launcher: LauncherIdentity {
            exec_path: PathBuf::from(LAUNCHER_EXEC),
            app_image: None,
            packaged: false,
            app_path: Some(PathBuf::from(LAUNCHER_APP)),
        },
    });
    let n = norm(&root);
    static SIZE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d+ файлов, )\d+( байт)").unwrap());

    let mut mismatches = Mismatches::default();
    let expected = items(&spec["results"]);
    let mut last_token: Option<String> = None;
    let mut last_ok = Value::Null;
    assert_eq!(
        items(&spec["steps"]).len(),
        items(&spec["results"]).len(),
        "шагов и результатов поровну"
    );
    for (index, step) in items(&spec["steps"]).iter().enumerate() {
        h.effects.lock().unwrap().clear();
        while h.events.try_recv().is_ok() {}
        h.log.lock().unwrap().clear();
        let mut result = Map::new();
        match str_of(&step["do"]) {
            "call" => {
                let channel = str_of(&step["channel"]);
                let args = fill_args(&step["args"], &root, last_token.as_deref(), &last_ok);
                let Some(mut value) = h.api.call(channel, &args).await else {
                    panic!("нет канала {channel}");
                };
                if value["status"] == "confirmation-required"
                    && let Some(token) = value["token"].as_str()
                {
                    last_token = Some(token.to_owned());
                    value["token"] = json!("<token>");
                }
                last_ok = value.clone();
                if value["kind"] == "pack" && value["size"].is_number() {
                    value["size"] = json!("<size>");
                }
                result.insert("ok".into(), without_times(&n.value(&value)));
            }
            "init" => h.api.initialize_service(),
            "install" => install(&root, step),
            "write" => {
                let full = path_in(&root, str_of(&step["file"]));
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                let bytes = match step["base64"].as_str() {
                    Some(text) => base64_decode(text),
                    None => str_of(&step["utf8"]).as_bytes().to_vec(),
                };
                std::fs::write(full, bytes).unwrap();
            }
            "running" => {
                *h.processes.running.lock().unwrap() = support::scenario::strings(&step["games"]);
            }
            "dialog" => h
                .dialogs
                .answers
                .lock()
                .unwrap()
                .extend(items(&step["answers"]).iter().cloned()),
            "openPath" => {
                *h.shell.open_path_failure.lock().unwrap() = str_of(&step["failure"]).to_owned();
            }
            "argv" => {
                let argv = support::scenario::strings(&step["argv"]);
                result.insert("ok".into(), n.value(&h.api.select_game_from_args(&argv)));
            }
            "focus" => h.api.check_on_focus(),
            "tree" => {
                result.insert(
                    "tree".into(),
                    tree_of(&root, &n, str_of(&step["rel"]), &STATE_FILE),
                );
            }
            other => panic!("неизвестный шаг {other}"),
        }
        h.api.settle().await;
        let mut events = Vec::new();
        while let Ok(event) = h.events.try_recv() {
            events.push(n.value(&project_event(&event)));
        }
        let effects: Vec<Value> = std::mem::take(&mut *h.effects.lock().unwrap())
            .iter()
            .map(|effect| n.value(effect))
            .collect();
        let log: Vec<Value> = std::mem::take(&mut *h.log.lock().unwrap())
            .into_iter()
            .map(|(level, message)| {
                let message = without_stack(&n.string(&message));
                json!({"level": level, "message": SIZE.replace(&message, "$1<size>$2")})
            })
            .collect();
        result.insert("events".into(), collapse(events));
        result.insert("effects".into(), collapse(effects));
        result.insert("log".into(), Value::Array(log));
        let mut actual = as_0_4_7(&Value::Object(result));
        let mut want = as_0_4_7(&expected[index]);
        if let Some(channel) = step["channel"].as_str() {
            let args = items(&step["args"]);
            actual = decoded_code(channel, args, &selected_as_0_4_7(channel, args, &actual));
            want = decoded_code(channel, args, &want);
        }
        let want = &want;
        let label = match step["channel"].as_str() {
            Some(channel) => format!("{name}, шаг {index} ({channel})"),
            None => format!("{name}, шаг {index} ({})", str_of(&step["do"])),
        };
        mismatches.check(&actual, want, &label);
        // Порядок ключей ответов и событий — тоже видимое поведение.
        if actual == *want && stringify(&actual, "") != stringify(want, "") {
            mismatches.check_eq(
                stringify(&actual, " "),
                stringify(want, " "),
                format!("{label}: порядок ключей"),
            );
        }
    }
    h.api.dispose();
    mismatches
}

fn base64_decode(text: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .unwrap()
}

fn scenario(name: &str) -> &'static Value {
    items(&FIXTURE["scenarios"])
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("нет сценария {name}"))
}

macro_rules! scenarios {
    ($($(#[$attr:meta])* $test:ident => $name:literal),* $(,)?) => {
        $(
            #[tokio::test]
            $(#[$attr])*
            async fn $test() {
                run_scenario(scenario($name)).await.finish($name);
            }
        )*

        /// Сценарии, у которых есть тест.
        const NAMES: &[&str] = &[$($name),*];
    };
}

scenarios! {
    #[cfg_attr(windows, ignore = "фикстура снята в Linux: ярлыки .desktop, байты путей в installed.json")]
    statuses_and_menu_match_0_4_7 => "статусы и меню игры",
    #[cfg_attr(windows, ignore = "фикстура снята в Linux: бинд удаления сейва «save /» (в Windows «save :»)")]
    settings_and_personalization_match_0_4_7 => "настройки игры и персонализация",
    #[ignore = "0.4.9 (задача A): «Конфиги» пишут в файл игрока, game-config-read/-write с sha256 — single_config.rs, config_selftests.rs"]
    configs_cfg_files_and_runs_match_0_4_7 => "конфиги, cfg-файлы и записи",
    #[ignore = "0.4.9 (задача A): сохранение настроек готовит файл игрока (шаблон autoexec.cfg) — размеры архива сборки другие"]
    build_export_and_import_match_0_4_7 => "экспорт и импорт сборки",
    tools_and_links_match_0_4_7 => "инструменты и ссылки",
}

#[test]
fn every_scenario_has_a_test() {
    for scenario in items(&FIXTURE["scenarios"]) {
        let name = str_of(&scenario["name"]);
        assert!(NAMES.contains(&name), "сценарий без теста: {name}");
    }
}
