//! Поиск Steam и аккаунт против 0.4.7: миры из `steam.json`
//! (`tools/reference/platform/steam.js`) проигрываются над деревом файлов в памяти, поддельными
//! реестром и инспектором процессов. Сверяются результаты, ошибки (код, текст, подробности)
//! и журнал. Настоящие реестр и Steam не читаются.
//!
//! Самотесты 0.4.7 внутри миров: `steam-detect-selftest.js` — миры «running: …» (Steam из реестра
//! по пути и по имени образа, чужой процесс под PID, процессы без пути — не «Steam с другими
//! правами», сверка пути); `linux-selftest.js` — миры «linux: Steam is not running» (Steam не
//! запущен), «path-case: …» (двойник по регистру), «linux: account» (аккаунт и ID2).

mod support;

use base64::Engine as _;
use hlsr_core::js::Platform;
use hlsr_delivery::ports::{FakeRegistry, Logger, MemoryLogger, RegistryHive, RegistryValue};
use hlsr_platform::linux::path_case::find_case_conflict;
use hlsr_platform::linux::steam_account::SteamAccount as LinuxAccount;
use hlsr_platform::linux::steam_locator::{
    LocatorOptions as LinuxLocatorOptions, SteamLocator as LinuxLocator,
};
use hlsr_platform::steam_account::{AccountOptions, SteamAccount};
use hlsr_platform::steam_fs::{MemoryFs, SteamFs};
use hlsr_platform::steam_locator::{
    AppInfo, LocatorOptions, RunningSteam, SteamLocating, SteamLocator, steam_summary,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use support::world::{FakeInspector, Shared, error_value, info_of, world};
use support::{Mismatches, fixture, items, str_of};

/// `pngOf(length)` скрипта: подпись PNG, строка и нули.
fn png_of(length: usize) -> Vec<u8> {
    let mut bytes = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(b"fake image data ");
    bytes.extend_from_slice(&[0x00, 0xff]);
    bytes.resize(length, 0);
    bytes
}

/// Элемент дерева из описания фикстуры.
fn load(fs: &MemoryFs, path: &str, spec: &Value) {
    match spec {
        Value::String(text) => fs.add_file(path, text.as_bytes()),
        Value::Object(object) if object.contains_key("dir") => {
            fs.add_dir(path);
            if let Some(uid) = object.get("uid").and_then(Value::as_u64) {
                fs.set_uid(path, uid as u32);
            }
        }
        Value::Object(object) if object.contains_key("link") => {
            fs.add_link(path, str_of(&object["link"]));
        }
        Value::Object(object) if object.contains_key("base64") => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(str_of(&object["base64"]))
                .unwrap();
            fs.add_file(path, bytes);
        }
        Value::Object(object) if object.contains_key("png") => {
            fs.add_file(path, png_of(object["png"].as_u64().unwrap() as usize));
        }
        other => panic!("неизвестное содержимое {other}"),
    }
}

/// `summarize` скрипта: длинный `avatarDataUri` — длиной и sha256 строки.
fn summarize(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut()
        && let Some(Value::String(uri)) = object.get("avatarDataUri")
        && uri.len() > 200
    {
        let summary = json!({
            "length": uri.len(),
            "sha256": hlsr_delivery::file_ops::hex(&Sha256::digest(uri.as_bytes())),
        });
        object.insert("avatarDataUri".to_owned(), summary);
    }
    value
}

fn registry_value(entry: &Value) -> RegistryValue {
    match str_of(&entry["type"]) {
        "REG_SZ" => RegistryValue::Sz(str_of(&entry["data"]).to_owned()),
        "REG_EXPAND_SZ" => RegistryValue::ExpandSz(str_of(&entry["data"]).to_owned()),
        "REG_DWORD" => RegistryValue::Dword(entry["data"].as_u64().unwrap() as u32),
        other => panic!("неизвестный тип {other}"),
    }
}

fn set_registry(fake: &FakeRegistry, previous: &mut Vec<(String, String)>, entries: &Value) {
    for (key, name) in previous.drain(..) {
        fake.remove(RegistryHive::CurrentUser, &key, &name);
    }
    for entry in entries.as_array().into_iter().flatten() {
        let (key, name) = (str_of(&entry["key"]), str_of(&entry["name"]));
        fake.set(RegistryHive::CurrentUser, key, name, registry_value(entry));
        previous.push((key.to_owned(), name.to_owned()));
    }
}

fn running_value(running: &RunningSteam, windows: bool) -> Value {
    if windows {
        json!({
            "pid": running.pid,
            "executablePath": running.executable_path,
            "startedAt": running.started_at,
            "source": running.source,
        })
    } else {
        json!({"pid": running.pid, "source": running.source})
    }
}

fn app_value(app: &AppInfo, windows: bool) -> Value {
    if !windows {
        return app.to_value();
    }
    json!({
        "appId": app.app_id,
        "library": app.library,
        "manifestPath": app.manifest_path,
        "installdir": app.installdir,
        "appPath": app.app_path,
        "stateFlags": app.state_flags.map(hlsr_core::json_text::js_number),
        "name": app.name,
    })
}

