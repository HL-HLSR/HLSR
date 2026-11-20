//! Депо Steam против 0.4.7: `tools/reference/platform/steam-depot.js` → `steam-depot.json`.
//! Чистые части (`assertDepot`, `catalogFor`, хвост консоли, `mergeInto`, `existsInsensitive`) идут
//! на любой ОС. Службу и установщик над настоящими папками фикстура снята на Windows (junction
//! через `cmd.exe /c mklink /J`, пути с `\`), поэтому их сценарии — `#[cfg(windows)]`; ветка Linux
//! (симлинк слота) проверяется отдельными тестами `#[cfg(unix)]`.

#[path = "support/depot_world.rs"]
mod depot_world;
mod support;

use depot_world::scrub_root;
use hlsr_core::js;
use hlsr_core::json_text::json_parse;
use hlsr_platform::depot_install::{REQUIRED_FILES, exists_insensitive, merge_into};
use hlsr_platform::steam_depot::{
    CATALOG, ConsoleTail, MAX_CONTENT_SLOT, assert_depot, catalog_for, responses,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use support::{Mismatches, fixture, items, str_of};

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Снимок дерева, как у скрипта: папки `depot_<число>` — числом файлов, ссылки — с целью, файлы —
/// размер и sha256 после замены корня.
fn describe(root: &Path, scrub: &dyn Fn(&str) -> String) -> Value {
    fn count_files(dir: &Path) -> u64 {
        let mut total = 0;
        for item in std::fs::read_dir(dir).unwrap().flatten() {
            let kind = item.file_type().unwrap();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                total += count_files(&item.path());
            } else {
                total += 1;
            }
        }
        total
    }
    fn walk(dir: &Path, prefix: &str, scrub: &dyn Fn(&str) -> String, out: &mut Vec<Value>) {
        for item in std::fs::read_dir(dir).unwrap().flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let full = item.path();
            let kind = item.file_type().unwrap();
            if kind.is_symlink() {
                let target = std::fs::read_link(&full).unwrap();
                let text = target.to_string_lossy().into_owned();
                let text = text
                    .strip_prefix("\\\\?\\")
                    .or_else(|| text.strip_prefix("\\??\\"))
                    .unwrap_or(&text)
                    .to_owned();
                out.push(json!({"path": rel, "kind": "symlink", "target": scrub(&text)}));
            } else if kind.is_dir() {
                let is_depot = name.strip_prefix("depot_").is_some_and(|rest| {
                    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                });
                if is_depot {
                    out.push(json!({"path": rel, "kind": "dir", "files": count_files(&full)}));
                } else {
                    out.push(json!({"path": rel, "kind": "dir"}));
                    walk(&full, &rel, scrub, out);
                }
            } else {
                let text = String::from_utf8_lossy(&std::fs::read(&full).unwrap()).into_owned();
                let bytes = scrub(&text).into_bytes();
                out.push(json!({
                    "path": rel,
                    "kind": "file",
                    "size": bytes.len(),
                    "sha256": sha256_hex(&bytes),
                }));
            }
        }
    }
    let mut entries = Vec::new();
    walk(root, "", scrub, &mut entries);
    entries.sort_by(|a, b| js::cmp_utf16(a["path"].as_str().unwrap(), b["path"].as_str().unwrap()));
    Value::Array(entries)
}

/// 0.4.9: autoexec.cfg свежей установки из депо — шаблон файла игрока (`playerConfig.template`), а
/// не четыре строки `AUTOEXEC` 0.4.7: в записи 0.4.7 его размер и sha256 заменяются.
#[cfg_attr(not(windows), allow(dead_code))]
fn with_player_template(tree: &Value, game_id: &str) -> Value {
    const AUTOEXEC_047_SHA256: &str =
        "26f6a4895e382fadd0153284fc77c57e1196301033e5fa57651f707791be9124";
    let template = hlsr_delivery::player_config::template(game_id, js::Platform::CURRENT);
    let mut tree = tree.clone();
    for entry in tree.as_array_mut().into_iter().flatten() {
        if entry["sha256"] == json!(AUTOEXEC_047_SHA256) {
            entry["size"] = json!(template.len());
            entry["sha256"] = json!(sha256_hex(template.as_bytes()));
        }
    }
    tree
}

