//! Тренировочные сейвы HL1 на диске против 0.4.7: фикстура `practice-saves.json` из
//! `tools/reference/delivery/practice-saves.js`. Сценарий — данные (серверы с маршрутами, файлы и шаги):
//! `PracticeSaves.ensure` 0.4.7 в Node 24 и этот крейт выполняют его на одинаковых деревьях во временных
//! папках, локальных серверах (`tests/support/http.rs`) и настоящем 7za; сравниваются итог каждого шага,
//! строки журнала, дерево файлов (путь, размер, sha256, текст latin1) и журнал запросов. Набор сейвов в
//! сценариях маленький: архивы `tests/fixtures/archives/saves/*.7z` собирает скрипт тем же 7za. Настоящий
//! архив `release/saves` (944 КБ, лежит у того, кто его собирал, в git его нет) проверяет отдельный тест,
//! как самотест 0.4.7, — при отсутствии файла тест пропускается.
//!
//! Сценарии, помеченные `unix`, зависят от ОС (права, ссылки) и на Windows пропускаются.

#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_core::js;
use hlsr_core::paths::Resources;
use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::delivery_endpoints::is_allowed_download_url;
use hlsr_delivery::download_rate_limiter::DownloadRateLimiter;
use hlsr_delivery::errors::Failure;
use hlsr_delivery::http_client::HttpClient;
use hlsr_delivery::paths::resolve_library;
use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::practice_saves::{
    DOWNLOAD_TIMEOUT, Outcome, Pack, PracticeSaves, hl1_pack, is_save_name,
};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use support::http::{RequestLog, Server, dead_port, fill, mutate, start_server, unfill};
use support::scenario::{self, common_step, normalize_text, path_in, temp_root};
use support::zip::hex;
use support::{Mismatches, fixture, fixture_path, items, opt_str, sha256_hex, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("practice-saves.json"));

/// Сообщения 7za зависят от локали (strerror), время записей — от пояса.
fn pinned_env() -> [(&'static str, &'static str); 3] {
    [("LC_ALL", "C.UTF-8"), ("LANG", "C.UTF-8"), ("TZ", "UTC")]
}

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("шестнадцатеричная запись"))
        .collect()
}

/// Байты архива сценария: файл из `tests/fixtures/archives` или запись в фикстуре (не архив).
fn archive_bytes(name: &str) -> Vec<u8> {
    let info = &FIXTURE["archives"][name];
    match opt_str(&info["file"]) {
        Some(file) => std::fs::read(fixture_path(&format!("archives/{file}")))
            .unwrap_or_else(|e| panic!("архив {file}: {e}")),
        None => from_hex(str_of(&info["hex"])),
    }
}

/// Маршруты сервера: архивы, на которые ссылается сценарий (`bodyFile`, `mutate`), заменяются телом
/// из байтов (`bodyHex`).
fn expand_routes(routes: &Value) -> Value {
    let mut out = Map::new();
    for (key, list) in routes.as_object().unwrap() {
        let list: Vec<Value> = items(list)
            .iter()
            .map(|step| {
                let mut step = step.clone();
                let object = step.as_object_mut().unwrap();
                if let Some(name) = object.remove("bodyFile") {
                    let how = object.remove("mutate");
                    let bytes = mutate(
                        archive_bytes(str_of(&name)),
                        how.as_ref().and_then(Value::as_str),
                    );
                    step["bodyHex"] = json!(hex(&bytes));
                }
                step
            })
            .collect();
        out.insert(key.clone(), Value::Array(list));
    }
    Value::Object(out)
}

