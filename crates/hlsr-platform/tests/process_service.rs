//! Автомат состояний процесса игры против 0.4.7: сценарии из `process-service.json`
//! (`tools/reference/platform/process-service.js`) проигрываются над `GameProcessService` с
//! поддельными инспектором, остановщиком и часами. Сверяются результаты операций, события,
//! журнал и байты `.hlsr/state/<id>.process.json`.
//!
//! Самотесты 0.4.7 внутри сценариев: `tray-selftest.js` «перезапуск GoldSrc — та же игра, без
//! события finished» — «exit: GoldSrc restarts itself, then really exits»; `linux-selftest.js`
//! «игра считается закрытой, когда пропал её процесс, даже если LiveSplit держит сессию
//! Proton» — «exit: host process session».

mod support;

use hlsr_core::js::Platform;
use hlsr_delivery::ports::{Logger, MemoryLogger};
use hlsr_platform::game_process_service::{
    ChildEnd, ChildHandle, GameProcessService, ProcessEvent, ServiceOptions, StartedProcess,
    StopOptions,
};
use hlsr_platform::process_stop::StopResult;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use support::world::{FakeInspector, FakeStopper, Shared, error_value, info_of, world};
use support::{Mismatches, fixture, items};

/// Дать задачам, которые ждут выхода дочерних процессов, отработать.
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

fn files_of(dir: &std::path::Path) -> Value {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        files.insert(
            entry.file_name().to_string_lossy().into_owned(),
            Value::String(std::fs::read_to_string(entry.path()).unwrap()),
        );
    }
    Value::Object(files.into_iter().collect())
}

struct Run {
    results: Vec<Value>,
    files: Value,
}

