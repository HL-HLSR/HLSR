//! Статистика игр против 0.4.7: сценарии `tools/reference/platform/game-stats.js` →
//! `game-stats.json` (начальный файл, шаги при поддельных часах, байты файла после каждого шага,
//! журнал, уведомления, карантин, таймер heartbeat) и несколько проверок рядом.

mod support;

use hlsr_core::js;
use hlsr_core::logger::MemoryLogger;
use hlsr_platform::game_stats::game_stats_store::{
    FILE_NAME, GameStatsStore, HEARTBEAT_MS, SCHEMA_VERSION, StatsNotifications, StatsOptions,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{Mismatches, fixture, items, str_of};

#[derive(Default)]
struct Notes {
    items: Mutex<Vec<Value>>,
    throwing: bool,
}

impl StatsNotifications for Notes {
    fn add(&self, level: &str, code: &str, detail: &str) -> Result<(), String> {
        self.items
            .lock()
            .unwrap()
            .push(json!({"level": level, "code": code, "detail": detail}));
        if self.throwing {
            Err("уведомление не принято".to_owned())
        } else {
            Ok(())
        }
    }
}

/// Текст системной ошибки зависит от ОС и временных путей — не сверяется.
fn opaque(text: String) -> String {
    const MARK: &str = "карантин не удался: ";
    match text.find(MARK) {
        Some(at) => format!("{}<opaque>", &text[..at + MARK.len()]),
        None => text,
    }
}

fn read_text(file: &Path) -> Value {
    std::fs::read(file).map_or(Value::Null, |bytes| {
        Value::String(String::from_utf8(bytes).expect("файл статистики — UTF-8"))
    })
}

/// Остальные файлы папки (имя → текст, папка — `null`).
fn files_in(dir: &Path) -> Value {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut out = serde_json::Map::new();
    for name in names {
        if name == FILE_NAME {
            continue;
        }
        let full = dir.join(&name);
        out.insert(
            name,
            if full.is_dir() {
                Value::Null
            } else {
                read_text(&full)
            },
        );
    }
    Value::Object(out)
}

struct Harness {
    file: PathBuf,
    dir: PathBuf,
    now: Arc<AtomicI64>,
    mono: Arc<AtomicI64>,
    logger: Arc<MemoryLogger>,
    notes: Arc<Notes>,
    heartbeat_ms: Option<u64>,
    use_notes: bool,
}

impl Harness {
    fn make(&self) -> Result<Arc<GameStatsStore>, hlsr_delivery::errors::RawError> {
        let now = self.now.clone();
        let mono = self.mono.clone();
        GameStatsStore::new(
            self.file.clone(),
            StatsOptions {
                logger: Some(self.logger.clone()),
                notifications: self
                    .use_notes
                    .then(|| self.notes.clone() as Arc<dyn StatsNotifications>),
                now: Some(Arc::new(move || now.load(Ordering::SeqCst))),
                monotonic: Some(Arc::new(move || mono.load(Ordering::SeqCst))),
                heartbeat_ms: self.heartbeat_ms,
                runtime: None,
            },
        )
    }

    /// Запись шага: файл, остальные файлы, журнал, уведомления.
    fn record(&self, store: Option<&Arc<GameStatsStore>>) -> serde_json::Map<String, Value> {
        let mut entry = serde_json::Map::new();
        entry.insert("file".into(), read_text(&self.file));
        entry.insert(
            "files".into(),
            if self.dir.is_dir() {
                files_in(&self.dir)
            } else {
                json!({})
            },
        );
        entry.insert(
            "logs".into(),
            Value::Array(
                self.logger
                    .take()
                    .into_iter()
                    .map(|(level, text)| Value::String(opaque(format!("{level}: {text}"))))
                    .collect(),
            ),
        );
        entry.insert(
            "notes".into(),
            Value::Array(std::mem::take(&mut *self.notes.items.lock().unwrap())),
        );
        entry.insert(
            "timer".into(),
            Value::Bool(store.is_some_and(|store| store.timer_active())),
        );
        entry.insert(
            "active".into(),
            json!(store.map(|store| store.active_games()).unwrap_or_default()),
        );
        entry
    }
}

// Часы tokio стоят: сердцебиение хранилища (40 мс) и паузы шагов `wait` идут по виртуальному
// времени, а не по скорости машины.
#[tokio::test(start_paused = true)]
async fn scenarios_like_0_4_7() {
    let fixture = fixture("game-stats.json");
    assert_eq!(str_of(&fixture["constants"]["FILE_NAME"]), FILE_NAME);
    assert_eq!(
        fixture["constants"]["SCHEMA_VERSION"],
        json!(SCHEMA_VERSION)
    );
    assert_eq!(fixture["constants"]["HEARTBEAT_MS"], json!(HEARTBEAT_MS));
    let t0 = fixture["t0"].as_i64().unwrap();

    let scenarios = items(&fixture["scenarios"]);
    assert!(scenarios.len() >= 30, "сценариев: {}", scenarios.len());
    let mut mismatches = Mismatches::default();

    for scenario in scenarios {
        let name = str_of(&scenario["name"]);
        let root = tempfile::tempdir().unwrap();
        let (dir, file) = if scenario["blocked"] == json!(true) {
            // Родитель файла — обычный файл: ни создать каталог, ни записать.
            std::fs::write(root.path().join("blocker"), "x").unwrap();
            let dir = root.path().join("blocker");
            let file = dir.join(FILE_NAME);
            (dir, file)
        } else {
            (root.path().to_path_buf(), root.path().join(FILE_NAME))
        };
        if let Some(initial) = scenario["initial"].as_str() {
            std::fs::write(&file, initial).unwrap();
        }
        let harness = Harness {
            file: file.clone(),
            dir,
            now: Arc::new(AtomicI64::new(t0)),
            mono: Arc::new(AtomicI64::new(1000)),
            logger: Arc::new(MemoryLogger::default()),
            notes: Arc::new(Notes {
                throwing: scenario["throwingNotifications"] == json!(true),
                ..Notes::default()
            }),
            heartbeat_ms: scenario["heartbeatMs"].as_u64(),
            use_notes: scenario["noNotifications"] != json!(true),
        };

        let steps = items(&scenario["steps"]);
        let mut store: Option<Arc<GameStatsStore>> = None;
        let mut created = Value::Null;
        match harness.make() {
            Ok(made) => store = Some(made),
            Err(_) => created = json!("threw"),
        }

        let context = |index: usize, what: &str| format!("«{name}», шаг {index}: {what}");
        let compare = |mismatches: &mut Mismatches,
                       index: usize,
                       expected: &Value,
                       result: Value,
                       threw: bool,
                       entry: serde_json::Map<String, Value>| {
            mismatches.check(&result, &expected["result"], context(index, "результат"));
            mismatches.check_eq(
                threw,
                expected["threw"] == json!(true),
                context(index, "бросил"),
            );
            for key in ["file", "files", "logs", "notes", "timer", "active"] {
                mismatches.check(&entry[key], &expected[key], context(index, key));
            }
        };

        // Первая запись — создание хранилища.
        let first = &steps[0];
        assert_eq!(first["step"]["op"], json!("create"));
        let entry = harness.record(store.as_ref());
        compare(&mut mismatches, 0, first, created, false, entry);
        let Some(mut current) = store else {
            continue;
        };

        for (index, recorded) in steps.iter().enumerate().skip(1) {
            let step = &recorded["step"];
            let mut result = Value::Null;
            let mut threw = false;
            match str_of(&step["op"]) {
                "sessionStart" => {
                    let pid = step["info"]["pid"].as_u64();
                    threw = current.session_start(str_of(&step["gameId"]), pid).is_err();
                }
                "sessionEnd" => match current.session_end(str_of(&step["gameId"])) {
                    Ok(Some(end)) => result = end.to_value(),
                    Ok(None) => {}
                    Err(_) => threw = true,
                },
                "heartbeat" => threw = current.heartbeat().is_err(),
                "summary" => result = current.summary(str_of(&step["gameId"])),
                "tick" => {
                    harness
                        .now
                        .fetch_add(step["ms"].as_i64().unwrap(), Ordering::SeqCst);
                    harness
                        .mono
                        .fetch_add(step["mono"].as_i64().unwrap(), Ordering::SeqCst);
                }
                "monotonic" => harness
                    .mono
                    .store(step["value"].as_i64().unwrap(), Ordering::SeqCst),
                "write" => std::fs::write(&file, str_of(&step["text"])).unwrap(),
                "remove" => {
                    let _ = std::fs::remove_file(&file);
                }
                "wait" => {
                    tokio::time::sleep(Duration::from_millis(step["ms"].as_u64().unwrap())).await
                }
                "dispose" => current.dispose(),
                "construct" => {
                    current.dispose();
                    match harness.make() {
                        Ok(made) => current = made,
                        Err(_) => threw = true,
                    }
                }
                "lockQuarantine" => {
                    let mut target = file.as_os_str().to_owned();
                    target.push(format!(
                        ".corrupt-{}",
                        js::base36(harness.now.load(Ordering::SeqCst) as u128)
                    ));
                    let target = PathBuf::from(target);
                    std::fs::create_dir(&target).unwrap();
                    std::fs::write(target.join("x"), "x").unwrap();
                }
                other => panic!("неизвестный шаг {other}"),
            }
            let entry = harness.record(Some(&current));
            compare(&mut mismatches, index, recorded, result, threw, entry);
        }
        current.dispose();
    }
    mismatches.finish("GameStatsStore");
}

// --- рядом со сценариями

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_real_notification_store_receives_the_corruption_notice() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(FILE_NAME);
    std::fs::write(&file, "{ не json").unwrap();
    let notifications = Arc::new(hlsr_core::notifications::NotificationStore::new(
        dir.path().join("notifications.json"),
    ));
    let store = GameStatsStore::new(
        file,
        StatsOptions {
            notifications: Some(notifications.clone()),
            ..StatsOptions::default()
        },
    )
    .unwrap();
    let (list, unread) = notifications.list();
    assert_eq!(unread, 1);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["code"], json!("GAME_STATS_CORRUPT"));
    assert_eq!(list[0]["level"], json!("warning"));
    store.dispose();
}