/// `normalizeResult` скрипта: пути — `<root>`/`<tmp>`, текст ошибки распаковки — первые 40 знаков.
fn normalize_result(root: &std::path::Path, value: &Value) -> Value {
    match value {
        Value::String(text) => {
            let text = normalize_text(root, text);
            const MARK: &str = "Ошибка распаковки: ";
            Value::String(match text.find(MARK) {
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
            })
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

/// Итог `ensure` как у скрипта: `{ok}`, `{error}` или `{raw}`.
fn outcome_value(result: &Result<Outcome, Failure>) -> Value {
    match result {
        Ok(outcome) => json!({"ok": outcome.to_value()}),
        Err(Failure::Delivery(error)) => json!({"error": {
            "code": error.code.as_str(),
            "message": error.message,
            "details": error.details,
        }}),
        Err(Failure::Raw(error)) => json!({"raw": {
            "code": error.code,
            "message": error.message,
        }}),
    }
}

struct Context {
    root: PathBuf,
    _temp: tempfile::TempDir,
    map: BTreeMap<String, String>,
    servers: BTreeMap<String, Server>,
    log: RequestLog,
    /// Сколько записей журнала запросов уже отдано шагом `requests`.
    seen: usize,
    default_pack: String,
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
        let archive = ArchiveService::new(
            Resources::detect().sevenzip(),
            Arc::new(MemoryLogger::default()),
        )
        .with_env(pinned_env());
        Context {
            root,
            _temp: temp,
            map,
            servers,
            log,
            seen: 0,
            default_pack: opt_str(&scenario["pack"]).unwrap_or("good").to_owned(),
            client: HttpClient::new(),
            archive,
            logger: Arc::new(MemoryLogger::default()),
        }
    }

    fn saves(&self, step: &Value) -> PracticeSaves {
        let name = opt_str(&step["pack"]).unwrap_or(&self.default_pack);
        let pack = Pack::from_value(&fill(&FIXTURE["packs"][name], &self.map));
        let library = resolve_library(path_in(&self.root, "lib").to_str().unwrap()).unwrap();
        let logger: Arc<MemoryLogger> = self.logger.clone();
        let mut saves = PracticeSaves::new(&library)
            .with_packs(vec![pack])
            .with_logger(logger)
            .with_http_client(self.client.clone());
        if step["archive"] != json!(false) {
            saves = saves.with_archive(self.archive.clone());
        }
        if let Some(limit) = step["limiter"].as_f64() {
            saves = saves.with_rate_limiter(Arc::new(DownloadRateLimiter::new(limit)));
        }
        saves
    }

    fn install_path(&self, step: &Value) -> PathBuf {
        match opt_str(&step["installDir"]) {
            None => path_in(&self.root, "lib/Games/hl1-won"),
            Some("") => PathBuf::new(),
            Some(dir) => path_in(&self.root, dir),
        }
    }

    fn logs(&self) -> Vec<Value> {
        self.logger
            .take()
            .into_iter()
            .map(|(level, message)| json!([level, message]))
            .collect()
    }

    async fn ensure(&self, step: &Value) -> Value {
        self.logger.take();
        let saves = self.saves(step);
        let install = self.install_path(step);
        let game = opt_str(&step["gameId"]).unwrap_or("hl1-won");
        let result = match step["during"].as_array() {
            None => saves.ensure(game, &install).await,
            Some(during) => {
                // Загрузка идёт, сервер держит ответ на воротах: сценарий меняет дерево и открывает ворота.
                let before = self.log.lock().unwrap().len();
                let control = async {
                    for waited in (0..).step_by(10) {
                        assert!(waited < 10_000, "запрос на сервер не пришёл");
                        if self.log.lock().unwrap().len() > before {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    for inner in during {
                        let done = common_step(&self.root, inner)
                            .unwrap_or_else(|| panic!("шаг {} во время загрузки", inner["op"]));
                        assert_eq!(done, Value::Null);
                    }
                    let server = &self.servers[opt_str(&step["server"]).unwrap_or("a")];
                    for gate in items(&step["release"]) {
                        server.release(str_of(gate));
                    }
                };
                let (result, ()) = tokio::join!(saves.ensure(game, &install), control);
                result
            }
        };
        let mut value = outcome_value(&result);
        value["logs"] = Value::Array(self.logs());
        normalize_result(&self.root, &value)
    }

    async fn parallel(&self, step: &Value) -> Value {
        self.logger.take();
        let saves = self.saves(step);
        let install = path_in(&self.root, "lib/Games/hl1-won");
        let calls = step.get("calls").map_or(2, |calls| items(calls).len());
        assert_eq!(calls, 2, "тест сводит два одновременных вызова");
        let (first, second) = tokio::join!(
            saves.ensure("hl1-won", &install),
            saves.ensure("hl1-won", &install)
        );
        let value = json!({
            "results": [outcome_value(&first), outcome_value(&second)],
            "jobs": saves.pending(),
            "logs": self.logs(),
        });
        normalize_result(&self.root, &value)
    }

    /// Запросы к серверам с прошлого шага `requests`: метод, адрес и `User-Agent`.
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
        json!({"requests": requests})
    }

    async fn step(&mut self, step: &Value) -> Value {
        if let Some(common) = common_step(&self.root, step) {
            return common;
        }
        match str_of(&step["op"]) {
            "ensure" => self.ensure(step).await,
            "parallel" => self.parallel(step).await,
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
async fn ensure_on_scenarios_with_a_local_server_and_the_real_7za() {
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
            let actual = unfill(&context.step(&filled).await, &context.map);
            mismatches.check(
                &actual,
                &scenario["results"][index],
                format!("{name}: шаг {index} {}", step["op"]),
            );
        }
        scenario::open_up(&context.root);
    }
    mismatches.finish("practice-saves: сценарии");
}

#[test]
fn the_scenario_archives_are_the_ones_the_fixture_describes() {
    for (name, info) in FIXTURE["archives"].as_object().unwrap() {
        let bytes = archive_bytes(name);
        assert_eq!(bytes.len() as u64, info["size"].as_u64().unwrap(), "{name}");
        assert_eq!(sha256_hex(&bytes), str_of(&info["sha256"]), "{name}");
    }
}

#[test]
fn the_download_timeout_is_the_one_of_0_4_7() {
    // Сценарии не видят тайм-аут: скрипт записывает то, что 0.4.7 передал клиенту.
    let seen = items(&FIXTURE["timeouts"]);
    assert_eq!(
        seen.as_slice(),
        &[json!(DOWNLOAD_TIMEOUT.as_millis() as u64)]
    );
}

#[test]
fn save_names_are_checked_the_way_0_4_7_does() {
    for name in [
        "e1.sav",
        "E1.sav",
        "e14-protopipe.sav",
        "a_b-c.sav",
        "9.sav",
    ] {
        assert!(is_save_name(name), "{name}");
    }
    for name in [
        "",
        ".sav",
        "e1.SAV",
        "e1.sav ",
        "e1.sav\n",
        "bad name.sav",
        "../evil.sav",
        "e1.txt",
        "e1",
        "е1.sav",
        "a/b.sav",
    ] {
        assert!(!is_save_name(name), "{name:?}");
    }
}

// ----------------------------------------------------------------- самотесты 0.4.7

#[test]
fn the_hl1_manifest_has_the_right_names_archive_and_sources() {
    let pack = hl1_pack();
    assert_eq!(pack.game_ids, ["hl1-won"]);
    assert_eq!(pack.target, "Half-Life/valve_WON/SAVE");
    assert_eq!(pack.files.len(), 63);
    let mut lower: Vec<String> = pack.files.iter().map(|f| f.name.to_lowercase()).collect();
    lower.sort();
    lower.dedup();
    assert_eq!(lower.len(), pack.files.len(), "имена не повторяются");
    let hex64 = Regex::new(r"^[0-9a-f]{64}$").unwrap();
    for file in &pack.files {
        assert!(is_save_name(&file.name), "{}", file.name);
        assert!(file.size > 0, "{}", file.name);
        assert!(hex64.is_match(&file.sha256), "{}", file.name);
    }
    assert!(pack.files.iter().any(|f| f.name == "e14-protopipe.sav"));

    assert!(hex64.is_match(&pack.archive.sha256));
    assert!(pack.archive.size > 0);
    let ids: Vec<&str> = pack
        .archive
        .providers
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(ids, ["hlsr-primary", "github-fallback"]);
    for provider in &pack.archive.providers {
        assert!(is_allowed_download_url(&provider.url), "{}", provider.url);
    }
}

/// Настоящий архив из `release/saves` через настоящий 7za: в нём ровно сейвы манифеста с их SHA-256, и они
/// доезжают до `SAVE` мода и русской папки. Источник архива — локальный сервер.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_real_archive_unpacks_with_7za_and_matches_the_manifest() {
    let manifest = hl1_pack();
    let archive_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../release/saves")
        .join(&manifest.archive.filename);
    let Ok(bytes) = std::fs::read(&archive_file) else {
        eprintln!("пропуск: нет {}", archive_file.display());
        return;
    };
    assert_eq!(bytes.len() as u64, manifest.archive.size);
    assert_eq!(sha256_hex(&bytes), manifest.archive.sha256);

    let (_temp, root) = temp_root();
    let log = RequestLog::default();
    let server = start_server("a", log.clone()).await;
    server
        .routes
        .lock()
        .unwrap()
        .insert("/saves.7z".to_owned(), json!([{"bodyHex": hex(&bytes)}]));
    let mut pack = manifest.clone();
    pack.archive.providers = vec![hlsr_delivery::practice_saves::Provider {
        id: "local".to_owned(),
        url: format!("http://{}/saves.7z", server.authority),
    }];

    let install = path_in(&root, "lib/Games/hl1-won");
    for dir in ["Half-Life/valve_WON", "Half-Life/valve_WON_russian"] {
        std::fs::create_dir_all(path_in(&install, dir)).unwrap();
    }
    let library = resolve_library(path_in(&root, "lib").to_str().unwrap()).unwrap();
    let saves = PracticeSaves::new(&library)
        .with_packs(vec![pack])
        .with_archive(
            ArchiveService::new(
                Resources::detect().sevenzip(),
                Arc::new(MemoryLogger::default()),
            )
            .with_env(pinned_env()),
        )
        .with_logger(Arc::new(MemoryLogger::default()));

    let outcome = saves.ensure("hl1-won", &install).await.unwrap();
    assert_eq!(
        outcome.to_value(),
        json!({"status": "installed", "added": 126})
    );

    let mut names: Vec<&str> = manifest.files.iter().map(|f| f.name.as_str()).collect();
    names.sort();
    for dir in [
        "Half-Life/valve_WON/SAVE",
        "Half-Life/valve_WON_russian/SAVE",
    ] {
        let mut present: Vec<String> = std::fs::read_dir(path_in(&install, dir))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        present.sort();
        assert_eq!(present, names, "{dir}");
        for file in &manifest.files {
            let data = std::fs::read(path_in(&install, &format!("{dir}/{}", file.name))).unwrap();
            assert_eq!(js::latin1_string(&data[..4]), "JSAV", "{}", file.name);
            assert_eq!(sha256_hex(&data), file.sha256, "{}", file.name);
        }
    }
}
