//! Паки HL1 на диске против 0.4.7: фикстура `goldsrc-packs.json` из
//! `tools/reference/delivery/goldsrc-packs.js`. Сценарий — данные (серверы с маршрутами, файлы и
//! шаги): `applyPacks` 0.4.7 в Node 24 и этот крейт выполняют его на одинаковых деревьях во
//! временных папках, локальном сервере (`tests/support/http.rs`) и настоящем 7za; сравниваются итог
//! каждого шага, строки журнала, дерево файлов (путь, размер, sha256, текст latin1; у архивов в
//! кеше — без текста) и журнал запросов. Адрес Google Drive подменён на локальный сервер; в
//! интернет тесты не ходят. Закреплённые размер и sha256 паков заменены каталогом сценария.
//!
//! Сценарии, помеченные `unix`, зависят от ОС (права, ссылки, каталог вместо файла) и на Windows
//! пропускаются. Текст ошибки 7za сверяется по началу (в 0.4.7 он зависит от разбиения вывода).

#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_core::js;
use hlsr_core::paths::Resources;
use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::errors::Failure;
use hlsr_delivery::goldsrc_packs::{self as packs, Catalogue, Pack, PackOptions};
use hlsr_delivery::http_client::HttpClient;
use hlsr_delivery::manifests;
use hlsr_delivery::ports::MemoryLogger;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use support::http::{RequestLog, Server, dead_port, start_server};
use support::scenario::{self, common_step, normalize_text, path_in, temp_root};
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("goldsrc-packs.json"));

/// Сообщения 7za зависят от локали (strerror), время записей — от пояса.
fn pinned_env() -> [(&'static str, &'static str); 3] {
    [("LC_ALL", "C.UTF-8"), ("LANG", "C.UTF-8"), ("TZ", "UTC")]
}

// ------------------------------------------------------------ подстановка адресов

fn fill_text(text: &str, map: &BTreeMap<String, String>) -> String {
    let mut out = text.to_owned();
    for (name, authority) in map {
        out = out.replace(&format!("{{{name}}}"), authority);
    }
    out
}

