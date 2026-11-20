//! Зависимости HL1 (BXT и Injector) против 0.4.7: фикстура `goldsrc-dependencies.json` из
//! `tools/reference/delivery/goldsrc-dependencies.js`. Сценарий — данные (серверы с маршрутами, файлы и
//! шаги): `updateDependencies` и `latestRelease` 0.4.7 в Node 24 и этот крейт выполняют его на одинаковых
//! деревьях во временных папках, локальном сервере (`tests/support/http.rs`) и настоящем 7za; сравниваются
//! итог каждого шага, строки журнала, дерево файлов (путь, размер, sha256, текст latin1) и журнал запросов
//! (адрес, `User-Agent`, `Accept`). GitHub подменён локальным сервером: API живёт по тому же пути
//! `/repos/<репозиторий>/releases/latest`, файлы релиза — по пути адреса с github.com; в интернет тесты не
//! ходят. Проверка «файл лежит на github.com» идёт по адресу из ответа API, как в 0.4.7.
//!
//! Сценарии, помеченные `unix`, зависят от ОС (права, ссылки, каталог вместо файла) и на Windows пропускаются.
//! Тексты внутренних ошибок Node/libuv и разбора JSON не воспроизводятся: у разбора JSON сверяется начало.

#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_core::paths::Resources;
use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::errors::Failure;
use hlsr_delivery::goldsrc_dependencies::{
    API_ROOT, CHECK_TIMEOUT, DOWNLOAD_TIMEOUT, GITHUB_ORIGIN, MAX_ASSET_BYTES, UpdateOptions,
    latest_release, latest_release_url, update_dependencies,
};
use hlsr_delivery::http_client::HttpClient;
use hlsr_delivery::manifests::{self, Dependency, Pin};
use hlsr_delivery::mutable_paths::matches_mutable;
use hlsr_delivery::ports::MemoryLogger;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use support::http::{
    RequestLog, Server, dead_port, fill, mutate, start_server, unfill, without_default_accept,
};
use support::scenario::{self, common_step, normalize_text, path_in, temp_root};
use support::zip::hex;
use support::{Mismatches, fixture, fixture_path, items, opt_str, sha256_hex, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("goldsrc-dependencies.json"));

/// Сообщения 7za зависят от локали (strerror), время записей — от пояса.
fn pinned_env() -> [(&'static str, &'static str); 3] {
    [("LC_ALL", "C.UTF-8"), ("LANG", "C.UTF-8"), ("TZ", "UTC")]
}

