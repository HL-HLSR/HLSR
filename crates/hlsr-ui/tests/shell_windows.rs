//! Логика окон оболочки против 0.4.7: `tests/fixtures/shell-windows.json` пишет
//! `tools/reference/ui/shell-windows.js` (скрипты `UnavailableModal`, `GameNotesModal`,
//! `ChangelogModal`, `LiveSplitLaunchModal`, `LibrarySetupModal` и `changelog.js` в Node).

use hlsr_ui::controllers::shell_flows::LiveSplitGame;
use hlsr_ui::controllers::shell_windows::{
    LibrarySetup, LiveSplitDialog, Release, all_releases, build_changelog, game_notes_items,
    game_notes_list, game_notes_title, game_title, is_focused, safe_extra_release,
    structured_changelog, unavailable_view,
};
use hlsr_ui::controllers::tasks::LocalTasks;
use hlsr_ui::services::{BoxFuture, MainProcess, Reply, ServiceEvent, Services, StoreRead};
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/shell-windows.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn marker(key: &str, args: &[&str]) -> String {
    if args.is_empty() {
        format!("T({key})")
    } else {
        format!("T({key}|{})", args.join("|"))
    }
}

#[test]
fn unavailable_views_match_every_reason() {
    let fixture = fixture();
    let cases = fixture["unavailable"].as_array().unwrap();
    assert_eq!(cases.len(), 23);
    for case in cases {
        let reason = case["reason"].as_str().unwrap();
        let view = unavailable_view(reason);
        let actions: Vec<Value> = view
            .actions
            .iter()
            .map(|action| json!({"id": action.id, "icon": action.icon, "key": action.key}))
            .collect();
        assert_eq!(
            json!({"reason": reason, "icon": view.icon, "title": view.title, "text": view.text, "actions": actions}),
            *case,
            "{reason}"
        );
    }
}

#[test]
#[ignore = "заметки 0.4.9 режут пункт до 300 знаков, перенос — G3"]
fn game_notes_keep_only_the_same_version() {
    for case in fixture()["gameNotes"].as_array().unwrap() {
        let input = &case["input"];
        let version = input["version"].as_str().unwrap();
        let title = game_notes_title(marker, input["gameTitle"].as_str().unwrap(), version);
        assert_eq!(title, case["title"].as_str().unwrap());
        let items = game_notes_items(Some(&input["notes"]), version);
        assert_eq!(json!(items), case["items"], "{input}");
    }
}

fn release_json(release: &Release) -> Value {
    json!({
        "version": release.version,
        "title": release.title,
        "upcoming": true,
        "groups": release.groups.iter().map(|group| json!({"title": group.title, "items": group.items})).collect::<Vec<_>>(),
    })
}

#[test]
fn changelog_matches_build_changelog() {
    let fixture = fixture();
    let changelog = &fixture["changelog"];
    for case in changelog["filtered"].as_array().unwrap() {
        let lang = case["lang"].as_str().unwrap();
        let version = case["version"].as_str().unwrap();
        let got: Vec<Value> = structured_changelog(lang, version)
            .iter()
            .map(|release| json!({"version": release.version, "title": release.title, "current": release.current}))
            .collect();
        let want = &case["releases"];
        assert_eq!(json!(got), *want, "{lang} {version}");
    }
    for case in changelog["extras"].as_array().unwrap() {
        let raw = &case["extraRelease"];
        let safe = safe_extra_release(Some(raw));
        assert_eq!(
            safe.as_ref().map(release_json).unwrap_or(Value::Null),
            case["safe"],
            "{raw}"
        );
        let base = vec![
            Release {
                version: Some("0.4.7".into()),
                title: String::new(),
                groups: Vec::new(),
                current: false,
                upcoming: false,
            },
            Release {
                version: Some("0.4.6".into()),
                title: String::new(),
                groups: Vec::new(),
                current: false,
                upcoming: false,
            },
        ];
        let versions: Vec<Value> = all_releases(base, safe)
            .iter()
            .map(|release| json!(release.version))
            .collect();
        assert_eq!(json!(versions), case["versions"], "{raw}");
    }
    for case in changelog["games"].as_array().unwrap() {
        let games = game_notes_list(&case["reply"]);
        let got: Vec<Value> = games
            .iter()
            .map(|game| {
                json!({
                    "gameId": game.game_id,
                    "launcherId": game.launcher_id,
                    "versions": game.versions.iter().map(|(version, items)| json!({"version": version, "items": items})).collect::<Vec<_>>(),
                })
            })
            .collect();
        assert_eq!(json!(got), case["gameNotes"], "{}", case["reply"]);
        let titles: Vec<String> = games.iter().map(game_title).collect();
        assert_eq!(json!(titles), case["titles"]);
    }
    for case in changelog["focus"].as_array().unwrap() {
        let initial = case["initialVersion"].as_str();
        let got: Vec<bool> = [Some("0.4.7"), Some("0.4.6"), None]
            .iter()
            .map(|version| {
                is_focused(
                    initial,
                    &Release {
                        version: version.map(str::to_owned),
                        title: String::new(),
                        groups: Vec::new(),
                        current: false,
                        upcoming: false,
                    },
                )
            })
            .collect();
        assert_eq!(json!(got), case["focused"]);
    }
}

