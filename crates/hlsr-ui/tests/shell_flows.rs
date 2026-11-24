//! Сценарии оболочки против 0.4.7: `tests/fixtures/shell-scenarios.json` пишет
//! `tools/reference/ui/shell-scenarios.js` — настоящий скрипт `LauncherShell.vue`, выполненный в
//! Node на тех же ответах каналов и ответах окон. Здесь те же шаги идут через [`ShellFlows`]; след
//! (вызовы каналов, тосты, окна подтверждения и LiveSplit, буфер обмена, записи prefs) и
//! состояние оболочки в конце должны совпасть. Переводчик — та же метка `T(ключ|аргументы)`
//! для ключей локали, иначе сам ключ. Сюда же перенесены части `launcher-ui-selftest.js` про
//! оболочку (язык туда и обратно, проверка обновлений игры, кнопка Proton в уведомлении).

use hlsr_core::language::Language;
use hlsr_ui::controllers::shell::{Confirm, ConfirmAnswer, ConfirmFuture};
use hlsr_ui::controllers::shell_flows::{
    LiveSplitFuture, LiveSplitLaunch, LiveSplitPick, ShellChange, ShellFlows, ShellInit, ShellPort,
};
use hlsr_ui::controllers::tasks::{LocalFuture, LocalTasks};
use hlsr_ui::fixtures::services::{FixtureReply, resolve_reply};
use hlsr_ui::services::{BoxFuture, MainProcess, Reply, ServiceEvent, StoreRead};
use serde_json::{Map, Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, oneshot};

type Trace = Arc<Mutex<Vec<Value>>>;

fn push(trace: &Trace, entry: Value) {
    trace.lock().unwrap().push(entry);
}

/// Каналы на ответах сценария: вызов — в след.
struct ScriptedServices {
    replies: Map<String, Value>,
    calls: Mutex<HashMap<String, usize>>,
    trace: Trace,
    events: broadcast::Sender<ServiceEvent>,
}

impl MainProcess for ScriptedServices {
    fn invoke(&self, channel: &str, args: Vec<Value>) -> BoxFuture<'static, Reply> {
        // 0.4.9 шлёт запуску вторым аргументом `{}` (без эпизода); в сценариях 0.4.7 его нет.
        let logged = match args.as_slice() {
            [game, options] if channel == "game-process-launch" && *options == json!({}) => {
                vec![game.clone()]
            }
            _ => args.clone(),
        };
        // 0.4.9 спрашивает опрос о конфиге перед запуском; в сценариях 0.4.7 его нет.
        let setup = channel.starts_with("player-config-setup-");
        if !setup || self.replies.contains_key(channel) {
            push(&self.trace, json!(["invoke", channel, logged]));
        }
        let index = {
            let mut calls = self.calls.lock().unwrap();
            let count = calls.entry(channel.to_owned()).or_default();
            *count += 1;
            *count - 1
        };
        let reply = match self.replies.get(channel) {
            Some(spec) => resolve_reply(spec, index, &args),
            None => FixtureReply::Value(Value::Null),
        };
        match reply {
            FixtureReply::Value(value) => Box::pin(std::future::ready(Ok(value))),
            FixtureReply::Unmatched => Box::pin(std::future::ready(Ok(Value::Null))),
            FixtureReply::Reject(text) => Box::pin(std::future::ready(Err(text))),
            FixtureReply::Pending => Box::pin(std::future::pending()),
        }
    }

    fn send(&self, _channel: &str, _args: Vec<Value>) {}

    fn send_sync(&self, channel: &str, _args: Vec<Value>) -> Value {
        assert_eq!(channel, "app-version");
        json!("0.4.7")
    }

    fn store_read(&self, _name: &str) -> StoreRead {
        StoreRead::Missing
    }

    fn store_write(&self, _name: &str, _data: Value) -> bool {
        true
    }

    fn store_quarantine(&self, _name: &str) -> Value {
        Value::Null
    }

    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
        self.events.subscribe()
    }
}

/// Перевод сверки: `T(key|args)`, если ключ есть в локали, иначе сам ключ.
fn marker(key: &str, args: &[&str]) -> String {
    if hlsr_ui::tr(Language::En, key, &[]) == key {
        return key.to_owned();
    }
    if args.is_empty() {
        format!("T({key})")
    } else {
        format!("T({key}|{})", args.join("|"))
    }
}

/// Таймер `setTimeout`: срок (мс) и действие.
type Timer = (u64, Box<dyn FnOnce()>);

struct PendingConfirm {
    choices: bool,
    reply: oneshot::Sender<ConfirmAnswer>,
}

/// Окна, тосты, prefs и часы сценария.
struct Port {
    trace: Trace,
    tasks: Rc<LocalTasks>,
    confirm: RefCell<Option<PendingConfirm>>,
    livesplit: RefCell<Option<oneshot::Sender<Option<LiveSplitPick>>>>,
    now: Cell<u64>,
    timers: RefCell<Vec<Timer>>,
    hour: u32,
}

fn opt(value: &Option<String>) -> Value {
    value
        .as_ref()
        .map(|text| json!(text))
        .unwrap_or(Value::Null)
}

impl ShellPort for Port {
    fn tr(&self, key: &str, args: &[&str]) -> String {
        marker(key, args)
    }

    fn notify(&self, text: &str, icon: &str) {
        let icon = if icon.is_empty() {
            "fas fa-info-circle"
        } else {
            icon
        };
        push(&self.trace, json!(["notify", text, icon]));
    }

    fn notify_for(&self, text: &str, icon: &str, ms: u64) {
        self.notify(text, icon);
        if ms > 0 {
            push(&self.trace, json!(["ms", ms]));
        }
    }

    fn ask_confirm(&self, request: Confirm) -> ConfirmFuture {
        if let Some(previous) = self.confirm.borrow_mut().take() {
            let _ = previous.reply.send(ConfirmAnswer::Declined);
        }
        let choices: Vec<Value> = request
            .choices
            .iter()
            .map(|(id, label)| json!({"id": id, "label": label}))
            .collect();
        push(
            &self.trace,
            json!(["confirm", {
                "title": request.title,
                "text": request.text,
                "items": request.items,
                "note": opt(&request.note),
                "confirmLabel": opt(&request.confirm_label),
                "cancelLabel": opt(&request.cancel_label),
                "danger": request.danger,
                "choices": choices,
                "defaultChoice": opt(&request.default_choice),
            }]),
        );
        let (reply, future) = ConfirmFuture::pair();
        *self.confirm.borrow_mut() = Some(PendingConfirm {
            choices: !request.choices.is_empty(),
            reply,
        });
        future
    }

