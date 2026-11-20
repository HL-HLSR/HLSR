//! Проверка обновлений на локальном сервере: наборы установленных версий против документов
//! канала, вшитые манифесты, снижение результата при сбое, интервал и кеш, события и общий
//! запуск. Сценарии источника HL1 0.4.7 (новая сборка по дате, записи `*.origin.json`) 0.4.9 убрал:
//! ставится только закреплённая сборка (`channel_selftests.rs`).

use super::scenario::{
    FIXTURE, check_entry, failure_json, fill, join_all, names, ok, scenario, start_world, unfill,
};
use super::support::tree::snapshot;
use super::support::{Mismatches, items, str_of};
use hlsr_delivery::channel_service::{ChannelOptions, ChannelService};
use hlsr_delivery::environment::Environment;
use hlsr_delivery::manifests;
use hlsr_delivery::origin_updates::{Baseline, Origin, OriginOptions, OriginUpdates};
use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::timing::{Clock, Random};
use hlsr_delivery::update_checker::{
    CheckOptions, CheckerOptions, InstalledVersionOf, LocalManifestOf, UpdateChecker, UpdateEvent,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn origins_of(spec: &Value) -> Vec<(String, Origin)> {
    spec.as_object()
        .unwrap()
        .iter()
        .map(|(game_id, origin)| {
            let baseline = &origin["baseline"];
            (
                game_id.clone(),
                Origin {
                    provider_id: str_of(&origin["providerId"]).to_owned(),
                    baseline: Baseline {
                        version: str_of(&baseline["version"]).to_owned(),
                        last_modified: str_of(&baseline["lastModified"]).to_owned(),
                        etag: str_of(&baseline["etag"]).to_owned(),
                        size: baseline["size"].as_u64().unwrap(),
                    },
                },
            )
        })
        .collect()
}

/// Манифест игры из сценария: `{"$embedded": id, "providers": [...]}` — вшитый манифест с
/// заменой источников базового архива; прочее — как есть.
fn local_manifest(spec: &Value) -> Option<Value> {
    if spec.is_null() {
        return None;
    }
    let Some(id) = spec.get("$embedded").and_then(Value::as_str) else {
        return Some(spec.clone());
    };
    // Фикстура снята со вшитым манифестом Ghosting 0.4.7 (1.0.9); в config/ теперь 0.4.9 (1.0.13).
    let mut manifest = if id == "ghosting-renewed" {
        serde_json::from_str(include_str!(
            "../fixtures/ghosting-renewed-0.4.7.manifest.json"
        ))
        .unwrap()
    } else {
        manifests::manifest_of(id).expect("вшитый манифест").clone()
    };
    if let Some(providers) = spec.get("providers") {
        manifest["baseArchive"]["providers"] = providers.clone();
    }
    Some(manifest)
}

fn event_json(checker: &UpdateChecker, event: &UpdateEvent) -> Value {
    match event {
        UpdateEvent::Checking { game_id, reason } => json!({
            "event": "checking",
            "data": { "gameId": game_id, "reason": reason },
            "isChecking": checker.is_checking(game_id),
        }),
        UpdateEvent::Result(result) => {
            let game_id = str_of(&result.to_json()["gameId"]).to_owned();
            json!({
                "event": "result",
                "data": result.to_json(),
                "isChecking": checker.is_checking(&game_id),
                "last": checker.last_result(&game_id).map_or(Value::Null, |last| last.to_json()),
            })
        }
    }
}

/// 0.4.9: у цели обновления нет `repairArchives` (`update-target.js`); в фикстуре 0.4.7 он есть —
/// у объектов вида цели (`source` и `repairMirror`) поле не сравнивается.
fn without_repair_archives(value: &Value) -> Value {
    match value {
        Value::Array(list) => Value::Array(list.iter().map(without_repair_archives).collect()),
        Value::Object(map) => {
            let target = map.contains_key("source") && map.contains_key("repairMirror");
            Value::Object(
                map.iter()
                    .filter(|(key, _)| !(target && key.as_str() == "repairArchives"))
                    .map(|(key, item)| (key.clone(), without_repair_archives(item)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

fn check_options(step: &Value) -> CheckOptions {
    CheckOptions {
        force: step["force"].as_bool().unwrap_or(false),
        reason: step["reason"].as_str().map(str::to_owned),
        ignore_cache: step["ignoreCache"].as_bool().unwrap_or(false),
    }
}

async fn run_scenario(spec: &Value) -> Mismatches {
    let name = str_of(&spec["name"]);
    let world = start_world(spec).await;
    let root = tempfile::tempdir().unwrap();
    let state_dir = root.path().join("state");
    let mut mask: BTreeMap<String, String> = world.map.clone();
    mask.insert(
        "root".to_owned(),
        root.path().to_string_lossy().into_owned(),
    );

    let now = Arc::new(AtomicI64::new(FIXTURE["now"].as_i64().unwrap()));
    let clock: Clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let random: Random = Arc::new(|| 0.5);
    let logger = Arc::new(MemoryLogger::default());

    let installed: Arc<Mutex<HashMap<String, Option<String>>>> = Arc::new(Mutex::new(
        spec["installed"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(game_id, version)| (game_id.clone(), version.as_str().map(str::to_owned)))
            .collect(),
    ));
    let installed_version_of: InstalledVersionOf = {
        let installed = installed.clone();
        Arc::new(move |game_id| installed.lock().unwrap().get(game_id).cloned().flatten())
    };
    let locals: HashMap<String, Option<Value>> = spec["local"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(game_id, manifest)| (game_id.clone(), local_manifest(&fill(manifest, &world.map))))
        .collect();
    let local_manifest_of: LocalManifestOf =
        Arc::new(move |game_id| locals.get(game_id).cloned().flatten());

    let origin = spec["origin"].as_object().map(|origin| {
        Arc::new(OriginUpdates::new(OriginOptions {
            logger: Some(logger.clone()),
            http: None,
            origins: Some(origins_of(&origin["origins"])),
        }))
    });
    let channel = spec["channel"].as_object().map(|channel| {
        let env = fill(&channel["env"], &world.map);
        let variables: Vec<(String, String)> = env
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| (name.clone(), str_of(value).to_owned()))
            .collect();
        let options = &channel["options"];
        Arc::new(ChannelService::new(ChannelOptions {
            logger: Some(logger.clone()),
            env: Some(Environment::new(variables, false)),
            http: None,
            timeout: options["timeout"].as_u64().map(Duration::from_millis),
            max_attempts: options["maxAttempts"].as_u64().map(|n| n as u32),
            backoff_base_ms: options["backoffBaseMs"].as_u64(),
            clock: Some(clock.clone()),
            random: Some(random.clone()),
            freshness_file: None,
        }))
    });

    let checker = Arc::new(UpdateChecker::new(CheckerOptions {
        logger: Some(logger.clone()),
        channel_service: channel,
        origin_updates: origin.clone(),
        installed_version_of: Some(installed_version_of),
        local_manifest_of: Some(local_manifest_of),
        launcher_version: spec["launcherVersion"].as_str().map(str::to_owned),
        min_interval_ms: spec["minIntervalMs"].as_i64(),
        clock: Some(clock),
    }));
    let events: Arc<Mutex<Vec<Value>>> = Arc::default();
    {
        let (weak, events) = (Arc::downgrade(&checker), events.clone());
        checker.on_event(move |event| {
            if let Some(checker) = weak.upgrade() {
                events.lock().unwrap().push(event_json(&checker, event));
            }
        });
    }

    let mut mark = 0;
    let mut m = Mismatches::default();
    for (index, (step, expected)) in items(&spec["steps"])
        .iter()
        .zip(items(&spec["results"]))
        .enumerate()
    {
        let _ = logger.take();
        events.lock().unwrap().clear();
        let game_id = step["gameId"].as_str().unwrap_or("");
        let mut entry = Map::new();
        let origin_ref = || origin.as_ref().expect("в сценарии есть источник");
        match str_of(&step["kind"]) {
            "check" => {
                let result = checker.check(game_id, &check_options(step)).await;
                entry.insert("result".to_owned(), result.to_json());
            }
            "concurrent" => {
                let checks = items(&step["checks"]).iter().map(|item| {
                    let checker = checker.clone();
                    async move {
                        let id = str_of(&item["gameId"]).to_owned();
                        checker.check(&id, &check_options(item)).await.to_json()
                    }
                });
                let all = join_all(checks).await;
                entry.insert(
                    "same".to_owned(),
                    json!(all.iter().all(|item| item == &all[0])),
                );
                entry.insert("results".to_owned(), Value::Array(all));
            }
            "installed" => {
                installed.lock().unwrap().insert(
                    game_id.to_owned(),
                    step["version"].as_str().map(str::to_owned),
                );
            }
            "clock" => {
                let advance = step["advanceMs"].as_i64().unwrap();
                entry.insert(
                    "now".to_owned(),
                    json!(now.fetch_add(advance, Ordering::SeqCst) + advance),
                );
            }
            "older" => {
                let result = checker
                    .check_if_older_than(
                        game_id,
                        step["maxAgeMs"].as_i64().unwrap(),
                        step["reason"].as_str(),
                    )
                    .await;
                entry.insert(
                    "result".to_owned(),
                    result.map_or(Value::Null, |result| result.to_json()),
                );
            }
            "invalidate" => checker.invalidate(game_id),
            "due" => {
                entry.insert("result".to_owned(), json!(checker.is_due(game_id)));
            }
            "last" => {
                entry.insert(
                    "result".to_owned(),
                    checker
                        .last_result(game_id)
                        .map_or(Value::Null, |result| result.to_json()),
                );
            }
            "supports" => {
                entry.insert(
                    "result".to_owned(),
                    ok(json!(origin_ref().supports(game_id))),
                );
            }
            "target" => {
                let manifest = local_manifest(&fill(&step["manifest"], &world.map));
                let result = match origin_ref().target(game_id, manifest.as_ref()).await {
                    Ok(target) => ok(target.unwrap_or(Value::Null)),
                    Err(failure) => failure_json(&failure),
                };
                entry.insert("result".to_owned(), result);
            }
            "tree" => {
                let tree = if state_dir.exists() {
                    snapshot(&state_dir, None)
                } else {
                    json!([])
                };
                entry.insert("tree".to_owned(), tree);
            }
            other => panic!("неизвестный шаг {other}"),
        }
        let logs: Vec<Value> = logger
            .take()
            .into_iter()
            .map(|(level, message)| json!({ "level": level, "message": message }))
            .collect();
        entry.insert("logs".to_owned(), Value::Array(logs));
        let seen = std::mem::take(&mut *events.lock().unwrap());
        if !seen.is_empty() {
            entry.insert("events".to_owned(), Value::Array(seen));
        }
        let (requests, next) = world.requests_since(mark);
        mark = next;
        if !requests.is_empty() {
            entry.insert("requests".to_owned(), Value::Array(requests));
        }
        let actual = without_repair_archives(&unfill(&Value::Object(entry), &mask));
        check_entry(
            &mut m,
            &actual,
            &without_repair_archives(expected),
            format!("{name}, шаг {index}: {step}"),
        );
    }
    m
}

macro_rules! scenario_tests {
    ($($test:ident => $name:literal,)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn $test() {
                run_scenario(scenario(&FIXTURE["checkers"], $name)).await.finish($name);
            }
        )*

        #[test]
        fn every_checker_scenario_is_checked() {
            // Источник HL1 0.4.7 (новая сборка по дате, записи) 0.4.9 убрал.
            assert_eq!(
                names(&FIXTURE["checkers"]),
                [$($name,)* "origin", "origin-targets", "origin-records", "origin-record-dir"]
            );
        }
    };
}

scenario_tests! {
    plans_for_installed_versions_match_0_4_7 => "plans",
    channel_documents_match_0_4_7 => "documents",
    failed_checks_degrade_the_last_result_like_0_4_7 => "degrade",
    a_tampered_channel_degrades_like_0_4_7 => "degrade-tampered",
    a_broken_answer_after_a_good_one_marks_the_result_stale => "stale-after-good",
    a_disabled_channel_falls_back_to_the_bundled_manifest => "disabled-channel",
    no_channel_service_falls_back_to_the_bundled_manifest => "no-channel-service",
    the_check_interval_and_cache_match_0_4_7 => "interval",
    a_zero_interval_always_checks => "zero-interval",
    concurrent_checks_share_one_run => "concurrent",
}