async fn play(scenario: &Value) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let shared: Shared = Arc::default();
    let logger = Arc::new(MemoryLogger::default());
    let clock = Arc::new(AtomicI64::new(
        scenario["scenario"]["clock"]
            .as_i64()
            .unwrap_or(1_792_152_000_000),
    ));
    let clock_fn = {
        let clock = clock.clone();
        Arc::new(move || clock.load(Ordering::SeqCst)) as hlsr_delivery::timing::Clock
    };
    let service = GameProcessService::new(ServiceOptions {
        state_dir: dir.path().to_path_buf(),
        // Папки игр сценариев — `C:\HLSR\Games\<id>` (0.4.9 сверяет exe со своей папкой игры).
        games_dir: Some(r"C:\HLSR\Games".into()),
        logger: Some(logger.clone() as Arc<dyn Logger>),
        inspector: Some(Arc::new(FakeInspector(shared.clone()))),
        stopper: Some(Arc::new(FakeStopper(shared.clone()))),
        stop_timeout_ms: scenario["scenario"]["stopTimeoutMs"].as_i64(),
        // Таймер опроса не срабатывает сам: шаг опроса вызывает сценарий.
        poll_interval_ms: Some(3_600_000),
        clock: Some(clock_fn),
        platform: Some(Platform::Win32),
        launcher_pid: Some(4242),
        runtime: None,
    });
    let events: Arc<Mutex<Vec<ProcessEvent>>> = Arc::default();
    {
        let events = events.clone();
        service.subscribe(Arc::new(move |event| {
            events.lock().unwrap().push(event.clone());
        }));
    }
    let mut children: HashMap<String, tokio::sync::oneshot::Sender<ChildEnd>> = HashMap::new();

    let mut results = Vec::new();
    for op in items(&scenario["scenario"]["ops"]) {
        let kind = op["op"].as_str().unwrap();
        let game_id = op["gameId"].as_str().unwrap_or_default();
        let mut ok: Option<Value> = None;
        let mut error: Option<Value> = None;
        let mut stop_calls = None;
        let signal = op["signal"].as_str();

        match kind {
            "clock" => clock.store(op["ms"].as_i64().unwrap(), Ordering::SeqCst),
            "alive" => {
                world(&shared).alive = items(&op["pids"])
                    .iter()
                    .map(|pid| pid.as_u64().unwrap() as u32)
                    .collect();
            }
            "inspect" => {
                world(&shared).inspect = op["map"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(pid, value)| (pid.parse().unwrap(), info_of(value)))
                    .collect();
            }
            "find" => {
                world(&shared).find = op["map"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(name, list)| {
                        (
                            name.clone(),
                            items(list).iter().filter_map(info_of).collect(),
                        )
                    })
                    .collect();
            }
            "register" => {
                let info = &op["info"];
                let child = (info["child"].as_bool() == Some(true)).then(|| {
                    let (sender, receiver) = tokio::sync::oneshot::channel::<ChildEnd>();
                    children.insert(game_id.to_owned(), sender);
                    ChildHandle {
                        pid: info["pid"].as_u64().unwrap() as u32,
                        wait: Box::pin(async move {
                            receiver
                                .await
                                .unwrap_or_else(|_| ChildEnd::Error("сценарий закончился".into()))
                        }),
                    }
                });
                let started = StartedProcess {
                    pid: info["pid"].as_i64(),
                    executable_path: info["executablePath"].as_str().map(str::to_owned),
                    install_path: info["installPath"].as_str().map(str::to_owned),
                    started_at: info["startedAt"].as_str().map(str::to_owned),
                    process_key: info["processKey"].as_str().map(str::to_owned),
                    host_process: info["hostProcess"].as_bool() == Some(true),
                    child,
                    episode: info["episode"].as_str().map(str::to_owned),
                };
                match service.register_started_process(game_id, started) {
                    Ok(state) => ok = Some(state),
                    Err(failure) => error = Some(error_value(&failure)),
                }
            }
            "mark-starting" => ok = Some(service.mark_starting(game_id)),
            "cancel-starting" => ok = Some(service.cancel_starting(game_id, op["reason"].as_str())),
            "unregister" => ok = Some(service.unregister_process(game_id, op.get("lastExit"))),
            "poll" => ok = Some(json!(service.poll_now(game_id))),
            "handle-exit" => service.handle_exit(
                game_id,
                op["exitCode"].as_i64(),
                signal,
                op["error"].as_str(),
                op["detected"].as_bool() == Some(true),
            ),
            "mark-exited" => {
                ok = Some(json!(
                    service.mark_exited(game_id, op["pid"].as_u64().unwrap() as u32)
                ));
            }
            "child-exit" => {
                let sender = children.remove(game_id).unwrap();
                let _ = sender.send(ChildEnd::Exit {
                    code: op["code"].as_i64(),
                    signal: signal.map(str::to_owned),
                });
                settle().await;
            }
            "child-error" => {
                let sender = children.remove(game_id).unwrap();
                let _ = sender.send(ChildEnd::Error(op["message"].as_str().unwrap().to_owned()));
                settle().await;
            }
            "request-stop" => {
                {
                    let mut world = world(&shared);
                    world.stop_result = op.get("stopResult").map(|result| StopResult {
                        ok: result["ok"].as_bool().unwrap(),
                        code: (!result["ok"].as_bool().unwrap())
                            .then(|| result["code"].to_string()),
                        output: result["output"].as_str().unwrap().to_owned(),
                    });
                    world.kills_on_stop = op["killsOnStop"].as_bool() == Some(true);
                    world.sequence = op.get("aliveSequence").map(|values| {
                        (
                            op["pid"].as_u64().unwrap() as u32,
                            items(values).iter().map(|v| v.as_bool().unwrap()).collect(),
                        )
                    });
                }
                let outcome = service
                    .request_stop(
                        game_id,
                        StopOptions {
                            force: op["force"].as_bool() == Some(true),
                            timeout_ms: op["timeoutMs"].as_i64(),
                        },
                    )
                    .await;
                match outcome {
                    Ok(status) => ok = Some(status),
                    Err(failure) => error = Some(error_value(&failure)),
                }
                let mut world = world(&shared);
                world.sequence = None;
                stop_calls = Some(std::mem::take(&mut world.stop_calls));
            }
            "reconcile" => {
                let ids: Vec<String> = op
                    .get("gameIds")
                    .map(|ids| {
                        items(ids)
                            .iter()
                            .map(|id| id.as_str().unwrap().to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                ok = Some(Value::Array(service.reconcile_after_launcher_restart(&ids)));
            }
            "write-file" => {
                std::fs::write(
                    dir.path().join(op["name"].as_str().unwrap()),
                    op["text"].as_str().unwrap(),
                )
                .unwrap();
            }
            "query" => {
                ok = Some(json!({
                    "publicState": service.public_state(game_id),
                    "isRunning": service.is_running(game_id),
                    "anyRunning": service.any_running(),
                }));
            }
            "files" => ok = Some(files_of(dir.path())),
            "dispose" => service.dispose(),
            other => panic!("неизвестная операция {other}"),
        }

        let mut result = Map::new();
        result.insert("op".into(), json!(kind));
        if op.get("gameId").is_some() {
            result.insert("gameId".into(), json!(game_id));
        }
        if let Some(ok) = ok {
            result.insert("ok".into(), ok);
        }
        if let Some(error) = error {
            result.insert("error".into(), error);
        }
        if let Some(calls) = stop_calls {
            result.insert("stopCalls".into(), json!(calls));
        }
        let seen: Vec<ProcessEvent> = std::mem::take(&mut *events.lock().unwrap());
        if !seen.is_empty() {
            result.insert(
                "events".into(),
                Value::Array(
                    seen.into_iter()
                        .map(|event| json!({"name": event.name, "payload": event.payload}))
                        .collect(),
                ),
            );
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

    let files = files_of(dir.path());
    service.dispose();
    Run { results, files }
}

/// Запись 0.4.7 не знает поля эпизода 0.4.9 (`episode` записи, `publicState` и `finished`): без
/// эпизода оно `null` — у результата такое поле убирается, в файлах записи — его строка.
fn without_null_episode(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.get("episode") == Some(&Value::Null) {
                map.remove("episode");
            }
            map.values_mut().for_each(without_null_episode);
        }
        Value::Array(items) => items.iter_mut().for_each(without_null_episode),
        Value::String(text) => {
            *text = text
                .replace(",\n  \"episode\": null\n", "\n")
                .replace("\n  \"episode\": null,", "");
        }
        _ => {}
    }
}

#[tokio::test(flavor = "current_thread")]
async fn process_scenarios_match_0_4_7() {
    let fixture = fixture("process-service.json");
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture) {
        let name = scenario["name"].as_str().unwrap();
        let mut run = play(scenario).await;
        run.results.iter_mut().for_each(without_null_episode);
        without_null_episode(&mut run.files);
        let expected = items(&scenario["results"]);
        mismatches.check_eq(
            run.results.len(),
            expected.len(),
            format!("{name}: число операций"),
        );
        for (at, (actual, expected)) in run.results.iter().zip(expected).enumerate() {
            mismatches.check(
                actual,
                expected,
                format!("{name}: операция {at} ({})", expected["op"]),
            );
        }
        mismatches.check(
            &run.files,
            &scenario["files"],
            format!("{name}: файлы состояния"),
        );
    }
    mismatches.finish("GameProcessService");
}

/// 0.4.9: эпизод в записи (только известный), `publicState` и `finished`; `gameId` записи — из имени
/// файла, а процесс сверяется со своей папкой игры (`Games/<gameId>`), не с `installPath` записи
/// (A-6: файл в общей библиотеке может написать другой пользователь ПК).
#[tokio::test(flavor = "current_thread")]
async fn episode_and_own_game_folder_like_0_4_9() {
    use hlsr_platform::game_process_service::process_key_of;
    let exe = r"C:\HLSR\Games\ghosting-renewed\hl2.exe";
    let t0 = "2026-10-16T12:00:00.000Z";
    let record = |game_id: &str, episode: &str, install: &str| {
        let mut text = serde_json::to_string_pretty(&json!({
            "version": 1, "gameId": game_id, "state": "running", "pid": 7,
            "processKey": process_key_of(7.0, Some(exe), Some(t0)),
            "executablePath": exe, "startedAt": t0, "installPath": install,
            "episode": episode,
        }))
        .unwrap();
        text.push('\n');
        text
    };
    let scenario = json!({"scenario": {"ops": [
        {"op": "alive", "pids": [7]},
        {"op": "inspect", "map": {"7": {"pid": 7, "executablePath": exe, "startedAt": t0}}},
        {"op": "register", "gameId": "ghosting-renewed", "info": {"pid": 7, "executablePath": exe,
            "installPath": r"C:\HLSR\Games\ghosting-renewed", "startedAt": t0, "episode": "ep1"}},
        {"op": "handle-exit", "gameId": "ghosting-renewed", "exitCode": 0},
        {"op": "register", "gameId": "ghosting-renewed", "info": {"pid": 7, "executablePath": exe,
            "installPath": r"C:\HLSR\Games\ghosting-renewed", "startedAt": t0, "episode": "ep9"}},
        {"op": "dispose"},
        // Чужая запись в файле Ghosting: другой gameId, эпизод «../x», installPath вне библиотеки.
        {"op": "write-file", "name": "ghosting-renewed.process.json",
            "text": record("hl1-won", "../x", r"C:\Elsewhere")},
        {"op": "reconcile", "gameIds": ["ghosting-renewed"]},
        {"op": "query", "gameId": "ghosting-renewed"},
        // installPath записи указывает на exe Ghosting, но папка hl1-won своя — процесс не её.
        {"op": "write-file", "name": "hl1-won.process.json",
            "text": record("hl1-won", "ep1", r"C:\HLSR\Games\ghosting-renewed")},
        {"op": "reconcile", "gameIds": ["hl1-won"]},
    ]}});
    let run = play(&scenario).await;
    let r = &run.results;
    assert_eq!(r[2]["ok"]["episode"], "ep1");
    let finished = r[3]["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["name"] == "finished")
        .expect("finished");
    assert_eq!(finished["payload"]["episode"], "ep1");
    assert_eq!(
        r[4]["ok"]["episode"],
        Value::Null,
        "незнакомый эпизод не пишется"
    );
    assert_eq!(
        r[7]["ok"],
        json!([{"gameId": "ghosting-renewed", "adopted": true, "pid": 7}])
    );
    assert_eq!(r[8]["ok"]["publicState"]["gameId"], "ghosting-renewed");
    assert_eq!(r[8]["ok"]["publicState"]["episode"], Value::Null);
    assert_eq!(
        r[10]["ok"],
        json!([{"gameId": "hl1-won", "adopted": false, "reason": "stale"}])
    );
}