    fn ask_livesplit(&self, request: LiveSplitLaunch) -> LiveSplitFuture {
        if let Some(previous) = self.livesplit.borrow_mut().take() {
            let _ = previous.send(None);
        }
        let games: Vec<Value> = request
            .games
            .iter()
            .map(|game| {
                json!({
                    "id": game.id,
                    "gameId": opt(&game.game_id),
                    "label": game.label,
                    "presets": game.presets,
                })
            })
            .collect();
        push(
            &self.trace,
            json!(["livesplit", {
                "games": games,
                "defaultGame": opt(&request.default_game),
                "chosen": request.chosen,
            }]),
        );
        let (reply, future) = LiveSplitFuture::pair();
        *self.livesplit.borrow_mut() = Some(reply);
        future
    }

    fn write_clipboard(&self, text: &str) {
        push(&self.trace, json!(["clipboard", text]));
    }

    fn save_prefs(&self, changes: &Map<String, Value>) {
        push(&self.trace, json!(["prefs", changes]));
    }

    fn hour(&self) -> u32 {
        self.hour
    }

    fn spawn(&self, task: LocalFuture) {
        self.tasks.spawn(task);
    }

    fn after(&self, delay: Duration, f: Box<dyn FnOnce()>) {
        let due = self.now.get() + delay.as_millis() as u64;
        self.timers.borrow_mut().push((due, f));
    }

    fn changed(&self, _what: ShellChange) {}
}

impl Port {
    /// Прокрутить часы на `ms`, выполняя таймеры по сроку.
    fn advance(&self, ms: u64) {
        let target = self.now.get() + ms;
        loop {
            let next = {
                let timers = self.timers.borrow();
                timers
                    .iter()
                    .enumerate()
                    .filter(|(_, (due, _))| *due <= target)
                    .min_by_key(|(index, (due, _))| (*due, *index))
                    .map(|(index, _)| index)
            };
            let Some(index) = next else { break };
            let (due, f) = self.timers.borrow_mut().remove(index);
            self.now.set(self.now.get().max(due));
            f();
        }
        self.now.set(target);
    }

    /// Ответ окна (`onConfirmResolved` / `onLiveSplitLaunchResolved`).
    fn answer(&self, value: &Value) -> bool {
        if let Some(pending) = self.confirm.borrow_mut().take() {
            let answer = match value {
                Value::Bool(true) => ConfirmAnswer::Accepted,
                Value::String(id) if pending.choices => ConfirmAnswer::Choice(id.clone()),
                _ => ConfirmAnswer::Declined,
            };
            let _ = pending.reply.send(answer);
            return true;
        }
        if let Some(reply) = self.livesplit.borrow_mut().take() {
            let pick = value.as_object().map(|pick| LiveSplitPick {
                game: pick["game"].as_str().unwrap().to_owned(),
                game_id: pick["gameId"].as_str().map(str::to_owned),
                preset: pick["preset"].as_str().map(str::to_owned),
            });
            let _ = reply.send(pick);
            return true;
        }
        false
    }
}

const PREF_DEFAULTS: &str = r#"{"speedLimit":0,"animations":true,"hideWhilePlaying":true,"hideSteamId":false,"timerOffered":false,"toolSplits":{},"liveSplitGame":"","coloredBoot":true,"launcherTheme":"hlsr","railExpanded":false,"welcomeSeen":false}"#;

struct Run {
    trace: Vec<Value>,
    state: Value,
}