/// `built` из `data/changelog.json` (выход `buildChangelog` эталона для его версии) — целиком.
#[test]
fn changelog_for_the_reference_version_is_built_whole() {
    let data: Value = serde_json::from_str(include_str!("../data/changelog.json")).unwrap();
    let version = data["version"].as_str().unwrap();
    for lang in ["en", "ru"] {
        let structured = structured_changelog(lang, version).len();
        let got: Vec<Value> = build_changelog(lang, version)
            .iter()
            .enumerate()
            .map(|(index, release)| {
                let mut entry = json!({
                    "version": release.version,
                    "title": release.title,
                    "groups": release.groups.iter().map(|group| json!({"title": group.title, "items": group.items})).collect::<Vec<_>>(),
                });
                if index < structured {
                    entry["current"] = json!(release.current);
                }
                entry
            })
            .collect();
        assert_eq!(json!(got), data["built"][lang], "{lang}");
    }
}

fn games(input: &Value) -> Vec<LiveSplitGame> {
    input["games"]
        .as_array()
        .unwrap()
        .iter()
        .map(|game| LiveSplitGame {
            id: game["id"].as_str().unwrap().into(),
            game_id: game["gameId"].as_str().map(str::to_owned),
            label: game["label"].as_str().unwrap().into(),
            presets: game["presets"].as_array().unwrap().clone(),
        })
        .collect()
}

#[test]
fn livesplit_dialog_picks_games_and_remembers_splits() {
    for case in fixture()["liveSplit"].as_array().unwrap() {
        let input = &case["input"];
        let chosen: Map<String, Value> = input["chosen"].as_object().unwrap().clone();
        let mut dialog = LiveSplitDialog::new(games(input), input["defaultGame"].as_str(), chosen);
        let snapshot = |dialog: &LiveSplitDialog| {
            json!({
                "gameId": dialog.game_id,
                "presetId": dialog.preset_id,
                "presets": dialog.presets().iter().map(|preset| preset["id"].clone()).collect::<Vec<_>>(),
            })
        };
        let log = case["log"].as_array().unwrap();
        let mut first = snapshot(&dialog);
        first["text"] = log[0]["text"].clone();
        assert_eq!(first, log[0]);
        let mut emitted = Vec::new();
        for (step, want) in input["steps"].as_array().unwrap().iter().zip(&log[1..]) {
            if let Some(id) = step["pick"].as_str() {
                dialog.pick_game(id);
            }
            if let Some(id) = step["preset"].as_str() {
                dialog.preset_id = Some(id.into());
            }
            if step["accept"] == true
                && let Some(pick) = dialog.accept()
            {
                emitted.push(json!(["resolve", {"game": pick.game, "gameId": pick.game_id, "preset": pick.preset}]));
            }
            if step["cancel"] == true {
                emitted.push(json!(["resolve", false]));
            }
            assert_eq!(snapshot(&dialog), *want, "{step}");
        }
        assert_eq!(json!(emitted), case["emitted"]);
        let presets: Vec<Value> = input["games"][2]["presets"]
            .as_array()
            .unwrap()
            .iter()
            .chain(input["games"][0]["presets"].as_array().unwrap())
            .cloned()
            .collect();
        let labels: Vec<Value> = presets
            .iter()
            .map(|preset| {
                let (title, small) = LiveSplitDialog::preset_label(preset, marker);
                json!({"title": title, "small": small})
            })
            .collect();
        assert_eq!(json!(labels), case["labels"]);
    }
}

