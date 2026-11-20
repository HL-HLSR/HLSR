//! Кеш файловых манифестов `.hlsr/manifests` на диске и загрузка через зеркала: байты
//! `files.json` и `trust.json`, дерево каталога после каждого шага, журнал и попытки
//! зеркал. Часы поддельные, так что `storedAt` — часть сверки.

use super::scenario::{
    FIXTURE, World, check_entry, delivery_json, failure_json, fill, join_all, names, ok, scenario,
    start_world, unfill,
};
use super::support::tree::snapshot;
use super::support::{Mismatches, items, str_of};
use hlsr_core::json_text::stringify;
use hlsr_delivery::environment::Environment;
use hlsr_delivery::files_manifest_store::{FetchOptions, FilesManifestStore, StoreOptions};
use hlsr_delivery::paths::resolve_library;
use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::timing::{Clock, Random};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

struct Stores {
    root: tempfile::TempDir,
    spec: Value,
    world: World,
    logger: Arc<MemoryLogger>,
    clock: Clock,
    random: Random,
    by_timeout: HashMap<u64, FilesManifestStore>,
}

impl Stores {
    fn root(&self) -> &Path {
        self.root.path()
    }

    /// Тайм-аут запроса мс: свой у шага или из сценария (0 — по умолчанию).
    fn effective(&self, step_timeout: u64) -> u64 {
        if step_timeout != 0 {
            step_timeout
        } else {
            self.spec["store"]["options"]["timeout"]
                .as_u64()
                .unwrap_or(0)
        }
    }

    /// Хранилище с заданным тайм-аутом запроса; все делят каталог на диске.
    fn ensure(&mut self, step_timeout: u64) {
        let timeout = self.effective(step_timeout);
        if self.by_timeout.contains_key(&timeout) {
            return;
        }
        let options = &self.spec["store"]["options"];
        let env = fill(&self.spec["store"]["env"], &self.world.map);
        let variables: Vec<(String, String)> = env
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| (name.clone(), str_of(value).to_owned()))
            .collect();
        let paths = resolve_library(&self.root().to_string_lossy()).unwrap();
        let store = FilesManifestStore::new(
            &paths,
            StoreOptions {
                logger: Some(self.logger.clone()),
                env: Some(Environment::new(variables, false)),
                http: None,
                timeout: (timeout != 0).then(|| Duration::from_millis(timeout)),
                max_attempts: options["maxAttempts"].as_u64().map(|n| n as u32),
                backoff_base_ms: options["backoffBaseMs"].as_u64(),
                mirrors: None,
                clock: Some(self.clock.clone()),
                random: Some(self.random.clone()),
            },
        );
        self.by_timeout.insert(timeout, store);
    }

    fn store(&self, step_timeout: u64) -> &FilesManifestStore {
        &self.by_timeout[&self.effective(step_timeout)]
    }
}