/// Окружение мира: Windows-поиск или Linux-поиск с аккаунтом.
enum Locator {
    Windows(Arc<SteamLocator>, SteamAccount),
    Linux(Arc<LinuxLocator>, LinuxAccount),
}

fn play(world_spec: &Value) -> Vec<Value> {
    let windows = str_of(&world_spec["world"]["platform"]) == "win32";
    let spec = &world_spec["world"];
    let platform = if windows {
        Platform::Win32
    } else {
        Platform::Linux
    };

    let fs = Arc::new(MemoryFs::new(platform));
    if let Some(tree) = spec["tree"].as_object() {
        for (path, content) in tree {
            load(&fs, path, content);
        }
    }
    let logger = Arc::new(MemoryLogger::default());
    let clock = Arc::new(AtomicI64::new(
        spec["clock"].as_i64().unwrap_or(1_792_152_000_000),
    ));
    let shared: Shared = Arc::default();
    {
        let mut state = world(&shared);
        state.alive = spec["alive"]
            .as_array()
            .map(|pids| {
                pids.iter()
                    .map(|pid| pid.as_u64().unwrap() as u32)
                    .collect()
            })
            .unwrap_or_default();
        state.failure = spec["failure"].as_str().map(str::to_owned);
        state.inspect = inspect_map(&spec["inspect"]);
        state.find = find_map(&spec["find"]);
    }
    let fake_registry = Arc::new(FakeRegistry::default());
    let mut registered = Vec::new();
    set_registry(&fake_registry, &mut registered, &spec["registry"]);

    let locator = if windows {
        let clock_fn = {
            let clock = clock.clone();
            Arc::new(move || clock.load(Ordering::SeqCst)) as hlsr_delivery::timing::Clock
        };
        let locator = Arc::new(SteamLocator::new(LocatorOptions {
            fs: Some(fs.clone()),
            registry: Some(fake_registry.clone()),
            inspector: Some(Arc::new(FakeInspector(shared.clone()))),
            platform: Some(match spec["locatorPlatform"].as_str() {
                Some("linux") => Platform::Linux,
                _ => Platform::Win32,
            }),
            logger: Some(logger.clone() as Arc<dyn Logger>),
            miss_ttl_ms: spec["missTtlMs"].as_i64(),
            now: Some(clock_fn),
        }));
        let account = SteamAccount::new(
            locator.clone() as Arc<dyn SteamLocating>,
            AccountOptions {
                fs: Some(fs.clone()),
                registry: Some(fake_registry.clone()),
                logger: Some(logger.clone() as Arc<dyn Logger>),
                platform: Some(Platform::Win32),
            },
        );
        Locator::Windows(locator, account)
    } else {
        let locator = Arc::new(LinuxLocator::new(LinuxLocatorOptions {
            fs: Some(fs.clone()),
            home: spec["home"].as_str().map(str::to_owned),
            logger: Some(logger.clone() as Arc<dyn Logger>),
            own_uid: Some(1000),
        }));
        let account =
            LinuxAccount::new(locator.clone() as Arc<dyn SteamLocating>, Some(fs.clone()));
        Locator::Linux(locator, account)
    };

    let mut results = Vec::new();
    for op in items(&world_spec["world"]["ops"]) {
        let kind = str_of(&op["op"]);
        let app_id = op["appId"].as_u64().map(|id| id as u32);
        let mut ok: Option<Value> = None;
        let mut error: Option<Value> = None;

        let mut outcome = |value: Result<Value, hlsr_delivery::errors::DeliveryError>| match value {
            Ok(value) => ok = Some(value),
            Err(failure) => error = Some(error_value(&failure)),
        };

        match kind {
            "clock" => clock.store(op["ms"].as_i64().unwrap(), Ordering::SeqCst),
            "alive" => {
                world(&shared).alive = items(&op["pids"])
                    .iter()
                    .map(|pid| pid.as_u64().unwrap() as u32)
                    .collect();
            }
            "inspect" => world(&shared).inspect = inspect_map(&op["map"]),
            "find" => world(&shared).find = find_map(&op["map"]),
            "failure" => world(&shared).failure = op["text"].as_str().map(str::to_owned),
            "registry" => set_registry(&fake_registry, &mut registered, &op["entries"]),
            "set-file" => {
                let content = if op.get("png").is_some() {
                    json!({"png": op["png"]})
                } else if op.get("base64").is_some() {
                    json!({"base64": op["base64"]})
                } else {
                    op["text"].clone()
                };
                load(&fs, str_of(&op["path"]), &content);
            }
            "remove" => fs.remove(str_of(&op["path"])),
            "case-conflict" => {
                let found = find_case_conflict(str_of(&op["target"]), fs.as_ref() as &dyn SteamFs);
                ok = Some(match found {
                    Some(conflict) => json!({"folder": conflict.folder, "twin": conflict.twin}),
                    None => Value::Null,
                });
            }
            _ => match &locator {
                Locator::Windows(locator, account) => match kind {
                    "locate" => outcome(locator.locate_steam().map(|steam| steam.to_value())),
                    "running" => outcome(
                        locator
                            .locate_steam()
                            .and_then(|steam| locator.assert_steam_running(&steam))
                            .map(|running| running_value(&running, true)),
                    ),
                    "libraries" => outcome(
                        locator
                            .locate_steam()
                            .map(|steam| json!(locator.steam_libraries(&steam))),
                    ),
                    "find-app" => outcome(
                        locator
                            .locate_steam()
                            .and_then(|steam| locator.find_app(&steam, app_id.unwrap_or(0)))
                            .map(|app| app_value(&app, true)),
                    ),
                    "sdk" => outcome(
                        locator
                            .locate_steam()
                            .and_then(|steam| locator.find_app(&steam, app_id.unwrap_or(0)))
                            .and_then(|app| locator.assert_source_sdk_complete(&app))
                            .map(|()| json!(true)),
                    ),
                    "preflight" => outcome(locator.preflight(app_id).map(|result| {
                        json!({
                            "steam": steam_summary(&result.steam, &result.running),
                            "app": result.app.to_value(),
                        })
                    })),
                    "availability" => ok = Some(locator.describe_availability(app_id)),
                    "steam-only" => ok = Some(locator.describe_steam_only()),
                    "account" => ok = Some(account.describe().to_value()),
                    "refresh" => ok = Some(json!(account.refresh_connection())),
                    "active-id" => ok = Some(json!(account.active_account_id())),
                    other => panic!("неизвестная операция {other}"),
                },
                Locator::Linux(locator, account) => match kind {
                    "locate" => outcome(locator.locate_steam().map(|steam| steam.to_value())),
                    "running" => outcome(
                        locator
                            .assert_steam_running()
                            .map(|running| running_value(&running, false)),
                    ),
                    "libraries" => outcome(
                        locator
                            .locate_steam()
                            .map(|steam| json!(locator.steam_libraries(&steam))),
                    ),
                    "find-app" => outcome(
                        locator
                            .locate_steam()
                            .and_then(|steam| locator.find_app(&steam, app_id.unwrap_or(0)))
                            .map(|app| app_value(&app, false)),
                    ),
                    "preflight" => outcome(locator.preflight(app_id).map(|result| {
                        json!({
                            "steam": steam_summary(&result.steam, &result.running),
                            "app": result.app.to_value(),
                        })
                    })),
                    "availability" => ok = Some(locator.describe_availability(app_id)),
                    "steam-only" => ok = Some(locator.describe_steam_only()),
                    "account" => ok = Some(account.describe().to_value()),
                    "refresh" => ok = Some(json!(account.refresh_connection())),
                    "active-id" => ok = Some(json!(account.active_account_id())),
                    other => panic!("неизвестная операция {other}"),
                },
            },
        }

        let mut result = Map::new();
        result.insert("op".into(), json!(kind));
        if let Some(ok) = ok {
            result.insert("ok".into(), summarize(ok));
        }
        if let Some(error) = error {
            result.insert("error".into(), error);
        }
        let lines = logger.take();
        if !lines.is_empty() {
            result.insert(
                "logs".into(),
                Value::Array(
                    lines
                        .into_iter()
                        .map(|(level, message)| json!({"level": level, "message": message}))
                        .collect(),
                ),
            );
        }
        results.push(Value::Object(result));
    }
    results
}