// --------------------------------------------------------------- чистые части

#[test]
fn constants_like_0_4_7() {
    let fixture = fixture("steam-depot.json");
    let constants = &fixture["constants"];
    assert_eq!(constants["MAX_CONTENT_SLOT"], json!(MAX_CONTENT_SLOT));
    assert_eq!(constants["REQUIRED_FILES"], json!(REQUIRED_FILES));
    // `AUTOEXEC` в 0.4.9 убран: autoexec.cfg — шаблон файла игрока (`with_player_template`).
    assert_eq!(
        constants["RESPONSES"]["downloadOk"],
        json!(responses::DOWNLOAD_OK)
    );
    assert_eq!(
        constants["RESPONSES"]["downloadFailed"],
        json!(responses::DOWNLOAD_FAILED)
    );
    assert_eq!(
        constants["RESPONSES"]["noLicense"],
        json!(responses::NO_LICENSE)
    );
    assert_eq!(
        constants["RESPONSES"]["purchased"],
        json!(responses::PURCHASED)
    );

    // CATALOG: сборка, лицензии и депо дословно.
    assert_eq!(CATALOG.len(), 1);
    let (id, entry) = CATALOG[0];
    let expected = &constants["CATALOG"][id];
    assert_eq!(expected["requiresAppIds"], json!(entry.requires_app_ids));
    let depots: Vec<Value> = entry
        .depots
        .iter()
        .map(|depot| {
            json!({
                "appId": depot.app_id,
                "depotId": depot.depot_id,
                "manifestId": depot.manifest_id,
                "fileFilter": depot.file_filter,
                "expectedFiles": depot.expected_files,
            })
        })
        .collect();
    assert_eq!(expected["depots"], Value::Array(depots));
}

#[test]
fn assert_depot_like_0_4_7() {
    let fixture = fixture("steam-depot.json");
    let cases = items(&fixture["assertDepot"]);
    assert!(cases.len() >= 30);
    let mut mismatches = Mismatches::default();
    for case in cases {
        let actual = match assert_depot(&case["input"]) {
            Ok(()) => json!({"result": "same", "error": null}),
            Err(error) => json!({
                "result": null,
                "error": {"code": error.code.as_str(), "message": error.message},
            }),
        };
        let expected = json!({"result": case["result"], "error": case["error"]});
        mismatches.check(
            &actual,
            &expected,
            format!("assertDepot({})", case["input"]),
        );
    }
    mismatches.finish("assertDepot");
}

#[test]
fn catalog_for_like_0_4_7() {
    let fixture = fixture("steam-depot.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["catalogFor"]) {
        let build_id = str_of(&case["buildId"]);
        let actual = match catalog_for(build_id) {
            Ok(entry) => {
                json!({"result": {"requiresAppIds": entry.requires_app_ids, "depots": entry.depots.len()}})
            }
            Err(error) => json!({"error": {
                "code": error.code.as_str(),
                "message": error.message,
                "details": error.details.clone().unwrap_or(Value::Null),
            }}),
        };
        mismatches.check(
            &actual,
            &json!({"result": case["result"], "error": case["error"]})
                .as_object()
                .map(|object| {
                    Value::Object(
                        object
                            .iter()
                            .filter(|(_, v)| !v.is_null())
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    )
                })
                .unwrap(),
            format!("catalogFor({build_id:?})"),
        );
    }
    mismatches.finish("catalogFor");
}