fn run(scenario: &Value) -> Run {
    let trace: Trace = Arc::default();
    let replies = scenario
        .get("replies")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let services = Arc::new(ScriptedServices {
        replies,
        calls: Mutex::default(),
        trace: trace.clone(),
        events: broadcast::channel(16).0,
    });
    let tasks = Rc::new(LocalTasks::new());
    let port = Rc::new(Port {
        trace: trace.clone(),
        tasks: tasks.clone(),
        confirm: RefCell::default(),
        livesplit: RefCell::default(),
        now: Cell::new(0),
        timers: RefCell::default(),
        hour: scenario.get("hour").and_then(Value::as_u64).unwrap_or(12) as u32,
    });
    let mut prefs: Map<String, Value> = serde_json::from_str(PREF_DEFAULTS).unwrap();
    if let Some(Value::Object(over)) = scenario.get("prefs") {
        for (key, value) in over {
            prefs.insert(key.clone(), value.clone());
        }
    }
    let mut library = json!({"lastLaunched": "", "libraryPath": "", "config": {}});
    if let Some(Value::Object(over)) = scenario.get("library") {
        for (key, value) in over {
            library[key] = value.clone();
        }
    }
    let language = scenario
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("en");
    let flows = ShellFlows::new(
        services.clone(),
        port.clone(),
        ShellInit {
            prefs,
            library: library.clone(),
            default_install_path: "C:\\HLSR".into(),
            language: language.into(),
        },
    );
    flows.with_state(|state| {
        if let Some(game) = scenario.get("game").and_then(Value::as_str) {
            state.selected_game_id = game.into();
        }
        if let Some(section) = scenario.get("section").and_then(Value::as_str) {
            state.section = section.into();
        }
        if let Some(Value::Object(delivery)) = scenario.get("delivery") {
            for (id, status) in delivery {
                state.delivery.insert(id.clone(), status.clone());
            }
        }
        if let Some(Value::Array(tools)) = scenario.get("tools") {
            state.tools = tools.clone();
        }
        state.download_settings_ready = scenario.get("ready") != Some(&Value::Bool(false));
        if let Some(status) = scenario.get("updateStatus") {
            state.update_status = status.clone();
        }
    });

    let mut lang_rev = 0;
    for step in scenario["run"].as_array().unwrap() {
        if step.get("mount").is_some() {
            flows.mount(library.get("lastLaunched"));
        } else if let Some(name) = step.get("call").and_then(Value::as_str) {
            let args = step.get("args").cloned().unwrap_or(json!([]));
            let text = |i: usize| args[i].as_str().unwrap_or_default().to_owned();
            match name {
                "onPlay" => flows.on_play(),
                "onStopGame" => flows.on_stop_game(),
                "onInstall" => flows.on_install(),
                "onResumeOperation" => flows.on_resume_operation(),
                "onApplyUpdate" => flows.on_apply_update(),
                "onPauseDownload" => flows.on_pause_download(),
                "onCancelDownload" => flows.on_cancel_download(args[0].as_str()),
                "onInstallEpisode" => flows.on_install_episode(),
                "onMenuAction" => flows.on_menu_action(&text(0)),
                "onSelectGame" => flows.on_select_game(&text(0)),
                "onSelectBuild" => flows.on_select_build(&text(0)),
                "onTool" => flows.on_tool(&text(0)),
                "onSelectTool" => flows.on_select_tool(&text(0)),
                "loadTools" => flows.load_tools(),
                "onToolInstall" => flows.on_tool_install(&text(0)),
                "onToolCancel" => flows.on_tool_cancel(&text(0)),
                "onToolLaunch" => flows.on_tool_launch(&text(0)),
                "onToolStop" => flows.on_tool_stop(&text(0)),
                "onToolRemove" => flows.on_tool_remove(&text(0)),
                "onToolFolder" => flows.on_tool_folder(&text(0)),
                "onToolHelp" => flows.on_tool_help(&text(0)),
                "loadDeliveryStatuses" => flows.load_delivery_statuses(),
                "onUnavailableAction" => flows.on_unavailable_action(&text(0)),
                "onWhatsNew" => flows.on_whats_new(&args[0]),
                "closeChangelog" => flows.close_changelog(),
                "onRunUpdateAction" => flows.on_run_update_action(&args[0]),
                "onOpenCrashDump" => flows.on_open_crash_dump(&text(0)),
                "onOpenDiscord" => flows.on_open_discord(),
                "onAccountAction" => flows.on_account_action(&text(0)),
                "openRunnerProfile" => flows.open_runner_profile(&text(0)),
                "checkLibrarySetup" => flows.check_library_setup(),
                "onLibraryConfirmed" => flows.on_library_confirmed(&text(0)),
                "onWelcomeLater" => flows.on_welcome_later(),
                "onWelcomeSignedIn" => flows.on_welcome_signed_in(),
                "initializeDownloadSettings" => flows.on_initialize_download_settings(),
                "confirmInstallNow" => flows.confirm_install_now(),
                "installOnQuit" => flows.install_on_quit(),
                "checkForUpdates" => flows.check_for_updates(),
                "cancelUpdateDownload" => flows.cancel_update_download(),
                // `setLanguage(id)` в приложении — окно настроек (`SettingsController`): тот же или
                // незнакомый язык — ничего; иначе язык оболочки и `syncUpdaterLanguage()`.
                "setLanguage" => {
                    let id = text(0);
                    if id != flows.language() && ["en", "ru"].contains(&id.as_str()) {
                        flows.language_changed(&id);
                        flows.sync_updater_language();
                        lang_rev += 1;
                    }
                }
                other => panic!("{}: нет метода {other}", scenario["id"]),
            }
        } else if let Some(chosen) = step.get("repair") {
            let paths = chosen.as_array().map(|paths| {
                paths
                    .iter()
                    .map(|path| path.as_str().unwrap().to_owned())
                    .collect()
            });
            flows.on_repair_resolved(paths);
        } else if let Some(choice) = step.get("configSetup") {
            flows.on_config_setup_choose(choice.as_str());
        } else if let Some(accepted) = step.get("ffmpeg").and_then(Value::as_bool) {
            flows.on_ffmpeg_gate(accepted);
        } else if step.get("selectDemo").is_some() {
            let trace = trace.clone();
            flows.on_select_demo(false, move || push(&trace, json!(["enter", "demo"])));
        } else if let Some(action) = step.get("crash").and_then(Value::as_str) {
            flows.on_crash_action(action);
        } else if let Some(answer) = step.get("answer") {
            assert!(port.answer(answer), "{}: ответ без окна", scenario["id"]);
        } else if let Some(channel) = step.get("event").and_then(Value::as_str) {
            assert!(
                flows.on_event(channel, &step["payload"]),
                "{}: событие {channel}",
                scenario["id"]
            );
        } else if let Some(ms) = step.get("wait").and_then(Value::as_u64) {
            port.advance(ms);
        } else {
            panic!("{}: шаг {step}", scenario["id"]);
        }
        tasks.run_until_stalled();
    }
    if port.confirm.borrow().is_some() {
        push(&trace, json!(["open", "confirm"]));
    }
    if port.livesplit.borrow().is_some() {
        push(&trace, json!(["open", "livesplit"]));
    }

    let s = flows.state();
    let tools: Vec<Value> = s
        .tools
        .iter()
        .map(|tool| {
            json!({
                "toolId": tool.get("toolId").cloned().unwrap_or(Value::Null),
                "state": tool.get("state").cloned().unwrap_or(Value::Null),
                "percent": tool.get("percent").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    let builds: Map<String, Value> = s
        .selected_builds
        .iter()
        .map(|(key, value)| (key.to_owned(), value.clone()))
        .collect();
    let mut delivery_ids: Vec<String> = s.delivery.keys().map(str::to_owned).collect();
    delivery_ids.sort();
    let stats: Map<String, Value> = s
        .stats
        .iter()
        .map(|(key, value)| (key.to_owned(), value.clone()))
        .collect();
    let state = json!({
        "section": s.section,
        "activeTab": s.active_tab,
        "selectedGameId": s.selected_game_id,
        "selectedToolId": s.selected_tool_id,
        "settingsOpen": s.settings_open,
        "settingsSection": s.settings_section,
        // 0.4.9: окна настроек игры нет, шестерёнка ведёт на «Персонализацию» (свой тест).
        "gameSettingsOpen": false,
        "changelogOpen": s.changelog_open,
        "changelogVersion": opt(&s.changelog_version),
        "changelogExtra": s.changelog_extra.clone().unwrap_or(Value::Null),
        "gameNotesOpen": s.game_notes_open,
        "gameNotesVersion": s.game_notes_version,
        "gameNotesData": s.game_notes_data.clone().unwrap_or(Value::Null),
        "unavailableOpen": s.unavailable_open,
        "crashReport": s.crash_report.clone().unwrap_or(Value::Null),
        "repairFiles": s.repair_files.as_deref().map_or(Value::Null, |files| Value::Array(files.to_vec())),
        "ffmpegGate": s.ffmpeg_gate.as_ref().map_or(Value::Null, |gate| json!({
            "installing": gate.installing, "percent": gate.percent, "error": opt(&gate.error),
        })),
        "librarySetupOpen": s.library_setup_open,
        "welcomeOpen": s.welcome_open,
        "profileOpen": s.profile_open,
        "updateDismissed": s.update_dismissed,
        "installPath": s.install_path,
        "selectedBuilds": builds,
        "autoUpdateStarted": s.auto_update_started.iter().collect::<Vec<_>>(),
        "downloadSettingsReady": s.download_settings_ready,
        "tools": tools,
        "liveSplitSplits": s.live_split_splits,
        "deliveryIds": delivery_ids,
        "stats": stats,
        "onlineStatus": s.online_snapshot.get("status").cloned().unwrap_or(Value::Null),
        "updateState": s.update_status.get("state").cloned().unwrap_or(Value::Null),
        "language": flows.language(),
        "langRev": lang_rev,
        "configSetupOpen": s.config_setup_open,
        "configsOpen": opt(&s.configs_open),
        "toolOpen": opt(&s.tool_open),
        "configsOpenToken": s.configs_open_token,
        "configsSection": s.configs_section,
        "configTourRequested": s.config_tour_requested,
        "configsImportRequested": s.configs_import_requested,
        "welcomeChecked": s.welcome_checked,
    });
    let trace = trace.lock().unwrap().clone();
    Run { trace, state }
}

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/shell-scenarios.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Сценарии 0.4.7 на ключах, которых в строках 0.4.9 нет. Перенос убирает их отсюда:
/// подтверждение ремонта → RepairModal (G3), `PACK_CORRUPT` → `restorePackOriginals` (C).
/// «Проверить файлы» в 0.4.9 идёт в main (`game-menu-verify-files`): проверка — свой тест (G2).
/// 0.4.9 (G3, свой тест): размер архива — `formatSize` языка; импорт с переходом к игре зовёт
/// `game-delivery-selected`; `game-process-crashed` без `requested` — молча (падение — окно).
/// 0.4.9: `mounted()` сначала зовёт `game-delivery-selected` (свой тест ниже). Шестерёнка игры —
/// «Персонализация», размер удаления — `formatSize` языка (свои тесты ниже).
const STALE: [&str; 21] = [
    "mount-first-run",
    "mount-returning",
    "menu-verify",
    "unavailable-verify-files",
    "menu-repair-mirror-large",
    "menu-repair-mirror-small",
    "menu-repair-download",
    "menu-repair-local",
    "menu-repair-no-bytes",
    "import-error",
    "export-build-pack",
    "import-pack-confirmed",
    "import-build-pending",
    "import-build-update-chain",
    "import-build-up-to-date",
    "process-crashed-code",
    "process-crashed-no-code",
    "menu-settings",
    "menu-uninstall-ok",
    "menu-uninstall-partial",
    "menu-uninstall-declined",
];

fn scenario(fixture: &Value, id: &str) -> Value {
    fixture["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap()
        .clone()
}

/// `openGameSettings()` 0.4.9: шестерёнка открывает «Персонализацию» выбранной игры на «Конфиге».
#[test]
fn game_settings_open_personalization_like_0_4_9() {
    let got = run(&scenario(&fixture(), "menu-settings"));
    assert!(got.trace.is_empty(), "{:?}", got.trace);
    assert_eq!(got.state["configsOpen"], "218");
    assert_eq!(got.state["configsOpenToken"], 1);
    assert_eq!(got.state["configsSection"], "configs");
}

/// `formatBytes` 0.4.9 в окне и тосте удаления: `formatSize` на языке окна.
#[test]
fn uninstall_size_in_window_language_like_0_4_9() {
    let fixture = fixture();
    for (id, old, new) in [
        (
            "menu-uninstall-ok",
            "3.4 GB",
            "3T(#L_DECIMAL)4\u{a0}T(#L_GB)",
        ),
        (
            "menu-uninstall-partial",
            "512 KB",
            "512T(#L_DECIMAL)0\u{a0}T(#L_KB)",
        ),
        (
            "menu-uninstall-declined",
            "700 MB",
            "700T(#L_DECIMAL)0\u{a0}T(#L_MB)",
        ),
    ] {
        let scenario = scenario(&fixture, id);
        let want: Value =
            serde_json::from_str(&scenario["trace"].to_string().replace(old, new)).unwrap();
        assert_eq!(Value::Array(run(&scenario).trace), want, "{id}");
    }
}

/// `offerTimer()`: «Да» — вкладка LiveSplit; вид узнаёт её из `tool_open`.
#[test]
fn timer_offer_accepted_opens_the_tool_tab() {
    let got = run(&scenario(&fixture(), "play-offer-timer-accepted"));
    assert_eq!(got.state["activeTab"], "tools");
    assert_eq!(got.state["toolOpen"], "livesplit");
}

/// `mounted()` 0.4.9: выбранная при запуске игра — `game-delivery-selected` первым вызовом, дальше
/// след 0.4.7.
#[test]
fn mount_selects_startup_game_first_like_0_4_9() {
    let fixture = fixture();
    for (id, start) in [("mount-first-run", "70"), ("mount-returning", "218")] {
        let scenario = fixture["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .unwrap();
        let mut want = vec![serde_json::json!([
            "invoke",
            "game-delivery-selected",
            [start]
        ])];
        want.extend(scenario["trace"].as_array().unwrap().iter().cloned());
        assert_eq!(run(scenario).trace, want, "{id}");
    }
}

#[test]
fn shell_scenarios_match_launcher_shell() {
    let fixture = fixture();
    let scenarios = fixture["scenarios"].as_array().unwrap();
    assert!(scenarios.len() > 150);
    let mut failures = Vec::new();
    for scenario in scenarios {
        let id = scenario["id"].as_str().unwrap();
        if STALE.contains(&id) {
            continue;
        }
        let got = run(scenario);
        if got.trace != *scenario["trace"].as_array().unwrap() {
            failures.push(format!(
                "{id}: след\n  ждали {}\n  вышло {}",
                serde_json::to_string(&scenario["trace"]).unwrap(),
                serde_json::to_string(&got.trace).unwrap()
            ));
        }
        let want = &scenario["state"];
        for (key, value) in want.as_object().unwrap() {
            if got.state[key] != *value {
                failures.push(format!(
                    "{id}: {key}: ждали {value}, вышло {}",
                    got.state[key]
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} расхождений:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// `launcher-ui-selftest.js`: «game update check requires an actual offer and a version before
/// reporting an update».
#[test]
fn game_update_check_requires_an_offer_and_a_version() {
    let cases = [
        json!({"update": {"installed": false, "updateAvailable": false, "availableVersion": null}, "plan": {"kind": "full"}}),
        json!({"update": {"upToDate": true, "updateAvailable": false, "availableVersion": "1.0.0"}, "plan": {"kind": "full", "reason": "downgrade"}}),
        json!({"update": {"updateAvailable": true, "availableVersion": null}, "plan": {"kind": "full"}}),
        json!({"update": {"updateAvailable": true, "availableVersion": "1.0.1"}, "plan": {"kind": "patches"}}),
    ];
    for (index, reply) in cases.iter().enumerate() {
        let scenario = json!({
            "id": format!("selftest-{index}"),
            "game": "220",
            "delivery": {"220": {"gameId": "hl2-4260", "launcherId": "220"}},
            "replies": {"game-menu-check-updates": reply},
            "run": [{"call": "onMenuAction", "args": ["updates"]}],
        });
        let notices: Vec<Value> = run(&scenario)
            .trace
            .into_iter()
            .filter(|entry| entry[0] == "notify")
            .map(|entry| entry[1].clone())
            .collect();
        let want = if index == 3 {
            "T(#L_U_G_AVAILABLE|1.0.1)"
        } else {
            "T(#L_GM_NO_UPDATES)"
        };
        assert_eq!(notices, [want], "случай {index}");
    }
}

/// 0.4.9 `offerConfigSetup`: игра без конфига перед запуском спрашивает, как его готовить.
/// «Пропустить» — игра запускается; «С нуля» и «Свой» — «Персонализация» на игре (обучение или
/// импорт), запуска нет; «Закрыть» — ни ответа, ни запуска. Конфиг уже есть — сразу запуск.
#[test]
fn config_setup_before_the_first_launch_like_049() {
    let case = |needed: bool, choice: Value| {
        let mut steps = vec![json!({"call": "onPlay"})];
        if needed {
            steps.push(json!({"configSetup": choice}));
        }
        let got = run(&json!({
            "id": "config-setup",
            "game": "70",
            "delivery": {"70": {"gameId": "hl1-won", "launcherId": "70"}},
            "replies": {
                "player-config-setup-state": {"needed": needed},
                "player-config-setup-done": {"status": "ok"},
            },
            "run": steps,
        }));
        let calls: Vec<Value> = got
            .trace
            .iter()
            .filter(|entry| entry[0] == "invoke")
            .map(|entry| json!([entry[1], entry[2]]))
            .collect();
        (calls, got.state)
    };
    let ask = json!(["player-config-setup-state", ["hl1-won"]]);
    let launch = json!(["game-process-launch", ["hl1-won"]]);
    let done = |choice: &str| json!(["player-config-setup-done", ["hl1-won", choice]]);

    let (calls, state) = case(false, Value::Null);
    assert_eq!(calls, [ask.clone(), launch.clone()]);
    assert_eq!(state["configSetupOpen"], false);

    let (calls, state) = case(true, json!("skip"));
    assert_eq!(calls, [ask.clone(), done("skip"), launch]);
    assert_eq!(state["configsOpen"], Value::Null);

    let (calls, state) = case(true, json!("scratch"));
    assert_eq!(calls, [ask.clone(), done("scratch")]);
    assert_eq!(state["configSetupOpen"], false);
    assert_eq!(state["configsOpen"], "70", "игра оболочки, не доставки");
    assert_eq!(state["configsOpenToken"], 1);
    assert_eq!(state["configsSection"], "configs");
    assert_eq!(state["configTourRequested"], true);
    assert_eq!(state["configsImportRequested"], false);

    let (calls, state) = case(true, json!("own"));
    assert_eq!(calls, [ask.clone(), done("own")]);
    assert_eq!(state["configsImportRequested"], true);
    assert_eq!(state["configTourRequested"], false);

    let (calls, state) = case(true, Value::Null);
    assert_eq!(calls, [ask]);
    assert_eq!(state["configsOpen"], Value::Null);
    assert_eq!(state["configsOpenToken"], 0);
}

/// 0.4.9 `welcomeChecked`: обучение ждёт проверку библиотеки и приветствие; окно выбора папки
/// его держит, сбой проверки — нет.
#[test]
fn welcome_checked_waits_for_the_library_and_the_welcome() {
    let checked = |library: Value, prefs: Value| {
        run(&json!({
            "id": "welcome-checked",
            "prefs": prefs,
            "replies": {"get-library-path": library, "runner-state": {"account": null}},
            "run": [{"call": "checkLibrarySetup"}],
        }))
        .state["welcomeChecked"]
            .clone()
    };
    let confirmed = json!({"path": "C:\\HLSR", "confirmed": true});
    assert_eq!(checked(confirmed.clone(), json!({})), true);
    assert_eq!(checked(confirmed, json!({"welcomeSeen": true})), true);
    assert_eq!(
        checked(json!({"path": "C:\\HLSR", "confirmed": false}), json!({})),
        false
    );
    assert_eq!(checked(Value::Null, json!({})), true);
}

/// `runner-profile-state` (`onRunnerProfileUpdated`): вход и выход меняют источник статистики —
/// оболочка заново берёт `game-stats-get` выбранной игры, блок статистики показывает новый ответ.
#[test]
fn runner_profile_state_reloads_stats_of_the_selected_game() {
    let server = json!({"available": true, "gameId": "hl2-4260", "totalPlaySeconds": 7200,
        "lastLaunchAt": "2026-10-01T12:00:00.000Z", "launchCount": 3, "isRunning": false,
        "source": "server"});
    let local = json!({"available": true, "gameId": "hl2-4260", "totalPlaySeconds": 600,
        "lastLaunchAt": null, "launchCount": 1});
    let signed_in = run(&json!({
        "id": "profile-signed-in",
        "game": "220",
        "replies": {"game-stats-get": server},
        "run": [{"event": "runner-profile-state", "payload": {"status": "signed-in"}}],
    }));
    assert_eq!(
        signed_in.trace,
        [json!(["invoke", "game-stats-get", ["220"]])]
    );
    assert_eq!(signed_in.state["stats"], json!({"220": server}));
    let signed_out = run(&json!({
        "id": "profile-signed-out",
        "game": "70",
        "replies": {"game-stats-get": {"$args": [[["70"], local]]}},
        "run": [{"event": "runner-profile-state", "payload": null}],
    }));
    assert_eq!(
        signed_out.trace,
        [json!(["invoke", "game-stats-get", ["70"]])]
    );
    assert_eq!(signed_out.state["stats"], json!({"70": local}));
}

/// Вход и профили — только в официальной сборке: `build-official` закрывает профиль, пока сборка
/// не официальная, профили не открываются и `runner-state` не спрашивается.
#[test]
fn profiles_open_only_in_the_official_build() {
    let other = "76561198000000222";
    let steps = |back: bool| {
        let mut steps = vec![
            json!({"call": "onAccountAction", "args": ["open-profile"]}),
            json!({"event": "build-official", "payload": "unofficial"}),
            json!({"call": "openRunnerProfile", "args": [other]}),
            json!({"call": "onAccountAction", "args": ["open-profile"]}),
            json!({"call": "onAccountAction", "args": ["friends"]}),
        ];
        if back {
            steps.push(json!({"event": "build-official", "payload": "official"}));
            steps.push(json!({"call": "openRunnerProfile", "args": [other]}));
        }
        Value::Array(steps)
    };
    let closed = run(&json!({"id": "unofficial", "run": steps(false)}));
    assert_eq!(closed.state["profileOpen"], false);
    assert_eq!(closed.trace, Vec::<Value>::new());
    let reopened = run(&json!({"id": "official-again", "run": steps(true)}));
    assert_eq!(reopened.state["profileOpen"], true);
    assert_eq!(reopened.trace, [json!(["invoke", "runner-state", []])]);
}

/// `launcher-ui-selftest.js`: «Proton notification button starts the explicit update and opens
/// its progress in settings».
#[test]
fn proton_notification_button_opens_settings_and_updates() {
    let got = run(&json!({
        "id": "selftest-proton",
        "game": "218",
        "run": [{"call": "onRunUpdateAction", "args": [{"target": "proton"}]}],
    }));
    assert_eq!(got.trace, [json!(["invoke", "proton-update", []])]);
    assert_eq!(got.state["settingsOpen"], true);
    assert_eq!(got.state["settingsSection"], "proton");
}

/// `launcher-ui-selftest.js`: «interface language can switch repeatedly in both directions».
#[test]
fn interface_language_switches_back_and_forth() {
    let got = run(&json!({
        "id": "selftest-language",
        "game": "218",
        "language": "ru",
        "run": [
            {"call": "setLanguage", "args": ["en"]},
            {"call": "setLanguage", "args": ["ru"]},
            {"call": "setLanguage", "args": ["en"]},
            {"call": "setLanguage", "args": ["ru"]},
            {"call": "setLanguage", "args": ["ru"]},
            {"call": "setLanguage", "args": ["unknown"]},
        ],
    }));
    let languages: Vec<&Value> = got.trace.iter().map(|entry| &entry[2][0]).collect();
    assert_eq!(languages, ["en", "ru", "en", "ru"]);
    assert_eq!(got.state["langRev"], 4);
}

/// Ghosting с эпизодами (`LauncherShell.vue` 0.4.9): статус с `episodes`.
fn ghosting_scenario(
    id: &str,
    episodes: Value,
    prefs: Value,
    run_steps: Value,
    replies: Value,
) -> Run {
    run(&json!({
        "id": id,
        "game": "218",
        "prefs": prefs,
        "delivery": {"218": {"gameId": "ghosting-renewed", "launcherId": "218", "state": "ready",
            "mainButton": {"state": "ready", "action": "play", "enabled": true},
            "episodes": episodes}},
        "replies": replies,
        "run": run_steps,
    }))
}

#[test]
fn ghosting_episodes_launch_install_cancel_and_remove() {
    let installed = json!({"ep1": {"installed": true}, "ep2": {"installed": true}});
    // Выбранный эпизод — в options запуска; без эпизода `{}` (журнал сценариев его прячет).
    let got = ghosting_scenario(
        "episode-play",
        installed.clone(),
        json!({"ghostingBuild": "Episode Two", "timerOffered": true}),
        json!([{"call": "onPlay"}]),
        json!({"game-process-launch": {"status": "ok"}}),
    );
    assert_eq!(
        got.trace,
        [json!(["invoke", "game-process-launch", ["ghosting-renewed", {"episode": "ep2"}]])]
    );
    // Сохранённого эпизода нет в списке — первая сборка, запуск без эпизода.
    let got = ghosting_scenario(
        "episode-play-unknown",
        installed.clone(),
        json!({"ghostingBuild": "Episode Three", "timerOffered": true}),
        json!([{"call": "onPlay"}]),
        json!({"game-process-launch": {"status": "ok"}}),
    );
    assert_eq!(
        got.trace,
        [json!([
            "invoke",
            "game-process-launch",
            ["ghosting-renewed"]
        ])]
    );
    // Выбор сборки Ghosting — только prefs, без main.
    let got = ghosting_scenario(
        "episode-select",
        installed.clone(),
        json!({}),
        json!([{"call": "onSelectBuild", "args": ["Episode One"]}]),
        json!({}),
    );
    assert_eq!(
        got.trace,
        [json!(["prefs", {"ghostingBuild": "Episode One"}])]
    );
    // Отмена паузы эпизода — его архив.
    let got = ghosting_scenario(
        "episode-cancel",
        json!({}),
        json!({}),
        json!([{"call": "onCancelDownload", "args": ["ep2"]}]),
        json!({}),
    );
    assert_eq!(
        got.trace,
        [json!(["invoke", "game-delivery-cancel", ["ghosting-renewed", {"episode": "ep2"}]])]
    );
    // «Удалить эпизод»: Episode One уносит скачанный Episode Two, подтверждение, тост, статус.
    let got = ghosting_scenario(
        "episode-remove",
        installed,
        json!({"ghostingBuild": "Episode One"}),
        json!([{"call": "onMenuAction", "args": ["uninstall"]}, {"answer": true}]),
        json!({"game-episode-remove": {"status": "ok"}}),
    );
    assert_eq!(
        got.trace,
        [
            json!(["confirm", {
                "title": "T(#L_GM_UNINSTALL_EPISODE)",
                "text": "T(#L_GM_EPISODE_REMOVE_BOTH|Episode One)",
                "items": [],
                "note": "T(#L_GM_EPISODE_REMOVE_NOTE)",
                "confirmLabel": "T(#L_C_UNINSTALL)",
                "cancelLabel": null,
                "danger": true,
                "choices": [],
                "defaultChoice": null
            }]),
            json!(["invoke", "game-episode-remove", ["ep1"]]),
            json!(["notify", "T(#L_GM_EPISODE_REMOVED)", "fas fa-check"]),
            json!(["invoke", "game-episodes-status", []]),
        ]
    );
}

#[test]
fn installing_an_episode_takes_only_the_selected_one_and_refreshes_sizes() {
    let services_trace = |build: &str, reply: Value| {
        let got = ghosting_scenario(
            "episode-install",
            json!({}),
            json!({"ghostingBuild": build}),
            json!([{"call": "onInstallEpisode"}]),
            json!({"game-episode-install": reply}),
        );
        got.trace
    };
    for (build, id) in [("Episode One", "ep1"), ("Episode Two", "ep2")] {
        assert_eq!(
            services_trace(build, json!({"status": "ok"})),
            [
                json!(["invoke", "game-episode-install", [id]]),
                json!(["invoke", "game-episodes-status", []]),
            ]
        );
    }
    // Эпизод не выбран — main не зовётся.
    assert!(services_trace("HL1 MOVEMENT", json!({"status": "ok"})).is_empty());
    let failed = services_trace(
        "Episode One",
        json!({"status": "error", "error": {"code": "CHECKSUM_MISMATCH"}}),
    );
    assert_eq!(failed[1][0], "notify");
    assert_eq!(failed[2], json!(["invoke", "game-episodes-status", []]));
}

#[test]
fn verify_files_reports_damage_and_stock_cfg_softly() {
    let verify = |reply: Value| {
        run(&json!({
            "id": "verify",
            "game": "70",
            "delivery": {"70": {"gameId": "hl1-won", "launcherId": "70", "state": "ready"}},
            "replies": {"game-menu-verify-files": reply},
            "run": [{"call": "onMenuAction", "args": ["verify"]}],
        }))
        .trace
    };
    let call = json!(["invoke", "game-menu-verify-files", ["hl1-won"]]);
    assert_eq!(
        verify(json!({"ok": true, "damaged": 0})),
        [
            call.clone(),
            json!(["notify", "T(#L_GM_NO_DAMAGE)", "fas fa-check"])
        ]
    );
    assert_eq!(
        verify(json!({"ok": true, "stockMissing": 2, "configChanged": 0})),
        [
            call.clone(),
            json!([
                "notify",
                "T(#L_GM_FILES_OK_CFG|T(#L_GM_CFG_MISSING_N|2))",
                "fas fa-check"
            ])
        ]
    );
    assert_eq!(
        verify(json!({"ok": false, "damaged": 3, "stockMissing": 1, "configChanged": 2})),
        [
            call.clone(),
            json!([
                "notify",
                "T(#L_D_FILES_DAMAGED_N|3). T(#L_GM_CFG_DIFF_N|T(#L_GM_CFG_MISSING_N|1), T(#L_GM_CFG_DIFFER_N|2)).",
                "fas fa-exclamation-circle"
            ])
        ]
    );
    // Ошибка канала — общий текст доставки, как у других пунктов меню.
    let failed = verify(json!({"status": "error", "error": {"code": "BUSY"}}));
    assert_eq!(failed[0], call);
    assert_eq!(failed[1][0], "notify");
}

/// Ghosting в «Играх» с ремонтом по файлам (G3).
fn ghosting(run_steps: Value, replies: Value) -> Run {
    run(&json!({
        "id": "g3",
        "game": "218",
        "delivery": {"218": {"gameId": "ghosting-renewed", "launcherId": "218", "state": "ready",
            "fileRepair": true}},
        "replies": replies,
        "run": run_steps,
    }))
}

#[test]
fn repair_window_downloads_chosen_files_and_offers_again() {
    let files = json!([{"path": "bin/a.dll", "size": 10, "kind": "damaged"},
        {"path": "cfg/autoexec.cfg", "size": 1, "kind": "cfg-changed"}]);
    let check = json!({"status": "confirmation-required", "files": files});
    let replies = json!({"game-menu-repair": {"$seq": [check.clone(),
        {"status": "ok", "repaired": ["bin/a.dll"], "remaining": ["bin/b.dll"]},
        {"status": "no-damage"}]}});
    // Окно с файлами → выбран один → итог с остатком → «Починить ещё раз» → повреждений нет.
    let got = ghosting(
        json!([{"call": "onMenuAction", "args": ["repair"]}, {"repair": ["bin/a.dll"]},
            {"answer": true}]),
        replies.clone(),
    );
    assert_eq!(
        got.trace,
        [
            json!(["invoke", "game-menu-repair", ["ghosting-renewed"]]),
            json!(["invoke", "game-menu-repair", ["ghosting-renewed",
                {"confirmed": true, "recheck": false, "files": ["bin/a.dll"]}]]),
            json!(["confirm", {"title": "T(#L_C_REPAIR_TITLE)",
                "text": "T(#L_GM_REPAIR_RESULT|1|1)", "items": ["bin/b.dll"], "note": null,
                "confirmLabel": "T(#L_GM_REPAIR_AGAIN)", "cancelLabel": "T(#L_CLOSE)",
                "danger": false, "choices": [], "defaultChoice": null}]),
            json!(["invoke", "game-menu-repair", ["ghosting-renewed"]]),
            json!(["notify", "T(#L_GM_NO_DAMAGE)", "fas fa-check"]),
        ]
    );
    // Окно открыто, пока нет ответа; отказ — без загрузки.
    let open = ghosting(
        json!([{"call": "onMenuAction", "args": ["repair"]}]),
        replies.clone(),
    );
    assert_eq!(open.state["repairFiles"], files);
    let declined = ghosting(
        json!([{"call": "onMenuAction", "args": ["repair"]}, {"repair": false}]),
        replies,
    );
    assert_eq!(declined.trace.len(), 1);
    assert_eq!(declined.state["repairFiles"], Value::Null);
    // Ошибка по файлу — с его именем; всё починено — итог со стоковыми cfg.
    let failed = ghosting(
        json!([{"call": "onMenuAction", "args": ["repair"]}]),
        json!({"game-menu-repair": {"status": "error",
            "error": {"code": "REPAIR_SOURCE_MISSING", "details": {"path": "bin/a.dll"}}}}),
    );
    assert_eq!(
        failed.trace[1],
        json!([
            "notify",
            "T(#L_GM_REPAIR_FILE_UNAVAILABLE|bin/a.dll)",
            "fas fa-exclamation-triangle"
        ])
    );
    let done = ghosting(
        json!([{"call": "onMenuAction", "args": ["repair"]}, {"repair": ["bin/a.dll"]}]),
        json!({"game-menu-repair": {"$seq": [check,
            {"status": "ok", "repaired": ["bin/a.dll"], "remaining": [],
             "integrity": {"stockMissing": 1}}]}}),
    );
    assert_eq!(
        done.trace[2],
        json!([
            "notify",
            "T(#L_GM_REPAIR_DONE_N|1). T(#L_GM_CFG_DIFF_N|T(#L_GM_CFG_MISSING_N|1)).",
            "fas fa-check"
        ])
    );
}

#[test]
fn crash_report_window_and_requested_stop() {
    let report = json!({"gameId": "ghosting-renewed", "launcherId": "218", "exitCode": -1073741819,
        "file": "C:\\dumps\\crash.zip", "archive": true});
    let got = ghosting(
        json!([{"event": "game-process-crashed", "payload": {"exitCode": 1}},
            {"event": "game-crash-report", "payload": report},
            {"crash": "open-folder"}]),
        json!({"game-crash-open-dump": {"status": "ok"}}),
    );
    assert_eq!(
        got.trace,
        [json!([
            "invoke",
            "game-crash-open-dump",
            ["ghosting-renewed"]
        ])]
    );
    assert_eq!(got.state["crashReport"], report);
    let closed = ghosting(
        json!([{"event": "game-crash-report", "payload": report},
            {"crash": "export-diagnostics"}]),
        json!({}),
    );
    assert_eq!(closed.state["crashReport"], Value::Null);
    assert_eq!(closed.state["settingsSection"], "diagnostics");
    // Остановка по просьбе игрока — тосты, как в 0.4.7.
    let stopped = ghosting(
        json!([{"event": "game-process-crashed", "payload": {"requested": true}}, {"wait": 600}]),
        json!({}),
    );
    assert_eq!(stopped.trace[0][1], "T(#L_P_CRASHED)");
    assert_eq!(stopped.trace[1][1], "T(#L_P_CRASH_HINT)");
}

#[test]
fn pack_export_confirms_modified_files_and_import_reports_them() {
    let request =
        json!({"status": "confirmation-required", "token": "t1", "modified": 3, "custom": true});
    let got = ghosting(
        json!([{"call": "onMenuAction", "args": ["exportBuild"]}, {"answer": true}]),
        json!({"game-build-export": request,
            "game-build-export-confirm": {"status": "ok", "kind": "pack", "size": 1536}}),
    );
    assert_eq!(got.trace[1][1]["title"], "T(#L_BT_PACK_MODIFIED_TITLE)");
    assert_eq!(got.trace[1][1]["text"], "T(#L_BT_PACK_MODIFIED_CUSTOM|3)");
    assert_eq!(
        got.trace[2],
        json!(["invoke", "game-build-export-confirm", ["t1"]])
    );
    assert_eq!(got.trace[3][0], "notify");
    let declined = ghosting(
        json!([{"call": "onMenuAction", "args": ["exportBuild"]}, {"answer": false}]),
        json!({"game-build-export": request}),
    );
    assert_eq!(declined.trace.len(), 2);

    let imported = ghosting(
        json!([{"call": "onMenuAction", "args": ["importBuild"]}]),
        json!({"game-build-import": {"status": "ok", "kind": "pack", "version": "1.0.13",
            "launcherId": "218", "modified": {"changed": 2, "restored": 2, "custom": 1,
            "customDir": "custom/imported", "rejected": 1}}}),
    );
    assert_eq!(
        imported.trace[1..],
        [
            json!(["invoke", "game-delivery-selected", ["218"]]),
            json!([
                "notify",
                "T(#L_BT_PACK_IMPORTED|1.0.13) T(#L_BT_PACK_RESTORED|2|2) \
                T(#L_BT_PACK_MOVED_CUSTOM|1|custom/imported) T(#L_BT_PACK_REJECTED|1)",
                "fas fa-file-archive"
            ]),
            json!(["ms", 15000]),
        ]
    );
}

#[test]
fn ffmpeg_gate_installs_before_demo_or_stays() {
    let status = json!({"status": "ok", "ffmpeg": {"state": "not-installed", "installable": true,
        "origins": [{"site": "github.com", "release": "n9.0"}, {"site": "hlsr.pro"}],
        "pinnedVersion": "9.0.2", "build": "gpl", "sizeBytes": 0}});
    // Скачан — сразу в «Демо».
    let ready = ghosting(
        json!([{"selectDemo": true}]),
        json!({"demo-ffmpeg-status": {"status": "ok", "ffmpeg": {"state": "ready"}}}),
    );
    assert_eq!(ready.trace[1], json!(["enter", "demo"]));
    // Нет — окно; «Скачать» → процент → готово → в «Демо».
    let got = ghosting(
        json!([{"selectDemo": true}, {"ffmpeg": true},
            {"event": "demo-ffmpeg-progress", "payload": {"percent": 41.6}}]),
        json!({"demo-ffmpeg-status": status, "demo-ffmpeg-install": {"$pending": true}}),
    );
    assert_eq!(
        got.state["ffmpegGate"],
        json!({"installing": true, "percent": 42, "error": null})
    );
    let entered = ghosting(
        json!([{"selectDemo": true}, {"ffmpeg": true}]),
        json!({"demo-ffmpeg-status": status, "demo-ffmpeg-install": {"status": "ok"}}),
    );
    assert_eq!(entered.trace.last().unwrap(), &json!(["enter", "demo"]));
    assert_eq!(entered.state["ffmpegGate"], Value::Null);
    // Ошибка — текст в окне; «Позже» закрывает без входа.
    let failed = ghosting(
        json!([{"selectDemo": true}, {"ffmpeg": true}]),
        json!({"demo-ffmpeg-status": status,
            "demo-ffmpeg-install": {"status": "error", "code": "NETWORK_ERROR"}}),
    );
    assert_eq!(
        failed.state["ffmpegGate"]["error"],
        "T(#L_DEMO_ERR_NETWORK)"
    );
    let later = ghosting(
        json!([{"selectDemo": true}, {"ffmpeg": false}]),
        json!({"demo-ffmpeg-status": status}),
    );
    assert_eq!(later.trace.len(), 1);
    assert_eq!(later.state["ffmpegGate"], Value::Null);
}
