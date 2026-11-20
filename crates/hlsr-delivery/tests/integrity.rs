//! Проверка файлов против 0.4.7: фикстура `integrity.json` из
//! `tools/reference/delivery/integrity.js`, и самотест `user-configs-selftest.js` (проверка).
//!
//! 0.4.9 разнёс cfg по своим спискам (пропавший — `configMissing`, изменённый — `configChanged`;
//! в 0.4.7 — `missing` и `skipped`), добавил `stockMissing` и два числа в строку журнала. Отчёт
//! сворачивается обратно ([`as_0_4_7`]) для сверки с фикстурой; поля 0.4.9 проверяют самотесты
//! ниже и `tests/repair_per_file.rs`.
//!
//! `IntegrityService` на повреждённых деревьях: отчёт целиком (списки и их порядок), лента
//! событий `progress` и строк журнала, отмена между файлами, одновременная вторая проверка,
//! отказы (нет манифеста, нет папки игры, путь наружу). Изменённый cfg — не порча,
//! `preservePaths` и изменяемые пути пропускаются, лишние файлы ищутся без учёта регистра имён
//! манифеста, ссылки не обходятся. Сценарии с пометкой `unix` (ссылки) в Windows не идут.

mod support;

use hlsr_delivery::install_service::IntegrityRequest;
use hlsr_delivery::integrity_service::{IntegrityOptions, IntegrityProgress, IntegrityService};
use hlsr_delivery::ports::Logger;
use serde_json::{Value, json};
use std::future::Future as _;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, Weak};
use support::library::{Norm, Timeline, TimelineLogger, outcome, push, take};
use support::scenario::{self, common_step, path_in, temp_root};
use support::{Mismatches, fixture, integrity_log_as_0_4_7, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("integrity.json"));

/// Отчёт 0.4.9 в виде 0.4.7: `configMissing` — снова в `missing` (по порядку манифеста),
/// `configChanged` — в конец `skipped`, `stockMissing` убран.
fn as_0_4_7(report: Value, manifest: &Value) -> Value {
    let Value::Object(mut map) = report else {
        return report;
    };
    let order = |path: &Value| {
        items(&manifest["files"])
            .iter()
            .position(|file| file["path"] == *path)
    };
    let mut missing = items(&map["missing"]).clone();
    missing.extend(items(&map["configMissing"]).iter().cloned());
    missing.sort_by_key(|path| order(path));
    let mut skipped = items(&map["skipped"]).clone();
    skipped.extend(items(&map["configChanged"]).iter().cloned());
    map.insert("missing".into(), Value::Array(missing));
    map.insert("skipped".into(), Value::Array(skipped));
    for key in ["configMissing", "configChanged", "stockMissing"] {
        map.shift_remove(key);
    }
    Value::Object(map)
}

fn preserve() -> Vec<String> {
    scenario::strings(&FIXTURE["preserve"])
}

/// Сервис сценария: журнал и события — в ленту; `cancel_after` — отменить проверку, когда
/// событие `hash` дошло до этого числа файлов.
fn service(
    timeline: &Timeline,
    norm: &Arc<Norm>,
    cancel_after: &Arc<Mutex<Option<u64>>>,
) -> Arc<IntegrityService> {
    let logger: Arc<dyn Logger> = Arc::new(TimelineLogger {
        timeline: timeline.clone(),
        norm: norm.clone(),
    });
    let service = Arc::new(IntegrityService::new(IntegrityOptions {
        logger: Some(logger),
    }));
    let weak: Weak<IntegrityService> = Arc::downgrade(&service);
    let (timeline, norm, cancel_after) = (timeline.clone(), norm.clone(), cancel_after.clone());
    service.on_progress(move |event: &IntegrityProgress| {
        push(
            &timeline,
            json!({"ev": "progress", "data": norm.value(&event.to_json())}),
        );
        let limit = *cancel_after.lock().unwrap();
        if let Some(limit) = limit
            && event.phase == "hash"
            && event.checked as u64 >= limit
            && let Some(service) = weak.upgrade()
        {
            service.cancel();
        }
    });
    service
}

struct Step<'a> {
    game_id: String,
    install: std::path::PathBuf,
    manifest: &'a Value,
    preserve: Vec<String>,
}