/// `inspect`: PID → сведения о процессе или `null`.
fn inspect_map(
    map: &Value,
) -> std::collections::HashMap<u32, Option<hlsr_platform::process_inspect::ProcessInfo>> {
    map.as_object()
        .map(|map| {
            map.iter()
                .map(|(pid, value)| (pid.parse().unwrap(), info_of(value)))
                .collect()
        })
        .unwrap_or_default()
}

/// `find`: имя образа → список процессов.
fn find_map(
    map: &Value,
) -> std::collections::HashMap<String, Vec<hlsr_platform::process_inspect::ProcessInfo>> {
    map.as_object()
        .map(|map| {
            map.iter()
                .map(|(name, list)| {
                    (
                        name.clone(),
                        items(list).iter().filter_map(info_of).collect(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn steam_worlds_match_0_4_7() {
    let fixture = fixture("steam.json");
    let mut mismatches = Mismatches::default();
    let mut ops = 0;
    for scenario in items(&fixture) {
        let name = str_of(&scenario["name"]);
        let results = play(scenario);
        let expected = items(&scenario["results"]);
        mismatches.check_eq(
            results.len(),
            expected.len(),
            format!("{name}: число операций"),
        );
        for (at, (actual, expected)) in results.iter().zip(expected).enumerate() {
            mismatches.check(
                actual,
                expected,
                format!("{name}: операция {at} ({})", expected["op"]),
            );
            ops += 1;
        }
    }
    assert!(ops > 250, "сценарии проиграны целиком: {ops} операций");
    mismatches.finish("поиск Steam и аккаунт");
}