#[test]
fn console_tail_like_0_4_7() {
    let fixture = fixture("steam-depot.json");
    let scenarios = items(&fixture["tail"]);
    assert!(scenarios.len() >= 10);
    let mut mismatches = Mismatches::default();
    for scenario in scenarios {
        let name = str_of(&scenario["name"]);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("console_log.txt");
        if let Some(initial) = scenario["initial"].as_str() {
            std::fs::write(&file, initial).unwrap();
        }
        let text = file.to_string_lossy().into_owned();
        let mut tail: Option<ConsoleTail> = None;
        for (index, recorded) in items(&scenario["steps"]).iter().enumerate() {
            let step = &recorded["step"];
            let mut result = Value::Null;
            match str_of(&step["op"]) {
                "create" => tail = Some(ConsoleTail::new(&text)),
                "read" => result = json!(tail.as_mut().unwrap().read()),
                "write" => std::fs::write(&file, str_of(&step["text"])).unwrap(),
                "append" | "appendHex" => {
                    use std::io::Write;
                    let bytes = match step["text"].as_str() {
                        Some(text) => text.as_bytes().to_vec(),
                        None => (0..str_of(&step["hex"]).len())
                            .step_by(2)
                            .map(|at| {
                                u8::from_str_radix(&str_of(&step["hex"])[at..at + 2], 16).unwrap()
                            })
                            .collect(),
                    };
                    let mut handle = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&file)
                        .unwrap();
                    handle.write_all(&bytes).unwrap();
                }
                "remove" => {
                    let _ = std::fs::remove_file(&file);
                }
                other => panic!("неизвестный шаг {other}"),
            }
            mismatches.check(
                &result,
                &recorded["result"],
                format!("«{name}», шаг {index}"),
            );
        }
    }
    mismatches.finish("ConsoleTail");
}

#[test]
fn merge_and_exists_like_0_4_7() {
    let fixture = fixture("steam-depot.json");
    let expected = &fixture["merge"];
    let root = tempfile::tempdir().unwrap();
    let put = |base: &Path, rel: &str, text: &str| {
        let full = rel
            .split('/')
            .fold(base.to_path_buf(), |path, part| path.join(part));
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    };
    let from = root.path().join("from");
    let to = root.path().join("to");
    put(&from, "a.txt", "a");
    put(&from, "d/b.txt", "b");
    put(&from, "d/e/c.txt", "c");
    std::fs::create_dir_all(from.join("empty").join("inner")).unwrap();
    put(&to, "a.txt", "старый a");
    put(&to, "d/keep.txt", "keep");

    let first = merge_into(&from, &to).unwrap();
    let second = merge_into(&root.path().join("nowhere"), &root.path().join("never")).unwrap();
    assert_eq!(json!(first), expected["first"]);
    assert_eq!(json!(second), expected["second"]);
    assert_eq!(
        json!(!root.path().join("never").exists()),
        expected["neverCreated"]
    );
    let scrub = |text: &str| scrub_root(root.path(), text);
    let mut mismatches = Mismatches::default();
    mismatches.check(
        &describe(root.path(), &scrub),
        &expected["tree"],
        "дерево после слияния".to_owned(),
    );
    for case in items(&expected["exists"]) {
        let rel = str_of(&case["rel"]);
        mismatches.check_eq(
            exists_insensitive(&to, rel),
            case["result"].as_bool().unwrap(),
            format!("existsInsensitive({rel:?})"),
        );
    }
    mismatches.finish("mergeInto / existsInsensitive");
}

#[test]
fn the_json_fixture_is_consistent() {
    // Разбор фикстуры как JSON 0.4.7 не теряет сценарии.
    let text = std::fs::read_to_string(support::fixture_path("steam-depot.json")).unwrap();
    let value = json_parse(&text).unwrap();
    assert!(value["service"].as_array().unwrap().len() >= 35);
    assert!(value["installer"].as_array().unwrap().len() >= 20);
}

// ------------------------------------------------------- служба и установщик (Windows)