fn step_of<'a>(root: &Path, step: &'a Value) -> Step<'a> {
    Step {
        game_id: opt_str(&step["gameId"]).unwrap_or_default().to_owned(),
        install: path_in(root, str_of(&step["install"])),
        manifest: match &step["manifest"] {
            Value::Null => &Value::Null,
            name => &FIXTURE["manifests"][str_of(name)],
        },
        preserve: if step["preserve"] == true {
            preserve()
        } else {
            Vec::new()
        },
    }
}

fn request<'a>(step: &'a Step<'_>) -> IntegrityRequest<'a> {
    IntegrityRequest {
        game_id: &step.game_id,
        install_path: &step.install,
        files_manifest: step.manifest,
        preserve_paths: &step.preserve,
        stock_configs: false,
    }
}

async fn run_scenario(spec: &Value, m: &mut Mismatches) {
    let (_temp, root) = temp_root();
    scenario::setup(&root, &spec["setup"]);
    let norm = Arc::new(Norm::new(&root, Vec::new(), false));
    let timeline: Timeline = Arc::default();
    let cancel_after: Arc<Mutex<Option<u64>>> = Arc::default();
    let service = service(&timeline, &norm, &cancel_after);
    let name = str_of(&spec["name"]);
    assert_eq!(
        items(&spec["steps"]).len(),
        items(&spec["results"]).len(),
        "шагов и результатов поровну"
    );
    for (index, (step, expected)) in items(&spec["steps"])
        .iter()
        .zip(items(&spec["results"]))
        .enumerate()
    {
        let actual = match str_of(&step["op"]) {
            "check" => {
                *cancel_after.lock().unwrap() = step["cancelAfter"].as_u64();
                let data = step_of(&root, step);
                let result = service
                    .check(request(&data))
                    .await
                    .map(|result| as_0_4_7(result.to_json(), data.manifest));
                json!({
                    "outcome": outcome(&norm, result),
                    "timeline": integrity_log_as_0_4_7(take(&timeline)),
                    "busy": service.is_busy(),
                })
            }
            "cancelIdle" => json!({"cancelled": service.cancel(), "busy": service.is_busy()}),
            "concurrent" => {
                *cancel_after.lock().unwrap() = None;
                let (first, second) = (
                    step_of(&root, &step["first"]),
                    step_of(&root, &step["second"]),
                );
                let (first_manifest, second_manifest) = (first.manifest, second.manifest);
                let mut task = Box::pin(service.check(request(&first)));
                // Один опрос: проверка заняла сервис и ждёт первого `stat`.
                let polled =
                    std::future::poll_fn(|cx| std::task::Poll::Ready(task.as_mut().poll(cx))).await;
                assert!(
                    polled.is_pending(),
                    "проверка не должна закончиться за один опрос"
                );
                let busy = service.is_busy();
                let second = service.check(request(&second)).await;
                let first = task.await;
                json!({
                    "busy": busy,
                    "second": outcome(&norm, second.map(|r| as_0_4_7(r.to_json(), second_manifest))),
                    "first": outcome(&norm, first.map(|r| as_0_4_7(r.to_json(), first_manifest))),
                    "timeline": integrity_log_as_0_4_7(take(&timeline)),
                })
            }
            _ => common_step(&root, step)
                .unwrap_or_else(|| panic!("{name}: неизвестный шаг {}", step["op"])),
        };
        m.check(
            &actual,
            expected,
            format!("{name}, шаг {index} ({})", step["op"]),
        );
    }
    scenario::open_up(&root);
}

#[tokio::test]
async fn integrity_scenarios_match_0_4_7() {
    let mut m = Mismatches::default();
    let mut ran = 0;
    for spec in items(&FIXTURE["scenarios"]) {
        if spec["unix"] == true && cfg!(not(unix)) {
            continue;
        }
        ran += 1;
        run_scenario(spec, &mut m).await;
    }
    assert!(ran > 0);
    m.finish("проверка файлов");
}

// ---------------------------------------------------------------- самотест 0.4.7

fn entry(path: &str, text: &str) -> Value {
    json!({"path": path, "size": text.len(), "sha256": support::sha256_hex(text.as_bytes())})
}

