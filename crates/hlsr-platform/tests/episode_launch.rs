//! Самотест 0.4.9 `episode-launch-selftest.js` (Windows-запуск): эпизод Ghosting — тот же `hl2.exe` с
//! `-game ep2` и `gameinfo.txt` эпизода, конфиг Ghosting в эпизоде уже на `spawn` (кроме стоковых cfg
//! сборки), отказы до «запускается», возврат висящего config.cfg перед запуском самой Ghosting.
//! Части про IPC, QA, процесс и Discord — в `platform_api.rs` и `process_service.rs`.

use hlsr_delivery::episodes;
use hlsr_delivery::errors::{DeliveryError, RawError};
use hlsr_delivery::manifests::launch_config_of;
use hlsr_delivery::ports::{MemoryLogger, Registry, RegistryHive, RegistryValue};
use hlsr_platform::game_process_service::StartedProcess;
use hlsr_platform::launch_service::{
    LaunchDelivery, LaunchLocator, LaunchProcesses, LaunchService, LaunchServiceOptions,
    LaunchSettings, LaunchSpawner, SpawnRequest,
};
use hlsr_platform::process_inspect::{ProcessInfo, ProcessInspector};
use hlsr_platform::spawn::{Child, ChildControl};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const GAME: &str = "ghosting-renewed";
const GAMEINFO: &str = "\"GameInfo\"\n{\n\tFileSystem\n\t{\n\t\tSteamAppId 243730\n\t}\n}\n";
const WATCHED: [&str; 6] = [
    "ep2/cfg/autoexec.cfg",
    "ep2/cfg/config.cfg",
    "ep2/cfg/tc_n.cfg",
    "ep2/cfg/chapter1.cfg",
    "ghostingmod/cfg/autoexec.cfg",
    "ghostingmod/cfg/config.cfg",
];

fn write(root: &Path, relative: &str, data: &str) {
    let file = root.join(relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, data).unwrap();
}

fn read(root: &Path, relative: &str) -> Option<String> {
    std::fs::read_to_string(root.join(relative)).ok()
}

/// Установленная Ghosting: cfg игрока и стоковый chapter1.cfg; Episode One и (если `ep2`) Two.
fn ghosting(root: &Path, ep2: bool) {
    write(root, "hl2.exe", "never spawned");
    write(root, "steam_appid.txt", "243730\n\0");
    write(root, "ghostingmod/gameinfo.txt", GAMEINFO);
    write(
        root,
        "ghostingmod/cfg/autoexec.cfg",
        "// файл игрока\nexec tc_n\n",
    );
    write(root, "ghostingmod/cfg/config.cfg", "bind w +forward\n");
    write(root, "ghostingmod/cfg/tc_n.cfg", "// свой cfg игрока\n");
    write(
        root,
        "ghostingmod/cfg/chapter1.cfg",
        "map d1_trainstation_01\n",
    );
    let ids: &[&str] = if ep2 { &["ep1", "ep2"] } else { &["ep1"] };
    for id in ids {
        let dir = episodes::episode_of(id).unwrap().dir;
        write(root, &format!("{dir}/gameinfo.txt"), GAMEINFO);
        write(
            root,
            &format!("{dir}/{}", episodes::MARKER),
            r#"{"version":"1.0.0"}"#,
        );
        write(
            root,
            &format!("{dir}/cfg/chapter1.cfg"),
            &format!("map {id}_first\n"),
        );
    }
}

/// `{file, argv, cwd, files}` в момент `spawn`.
type Spawned = (
    PathBuf,
    Vec<String>,
    PathBuf,
    BTreeMap<String, Option<String>>,
);

#[derive(Default)]
struct Calls {
    spawned: Vec<Spawned>,
    registered: Vec<Option<String>>,
}

struct World {
    root: PathBuf,
    settings: Value,
    calls: Mutex<Calls>,
    starting: AtomicUsize,
}

struct Fake(Arc<World>);

impl LaunchProcesses for Fake {
    fn is_running(&self, _game_id: &str) -> bool {
        false
    }
    fn get_process_state(&self, _game_id: &str) -> Value {
        Value::Null
    }
    fn mark_starting(&self, _game_id: &str) {
        self.0.starting.fetch_add(1, Ordering::SeqCst);
    }
    fn cancel_starting(&self, _game_id: &str, _reason: &str) {}
    fn mark_exited(&self, _game_id: &str, _pid: u32) -> bool {
        false
    }
}