#[cfg(windows)]
mod windows_world {
    use super::*;
    use hlsr_core::logger::MemoryLogger;
    use hlsr_delivery::errors::{DeliveryError, Failure};
    use hlsr_delivery::paths::resolve_library;
    use hlsr_delivery::providers::BoxFuture;
    use hlsr_platform::depot_install::{
        DepotDownloader, DepotInstaller, InstallerOptions, RegistryWriter,
    };
    use hlsr_platform::steam_depot::{
        DepotOptions, DepotSteam, DownloadOptions, DownloadProgress, SteamDepotService,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use super::depot_world::*;

    /// Создать junction: настоящий `cmd /c mklink /J`.
    fn junction(link: &Path, target: &Path) {
        let status = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J {link:?} {target:?}");
    }

    async fn run_service(scenario: &Value) {
        let name = str_of(&scenario["name"]);
        // 0.4.9 (A-4): junction без `cmd.exe /c mklink` — отказа mklink больше нет; сбой ссылки
        // проверяет `a_junction_that_cannot_be_made_is_steam_link_failed`.
        if scenario["steam"]["mklink"] == json!("fail") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let steam_root = root.join("Steam");
        let lib = root.join("Lib");
        let elsewhere = root.join("Elsewhere");
        let content_dir = steam_root.join("steamapps").join("content");
        let console_log = steam_root.join("logs").join("console_log.txt");
        for sub in [
            steam_root.join("logs"),
            lib.join("Games"),
            lib.join(".hlsr").join("staging"),
            lib.join("Downloads"),
            elsewhere.clone(),
        ] {
            std::fs::create_dir_all(sub).unwrap();
        }
        // `consoleLog: null` — файла нет; нет ключа — пустой файл.
        let console_text = match scenario.get("consoleLog") {
            Some(Value::Null) => None,
            Some(text) => Some(str_of(text).to_owned()),
            None => Some(String::new()),
        };
        if let Some(text) = console_text {
            std::fs::write(&console_log, text).unwrap();
        }
        let slots = scenario["slots"].as_array().cloned().unwrap_or_default();
        if scenario["contentDir"] == json!(true) || !slots.is_empty() {
            std::fs::create_dir_all(&content_dir).unwrap();
        }
        let mut made_links: Vec<PathBuf> = Vec::new();
        for slot in &slots {
            let slot_name = str_of(&slot["name"]);
            let full = content_dir.join(slot_name);
            match str_of(&slot["kind"]) {
                "dir" => std::fs::create_dir(&full).unwrap(),
                "file" => std::fs::write(&full, "x").unwrap(),
                "orphan" => {
                    let target = lib.join("Downloads").join(slot_name);
                    std::fs::create_dir_all(&target).unwrap();
                    junction(&full, &target);
                    made_links.push(full);
                }
                "foreign" => {
                    let target = elsewhere.join(slot_name);
                    std::fs::create_dir_all(&target).unwrap();
                    junction(&full, &target);
                    made_links.push(full);
                }
                "dangling" => {
                    let target = lib.join("Downloads").join("gone");
                    std::fs::create_dir_all(&target).unwrap();
                    junction(&full, &target);
                    std::fs::remove_dir(&target).unwrap();
                    made_links.push(full);
                }
                other => panic!("неизвестный слот {other}"),
            }
        }

        let world = Arc::new(World {
            root: root.clone(),
            steam_root,
            lib: lib.clone(),
            elsewhere: elsewhere.clone(),
            content_dir: content_dir.clone(),
            console_log,
            eol: scenario["eol"].as_str().unwrap_or("\n").to_owned(),
            scenario: scenario.clone(),
            state: Mutex::new(WorldState {
                now: 1_000_000,
                polls: 0,
                queue: Vec::new(),
                depot_calls: HashMap::new(),
                commands: Vec::new(),
                pid: match scenario.get("pid") {
                    None => Some(100),
                    Some(Value::Null) => None,
                    Some(pid) => pid.as_u64().map(|p| p as u32),
                },
                locate_error: None,
                running_error: None,
            }),
        });
        let logger = Arc::new(MemoryLogger::default());
        let fake = Arc::new(FakeSteam(world.clone()));
        let now_world = world.clone();
        let service = SteamDepotService::new(DepotOptions {
            logger: Some(logger.clone()),
            locator: (scenario["locator"] != json!("none"))
                .then(|| Arc::new(FakeLocator(world.clone())) as Arc<dyn DepotSteam>),
            run: Some(fake.clone()),
            sleep: Some(fake.clone()),
            now: Some(Arc::new(move || now_world.state().now)),
            command_timeout_ms: scenario["commandTimeoutMs"].as_u64(),
            timeout_ms: scenario["timeoutMs"].as_u64(),
            poll_ms: scenario["pollMs"].as_u64(),
            platform: Some(js::Platform::Win32),
        });

        let staging = lib
            .join(".hlsr")
            .join("staging")
            .join("hl2-5288-depot-test");
        let mut mismatches = Mismatches::default();
        for (index, recorded) in items(&scenario["calls"]).iter().enumerate() {
            let call = &recorded["call"];
            let mut result = Value::Null;
            let mut error = Value::Null;
            let mut progress: Vec<Value> = Vec::new();
            let context = |what: &str| format!("«{name}», вызов {index} ({}): {what}", call["op"]);

            match str_of(&call["op"]) {
                "assertOwns" => {
                    let app_ids: Vec<u32> = items(&call["appIds"])
                        .iter()
                        .map(|id| id.as_u64().unwrap() as u32)
                        .collect();
                    match service.assert_owns(&app_ids).await {
                        Ok(()) => result = json!(true),
                        Err(failure) => error = error_value(&world, &Failure::Delivery(failure)),
                    }
                }
                "ownershipOf" => {
                    let timeout = call["opts"]["timeoutMs"].as_u64();
                    result = world.scrub_value(
                        &service
                            .ownership_of(&call["appId"], timeout)
                            .await
                            .to_value(),
                    );
                }
                "download" => {
                    let target = match call["targetDir"].as_str() {
                        Some("outside") => world.elsewhere.join("target"),
                        _ => staging.clone(),
                    };
                    let library_root = (call["libraryRoot"] != json!(false))
                        .then(|| world.lib.to_string_lossy().into_owned());
                    let collected = Mutex::new(Vec::<Value>::new());
                    let on_progress = |event: &DownloadProgress| {
                        collected.lock().unwrap().push(json!({
                            "depotId": event.depot_id,
                            "index": event.index,
                            "total": event.total,
                            "files": event.files,
                            "expectedFiles": event.expected_files,
                        }));
                    };
                    let outcome = service
                        .download(
                            str_of(&call["buildId"]),
                            &target.to_string_lossy(),
                            DownloadOptions {
                                library_root: library_root.as_deref(),
                                on_progress: Some(&on_progress),
                            },
                        )
                        .await;
                    progress = collected.into_inner().unwrap();
                    match outcome {
                        Ok(done) => result = world.scrub_value(&done.to_value()),
                        Err(failure) => error = error_value(&world, &failure),
                    }
                }
                "sweep" => {
                    let root = (call["libraryRoot"] != json!(false))
                        .then(|| world.lib.to_string_lossy().into_owned());
                    result = json!(
                        service
                            .sweep_content_slots(&content_dir.to_string_lossy(), root.as_deref())
                    );
                }
                "unlink" => {
                    result =
                        json!(service.unlink_content_slot(content_dir.join(str_of(&call["name"]))));
                }
                "countFiles" => {
                    let dir = match str_of(&call["dir"]) {
                        "staging" => staging.clone(),
                        other => root.join(other),
                    };
                    result = json!(service.count_files(&dir.to_string_lossy()));
                }
                "steamPid" => {
                    world.state().pid = call["pid"].as_u64().map(|p| p as u32);
                }
                "locateError" => world.state().locate_error = Some(call["error"].clone()),
                "runningError" => world.state().running_error = Some(call["error"].clone()),
                "clearErrors" => {
                    let mut state = world.state();
                    state.locate_error = None;
                    state.running_error = None;
                }
                "appendLog" => {
                    let lines: Vec<String> = items(&call["lines"])
                        .iter()
                        .map(|l| str_of(l).to_owned())
                        .collect();
                    world.append(&lines);
                }
                "removeSlot" => {
                    let _ = std::fs::remove_dir_all(content_dir.join(str_of(&call["name"])));
                }
                other => panic!("неизвестный вызов {other}"),
            }

            let commands = std::mem::take(&mut world.state().commands);
            let logs: Vec<Value> = logger
                .take()
                .into_iter()
                .map(|(level, text)| Value::String(world.scrub(&format!("{level}: {text}"))))
                .collect();
            let tree = describe(&root, &|text| world.scrub(text));
            let polls = world.state().polls;

            mismatches.check(&result, &recorded["result"], context("результат"));
            mismatches.check(&error, &recorded["error"], context("ошибка"));
            mismatches.check(
                &Value::Array(progress),
                &recorded["progress"],
                context("ход"),
            );
            // Запись 0.4.7 — без `cmd.exe /c mklink /J`: в 0.4.9 junction делает сам лаунчер.
            let recorded_commands: Vec<Value> = items(&recorded["commands"])
                .iter()
                .filter(|command| command["file"] != json!("cmd.exe"))
                .cloned()
                .collect();
            mismatches.check(
                &Value::Array(commands),
                &Value::Array(recorded_commands),
                context("команды"),
            );
            mismatches.check(&Value::Array(logs), &recorded["logs"], context("журнал"));
            mismatches.check(&tree, &recorded["tree"], context("дерево"));
            mismatches.check_eq(
                polls,
                recorded["polls"].as_u64().unwrap(),
                context("опросы"),
            );
        }

        // Ссылки снимаются, не заходя в цель.
        for link in made_links {
            let _ = std::fs::remove_dir(link);
        }
        if let Ok(entries) = std::fs::read_dir(&content_dir) {
            for item in entries.flatten() {
                if item.file_type().is_ok_and(|kind| kind.is_symlink()) {
                    let _ = std::fs::remove_dir(item.path());
                }
            }
        }
        mismatches.finish(&format!("SteamDepotService «{name}»"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn service_like_0_4_7() {
        let fixture = fixture("steam-depot.json");
        let scenarios = items(&fixture["service"]);
        assert!(scenarios.len() >= 35, "сценариев: {}", scenarios.len());
        for scenario in scenarios {
            run_service(scenario).await;
        }
    }

    // ------------------------------------------------------------------ установщик

    struct FakeDownloader {
        world: Arc<InstallWorld>,
    }

    struct InstallWorld {
        root: PathBuf,
        scenario: Value,
        calls: Mutex<Vec<Value>>,
        received: Mutex<Vec<Value>>,
        registry: Mutex<Vec<Value>>,
    }

    impl InstallWorld {
        fn scrub(&self, text: &str) -> String {
            scrub_root(&self.root, text)
        }
    }

    fn put(base: &Path, files: &Value) {
        for (rel, spec) in files.as_object().into_iter().flatten() {
            let full = rel
                .split('/')
                .fold(base.to_path_buf(), |path, part| path.join(part));
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, str_of(&spec["text"])).unwrap();
        }
    }

    impl DepotDownloader for FakeDownloader {
        fn download<'a>(
            &'a self,
            build_id: &'a str,
            target_dir: &'a str,
            library_root: &'a str,
            on_progress: Option<hlsr_platform::steam_depot::ProgressFn<'a>>,
        ) -> BoxFuture<'a, Result<hlsr_platform::steam_depot::Downloaded, Failure>> {
            Box::pin(async move {
                let world = &self.world;
                world.calls.lock().unwrap().push(json!({
                    "gameId": build_id,
                    "staging": world.scrub(target_dir),
                    "libraryRoot": world.scrub(library_root),
                    "hasProgress": on_progress.is_some(),
                }));
                if let Some(on_progress) = on_progress {
                    for event in world.scenario["progress"].as_array().into_iter().flatten() {
                        on_progress(&DownloadProgress {
                            depot_id: event["depotId"].as_u64().unwrap() as u32,
                            index: event["index"].as_u64().unwrap() as usize,
                            total: event["total"].as_u64().unwrap() as usize,
                            files: event["files"].as_u64().unwrap(),
                            expected_files: event["expectedFiles"].as_u64(),
                        });
                    }
                }
                let mut depots = Vec::new();
                for depot in world.scenario["depots"].as_array().into_iter().flatten() {
                    let id = depot["id"].as_u64().unwrap() as u32;
                    let dir = Path::new(target_dir).join(format!("depot_{id}"));
                    std::fs::create_dir_all(&dir).unwrap();
                    put(&dir, &depot["files"]);
                    for rel in depot["dirs"].as_array().into_iter().flatten() {
                        let nested = str_of(rel)
                            .split('/')
                            .fold(dir.clone(), |path, part| path.join(part));
                        std::fs::create_dir_all(nested).unwrap();
                    }
                    depots.push((id, dir.to_string_lossy().into_owned()));
                }
                if let Some(error) = world.scenario.get("downloadError") {
                    let code = [
                        hlsr_delivery::errors::Code::SteamDepotFailed,
                        hlsr_delivery::errors::Code::SteamTimeout,
                    ]
                    .into_iter()
                    .find(|code| code.as_str() == str_of(&error["code"]))
                    .unwrap();
                    return Err(DeliveryError::new(code, str_of(&error["message"])).into());
                }
                Ok(hlsr_platform::steam_depot::Downloaded {
                    build_id: build_id.to_owned(),
                    target_dir: target_dir.to_owned(),
                    depots,
                })
            })
        }
    }

    struct FakeRegistry {
        world: Arc<InstallWorld>,
        throws: bool,
    }

    impl RegistryWriter for FakeRegistry {
        fn set_dword(&self, key_path: &str, value_name: &str, value: u32) -> Result<(), String> {
            self.world
                .registry
                .lock()
                .unwrap()
                .push(json!([key_path, value_name, value]));
            if self.throws {
                Err("реестр недоступен".to_owned())
            } else {
                Ok(())
            }
        }
    }

    const T0: i64 = 1_772_359_200_000; // 2026-03-01T10:00:00.000Z
    const INSTALLED_AT: &str = "2026-03-01T10:00:00.000Z";

    async fn run_installer(scenario: &Value) {
        let name = str_of(&scenario["name"]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let lib = root.join("Lib");
        std::fs::create_dir_all(lib.join("Games")).unwrap();
        std::fs::create_dir_all(lib.join(".hlsr").join("staging")).unwrap();
        let paths = resolve_library(&lib.to_string_lossy()).unwrap();
        let game_id = str_of(&scenario["gameId"]);
        let install_path = lib.join("Games").join(game_id);

        if let Some(existing) = scenario.get("existing") {
            std::fs::create_dir_all(&install_path).unwrap();
            put(&install_path, existing);
        }
        if scenario["existingDir"] == json!(true) {
            std::fs::create_dir_all(&install_path).unwrap();
        }
        if let Some(backup) = scenario.get("backup") {
            let backup_dir = lib.join("Games").join(format!("{game_id}.hlsr-backup"));
            std::fs::create_dir_all(&backup_dir).unwrap();
            put(&backup_dir, backup);
        }

        let world = Arc::new(InstallWorld {
            root: root.clone(),
            scenario: scenario.clone(),
            calls: Mutex::default(),
            received: Mutex::default(),
            registry: Mutex::default(),
        });
        let logger = Arc::new(MemoryLogger::default());
        let registry: Option<Arc<dyn RegistryWriter>> = match scenario["registry"].as_str() {
            Some("ok") => Some(Arc::new(FakeRegistry {
                world: world.clone(),
                throws: false,
            })),
            Some("throws") => Some(Arc::new(FakeRegistry {
                world: world.clone(),
                throws: true,
            })),
            _ => None,
        };
        let installer = DepotInstaller::new(InstallerOptions {
            paths,
            depot_service: Arc::new(FakeDownloader {
                world: world.clone(),
            }),
            logger: Some(logger.clone()),
            set_registry_value: registry,
            now: Some(Arc::new(|| INSTALLED_AT.to_owned())),
            clock: Some(Arc::new(|| T0)),
        });

        let received = world.clone();
        let outcome = installer
            .install(
                game_id,
                scenario["version"].as_str(),
                Some(Arc::new(
                    move |progress: &hlsr_delivery::ports::DepotProgress| {
                        received.received.lock().unwrap().push(json!({
                            "files": progress.files,
                            "expectedFiles": progress.expected_files,
                            "index": progress.index,
                            "total": progress.total,
                        }));
                    },
                )),
            )
            .await;

        let scrub = |text: &str| world.scrub(text);
        let (result, error) = match &outcome {
            Ok(installed) => (hlsr_json_scrub(installed, &scrub), Value::Null),
            Err(Failure::Delivery(error)) => (
                Value::Null,
                json!({
                    "code": error.code.as_str(),
                    "message": scrub(&error.message),
                    "details": error.details.as_ref().map_or(Value::Null, |d| hlsr_json_scrub(d, &scrub)),
                }),
            ),
            Err(Failure::Raw(error)) => (Value::Null, json!({"raw": true, "code": error.code})),
        };
        let expected = &scenario["outcome"];
        let mut mismatches = Mismatches::default();
        let context = |what: &str| format!("установщик «{name}»: {what}");
        mismatches.check(&result, &expected["result"], context("результат"));
        mismatches.check(&error, &expected["error"], context("ошибка"));
        let logs: Vec<Value> = logger
            .take()
            .into_iter()
            .map(|(level, text)| Value::String(scrub(&format!("{level}: {text}"))))
            .collect();
        mismatches.check(&Value::Array(logs), &expected["logs"], context("журнал"));
        mismatches.check(
            &Value::Array(world.registry.lock().unwrap().clone()),
            &expected["registry"],
            context("реестр"),
        );
        mismatches.check(
            &Value::Array(world.calls.lock().unwrap().clone()),
            &expected["download"],
            context("вызов скачивания"),
        );
        // Ход скачивания доходит до окна без номера депо.
        let expected_progress: Vec<Value> = items(&expected["progress"])
            .iter()
            .map(|event| {
                json!({
                    "files": event["files"],
                    "expectedFiles": event["expectedFiles"],
                    "index": event["index"],
                    "total": event["total"],
                })
            })
            .collect();
        mismatches.check(
            &Value::Array(world.received.lock().unwrap().clone()),
            &Value::Array(expected_progress),
            context("ход"),
        );
        if scenario["skipTree"] != json!(true) {
            mismatches.check(
                &describe(&lib, &scrub),
                &with_player_template(&expected["tree"], game_id),
                context("дерево"),
            );
        }
        mismatches.finish(&format!("DepotInstaller «{name}»"));
    }

    fn hlsr_json_scrub(value: &Value, scrub: &dyn Fn(&str) -> String) -> Value {
        match value {
            Value::String(text) => Value::String(scrub(text)),
            Value::Array(list) => {
                Value::Array(list.iter().map(|v| hlsr_json_scrub(v, scrub)).collect())
            }
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), hlsr_json_scrub(v, scrub)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn installer_like_0_4_7() {
        let fixture = fixture("steam-depot.json");
        let scenarios = items(&fixture["installer"]);
        assert!(scenarios.len() >= 20, "сценариев: {}", scenarios.len());
        for scenario in scenarios {
            run_installer(scenario).await;
        }
    }
}