/// `user-configs-selftest.js`: «проверка файлов: изменённый cfg не повреждение, пропавший
/// восстанавливается»; в 0.4.9 — свои списки `configChanged` и `configMissing`.
#[tokio::test]
async fn edited_config_is_not_damage_but_a_missing_one_is() {
    let root = tempfile::tempdir().unwrap();
    for (rel, text) in [
        ("mod/edited.cfg", "мой бинд\n"),
        ("mod/edited-same-size.cfg", "abc\n"),
        ("mod/client.dll", "битый"),
    ] {
        let file = path_in(root.path(), rel);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let manifest = json!({"gameId": "hl1-won", "version": "1.0.0", "files": [
        entry("mod/edited.cfg", "стоковый cfg\n"),
        entry("mod/edited-same-size.cfg", "xyz\n"),
        entry("mod/missing.cfg", "есть в сборке\n"),
        entry("mod/client.dll", "целый"),
    ]});
    let service = IntegrityService::new(IntegrityOptions {
        logger: Some(Arc::new(hlsr_delivery::ports::MemoryLogger::default())),
    });
    let result = service
        .check(IntegrityRequest {
            game_id: "",
            install_path: root.path(),
            files_manifest: &manifest,
            preserve_paths: &[],
            stock_configs: false,
        })
        .await
        .unwrap();
    assert!(result.missing.is_empty());
    assert_eq!(result.config_missing, ["mod/missing.cfg"]);
    assert_eq!(
        result.config_changed,
        ["mod/edited.cfg", "mod/edited-same-size.cfg"]
    );
    assert!(result.skipped.is_empty());
    let corrupted: Vec<&str> = result.corrupted.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(corrupted, ["mod/client.dll"]);
    assert_eq!(result.damaged, 2);
    assert_eq!(result.stock_missing, 0);
}

/// `repair-per-file-selftest.js`: стоковые cfg из папок игрока сверяются только по запросу;
/// пропавший — `configMissing` и `stockMissing`, но не повреждение; изменённый — `configChanged`.
#[tokio::test]
async fn stock_configs_are_checked_only_on_request_and_are_not_damage() {
    let root = tempfile::tempdir().unwrap();
    for (rel, text) in [
        ("ghostingmod/cfg/skill_manifest.cfg", "мой skill\n"),
        ("hl2.exe", "MZ"),
    ] {
        let file = path_in(root.path(), rel);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let manifest = json!({"gameId": "ghosting-renewed", "version": "1.0.12",
    "files": [entry("hl2.exe", "MZ")],
    "stockConfigs": [
        entry("ghostingmod/cfg/chapter3.cfg", "map d1_canals_01\n"),
        entry("ghostingmod/cfg/skill_manifest.cfg", "exec skill.cfg\n"),
    ]});
    let service = IntegrityService::new(IntegrityOptions {
        logger: Some(Arc::new(hlsr_delivery::ports::MemoryLogger::default())),
    });
    for stock_configs in [false, true] {
        let result = service
            .check(IntegrityRequest {
                game_id: "",
                install_path: root.path(),
                files_manifest: &manifest,
                preserve_paths: &[],
                stock_configs,
            })
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(result.damaged, 0);
        if stock_configs {
            assert_eq!(result.total, 3);
            assert_eq!(result.config_missing, ["ghostingmod/cfg/chapter3.cfg"]);
            assert_eq!(
                result.config_changed,
                ["ghostingmod/cfg/skill_manifest.cfg"]
            );
            assert_eq!(result.stock_missing, 1);
        } else {
            assert_eq!(result.total, 1);
            assert!(result.config_missing.is_empty() && result.config_changed.is_empty());
        }
    }
}

/// Не из 0.4.7 (там обещание не бросить): брошенная проверка освобождает сервис, как `finally`.
#[tokio::test]
async fn dropped_check_releases_service() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "a").unwrap();
    let manifest = json!({"gameId": "g", "version": "1", "files": [entry("a.txt", "a")]});
    let service = IntegrityService::new(IntegrityOptions {
        logger: Some(Arc::new(hlsr_delivery::ports::MemoryLogger::default())),
    });
    {
        let mut task = Box::pin(service.check(IntegrityRequest {
            game_id: "g",
            install_path: root.path(),
            files_manifest: &manifest,
            preserve_paths: &[],
            stock_configs: false,
        }));
        let polled =
            std::future::poll_fn(|cx| std::task::Poll::Ready(task.as_mut().poll(cx))).await;
        assert!(polled.is_pending());
        assert!(service.is_busy());
    }
    assert!(!service.is_busy());
    assert!(!service.cancel());
}
