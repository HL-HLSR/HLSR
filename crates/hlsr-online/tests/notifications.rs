//! Уведомления против 0.4.7: фикстура `notifications.json` из
//! `tools/reference/online/notifications.js` (`notifications/{catalog,NotificationStore,
//! version-events}.js` в Node 24) — уровни каталога, сценарии хранилища и событий версий: итоги,
//! журнал, сводки `onChange`, байты файлов.

mod support;

use hlsr_core::notifications::catalog::level_for_code;
use hlsr_core::notifications::{NotificationStore, Summary};
use hlsr_delivery::ports::MemoryLogger;
use hlsr_online::notifications::version_events::{VersionEvents, VersionOptions};
use hlsr_online::telemetry::launcher_update_events::AnnouncedVersions;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use support::{FakeClock, Mismatches, files, fixture, items, log_lines, setup, str_of};

#[test]
fn catalog_matches_node() {
    let fixture = fixture("notifications.json");
    for case in items(&fixture["catalog"]) {
        assert_eq!(
            json!(level_for_code(str_of(&case[0]))),
            case[1],
            "levelForCode({})",
            case[0]
        );
    }
}

struct Run {
    root: std::path::PathBuf,
    clock: FakeClock,
    store: Arc<NotificationStore>,
    versions: VersionEvents,
}

fn summary_json(summary: Summary) -> Value {
    json!({"unread": summary.unread, "total": summary.total})
}

fn opt_str(value: &Value) -> Option<&str> {
    value.as_str()
}

fn version_options(value: &Value) -> VersionOptions {
    VersionOptions {
        has_release_notes: value["hasReleaseNotes"] == json!(true),
        launcher_id: value["launcherId"].as_str().map(str::to_owned),
    }
}

fn published(result: Result<Option<Value>, String>) -> Value {
    result.unwrap().unwrap_or(Value::Null)
}

impl Run {
    fn new(root: &Path, now: i64, logger: Arc<MemoryLogger>) -> Run {
        let clock = FakeClock::new(now);
        let store = Arc::new(NotificationStore::with(
            root.join("notifications.json"),
            logger.clone(),
            clock.clock(),
        ));
        let versions = VersionEvents::new(
            root.join("version-state.json"),
            store.clone(),
            logger,
            clock.clock(),
        );
        Run {
            root: root.to_owned(),
            clock,
            store,
            versions,
        }
    }

    fn store_step(&self, step: &Value) -> Option<Value> {
        let store = &self.store;
        let item_id = |index: Option<usize>| {
            let items = store.read_all();
            let item = match index {
                Some(index) => &items[index],
                None => items.last().unwrap(),
            };
            str_of(&item["id"]).to_owned()
        };
        Some(match str_of(&step["op"]) {
            "add" => {
                let meta = step.get("meta").filter(|meta| !meta.is_null());
                store
                    .add(
                        str_of(&step["level"]),
                        str_of(&step["code"]),
                        step["detail"].as_str(),
                        meta,
                    )
                    .unwrap()
                    .unwrap_or(Value::Null)
            }
            "advance" => {
                self.clock.advance(step["ms"].as_i64().unwrap());
                Value::Null
            }
            "list" => {
                let (items, unread) = store.list();
                json!({"items": items, "unread": unread})
            }
            "listIds" => Value::Array(store.list().0.iter().map(|i| i["id"].clone()).collect()),
            "summarize" => summary_json(store.summarize()),
            "markRead" => json!(store.mark_read(str_of(&step["id"])).unwrap()),
            "markReadLast" => json!(store.mark_read(&item_id(None)).unwrap()),
            "markReadIndex" => {
                let index = step["index"].as_u64().unwrap() as usize;
                json!(store.mark_read(&item_id(Some(index))).unwrap())
            }
            "markAllRead" => json!(store.mark_all_read().unwrap()),
            "removeWhere" => {
                let code = str_of(&step["code"]);
                json!(
                    store
                        .remove_where(|item| item["code"].as_str() == Some(code))
                        .unwrap()
                )
            }
            "clear" => json!(store.clear().unwrap()),
            "files" => files(&self.root),
            _ => return None,
        })
    }

    fn version_step(&self, step: &Value) -> Value {
        let versions = &self.versions;
        let args = &step["args"];
        let arg = |i: usize| opt_str(&args[i]);
        match str_of(&step["op"]) {
            "launcherUpdateAvailable" => published(versions.launcher_update_available(
                arg(0),
                arg(1),
                args[2]["hasReleaseNotes"] == json!(true),
            )),
            "launcherUpdated" => published(versions.launcher_updated(arg(0), arg(1))),
            "gameUpdateAvailable" => published(versions.game_update_available(
                arg(0),
                arg(1),
                arg(2),
                &version_options(&args[3]),
            )),
            "gameUpdated" => {
                published(versions.game_updated(arg(0), arg(1), arg(2), &version_options(&args[3])))
            }
            "gameInstalled" => {
                published(versions.game_installed(arg(0), arg(1), &version_options(&args[2])))
            }
            "resolveUpdateOffers" => {
                json!(versions.resolve_update_offers(str_of(&args[0]), arg(1), arg(2)))
            }
            "writePendingReceipt" => versions
                .write_pending_receipt(arg(0), arg(1))
                .unwrap_or(Value::Null),
            "peekReceipt" => versions
                .peek_receipt()
                .map_or(Value::Null, |r| json!({"from": r.from, "to": r.to})),
            "consumeReceipt" => published(versions.consume_receipt(str_of(&args[0]))),
            "clearReceipt" => json!(versions.clear_receipt()),
            "seenKeys" => {
                let state = versions.read_state();
                Value::Array(
                    state["seen"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .map(|key| json!(key))
                        .collect(),
                )
            }
            "alreadyAnnounced" => json!(
                versions
                    .already_announced(str_of(&args[0]), str_of(&args[1]))
                    .unwrap()
            ),
            "markAnnounced" => {
                versions
                    .mark_announced(str_of(&args[0]), str_of(&args[1]))
                    .unwrap();
                Value::Null
            }
            "write" => {
                setup(&self.root, &json!([step]));
                Value::Null
            }
            other => panic!("неизвестный шаг {other}"),
        }
    }
}

fn run_scenarios(section: &str) {
    let fixture = fixture("notifications.json");
    let now = fixture["now"].as_i64().unwrap();
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture[section]) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        setup(root, &scenario["setup"]);
        let logger = Arc::new(MemoryLogger::default());
        let run = Run::new(root, now, logger.clone());
        let changes: Arc<Mutex<Vec<Value>>> = Arc::default();
        let sink = changes.clone();
        run.store.set_on_change(Some(Box::new(move |summary| {
            sink.lock().unwrap().push(summary_json(summary));
        })));
        let name = str_of(&scenario["name"]);
        for (step, expected) in items(&scenario["steps"])
            .iter()
            .zip(items(&scenario["results"]))
        {
            let result = run
                .store_step(step)
                .unwrap_or_else(|| run.version_step(step));
            let actual = json!({
                "result": result,
                "logs": log_lines(&logger),
                "changes": std::mem::take(&mut *changes.lock().unwrap()),
            });
            mismatches.check(&actual, expected, format!("{name}: {step}"));
        }
    }
    mismatches.finish(section);
}

#[test]
fn store_matches_node() {
    run_scenarios("store");
}

#[test]
fn version_events_match_node() {
    run_scenarios("versions");
}