fn fill(value: &Value, map: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(text) => Value::String(fill_text(text, map)),
        Value::Array(list) => Value::Array(list.iter().map(|v| fill(v, map)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (fill_text(k, map), fill(v, map)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Обратная замена: `127.0.0.1:<порт>` → `{имя}`; длинные адреса первыми.
fn unfill(value: &Value, map: &BTreeMap<String, String>) -> Value {
    let mut pairs: Vec<(&String, &String)> = map.iter().collect();
    pairs.sort_by_key(|(_, authority)| std::cmp::Reverse(authority.len()));
    let text = |t: &str| {
        pairs.iter().fold(t.to_owned(), |t, (name, authority)| {
            t.replace(*authority, &format!("{{{name}}}"))
        })
    };
    match value {
        Value::String(t) => Value::String(text(t)),
        Value::Array(list) => Value::Array(list.iter().map(|v| unfill(v, map)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (text(k), unfill(v, map)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Маршруты сервера: архивы, на которые ссылается сценарий (`bodyZip`), заменяются телом из
/// байтов (`bodyHex`).
fn expand_routes(routes: &Value) -> Value {
    let mut out = Map::new();
    for (key, list) in routes.as_object().unwrap() {
        let list: Vec<Value> = items(list)
            .iter()
            .map(|step| {
                let mut step = step.clone();
                if let Some(name) = step.as_object_mut().unwrap().remove("bodyZip") {
                    step["bodyHex"] = FIXTURE["zips"][str_of(&name)].clone();
                }
                step
            })
            .collect();
        out.insert(key.clone(), Value::Array(list));
    }
    Value::Object(out)
}

// ------------------------------------------------------------------- каталоги

fn catalogue_of(name: &str) -> Catalogue {
    items(&FIXTURE["catalogues"][name])
        .iter()
        .map(|pair| {
            let entry = &pair[1];
            (
                str_of(&pair[0]).to_owned(),
                Pack {
                    kind: str_of(&entry["kind"]).to_owned(),
                    title: str_of(&entry["title"]).to_owned(),
                    author: str_of(&entry["author"]).to_owned(),
                    drive_id: str_of(&entry["driveId"]).to_owned(),
                    size: entry["size"].as_u64().unwrap() as usize,
                    sha256: str_of(&entry["sha256"]).to_owned(),
                    root: str_of(&entry["root"]).to_owned(),
                },
            )
        })
        .collect()
}

// -------------------------------------------------------------------- сценарий

static UNPACKED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"unpacked-\d+").unwrap());

/// `normalizeResult` скрипта: пути — `<root>`/`<tmp>`, pid в имени временной папки — метка, текст
/// ошибки распаковки — первые 40 знаков (до конца строки).
fn normalize_result(root: &std::path::Path, value: &Value) -> Value {
    match value {
        Value::String(text) => {
            let text = normalize_text(root, text);
            let text = UNPACKED.replace_all(&text, "unpacked-<pid>").into_owned();
            const MARK: &str = "Ошибка распаковки: ";
            let text = match text.find(MARK) {
                Some(at) => {
                    let rest = &text[at + MARK.len()..];
                    let kept: String = rest
                        .chars()
                        .take_while(|c| !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
                        .take(40)
                        .collect();
                    format!("{}{MARK}{kept}", &text[..at])
                }
                None => text,
            };
            Value::String(text)
        }
        Value::Array(list) => {
            Value::Array(list.iter().map(|v| normalize_result(root, v)).collect())
        }
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (k.clone(), normalize_result(root, v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// В Windows `readdir` отдаёт имена в порядке тома, а не по байтам, как в Linux, где снята фикстура;
/// от порядка зависит порядок ключей `files` в applied.json. Там обе стороны сравниваются с
/// ключами, отсортированными по байтам (содержимое записей сверяется как есть).
#[cfg(windows)]
fn sort_state_keys(result: &mut Value) {
    let Some(tree) = result.get_mut("tree").and_then(Value::as_array_mut) else {
        return;
    };
    for entry in tree {
        if !str_of(&entry["path"]).ends_with("applied.json") || entry.get("latin1").is_none() {
            continue;
        }
        let bytes = js::latin1_bytes(str_of(&entry["latin1"]));
        let Ok(mut state) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if let Some(files) = state.get_mut("files").and_then(Value::as_object_mut) {
            let mut names: Vec<String> = files.keys().cloned().collect();
            names.sort();
            let sorted: Map<String, Value> = names
                .into_iter()
                .map(|name| {
                    let value = files[&name].clone();
                    (name, value)
                })
                .collect();
            *files = sorted;
        }
        let text = hlsr_core::json_text::stringify(&state, "  ");
        let bytes = text.into_bytes();
        entry["size"] = json!(bytes.len());
        entry["sha256"] = json!(sha256_hex(&bytes));
        entry["latin1"] = json!(js::latin1_string(&bytes));
    }
}

#[cfg(windows)]
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Текст файла попадает в снимок, если файл не больше; у архивов `.zip` текста нет.
const TEXT_LIMIT: u64 = 6000;

fn tree_of(root: &std::path::Path) -> Value {
    let mut tree = support::tree::snapshot(root, None);
    for entry in tree.as_array_mut().unwrap() {
        let relative = str_of(&entry["path"]).to_owned();
        match str_of(&entry["kind"]) {
            "symlink" => {
                let target = str_of(&entry["target"]).replace(root.to_str().unwrap(), "<root>");
                entry["target"] = json!(target);
            }
            "file"
                if entry["size"].as_u64().unwrap() <= TEXT_LIMIT && !relative.ends_with(".zip") =>
            {
                let bytes = std::fs::read(path_in(root, &relative)).unwrap();
                entry["latin1"] = json!(js::latin1_string(&bytes));
            }
            _ => {}
        }
    }
    tree
}

struct Context {
    root: PathBuf,
    _temp: tempfile::TempDir,
    map: BTreeMap<String, String>,
    servers: BTreeMap<String, Server>,
    log: RequestLog,
    /// Сколько записей журнала запросов уже отдано шагом `requests`.
    seen: usize,
    drive: String,
    default_catalogue: String,
    client: HttpClient,
    archive: ArchiveService,
    logger: Arc<MemoryLogger>,
}

impl Context {
    async fn new(scenario: &Value) -> Context {
        let (temp, root) = temp_root();
        let log = RequestLog::default();
        let mut map = BTreeMap::new();
        map.insert(
            "dead".to_owned(),
            format!("127.0.0.1:{}", dead_port().await),
        );
        let mut servers = BTreeMap::new();
        for name in scenario["servers"].as_object().unwrap().keys() {
            let server = start_server(name, log.clone()).await;
            map.insert(name.clone(), server.authority.clone());
            servers.insert(name.clone(), server);
        }
        for (name, server) in &servers {
            let routes = fill(&expand_routes(&scenario["servers"][name]), &map);
            *server.routes.lock().unwrap() = routes.as_object().unwrap().clone();
        }
        let archive_logger = Arc::new(MemoryLogger::default());
        let archive = ArchiveService::new(Resources::detect().sevenzip(), archive_logger)
            .with_env(pinned_env());
        Context {
            root,
            _temp: temp,
            map,
            servers,
            log,
            seen: 0,
            drive: opt_str(&scenario["drive"]).unwrap_or("a").to_owned(),
            default_catalogue: opt_str(&scenario["catalogue"]).unwrap_or("good").to_owned(),
            client: HttpClient::new(),
            archive,
            logger: Arc::new(MemoryLogger::default()),
        }
    }

    async fn apply(&self, step: &Value) -> Value {
        self.logger.take();
        let catalogue =
            catalogue_of(opt_str(&step["catalogue"]).unwrap_or(&self.default_catalogue));
        let base = format!("http://{}/download", self.map[&self.drive]);
        let logger: Arc<MemoryLogger> = self.logger.clone();
        let options = PackOptions {
            client: Some(&self.client),
            archive: (step["archive"] != json!(false)).then_some(&self.archive),
            logger: Some(logger.as_ref()),
            catalogue: Some(&catalogue),
            drive_download: Some(&base),
        };
        let install = match opt_str(&step["installDir"]) {
            Some(dir) => path_in(&self.root, dir),
            None => self.root.clone(),
        };
        let settings = &step["settings"];
        let result = match &step["config"] {
            Value::Null => {
                let config = manifests::launch_config_of("hl1-won").unwrap();
                packs::apply_packs(&install, config, settings, &options).await
            }
            Value::String(id) => {
                let config = manifests::launch_config_of(id).expect("настройки запуска игры");
                packs::apply_packs(&install, config, settings, &options).await
            }
            config => {
                packs::apply_packs_in(
                    &install,
                    str_of(&config["spritesDir"]),
                    str_of(&config["baseSpritesDir"]),
                    settings,
                    &options,
                )
                .await
            }
        };
        let logs: Vec<Value> = self
            .logger
            .take()
            .into_iter()
            .map(|(level, message)| json!([level, message]))
            .collect();
        let value = match result {
            Ok(outcome) => json!({"ok": outcome.to_value(), "logs": logs}),
            Err(Failure::Delivery(error)) => json!({"error": {
                "code": error.code.as_str(),
                "message": error.message,
                "details": error.details,
            }, "logs": logs}),
            Err(Failure::Raw(error)) => json!({"raw": {
                "code": error.code,
                "message": error.message,
            }, "logs": logs}),
        };
        normalize_result(&self.root, &value)
    }

    /// Запросы к серверам с прошлого шага `requests`: метод, адрес и `User-Agent`; `urls` —
    /// адреса на Google Drive, как их строит модуль.
    fn requests(&mut self) -> Value {
        let log = self.log.lock().unwrap();
        let fresh: Vec<Value> = log[self.seen..].to_vec();
        self.seen = log.len();
        let requests: Vec<Value> = fresh
            .iter()
            .map(|entry| {
                json!({
                    "method": entry["method"],
                    "url": entry["url"],
                    "headers": {"user-agent": entry["headers"]["user-agent"]},
                })
            })
            .collect();
        // Первые запросы пака идут на /download?id=…; редиректы — на другие пути.
        let urls: Vec<String> = fresh
            .iter()
            .filter_map(|entry| {
                let url = str_of(&entry["url"]);
                let query = url.strip_prefix("/download?id=")?;
                let id = query.strip_suffix("&export=download")?;
                let decoded = url::form_urlencoded::parse(format!("id={id}").as_bytes())
                    .next()
                    .map(|(_, value)| value.into_owned())?;
                Some(packs::download_url(packs::DRIVE_DOWNLOAD, &decoded))
            })
            .collect();
        json!({"requests": requests, "urls": urls})
    }

    async fn step(&mut self, step: &Value) -> Value {
        if str_of(&step["op"]) == "tree" {
            return json!({"tree": tree_of(&self.root)});
        }
        if let Some(common) = common_step(&self.root, step) {
            return common;
        }
        match str_of(&step["op"]) {
            "apply" => self.apply(step).await,
            "requests" => self.requests(),
            "routes" => {
                let server = &self.servers[opt_str(&step["server"]).unwrap_or("a")];
                let routes = expand_routes(&step["routes"]);
                *server.routes.lock().unwrap() = routes.as_object().unwrap().clone();
                Value::Null
            }
            other => panic!("неизвестный шаг {other}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_packs_on_scenarios_with_a_local_server_and_the_real_7za() {
    let scenarios = items(&FIXTURE["scenarios"]);
    assert!(scenarios.len() > 50, "сценариев: {}", scenarios.len());
    let mut mismatches = Mismatches::default();
    for scenario in scenarios {
        let name = str_of(&scenario["name"]);
        if scenario["unix"].as_bool().unwrap_or(false) && cfg!(windows) {
            continue;
        }
        if name.starts_with("права") && scenario::is_root_user() {
            continue;
        }
        let mut context = Context::new(scenario).await;
        scenario::setup(&context.root, &scenario["setup"]);
        assert_eq!(
            items(&scenario["steps"]).len(),
            items(&scenario["results"]).len(),
            "шагов и результатов поровну"
        );
        for (index, step) in items(&scenario["steps"]).iter().enumerate() {
            let filled = fill(step, &context.map);
            #[allow(unused_mut)]
            let mut actual = unfill(&context.step(&filled).await, &context.map);
            #[allow(unused_mut)]
            let mut expected = scenario["results"][index].clone();
            #[cfg(windows)]
            {
                sort_state_keys(&mut actual);
                sort_state_keys(&mut expected);
            }
            mismatches.check(
                &actual,
                &expected,
                format!("{name}: шаг {index} {}", step["op"]),
            );
        }
        scenario::open_up(&context.root);
    }
    mismatches.finish("goldsrc-packs: сценарии");
}

#[test]
fn the_real_drive_address_is_what_the_module_builds_for_every_pack() {
    // Сценарии подменяют адрес; настоящий — константа 0.4.7 и тот же порядок параметров.
    assert_eq!(packs::DRIVE_DOWNLOAD, str_of(&FIXTURE["drive"]));
    let known: Vec<&str> = items(&FIXTURE["scenarios"])
        .iter()
        .flat_map(|scenario| items(&scenario["results"]))
        .filter_map(|result| result.get("urls"))
        .flat_map(items)
        .map(str_of)
        .collect();
    assert!(!known.is_empty());
    for (_, pack) in catalogue_of("good") {
        let url = packs::download_url(packs::DRIVE_DOWNLOAD, &pack.drive_id);
        assert!(known.contains(&url.as_str()), "{url}");
    }
    // Настоящие пакеты закреплены, и адрес для них строится так же.
    let url = packs::download_url(packs::DRIVE_DOWNLOAD, &packs::catalogue()[0].1.drive_id);
    assert_eq!(
        url,
        "https://drive.usercontent.google.com/download?id=1_nmTVsEVha-bbQwCGnC7eHfshvCSqIw7&export=download"
    );
}