// Настоящие часы считают секунды игры, часы tokio стоят: сердцебиение (30 мс) идёт по виртуальному
// времени, а «прошло не больше секунды» не зависит от нагрузки на машину.
#[tokio::test(start_paused = true)]
async fn real_clocks_count_whole_seconds_and_the_timer_stops_with_the_last_game() {
    let dir = tempfile::tempdir().unwrap();
    let store = GameStatsStore::new(
        dir.path().join(FILE_NAME),
        StatsOptions {
            heartbeat_ms: Some(30),
            ..StatsOptions::default()
        },
    )
    .unwrap();
    let started = std::time::Instant::now();
    store.session_start("hl1-won", Some(77)).unwrap();
    assert!(store.timer_active());
    tokio::time::sleep(Duration::from_millis(120)).await;
    let summary = store.summary("hl1-won");
    assert_eq!(summary["sessionActive"], json!(true));
    assert_eq!(summary["launchCount"], json!(1));
    let end = store.session_end("hl1-won").unwrap().unwrap();
    // Целые секунды по настоящим часам: не больше настоящего времени теста, округлённого вверх
    // (под нагрузкой тест может стоять дольше секунды, и ждать «меньше секунды» нельзя).
    let real = started.elapsed().as_secs_f64();
    assert!(
        end.seconds <= real.ceil(),
        "прошло {} с при {real} с настоящего времени",
        end.seconds
    );
    assert!(!store.timer_active());
    assert_eq!(store.session_end("hl1-won").unwrap(), None);
}