/// Каналы на списках ответов: вызовы — в журнал.
struct Replies {
    lists: HashMap<String, Vec<Value>>,
    confirm: Value,
    calls: Mutex<Vec<Value>>,
    counts: Mutex<HashMap<String, usize>>,
    events: broadcast::Sender<ServiceEvent>,
}

impl MainProcess for Replies {
    fn invoke(&self, channel: &str, args: Vec<Value>) -> BoxFuture<'static, Reply> {
        self.calls.lock().unwrap().push(json!([channel, args]));
        let reply = if channel == "set-library-path" {
            self.confirm.clone()
        } else {
            let list = self
                .lists
                .get(channel)
                .cloned()
                .unwrap_or(vec![Value::Null]);
            let mut counts = self.counts.lock().unwrap();
            let count = counts.entry(channel.to_owned()).or_default();
            let reply = list[(*count).min(list.len() - 1)].clone();
            *count += 1;
            reply
        };
        Box::pin(std::future::ready(Ok(reply)))
    }
    fn send(&self, _: &str, _: Vec<Value>) {}
    fn send_sync(&self, _: &str, _: Vec<Value>) -> Value {
        Value::Null
    }
    fn store_read(&self, _: &str) -> StoreRead {
        StoreRead::Missing
    }
    fn store_write(&self, _: &str, _: Value) -> bool {
        true
    }
    fn store_quarantine(&self, _: &str) -> Value {
        Value::Null
    }
    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
        self.events.subscribe()
    }
}

#[test]
fn library_setup_validates_chooses_and_confirms() {
    for case in fixture()["librarySetup"].as_array().unwrap() {
        let input = &case["input"];
        let lists: HashMap<String, Vec<Value>> = input["replies"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), value.as_array().unwrap().clone()))
            .collect();
        let replies = Arc::new(Replies {
            lists,
            confirm: input["confirm"].clone(),
            calls: Mutex::default(),
            counts: Mutex::default(),
            events: broadcast::channel(4).0,
        });
        let services: Services = replies.clone();
        let state = Rc::new(RefCell::new(LibrarySetup::new("C:\\HLSR")));
        let emitted = Rc::new(RefCell::new(Vec::new()));
        let tasks = LocalTasks::new();
        let mut states = Vec::new();
        for step in input["steps"].as_array().unwrap() {
            let step = step.as_str().unwrap().to_owned();
            let (services, shared, emitted) = (services.clone(), state.clone(), emitted.clone());
            let name = step.clone();
            tasks.spawn(async move {
                let state = shared;
                match name.as_str() {
                    "mount" => {
                        let path = state.borrow().path.clone();
                        LibrarySetup::validate(&services, &state, &path).await;
                    }
                    "choose" => LibrarySetup::choose(&services, &state).await,
                    "confirm" => {
                        if let Some(path) = LibrarySetup::confirm(&services, &state).await {
                            emitted.borrow_mut().push(json!(["confirmed", path]));
                        }
                    }
                    other => panic!("шаг {other}"),
                }
            });
            tasks.run_until_stalled();
            let s = state.borrow();
            states.push(json!({
                "step": step,
                "path": s.path,
                "freeSpaceGb": s.free_space_gb(),
                "hasCyrillic": s.has_cyrillic,
                "error": s.error.map(|key| marker(key, &[])),
            }));
        }
        assert_eq!(json!(states), case["states"], "{}", input["name"]);
        assert_eq!(json!(*replies.calls.lock().unwrap()), case["calls"]);
        assert_eq!(json!(*emitted.borrow()), case["emitted"]);
    }
}

#[test]
fn library_setup_busy_folder_0_4_9() {
    // 0.4.9: `set-library-path` отказал с `busy` — «папка занята» (у проверки пути — как прежде).
    let replies = Arc::new(Replies {
        lists: HashMap::new(),
        confirm: json!({"ok": false, "error": "busy"}),
        calls: Mutex::default(),
        counts: Mutex::default(),
        events: broadcast::channel(4).0,
    });
    let services: Services = replies.clone();
    let state = Rc::new(RefCell::new(LibrarySetup::new("C:\\HLSR")));
    let tasks = LocalTasks::new();
    let (shared, confirmed) = (state.clone(), Rc::new(RefCell::new(None)));
    let out = confirmed.clone();
    tasks.spawn(async move {
        *out.borrow_mut() = Some(LibrarySetup::confirm(&services, &shared).await);
    });
    tasks.run_until_stalled();
    assert_eq!(*confirmed.borrow(), Some(None));
    assert_eq!(state.borrow().error, Some("#L_S_PATH_BUSY"));
}