fn manifest_summary(manifest: &Value) -> Value {
    let mut out = Map::new();
    for key in ["totalFiles", "manifestSha256", "version"] {
        if let Some(value) = manifest.get(key) {
            out.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(out)
}

async fn fetch_call(stores: &Stores, call: &Value) -> Value {
    let timeout = call["timeout"].as_u64().unwrap_or(0);
    let store = stores.store(timeout);
    let descriptor = fill(&call["descriptor"], &stores.world.map);
    let options = match call.get("options") {
        None => FetchOptions::default(),
        Some(options) => FetchOptions {
            channel_sha256: options["channelSha256"].as_str().map(str::to_owned),
            source: options.get("source").cloned(),
        },
    };
    let result = match store
        .fetch(str_of(&call["gameId"]), &descriptor, &options)
        .await
    {
        Ok(manifest) => ok(manifest_summary(&manifest)),
        Err(error) => delivery_json(&error),
    };
    json!({
        "result": result,
        "attempts": store.last_mirror_attempts().iter().map(|a| a.to_json()).collect::<Vec<_>>(),
    })
}

fn call_of(stores: &Stores, call: &Value) -> Value {
    let store = stores.store(0);
    let game_id = str_of(&call["gameId"]);
    let version = call["version"].as_str().unwrap_or("");
    match str_of(&call["call"]) {
        "has" => ok(json!(store.has(game_id, version))),
        "list" => ok(json!(store.list(game_id))),
        "load" => ok(store.load(game_id, version).unwrap_or(Value::Null)),
        "store" => {
            let meta = call.get("meta").and_then(Value::as_object);
            match store.store(game_id, version, str_of(&call["raw"]).as_bytes(), meta) {
                Ok(path) => ok(json!(path.to_string_lossy())),
                Err(failure) => failure_json(&failure),
            }
        }
        other => panic!("неизвестный вызов {other}"),
    }
}

fn write_files(root: &Path, files: &Value) {
    for file in files.as_array().into_iter().flatten() {
        let target = root.join(str_of(&file["path"]));
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, str_of(&file["text"])).unwrap();
    }
}

async fn run_scenario(spec: &Value) -> Mismatches {
    let name = str_of(&spec["name"]);
    let world = start_world(spec).await;
    let now = FIXTURE["now"].as_i64().unwrap();
    let mut stores = Stores {
        root: tempfile::tempdir().unwrap(),
        spec: spec.clone(),
        world,
        logger: Arc::new(MemoryLogger::default()),
        clock: Arc::new(move || now),
        random: Arc::new(|| 0.5),
        by_timeout: HashMap::new(),
    };
    stores.ensure(0);
    let mut mask: BTreeMap<String, String> = stores.world.map.clone();
    mask.insert(
        "root".to_owned(),
        stores.root().to_string_lossy().into_owned(),
    );
    let mut mark = 0;
    let mut m = Mismatches::default();

    for (index, (step, expected)) in items(&spec["steps"])
        .iter()
        .zip(items(&spec["results"]))
        .enumerate()
    {
        let _ = stores.logger.take();
        let mut entry = Map::new();
        match str_of(&step["kind"]) {
            "write" | "mkdir" | "big" => {
                for dir in step["paths"].as_array().into_iter().flatten() {
                    std::fs::create_dir_all(stores.root().join(str_of(dir))).unwrap();
                }
                write_files(stores.root(), &step["files"]);
                if str_of(&step["kind"]) == "big" {
                    let target = stores.root().join(str_of(&step["path"]));
                    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                    let file = std::fs::File::create(&target).unwrap();
                    file.set_len(step["size"].as_u64().unwrap()).unwrap();
                    let meta = stores.root().join(str_of(&step["metaFile"]["path"]));
                    std::fs::write(meta, stringify(&step["metaFile"]["meta"], "  ") + "\n")
                        .unwrap();
                }
                if step.get("then").is_some() {
                    entry.insert("result".to_owned(), call_of(&stores, &step["then"]));
                }
            }
            "call" => {
                entry.insert("result".to_owned(), call_of(&stores, step));
            }
            "fetch" => {
                let timeout = step["timeout"].as_u64().unwrap_or(0);
                stores.ensure(timeout);
                let done = fetch_call(&stores, step).await;
                entry.insert("result".to_owned(), done["result"].clone());
                entry.insert("attempts".to_owned(), done["attempts"].clone());
            }
            "parallel" => {
                for call in items(&step["calls"]) {
                    stores.ensure(call["timeout"].as_u64().unwrap_or(0));
                }
                let calls = items(&step["calls"]).iter().map(|call| {
                    let stores = &stores;
                    async move {
                        if let Some(ms) = call["startAfterMs"].as_u64() {
                            tokio::time::sleep(Duration::from_millis(ms)).await;
                        }
                        fetch_call(stores, call).await
                    }
                });
                entry.insert("parallel".to_owned(), Value::Array(join_all(calls).await));
            }
            other => panic!("неизвестный шаг {other}"),
        }
        let logs: Vec<Value> = stores
            .logger
            .take()
            .into_iter()
            .map(|(level, message)| json!({ "level": level, "message": message }))
            .collect();
        entry.insert("logs".to_owned(), Value::Array(logs));
        if step["tree"].as_bool() == Some(true) {
            entry.insert("tree".to_owned(), snapshot(stores.root(), None));
        }
        let (requests, next) = stores.world.requests_since(mark);
        mark = next;
        if !requests.is_empty() {
            entry.insert("requests".to_owned(), Value::Array(requests));
        }
        let actual = unfill(&Value::Object(entry), &mask);
        check_entry(
            &mut m,
            &actual,
            expected,
            format!("{name}, шаг {index}: {step}"),
        );
    }
    m
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetching_into_the_cache_matches_0_4_7() {
    run_scenario(scenario(&FIXTURE["stores"], "fetch"))
        .await
        .finish("fetch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn descriptor_checks_match_0_4_7() {
    run_scenario(scenario(&FIXTURE["stores"], "descriptors"))
        .await
        .finish("descriptors");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_files_on_disk_match_0_4_7() {
    run_scenario(scenario(&FIXTURE["stores"], "disk"))
        .await
        .finish("disk");
}

#[test]
fn every_store_scenario_is_checked() {
    assert_eq!(names(&FIXTURE["stores"]), ["fetch", "descriptors", "disk"]);
}