impl LaunchDelivery for Fake {
    fn is_busy(&self) -> bool {
        false
    }
    fn installed_of(&self, _game_id: &str) -> Result<Value, DeliveryError> {
        Ok(json!({"installPath": self.0.root.to_string_lossy(), "version": "1.0.13"}))
    }
    fn register_started_process(
        &self,
        game_id: &str,
        info: StartedProcess,
    ) -> Result<Value, DeliveryError> {
        let episode = info.episode.clone();
        self.0.calls.lock().unwrap().registered.push(info.episode);
        Ok(json!({"gameId": game_id, "state": "running", "episode": episode}))
    }
    fn files_manifest_for(
        &self,
        _game_id: &str,
        _version: Option<&str>,
    ) -> Result<Value, DeliveryError> {
        Ok(
            json!({"files": [{"path": "ghostingmod/cfg/chapter1.cfg"}, {"path": "ghostingmod/cfg/config.cfg"}]}),
        )
    }
}

impl LaunchSettings for Fake {
    fn get(&self, _game_id: &str) -> Value {
        self.0.settings.clone()
    }
    fn save(&self, _game_id: &str, _settings: &Value) -> bool {
        panic!("запуск настройки не пишет")
    }
}

struct NoSignals;

impl ChildControl for NoSignals {
    fn kill(&self, _signal: &str) -> bool {
        false
    }
    fn kill_group(&self, _signal: &str) -> bool {
        false
    }
}

impl LaunchSpawner for Fake {
    fn spawn(
        &self,
        file: &Path,
        args: &[String],
        request: SpawnRequest,
    ) -> Result<Child, RawError> {
        let files = WATCHED
            .iter()
            .map(|relative| ((*relative).to_owned(), read(&self.0.root, relative)))
            .collect();
        self.0.calls.lock().unwrap().spawned.push((
            file.to_path_buf(),
            args.to_vec(),
            request.cwd,
            files,
        ));
        let (child, controller) = Child::new(Some(4242), Arc::new(NoSignals));
        controller.spawned();
        Ok(child)
    }
}

impl ProcessInspector for Fake {
    fn is_alive(&self, _pid: u32) -> bool {
        true
    }
    fn inspect(&self, _pid: u32) -> Option<ProcessInfo> {
        None
    }
}

impl LaunchLocator for Fake {
    fn preflight(&self, _app_id: u32) -> Result<Value, DeliveryError> {
        Ok(json!({"steam": {"pid": 1}}))
    }
    fn describe_steam_only(&self) -> Value {
        json!({"ok": true, "steam": {"pid": 1}})
    }
    fn describe_availability(&self, _app_id: u32) -> Value {
        json!({"ok": true})
    }
}

struct NoRegistry;

impl Registry for NoRegistry {
    fn read_value(&self, _hive: RegistryHive, _key: &str, _name: &str) -> Option<RegistryValue> {
        None
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    world: Arc<World>,
    service: LaunchService,
}

/// `launcher(t, options)` самотеста: `LaunchService` с заглушками Steam, процесса и `spawn`.
fn launcher(ep2: bool, settings: Value) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Ghosting");
    ghosting(&root, ep2);
    let world = Arc::new(World {
        root,
        settings,
        calls: Mutex::default(),
        starting: AtomicUsize::new(0),
    });
    let fake = Arc::new(Fake(world.clone()));
    let service = LaunchService::new(LaunchServiceOptions {
        delivery: Some(fake.clone()),
        processes: Some(fake.clone()),
        settings_store: Some(fake.clone()),
        spawner: Some(fake.clone()),
        inspector: Some(fake.clone()),
        steam_locator: Some(fake),
        launch_registry: Some(Arc::new(|id: &str| {
            [GAME, "hl2-4260"]
                .contains(&id)
                .then(|| launch_config_of(id))
                .flatten()
        })),
        early_exit_window: Some(Duration::ZERO),
        registry: Some(Arc::new(NoRegistry)),
        logger: Some(Arc::new(MemoryLogger::default())),
        platform: Some(hlsr_core::js::Platform::Win32),
        env: Some([("PATH".to_owned(), "x".to_owned())].into_iter().collect()),
        ..LaunchServiceOptions::default()
    });
    Rig {
        _dir: dir,
        world,
        service,
    }
}

fn game_arg(argv: &[String]) -> &str {
    let at = argv.iter().position(|arg| arg == "-game").expect("-game");
    &argv[at + 1]
}