/// Байты архива сценария из `tests/fixtures/archives`.
fn archive_bytes(name: &str) -> Vec<u8> {
    let file = str_of(&FIXTURE["archives"][name]["file"]);
    std::fs::read(fixture_path(&format!("archives/{file}")))
        .unwrap_or_else(|e| panic!("архив {file}: {e}"))
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

fn leak(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}

/// Зависимости сценария: встроенные HL1 (`both`, `bxt`, `injector`) или список из данных. Как в
/// `hl1-selftest.js` 0.4.9, обновление с GitHub проверяется на копии без `pin` (закрепление 0.4.7
/// не знает; его проверяют самотесты ниже).
fn dependencies_of(spec: &Value) -> Vec<Dependency> {
    match spec {
        Value::String(name) => {
            let all = manifests::launch_config_of("hl1-won")
                .expect("настройки запуска HL1")
                .dependencies;
            all.iter()
                .filter(|d| name == "both" || d.id == name)
                .map(|d| Dependency {
                    pin: None,
                    ..d.clone()
                })
                .collect()
        }
        list => items(list)
            .iter()
            .map(|d| Dependency {
                id: leak(str_of(&d["id"])),
                title: leak(str_of(&d["title"])),
                repo: leak(str_of(&d["repo"])),
                asset: leak(str_of(&d["asset"])),
                extract: opt_str(&d["extract"]).map(leak),
                target: leak(str_of(&d["target"])),
                version_file: leak(str_of(&d["versionFile"])),
                pin: d["pin"].is_object().then(|| Pin {
                    tag: leak(str_of(&d["pin"]["tag"])),
                    sha256: leak(str_of(&d["pin"]["sha256"])),
                }),
            })
            .collect(),
    }
}

fn dependency_of(spec: &Value) -> Dependency {
    match spec {
        Value::String(_) => dependencies_of(spec).remove(0),
        object => dependencies_of(&json!([object])).remove(0),
    }
}

/// `normalizeResult` скрипта: пути — `<root>`/`<tmp>`, pid во временных файлах — метка, текст разбора JSON
/// отрезан после двоеточия.
fn normalize_result(root: &std::path::Path, value: &Value) -> Value {
    static PID: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\.hlsr-update-[a-z]+-)\d+").unwrap());
    match value {
        Value::String(text) => {
            let text = normalize_text(root, text);
            let text = PID.replace_all(&text, "${1}<pid>").into_owned();
            const MARK: &str = "Ответ не является JSON: ";
            Value::String(match text.find(MARK) {
                Some(at) => text[..at + MARK.len()].to_owned(),
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

fn failure_value(failure: &Failure) -> Value {
    match failure {
        Failure::Delivery(error) => json!({"error": {
            "code": error.code.as_str(),
            "message": error.message,
            "details": error.details,
        }}),
        Failure::Raw(error) => json!({"raw": {
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
    api_server: String,
    download_server: String,
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
            api_server: opt_str(&scenario["api"]).unwrap_or("a").to_owned(),
            download_server: opt_str(&scenario["download"]).unwrap_or("a").to_owned(),
            client: HttpClient::new(),
            archive,
            logger: Arc::new(MemoryLogger::default()),
        }
    }

    /// Адрес сервера сценария; сервера нет (пустой сценарий) — `undefined`, как в скрипте.
    fn authority(&self, name: &str) -> &str {
        self.map.get(name).map_or("undefined", String::as_str)
    }

    fn api_root(&self) -> String {
        format!("http://{}/repos/", self.authority(&self.api_server))
    }

    fn download_origin(&self) -> String {
        format!("http://{}", self.authority(&self.download_server))
    }

    fn logs(&self) -> Vec<Value> {
        self.logger
            .take()
            .into_iter()
            .map(|(level, message)| json!([level, message]))
            .collect()
    }

    async fn update(&self, step: &Value) -> Value {
        self.logger.take();
        let dependencies = dependencies_of(&step["deps"]);
        let install = path_in(&self.root, opt_str(&step["installDir"]).unwrap_or("game"));
        let api_root = self.api_root();
        let origin = self.download_origin();
        let logger: Arc<MemoryLogger> = self.logger.clone();
        let options = UpdateOptions {
            client: Some(&self.client),
            archive: (step["archive"] != json!(false)).then_some(&self.archive),
            logger: Some(logger.as_ref()),
            api_root: Some(&api_root),
            download_origin: Some(&origin),
        };
        let mut value = match update_dependencies(&install, &dependencies, &options).await {
            Ok(results) => json!({"ok": results.iter().map(|r| r.to_value()).collect::<Vec<_>>()}),
            Err(error) => failure_value(&Failure::Delivery(error)),
        };
        value["logs"] = Value::Array(self.logs());
        normalize_result(&self.root, &value)
    }

    async fn latest(&self, step: &Value) -> Value {
        let dependency = dependency_of(&step["dep"]);
        let value = match latest_release(&dependency, &self.client, &self.api_root()).await {
            Ok(release) => json!({"ok": release.to_value()}),
            Err(failure) => failure_value(&failure),
        };
        normalize_result(&self.root, &value)
    }

    /// Запросы к серверам с прошлого шага `requests`: метод, адрес, `User-Agent` и `Accept` (кроме
    /// `*/*`, который reqwest шлёт сам).
    fn requests(&mut self) -> Value {
        let log = self.log.lock().unwrap();
        let fresh = without_default_accept(&log[self.seen..]);
        self.seen = log.len();
        let requests: Vec<Value> = fresh
            .iter()
            .map(|entry| {
                let mut headers = Map::new();
                headers.insert("user-agent".into(), entry["headers"]["user-agent"].clone());
                if let Some(accept) = entry["headers"].get("accept") {
                    headers.insert("accept".into(), accept.clone());
                }
                json!({"method": entry["method"], "url": entry["url"], "headers": headers})
            })
            .collect();
        json!({"requests": requests})
    }

    async fn step(&mut self, step: &Value) -> Value {
        if let Some(common) = common_step(&self.root, step) {
            return common;
        }
        match str_of(&step["op"]) {
            "update" => self.update(step).await,
            "latest" => self.latest(step).await,
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

/// Сценарии 0.4.7, где GitHub не дал SHA-256 (или дал не тот вид): 0.4.7 ставил файл без проверки,
/// 0.4.9 — нет (`CHECKSUM_MISMATCH`). Их проверяет `a_file_without_a_sha256_from_github_is_not_installed`.
const UNHASHED_0_4_9: [&str; 3] = [
    "API: digest отсутствует",
    "API: digest заглавными",
    "API: digest sha1",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_dependencies_on_scenarios_with_a_local_server_and_the_real_7za() {
    let scenarios = items(&FIXTURE["scenarios"]);
    assert!(scenarios.len() > 50, "сценариев: {}", scenarios.len());
    let mut mismatches = Mismatches::default();
    for scenario in scenarios {
        let name = str_of(&scenario["name"]);
        if scenario["unix"].as_bool().unwrap_or(false) && cfg!(windows) {
            continue;
        }
        if UNHASHED_0_4_9.contains(&name) {
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
    mismatches.finish("goldsrc-dependencies: сценарии");
}

// ----------------------------------------------------------------- самотесты 0.4.7

fn dependency_json(d: &Dependency) -> Value {
    let mut object = json!({
        "id": d.id, "title": d.title, "repo": d.repo, "asset": d.asset,
        "target": d.target, "versionFile": d.version_file,
    });
    if let Some(extract) = d.extract {
        object["extract"] = json!(extract);
    }
    object
}

#[test]
fn hl1_dependencies_are_declared_and_are_not_a_corruption() {
    let config = manifests::launch_config_of("hl1-won").unwrap();
    let repos: Vec<&str> = config.dependencies.iter().map(|d| d.repo).collect();
    assert_eq!(repos, ["YaLTeR/BunnymodXT", "YaLTeR/BunnymodXT-Injector"]);
    // Те же зависимости, что считал скрипт по manifests.js 0.4.7.
    let declared: Vec<Value> = config.dependencies.iter().map(dependency_json).collect();
    assert_eq!(Value::Array(declared), FIXTURE["dependencies"]);

    let bxt = regress::Regex::with_flags(config.dependencies[0].asset, "i").unwrap();
    assert!(bxt.find("1-BunnymodXT-Windows.7z").is_some());
    assert!(
        bxt.find("3-BunnymodXT-CoFSteam-Windows.7z").is_none(),
        "сборка для Cry of Fear не подходит"
    );
    for dependency in config.dependencies {
        assert!(matches_mutable(dependency.target), "{}", dependency.target);
        assert!(
            matches_mutable(dependency.version_file),
            "{}",
            dependency.version_file
        );
        assert_eq!(FIXTURE["mutable"][dependency.target], json!(true));
        assert_eq!(FIXTURE["mutable"][dependency.version_file], json!(true));
    }
}

#[test]
fn github_addresses_limits_and_timeouts_are_the_ones_of_0_4_7() {
    assert_eq!(API_ROOT, str_of(&FIXTURE["apiRoot"]));
    assert_eq!(format!("{GITHUB_ORIGIN}/"), str_of(&FIXTURE["github"]));
    let urls = FIXTURE["apiUrls"].as_object().unwrap();
    assert!(urls.len() >= 2);
    for (repo, url) in urls {
        assert_eq!(latest_release_url(API_ROOT, repo), url.as_str().unwrap());
    }
    assert_eq!(MAX_ASSET_BYTES, FIXTURE["maxAssetBytes"].as_u64().unwrap());
    // Сценарии не видят тайм-ауты: скрипт записывает то, что 0.4.7 передал клиенту.
    assert_eq!(
        FIXTURE["timeouts"]["json"],
        json!([CHECK_TIMEOUT.as_millis() as u64])
    );
    assert_eq!(
        FIXTURE["timeouts"]["download"],
        json!([DOWNLOAD_TIMEOUT.as_millis() as u64])
    );
}

#[test]
fn the_scenario_archives_are_the_ones_the_fixture_describes() {
    for (name, info) in FIXTURE["archives"].as_object().unwrap() {
        let bytes = archive_bytes(name);
        assert_eq!(bytes.len() as u64, info["size"].as_u64().unwrap(), "{name}");
        assert_eq!(sha256_hex(&bytes), str_of(&info["sha256"]), "{name}");
    }
}

// ----------------------------------------------------------------- самотесты 0.4.9 (S-2)

fn scenario_of(name: &str) -> &'static Value {
    items(&FIXTURE["scenarios"])
        .iter()
        .find(|scenario| str_of(&scenario["name"]) == name)
        .unwrap_or_else(|| panic!("нет сценария {name}"))
}

/// Пути запросов шага `requests`.
fn request_urls(requests: &Value) -> Vec<String> {
    items(&requests["requests"])
        .iter()
        .map(|request| str_of(&request["url"]).to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_without_a_sha256_from_github_is_not_installed() {
    for name in UNHASHED_0_4_9 {
        let scenario = scenario_of(name);
        let mut context = Context::new(scenario).await;
        scenario::setup(&context.root, &scenario["setup"]);
        let tree = json!({"op": "tree"});
        let before = context.step(&tree).await;
        let update = fill(&items(&scenario["steps"])[0], &context.map);
        let result = context.step(&update).await;
        let first = &result["ok"][0];
        assert_eq!(first["status"], "failed", "{name}: {result}");
        assert_eq!(
            first["error"]["code"], "CHECKSUM_MISMATCH",
            "{name}: {result}"
        );
        assert_eq!(
            context.step(&tree).await,
            before,
            "{name}: файлы не тронуты"
        );
        let urls = request_urls(&context.requests());
        assert!(
            urls.iter().all(|url| !url.contains("/download/")),
            "{name}: файл не качался: {urls:?}"
        );
        scenario::open_up(&context.root);
    }
}

const INJECTOR_URL: &str =
    "https://github.com/YaLTeR/BunnymodXT-Injector/releases/download/v2/Injector.exe";

/// Сервер с релизом `v2` Injector по тегу (`digest` — что GitHub сообщает о файле) и самим файлом.
fn pinned_scenario(body: &str, digest: Option<String>) -> Value {
    let mut asset = json!({
        "name": "Injector.exe",
        "size": body.len(),
        "browser_download_url": INJECTOR_URL,
    });
    if let Some(digest) = digest {
        asset["digest"] = json!(digest);
    }
    let release = json!({"tag_name": "v2", "assets": [asset]});
    json!({
        "name": "pin",
        "setup": [
            {"file": "game/Bunnymod XT/.hlsr-injector-version", "utf8": "v1"},
            {"file": "game/Bunnymod XT/Injector.exe", "utf8": "MZ"},
        ],
        "servers": {"a": {
            "/repos/YaLTeR/BunnymodXT-Injector/releases/tags/v2": [{"body": release.to_string()}],
            "/YaLTeR/BunnymodXT-Injector/releases/download/v2/Injector.exe": [{"body": body}],
        }},
    })
}

/// Встроенный Injector с `pin` из теста.
fn pinned_injector(sha256: &str) -> Value {
    let injector = manifests::launch_config_of("hl1-won")
        .unwrap()
        .dependencies
        .iter()
        .find(|d| d.id == "injector")
        .cloned()
        .unwrap();
    let mut spec = dependency_json(&injector);
    spec["pin"] = json!({"tag": "v2", "sha256": sha256});
    json!({"op": "update", "deps": [spec]})
}

fn injector_text(context: &Context) -> String {
    std::fs::read_to_string(path_in(&context.root, "game/Bunnymod XT/Injector.exe")).unwrap()
}

#[test]
fn the_injector_is_pinned_because_github_gives_no_sha256_for_its_release() {
    let config = manifests::launch_config_of("hl1-won").unwrap();
    let injector = config
        .dependencies
        .iter()
        .find(|d| d.id == "injector")
        .unwrap();
    let pin = injector.pin.as_ref().expect("Injector закреплён");
    assert_eq!(pin.tag, "v3.2");
    assert!(Regex::new("^[a-f0-9]{64}$").unwrap().is_match(pin.sha256));
    let bxt = config.dependencies.iter().find(|d| d.id == "bxt").unwrap();
    assert!(
        bxt.pin.is_none(),
        "BXT берёт последний релиз с хешем от GitHub"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pinned_version_is_taken_by_tag_and_checked_against_the_launcher_hash() {
    let body = "MZ pinned injector";
    let scenario = pinned_scenario(body, None);
    let mut context = Context::new(&scenario).await;
    scenario::setup(&context.root, &scenario["setup"]);
    let update = pinned_injector(&sha256_hex(body.as_bytes()));

    let result = context.step(&update).await;
    assert_eq!(result["ok"][0]["status"], "updated", "{result}");
    assert_eq!(result["ok"][0]["to"], "v2");
    let urls = request_urls(&context.requests());
    assert!(
        urls[0].ends_with("/repos/YaLTeR/BunnymodXT-Injector/releases/tags/v2"),
        "{urls:?}"
    );
    assert_eq!(injector_text(&context), body);

    let again = context.step(&update).await;
    assert_eq!(again["ok"][0]["status"], "up-to-date", "{again}");
    assert_eq!(
        request_urls(&context.requests()),
        Vec::<String>::new(),
        "закреплённая версия на месте: в сеть не ходим"
    );
    scenario::open_up(&context.root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_installed_current_release_without_a_sha256_from_github_is_up_to_date() {
    let mut scenario = pinned_scenario("MZ", None);
    let routes = scenario["servers"]["a"].as_object_mut().unwrap();
    let release = routes
        .remove("/repos/YaLTeR/BunnymodXT-Injector/releases/tags/v2")
        .unwrap();
    routes.insert(
        "/repos/YaLTeR/BunnymodXT-Injector/releases/latest".to_owned(),
        release,
    );
    scenario["setup"][0]["utf8"] = json!("v2");
    let mut context = Context::new(&scenario).await;
    scenario::setup(&context.root, &scenario["setup"]);
    let mut update = pinned_injector("");
    update["deps"][0].as_object_mut().unwrap().remove("pin");

    let result = context.step(&update).await;
    assert_eq!(result["ok"][0]["status"], "up-to-date", "{result}");
    let urls = request_urls(&context.requests());
    assert!(
        urls.iter().all(|url| !url.contains("/download/")),
        "файл не качался: {urls:?}"
    );
    scenario::open_up(&context.root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pinned_version_rejects_another_file_even_if_github_confirms_its_hash() {
    let body = "MZ tampered release";
    let digest = format!("sha256:{}", sha256_hex(body.as_bytes()));
    let scenario = pinned_scenario(body, Some(digest));
    let mut context = Context::new(&scenario).await;
    scenario::setup(&context.root, &scenario["setup"]);

    let result = context.step(&pinned_injector(&"0".repeat(64))).await;
    assert_eq!(result["ok"][0]["status"], "failed", "{result}");
    assert_eq!(result["ok"][0]["error"]["code"], "CHECKSUM_MISMATCH");
    assert_eq!(injector_text(&context), "MZ");
    scenario::open_up(&context.root);
}
