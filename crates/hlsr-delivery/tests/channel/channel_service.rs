//! Служба канала на локальном сервере: ответы 200 и 304 с ETag и Last-Modified, кеш в памяти,
//! повторы, подпись и схема, QA-переопределение адреса, ключа и имени канала.

use super::scenario::{
    FIXTURE, World, check, check_entry, delivery_json, fill, names, ok, scenario, start_world,
    unfill,
};
use super::support::{Mismatches, items, str_of};
use hlsr_delivery::channel_config::ChannelConfig;
use hlsr_delivery::channel_service::{CacheEntry, ChannelOptions, ChannelService};
use hlsr_delivery::environment::Environment;
use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::timing::{Clock, Random};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

pub fn config_json(config: &ChannelConfig) -> Value {
    json!({
        "gameId": config.game_id,
        "url": config.url,
        "channel": config.channel,
        "requireSignature": config.require_signature,
        "publicKeys": config.public_keys,
        "source": config.source.map(|source| source.as_str()),
    })
}

fn cache_json(entry: Option<Arc<CacheEntry>>) -> Value {
    entry.map_or(Value::Null, |entry| entry.to_json())
}

/// Служба по описанию из фикстуры: `{env, options}`; адреса серверов подставлены.
fn make_service(
    def: &Value,
    world: &World,
    logger: Arc<MemoryLogger>,
    clock: Clock,
    random: Random,
) -> ChannelService {
    let env = fill(&def["env"], &world.map);
    let variables: Vec<(String, String)> = env
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, value)| (name.clone(), str_of(value).to_owned()))
        .collect();
    let options = &def["options"];
    ChannelService::new(ChannelOptions {
        logger: Some(logger),
        env: Some(Environment::new(variables, false)),
        http: None,
        timeout: options["timeout"].as_u64().map(Duration::from_millis),
        max_attempts: options["maxAttempts"].as_u64().map(|n| n as u32),
        backoff_base_ms: options["backoffBaseMs"].as_u64(),
        clock: Some(clock),
        random: Some(random),
        freshness_file: None,
    })
}

async fn run_scenario(spec: &Value) -> Mismatches {
    let name = str_of(&spec["name"]);
    let world = start_world(spec).await;
    let now = Arc::new(AtomicI64::new(FIXTURE["now"].as_i64().unwrap()));
    let clock: Clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let random: Random = Arc::new(|| 0.5);
    let logger = Arc::new(MemoryLogger::default());
    let mut services: HashMap<String, ChannelService> = HashMap::new();
    let mut m = Mismatches::default();

    for (index, (step, expected)) in items(&spec["steps"])
        .iter()
        .zip(items(&spec["results"]))
        .enumerate()
    {
        let _ = logger.take();
        let service_name = step["service"].as_str().unwrap_or("main");
        if str_of(&step["kind"]) != "clock" && !services.contains_key(service_name) {
            let def = &spec["services"][service_name];
            services.insert(
                service_name.to_owned(),
                make_service(def, &world, logger.clone(), clock.clone(), random.clone()),
            );
        }
        let game_id = step["gameId"].as_str().unwrap_or("");
        let mut cache = None;
        let result = match str_of(&step["kind"]) {
            "fetch" => {
                let service = &services[service_name];
                let ignore = step["ignoreCache"].as_bool().unwrap_or(false);
                let result = match service.fetch(game_id, ignore).await {
                    Ok(fetched) => ok(json!({
                        "status": fetched.status.as_str(),
                        "manifest": fetched.manifest,
                        "config": config_json(&fetched.config),
                    })),
                    Err(error) => delivery_json(&error),
                };
                cache = Some(cache_json(service.cached_for(game_id)));
                result
            }
            "cached" => ok(cache_json(services[service_name].cached_for(game_id))),
            "config" => ok(services[service_name]
                .config_for(game_id)
                .map_or(Value::Null, |config| config_json(&config))),
            "clock" => {
                let advance = step["advanceMs"].as_i64().unwrap();
                ok(json!(now.fetch_add(advance, Ordering::SeqCst) + advance))
            }
            other => panic!("неизвестный шаг {other}"),
        };
        let logs: Vec<Value> = logger
            .take()
            .into_iter()
            .map(|(level, message)| json!({ "level": level, "message": message }))
            .collect();
        let mut entry = json!({ "result": result, "logs": logs });
        if let Some(cache) = cache {
            entry["cache"] = cache;
        }
        let actual = unfill(&entry, &world.map);
        check_entry(
            &mut m,
            &actual,
            expected,
            format!("{name}, шаг {index}: {step}"),
        );
    }
    let (requests, _) = world.requests_since(0);
    check(
        &mut m,
        &unfill(&Value::Array(requests), &world.map),
        &spec["requests"],
        format!("{name}: журнал запросов"),
    );
    m
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conditional_requests_and_cache_match_0_4_7() {
    run_scenario(scenario(&FIXTURE["channel"], "conditional"))
        .await
        .finish("conditional");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_modified_answers_match_0_4_7() {
    run_scenario(scenario(&FIXTURE["channel"], "not-modified"))
        .await
        .finish("not-modified");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_and_failures_match_0_4_7() {
    run_scenario(scenario(&FIXTURE["channel"], "retries"))
        .await
        .finish("retries");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qa_override_and_disabled_channel_match_0_4_7() {
    run_scenario(scenario(&FIXTURE["channel"], "config"))
        .await
        .finish("config");
}

#[test]
fn every_channel_scenario_is_checked() {
    assert_eq!(
        names(&FIXTURE["channel"]),
        ["conditional", "not-modified", "retries", "config"]
    );
}