#[tokio::test]
async fn an_episode_runs_hl2_with_its_game_dir_and_the_ghosting_config() {
    let rig = launcher(true, Value::Null);
    let root = rig.world.root.clone();
    // gameinfo.txt — эпизода: без gameinfo.txt Ghosting эпизод запускается.
    std::fs::remove_file(root.join("ghostingmod/gameinfo.txt")).unwrap();
    let result = rig
        .service
        .launch_episode(GAME, Some("ep2"))
        .await
        .expect("запуск");
    assert_eq!(result["status"], "started");

    let calls = rig.world.calls.lock().unwrap();
    assert_eq!(calls.spawned.len(), 1);
    let (file, argv, cwd, files) = &calls.spawned[0];
    assert_eq!(*file, root.join("hl2.exe"));
    assert_eq!(*cwd, root);
    assert_eq!(game_arg(argv), "ep2");
    assert!(!argv.iter().any(|arg| arg == "ghostingmod"), "{argv:?}");

    assert!(
        files["ep2/cfg/autoexec.cfg"].is_some(),
        "файл игрока в эпизоде"
    );
    assert_eq!(
        files["ep2/cfg/autoexec.cfg"],
        read(&root, "ghostingmod/cfg/autoexec.cfg"),
        "файл игрока"
    );
    assert_eq!(
        files["ep2/cfg/config.cfg"].as_deref(),
        Some("bind w +forward\n")
    );
    assert_eq!(
        files["ep2/cfg/tc_n.cfg"].as_deref(),
        Some("// свой cfg игрока\n")
    );
    assert_eq!(
        files["ep2/cfg/chapter1.cfg"].as_deref(),
        Some("map ep2_first\n"),
        "стоковый cfg Ghosting в эпизод не идёт"
    );
    assert_eq!(calls.registered, [Some("ep2".to_owned())]);
    assert_eq!(
        episodes::pending_sync(Some(root.as_path())),
        ["ep2"],
        "отметка для возврата config.cfg"
    );
}

#[tokio::test]
async fn the_players_own_game_argument_does_not_override_the_episode() {
    for launch_args in [
        "-game ghostingmod",
        "-novid +game=ghostingmod",
        "-GAME ghostingmod",
    ] {
        let rig = launcher(true, json!({"launchArgs": launch_args}));
        rig.service
            .launch_episode(GAME, Some("ep2"))
            .await
            .expect("запуск");
        let calls = rig.world.calls.lock().unwrap();
        let argv = &calls.spawned[0].1;
        let games: Vec<&String> = argv
            .iter()
            .filter(|arg| {
                let rest = arg.trim_start_matches(['-', '+', '/']);
                rest.len() != arg.len() && rest.to_lowercase().starts_with("game")
            })
            .collect();
        assert_eq!(games, ["-game"], "{launch_args}");
        assert_eq!(game_arg(argv), "ep2", "{launch_args}");
        assert!(
            !argv.iter().any(|arg| arg == "ghostingmod"),
            "{launch_args}"
        );
    }
}

#[tokio::test]
async fn an_episode_that_is_not_installed_is_not_installed_without_starting() {
    let rig = launcher(false, Value::Null);
    let error = rig
        .service
        .launch_episode(GAME, Some("ep2"))
        .await
        .unwrap_err();
    assert_eq!(error.code.as_str(), "NOT_INSTALLED");
    assert!(rig.world.calls.lock().unwrap().spawned.is_empty());
    assert_eq!(rig.world.starting.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_foreign_episode_or_a_game_without_episodes_is_launch_config_invalid() {
    let rig = launcher(true, Value::Null);
    for (game, episode) in [(GAME, "ep3"), (GAME, "__proto__"), ("hl2-4260", "ep1")] {
        let error = rig
            .service
            .launch_episode(game, Some(episode))
            .await
            .unwrap_err();
        assert_eq!(
            error.code.as_str(),
            "LAUNCH_CONFIG_INVALID",
            "{game} {episode}"
        );
    }
    assert!(rig.world.calls.lock().unwrap().spawned.is_empty());
    assert_eq!(rig.world.starting.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn launching_ghosting_first_returns_a_pending_episode_config() {
    let rig = launcher(true, Value::Null);
    let root = rig.world.root.clone();
    episodes::sync_config_in(&root, "ep2", None).unwrap();
    write(
        &root,
        "ep2/cfg/config.cfg",
        "bind w +forward\nbind e +use\n",
    );

    rig.service.launch(GAME).await.expect("запуск");

    let calls = rig.world.calls.lock().unwrap();
    let (_, argv, _, files) = &calls.spawned[0];
    assert_eq!(game_arg(argv), "ghostingmod");
    assert_eq!(
        files["ghostingmod/cfg/config.cfg"].as_deref(),
        Some("bind w +forward\nbind e +use\n")
    );
    assert!(episodes::pending_sync(Some(root.as_path())).is_empty());
    assert_eq!(calls.registered, [None]);
}
