//! Самотесты 0.4.7 для запуска игр на Windows (`game-delivery/LaunchService.js`):
//! `hl1-selftest.js` (Windows: после инжектора отслеживается сам `hl.exe`, ошибки инжектора, слой
//! DEP только на Windows, RInput без окна консоли, патч курсора RInput через память),
//! `ghosting-settings-selftest.js` (Windows: перенос настроек Ghosting перед запуском, возврат
//! `steam_appid.txt`, отказ без Steam), `livesplit-overlay-selftest.js` (Windows: инжектор
//! оверлея отдельно, ждёт BXT у HL1). Сверх них — окружение игры без переменных окна лаунчера и,
//! на Windows, настоящие процессы теста: `spawn` (флаги, окружение, ошибки), ожидание старта с
//! системным инспектором и маска ЦП (`SetProcessAffinityMask`).

mod proton_support;

use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::logger::{Logger, MemoryLogger};
use hlsr_delivery::errors::{Code, DeliveryError, RawError};
use hlsr_delivery::ghosting_personalization;
use hlsr_delivery::manifests::{LaunchConfig, launch_config_of};
use hlsr_delivery::ports::{Registry, RegistryHive, RegistryValue};
use hlsr_delivery::{launch_args, vulkan_backend};
use hlsr_platform::dep_compat::Exemption;
use hlsr_platform::game_process_service::StartedProcess;
use hlsr_platform::launch_service::{
    DepCompat, LaunchDelivery, LaunchLocator, LaunchProcesses, LaunchService, LaunchServiceOptions,
    LaunchSettings, LaunchSpawner, STRIPPED_ENV, SpawnRequest, normalize_app_id_text,
};
use hlsr_platform::livesplit_overlay::{self, LiveSplitOverlay, OverlayOptions};
use hlsr_platform::process_inspect::{ProcessInfo, ProcessInspector};
use hlsr_platform::rinput_menu_fix::{MenuSession, PatchStatus};
use hlsr_platform::spawn::{Child, ChildControl, ChildController, Env};
use proton_support::{Root, write_file};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ---------------------------------------------------------------- подделки

/// Пустой реестр: настоящий реестр владельца в проверках не читается.
struct NoRegistry;

impl Registry for NoRegistry {
    fn read_value(&self, _hive: RegistryHive, _key: &str, _name: &str) -> Option<RegistryValue> {
        None
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

/// Поддельный процесс: `exit` — завершился сразу с этим кодом, иначе жив.
fn fake_child(pid: u32, exit: Option<i32>) -> (Child, ChildController) {
    let (child, controller) = Child::new(Some(pid), Arc::new(NoSignals));
    controller.spawned();
    if let Some(code) = exit {
        controller.exit(Some(code), None);
    }
    (child, controller)
}

type SpawnFn = dyn Fn(&Path, &[String], &SpawnRequest) -> Result<Child, RawError> + Send + Sync;

/// `spawnFn` проверки: вызовы и ответ замыкания.
struct Spawns {
    calls: Mutex<Vec<(PathBuf, Vec<String>, SpawnRequest)>>,
    answer: Box<SpawnFn>,
}

impl Spawns {
    fn new(
        answer: impl Fn(&Path, &[String], &SpawnRequest) -> Result<Child, RawError>
        + Send
        + Sync
        + 'static,
    ) -> Arc<Spawns> {
        Arc::new(Spawns {
            calls: Mutex::new(Vec::new()),
            answer: Box::new(answer),
        })
    }

    /// Процессы, которые сразу выходят с кодом 0.
    fn exiting() -> Arc<Spawns> {
        Spawns::new(|_, _, _| Ok(fake_child(100, Some(0)).0))
    }

    fn calls(&self) -> Vec<(PathBuf, Vec<String>, SpawnRequest)> {
        self.calls.lock().unwrap().clone()
    }
}

impl LaunchSpawner for Spawns {
    fn spawn(
        &self,
        file: &Path,
        args: &[String],
        request: SpawnRequest,
    ) -> Result<Child, RawError> {
        self.calls
            .lock()
            .unwrap()
            .push((file.to_path_buf(), args.to_vec(), request.clone()));
        (self.answer)(file, args, &request)
    }
}

/// Инспектор проверки: `findByImageName` — замыкание, процессы живы.
struct Inspector {
    find: Box<dyn Fn() -> Vec<ProcessInfo> + Send + Sync>,
}

impl ProcessInspector for Inspector {
    fn is_alive(&self, _pid: u32) -> bool {
        true
    }
    fn inspect(&self, _pid: u32) -> Option<ProcessInfo> {
        None
    }
    fn find_by_image_name(&self, _image_name: &str) -> Vec<ProcessInfo> {
        (self.find)()
    }
}

fn inspector(find: impl Fn() -> Vec<ProcessInfo> + Send + Sync + 'static) -> Arc<Inspector> {
    Arc::new(Inspector {
        find: Box::new(find),
    })
}

fn info(pid: u32, executable: &str, started_at: Option<String>) -> ProcessInfo {
    ProcessInfo {
        pid,
        executable_path: Some(executable.to_owned()),
        name: Some("hl.exe".to_owned()),
        started_at,
        source: None,
    }
}

fn hl1() -> &'static LaunchConfig {
    launch_config_of("hl1-won").unwrap()
}

/// `fakeInstall(root)` самотеста HL1.
fn fake_install(root: &Path) {
    write_file(&root.join("Half-Life/hl.exe"), "MZ", None);
    write_file(
        &root.join("Half-Life/valve_WON/liblist.gam"),
        "game \"Half-Life\"\n",
        None,
    );
    write_file(&root.join("Half-Life/steam_appid.txt"), "220", None);
    write_file(&root.join("Bunnymod XT/Injector.exe"), "MZ", None);
}

fn base_options() -> LaunchServiceOptions {
    LaunchServiceOptions {
        registry: Some(Arc::new(NoRegistry)),
        logger: Some(Arc::new(MemoryLogger::default())),
        ..LaunchServiceOptions::default()
    }
}

fn iso(ms: f64) -> String {
    js::iso_string(std::time::UNIX_EPOCH + Duration::from_millis(ms as u64))
}

// ---------------------------------------------------------------- hl1-selftest.js (Windows)

/// «HL1 Windows: после инжектора отслеживается сам hl.exe».
#[tokio::test]
async fn hl1_windows_tracks_hl_exe_after_the_injector() {
    let path = NodePath::native();
    let executable = path.resolve(&["/games/hl1-won/Half-Life/hl.exe"]);
    let other = path.resolve(&["/other/hl.exe"]);
    let spawned_at = 1_792_152_000_000.0;
    let polls = Arc::new(AtomicUsize::new(0));
    let found = {
        let polls = polls.clone();
        let executable = executable.clone();
        move || {
            if polls.fetch_add(1, Ordering::SeqCst) + 1 < 3 {
                return Vec::new();
            }
            vec![
                info(10, &executable, Some(iso(spawned_at - 60_000.0))),
                info(11, &other, Some(iso(spawned_at))),
                info(12, &executable, Some(iso(spawned_at + 100.0))),
            ]
        }
    };
    let service = LaunchService::new(LaunchServiceOptions {
        inspector: Some(inspector(found)),
        // Игра находится на третьем опросе; срок большой, чтобы нагрузка машины его не съела.
        injected_game_timeout: Some(Duration::from_millis(30_000)),
        injected_game_poll: Some(Duration::from_millis(5)),
        ..base_options()
    });
    let (child, _controller) = fake_child(1, Some(0));
    let started = service
        .wait_for_injected_game(&child, &executable, spawned_at)
        .await
        .unwrap();
    assert_eq!(started.pid, 12, "старый hl.exe и чужая копия игнорируются");
    assert_eq!(started.started_at, iso(spawned_at + 100.0));
}

/// «HL1 Windows: ошибка инжектора не превращается в зависший запуск».
#[tokio::test]
async fn hl1_windows_injector_failure_does_not_hang() {
    let executable = NodePath::native().resolve(&["/games/hl1-won/Half-Life/hl.exe"]);
    let service = |timeout: u64, grace: Option<u64>| {
        LaunchService::new(LaunchServiceOptions {
            inspector: Some(inspector(Vec::new)),
            injected_game_timeout: Some(Duration::from_millis(timeout)),
            injected_game_poll: Some(Duration::from_millis(5)),
            injector_exit_grace: grace.map(Duration::from_millis),
            ..base_options()
        })
    };

    let (crashed, _crashed) = fake_child(1, Some(3));
    let error = service(2000, None)
        .wait_for_injected_game(&crashed, &executable, 0.0)
        .await
        .unwrap_err();
    assert_eq!(error.code, Code::LaunchFailed);
    assert!(error.message.contains("кодом 3"), "{}", error.message);
    assert_eq!(error.details, Some(json!({"exitCode": 3})));

    let (alive, _alive) = fake_child(1, None);
    let error = service(50, None)
        .wait_for_injected_game(&alive, &executable, 0.0)
        .await
        .unwrap_err();
    assert!(
        error.message.contains("за отведённое время"),
        "{}",
        error.message
    );

    let (done, _done) = fake_child(1, Some(0));
    let error = service(2000, Some(30))
        .wait_for_injected_game(&done, &executable, 0.0)
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "Инжектор Bunnymod XT завершился, но игра не запустилась"
    );

    let failed = Child::failed(RawError::new("spawn Injector.exe EACCES"));
    let error = service(2000, None)
        .wait_for_injected_game(&failed, &executable, 0.0)
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "Инжектор Bunnymod XT не запустился: spawn Injector.exe EACCES"
    );
}

struct FakeDep {
    calls: Mutex<Vec<String>>,
}

impl DepCompat for FakeDep {
    fn ensure_exemption(&self, executable: &str, _logger: &dyn Logger) -> Exemption {
        self.calls.lock().unwrap().push(executable.to_owned());
        Exemption::Added
    }
    fn query_policy(&self) -> Option<i64> {
        Some(1)
    }
}

/// «HL1 Windows: слой ставится перед запуском только на Windows».
#[tokio::test]
async fn hl1_windows_dep_layer_only_on_windows() {
    let dep = Arc::new(FakeDep {
        calls: Mutex::new(Vec::new()),
    });
    let win = LaunchService::new(LaunchServiceOptions {
        dep_compat: Some(dep.clone()),
        platform: Some(Platform::Win32),
        ..base_options()
    });
    assert!(win.prepare_dep("C:\\hl.exe").is_some());
    win.settled().await;
    assert_eq!(*dep.calls.lock().unwrap(), ["C:\\hl.exe"]);
    assert_eq!(win.dep_crash_hint(None), Some("DEP_ALWAYS_ON"));

    let linux = LaunchService::new(LaunchServiceOptions {
        dep_compat: Some(dep.clone()),
        platform: Some(Platform::Linux),
        ..base_options()
    });
    assert_eq!(linux.prepare_dep("C:\\hl.exe"), None);
    assert_eq!(dep.calls.lock().unwrap().len(), 1);
    assert_eq!(linux.dep_crash_hint(None), None);
}

/// «HL1 Windows: RInput внедряется в найденный hl.exe без окна консоли».
#[tokio::test]
async fn hl1_windows_rinput_starts_without_console_window() {
    let root = Root::new("hlsr-hl1-");
    let install = Path::new(&root.text);
    fake_install(install);
    write_file(&install.join("RInput/RInput.exe"), "MZ", None);
    let spawns = Spawns::exiting();
    let service = LaunchService::new(LaunchServiceOptions {
        spawner: Some(spawns.clone()),
        ..base_options()
    });

    service.start_rinput(&root.text, hl1(), None, None).unwrap();
    let calls = spawns.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].0,
        PathBuf::from(NodePath::native().join(&[&root.text, "RInput/RInput.exe"]))
    );
    assert_eq!(calls[0].1, ["hl.exe"]);
    assert!(calls[0].2.windows_hide);
    assert!(calls[0].2.detached);
    assert_eq!(
        calls[0].2.env, None,
        "окружение лаунчера без переменных окна"
    );

    service
        .start_rinput(&root.text, hl1(), Some(&json!({"rinput": false})), None)
        .unwrap();
    assert_eq!(spawns.calls().len(), 1, "выключенный RInput не запускается");
    service.settled().await;
}

struct FakeSession {
    statuses: Arc<Mutex<Vec<PatchStatus>>>,
    calls: Arc<Mutex<Vec<String>>>,
    closed: Arc<AtomicUsize>,
}

impl MenuSession for FakeSession {
    fn patch(&mut self, process_name: &str) -> Result<PatchStatus, String> {
        self.calls.lock().unwrap().push(process_name.to_owned());
        Ok(self.statuses.lock().unwrap().remove(0))
    }
    fn close(&mut self) {
        self.closed.fetch_add(1, Ordering::SeqCst);
    }
}

/// «HL1 Windows: курсор RInput в меню патчится через память hl.exe».
#[tokio::test]
async fn hl1_windows_rinput_menu_cursor_is_patched_in_memory() {
    let root = Root::new("hlsr-hl1-");
    let install = Path::new(&root.text);
    fake_install(install);
    write_file(&install.join("RInput/RInput.exe"), "MZ", None);
    let statuses = Arc::new(Mutex::new(vec![PatchStatus::Waiting, PatchStatus::Patched]));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let closed = Arc::new(AtomicUsize::new(0));
    let sessions = Arc::new(AtomicUsize::new(0));
    let logger = Arc::new(MemoryLogger::default());
    let factory = {
        let (statuses, calls, closed, sessions) = (
            statuses.clone(),
            calls.clone(),
            closed.clone(),
            sessions.clone(),
        );
        Arc::new(move || {
            sessions.fetch_add(1, Ordering::SeqCst);
            Box::new(FakeSession {
                statuses: statuses.clone(),
                calls: calls.clone(),
                closed: closed.clone(),
            }) as Box<dyn MenuSession>
        })
    };
    let service = LaunchService::new(LaunchServiceOptions {
        logger: Some(logger.clone()),
        inspector: Some(inspector(Vec::new)),
        rinput_patch_poll: Some(Duration::from_millis(1)),
        rinput_menu_session: Some(factory),
        spawner: Some(Spawns::exiting()),
        ..base_options()
    });

    service
        .start_rinput(
            &root.text,
            hl1(),
            Some(&json!({"rinputMenuFix": true})),
            Some(4242),
        )
        .unwrap();
    service.rinput_patched().await;
    assert_eq!(
        *calls.lock().unwrap(),
        ["hl.exe", "hl.exe"],
        "ждём, пока RInput поставит перехваты"
    );
    assert_eq!(
        closed.load(Ordering::SeqCst),
        1,
        "сессия закрывается после патча"
    );
    assert!(
        logger
            .take()
            .iter()
            .any(|(_, message)| message.contains("курсор RInput в меню исправлен"))
    );

    service
        .start_rinput(&root.text, hl1(), None, Some(4242))
        .unwrap();
    service.settled().await;
    assert_eq!(
        sessions.load(Ordering::SeqCst),
        1,
        "по умолчанию память игры не трогаем"
    );
}

// ---------------------------------------------------------------- ghosting-settings-selftest.js

#[derive(Default)]
struct GhostingState {
    settings: Value,
    fail_save: bool,
    events: Vec<(String, String)>,
}

struct GhostingWorld {
    state: Mutex<GhostingState>,
    file: PathBuf,
}

impl GhostingWorld {
    /// Файл персонализации 0.4.7; переход 0.4.9 уносит его в копию — тогда пусто.
    fn cfg(&self) -> String {
        std::fs::read_to_string(&self.file).unwrap_or_default()
    }
}

struct GhostingDelivery(String);

impl LaunchProcesses for GhostingDelivery {
    fn is_running(&self, _game_id: &str) -> bool {
        false
    }
    fn get_process_state(&self, _game_id: &str) -> Value {
        Value::Null
    }
    fn mark_starting(&self, _game_id: &str) {}
    fn cancel_starting(&self, _game_id: &str, _reason: &str) {}
    fn mark_exited(&self, _game_id: &str, _pid: u32) -> bool {
        false
    }
}

impl LaunchDelivery for GhostingDelivery {
    fn is_busy(&self) -> bool {
        false
    }
    fn installed_of(&self, _game_id: &str) -> Result<Value, DeliveryError> {
        Ok(json!({"installPath": self.0}))
    }
    fn register_started_process(
        &self,
        _game_id: &str,
        _info: StartedProcess,
    ) -> Result<Value, DeliveryError> {
        unreachable!("запуск останавливается на spawn")
    }
}

struct GhostingSettings(Arc<GhostingWorld>);

impl LaunchSettings for GhostingSettings {
    fn get(&self, _game_id: &str) -> Value {
        self.0.state.lock().unwrap().settings.clone()
    }
    fn save(&self, _game_id: &str, settings: &Value) -> bool {
        let cfg = self.0.cfg();
        let mut state = self.0.state.lock().unwrap();
        state.events.push(("save".to_owned(), cfg));
        if state.fail_save {
            return false;
        }
        state.settings = settings.clone();
        true
    }
}

struct GhostingSteam(Mutex<Value>);

impl LaunchLocator for GhostingSteam {
    fn preflight(&self, _app_id: u32) -> Result<Value, DeliveryError> {
        unreachable!("Ghosting не требует Source SDK")
    }
    fn describe_steam_only(&self) -> Value {
        self.0.lock().unwrap().clone()
    }
    fn describe_availability(&self, _app_id: u32) -> Value {
        unreachable!()
    }
}

/// `launchFixture(t, LaunchService)` самотеста: папка Ghosting, настройки с прицелом 8 и
/// `gh_timer: false`, `spawn` бросает после записи персонализации.
struct GhostingFixture {
    root: Root,
    world: Arc<GhostingWorld>,
    steam: Arc<GhostingSteam>,
    service: LaunchService,
}

/// `launchRegistry` самотеста: Ghosting без `ownsSteamAppIdFile`.
fn selftest_ghosting_config() -> &'static LaunchConfig {
    let mut config = launch_config_of("ghosting-renewed").unwrap().clone();
    config.executable = "hl2.exe";
    config.game_directory = "ghostingmod";
    config.arguments = &["-game", "ghostingmod"];
    config.steam_app_id = 243_730;
    config.source_sdk_required = false;
    config.owns_steam_app_id_file = false;
    config.steam_app_id_file_text = None;
    config.presence_bridge = None;
    config.steam_bridge = None;
    Box::leak(Box::new(config))
}

fn ghosting_fixture(config: &'static LaunchConfig) -> GhostingFixture {
    let root = Root::new("hlsr-ghosting-settings-");
    let install = Path::new(&root.text);
    let cfg = install.join(ghosting_personalization::CFG_DIR);
    let file = cfg.join(ghosting_personalization::PERSONALIZATION_FILE);
    write_file(
        &install.join("hl2.exe"),
        "test executable; never spawned",
        None,
    );
    write_file(
        &install.join("ghostingmod").join("gameinfo.txt"),
        "\"GameInfo\"\n{\nSteamAppId 243730\n}\n",
        None,
    );
    write_file(&install.join("steam_appid.txt"), "243730\n", None);
    write_file(
        &cfg.join("config.cfg"),
        "viewmodel_fov \"84\"\ngh_timer \"1\"\ngh_cross_size \"30\"\n",
        None,
    );
    let old_file = "// Original personalization\ngh_cross_size \"8\"\n";
    write_file(&file, old_file, None);

    let mut personalization = ghosting_personalization::default_personalization().to_value();
    personalization["hudEnabled"] = json!(true);
    personalization["crossSize"] = json!(8);
    personalization["extra"] = json!({"gh_timer": false});
    let world = Arc::new(GhostingWorld {
        state: Mutex::new(GhostingState {
            settings: json!({"personalization": personalization}),
            ..GhostingState::default()
        }),
        file,
    });
    let steam = Arc::new(GhostingSteam(Mutex::new(
        json!({"ok": true, "code": null, "steam": {"pid": 1}}),
    )));
    let warnings = Arc::new(MemoryLogger::default());
    let spawn_world = world.clone();
    let service = LaunchService::new(LaunchServiceOptions {
        delivery: Some(Arc::new(GhostingDelivery(root.text.clone()))),
        processes: Some(Arc::new(GhostingDelivery(root.text.clone()))),
        launch_registry: Some(Arc::new(move |id: &str| {
            (id == "ghosting-renewed").then_some(config)
        })),
        settings_store: Some(Arc::new(GhostingSettings(world.clone()))),
        spawner: Some(Spawns::new(move |_, _, _| {
            let cfg = spawn_world.cfg();
            spawn_world
                .state
                .lock()
                .unwrap()
                .events
                .push(("spawn".to_owned(), cfg));
            Err(RawError::new("STOP_AFTER_PERSONALIZATION"))
        })),
        steam_locator: Some(steam.clone()),
        logger: Some(warnings),
        ..base_options()
    });
    GhostingFixture {
        root,
        world,
        steam,
        service,
    }
}

fn actions(world: &GhostingWorld) -> Vec<String> {
    world
        .state
        .lock()
        .unwrap()
        .events
        .iter()
        .map(|(action, _)| action.clone())
        .collect()
}

/// «Windows: запуск не пишет настройки и не переписывает extras из config.cfg» (0.4.9): раньше
/// запуск сам переносил extras в настройки и переписывал cfg персонализации; теперь файл игрока
/// готовит `prepare_for_launch`.
#[tokio::test]
async fn ghosting_launch_does_not_write_settings_or_extras() {
    let fixture = ghosting_fixture(selftest_ghosting_config());
    let before = fixture.world.state.lock().unwrap().settings.clone();
    let error = fixture
        .service
        .launch("ghosting-renewed")
        .await
        .unwrap_err();
    assert_eq!(error.message, "STOP_AFTER_PERSONALIZATION");
    assert_eq!(actions(&fixture.world), ["spawn"]);
    assert_eq!(fixture.world.state.lock().unwrap().settings, before);
    let cfg = Path::new(&fixture.root.text)
        .join("ghostingmod")
        .join("cfg");
    assert!(
        cfg.join("autoexec.cfg").exists(),
        "файл игрока создан или получен переходом"
    );
    assert_eq!(
        std::fs::read_to_string(cfg.join("config.cfg")).unwrap(),
        "viewmodel_fov \"84\"\ngh_timer \"1\"\ngh_cross_size \"30\"\n"
    );
}
/// «Windows launch restores steam_appid.txt that hl2.exe rewrote to HL2».
#[tokio::test]
async fn ghosting_launch_restores_steam_app_id_file() {
    let fixture = ghosting_fixture(launch_config_of("ghosting-renewed").unwrap());
    let app_id_file = Path::new(&fixture.root.text).join("steam_appid.txt");
    // hl2.exe без -game пишет appid HL2 в том же формате, что и движок (с \0).
    std::fs::write(&app_id_file, b"220\n\0").unwrap();
    fixture
        .service
        .launch("ghosting-renewed")
        .await
        .unwrap_err();

    let files: Value = serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../config/games/ghosting-renewed-1.0.7.files.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let list = files
        .get("files")
        .or_else(|| files.get("entries"))
        .unwrap_or(&files);
    let entry = list
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["path"] == "steam_appid.txt")
        .unwrap();
    let sha = hlsr_delivery::file_ops::hex(&Sha256::digest(std::fs::read(&app_id_file).unwrap()));
    assert_eq!(
        json!(sha),
        entry["sha256"],
        "restored file must pass the integrity check"
    );
}

/// «Windows: без запущенного Steam Ghosting не запускается, а игра не трогается».
#[tokio::test]
async fn ghosting_does_not_launch_without_steam() {
    let fixture = ghosting_fixture(selftest_ghosting_config());
    *fixture.steam.0.lock().unwrap() =
        json!({"ok": false, "code": "STEAM_NOT_RUNNING", "message": "Steam не запущен"});
    let error = fixture
        .service
        .launch("ghosting-renewed")
        .await
        .unwrap_err();
    assert_eq!(error.code, Code::SteamNotRunning);
    assert!(actions(&fixture.world).is_empty(), "ни cfg, ни запуска");
    *fixture.steam.0.lock().unwrap() =
        json!({"ok": false, "code": "STEAM_NOT_INSTALLED", "message": "Steam не найден"});
    let error = fixture
        .service
        .launch("ghosting-renewed")
        .await
        .unwrap_err();
    assert_eq!(error.code, Code::SteamNotInstalled);
}

// ---------------------------------------------------------------- livesplit-overlay-selftest.js

/// `fakeResources(root)`: прокси оверлея с меткой внутри (Windows).
fn overlay_service(root: &Path, resources: &str) -> Arc<LiveSplitOverlay> {
    let dir = root
        .join(resources)
        .join(livesplit_overlay::RESOURCE_DIRECTORY);
    for name in livesplit_overlay::FILES_WINDOWS {
        let mut bytes = format!("{name} ").into_bytes();
        bytes.extend_from_slice(livesplit_overlay::PROXY_MARKER);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    Arc::new(LiveSplitOverlay::new(OverlayOptions {
        base_dir: Some(root.join("data")),
        resources_path: Some(root.join(resources)),
        platform: Some(Platform::Win32),
        ..OverlayOptions::default()
    }))
}

/// «Оверлей LiveSplit на Windows: запуск игры готовит прокси и ничего не запускает».
#[test]
fn overlay_on_windows_prepares_proxy_and_spawns_nothing() {
    let root = Root::new("hlsr-lso-");
    let base = Path::new(&root.text);
    let spawns = Spawns::exiting();
    let service = LaunchService::new(LaunchServiceOptions {
        live_split_overlay: Some(overlay_service(base, "resources")),
        spawner: Some(spawns.clone()),
        ..base_options()
    });
    let install = base.join("game");
    std::fs::create_dir_all(install.join("Half-Life")).unwrap();
    let install_text = install.to_string_lossy().into_owned();
    let mut config = hl1().clone();
    config.executable = "Half-Life/hl.exe";

    assert!(
        service
            .prepare_overlay("hl1-won", &install_text, &config, Some(&json!({})), false)
            .is_none()
    );
    let prepared = service
        .prepare_overlay(
            "hl1-won",
            &install_text,
            &config,
            Some(&json!({"liveSplitOverlay": {"enabled": true}})),
            false,
        )
        .expect("оверлей включён");
    assert_eq!(prepared.proxy, "opengl32.dll");
    assert!(install.join("Half-Life").join("opengl32.dll").exists());
    assert!(spawns.calls().is_empty(), "никакие процессы не запускаются");

    // Сбой подготовки игру не останавливает.
    let broken = LaunchService::new(LaunchServiceOptions {
        live_split_overlay: Some(Arc::new(LiveSplitOverlay::new(OverlayOptions {
            base_dir: Some(base.to_path_buf()),
            resources_path: Some(base.join("nowhere")),
            platform: Some(Platform::Win32),
            ..OverlayOptions::default()
        }))),
        ..base_options()
    });
    assert!(
        broken
            .prepare_overlay(
                "hl1-won",
                &install_text,
                &config,
                Some(&json!({"liveSplitOverlay": {"enabled": true}})),
                false,
            )
            .is_none()
    );
}

// ---------------------------------------------------------------- ghosting-vulkan-selftest.js

struct FixedSettings(Value);

impl LaunchSettings for FixedSettings {
    fn get(&self, _game_id: &str) -> Value {
        self.0.clone()
    }
    fn save(&self, _game_id: &str, _settings: &Value) -> bool {
        true
    }
}

/// `ghostingInstall(root, withBackend)`: папка Ghosting с `bin\` и, по желанию, DXVK из сборки
/// 1.0.13.
fn vulkan_install(root: &Path, with_backend: bool) -> PathBuf {
    let install = root.join("game");
    std::fs::create_dir_all(install.join("bin")).unwrap();
    write_file(
        &install.join("hl2.exe"),
        "test executable; never spawned",
        None,
    );
    write_file(
        &install.join("ghostingmod").join("gameinfo.txt"),
        "\"GameInfo\"\n{\nSteamAppId 243730\n}\n",
        None,
    );
    write_file(&install.join("steam_appid.txt"), "243730\n", None);
    if with_backend {
        write_file(&install.join("backend").join("dxvk_d3d9.dll"), "dxvk", None);
    }
    install
}

struct Launched {
    env: Env,
    argv: Vec<String>,
    warnings: Vec<String>,
}

/// `launchEnv(installPath, settings, options)`: настоящий `launch()` до spawn — окружение, с
/// которым стартовала бы игра.
async fn launch_env(
    install: &Path,
    settings: Value,
    overlay: Option<Arc<LiveSplitOverlay>>,
) -> Launched {
    let logger = Arc::new(MemoryLogger::default());
    let spawns = Spawns::new(|_, _, _| Err(RawError::new("STOP_BEFORE_GAME")));
    let env: Env = [
        ("PATH", "x"),
        ("DXVK_HUD", "fps"),
        ("hlsr_d3d9_backend", "vulkan"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect();
    let config = selftest_ghosting_config();
    let install_text = install.to_string_lossy().into_owned();
    let service = LaunchService::new(LaunchServiceOptions {
        platform: Some(Platform::Win32),
        env: Some(env),
        delivery: Some(Arc::new(GhostingDelivery(install_text.clone()))),
        processes: Some(Arc::new(GhostingDelivery(install_text))),
        launch_registry: Some(Arc::new(move |id: &str| {
            (id == "ghosting-renewed").then_some(config)
        })),
        settings_store: Some(Arc::new(FixedSettings(
            launch_args::sanitize_settings(&settings)
                .settings
                .to_value(),
        ))),
        live_split_overlay: overlay,
        spawner: Some(spawns.clone()),
        steam_locator: Some(Arc::new(GhostingSteam(Mutex::new(
            json!({"ok": true, "code": null, "steam": {"pid": 1}}),
        )))),
        logger: Some(logger.clone()),
        ..base_options()
    });
    let error = service.launch("ghosting-renewed").await.unwrap_err();
    assert_eq!(error.message, "STOP_BEFORE_GAME");
    let mut calls = spawns.calls();
    assert_eq!(calls.len(), 1);
    let (_, argv, request) = calls.remove(0);
    Launched {
        env: request.env.expect("окружение игры"),
        argv,
        warnings: logger
            .take()
            .into_iter()
            .filter(|(level, _)| *level == "warn")
            .map(|(_, message)| message)
            .collect(),
    }
}

fn has_key(env: &Env, name: &str) -> bool {
    env.keys().any(|key| key.eq_ignore_ascii_case(name))
}

#[test]
fn vulkan_switch_only_for_ghosting_on_windows_and_available_by_backend_dll() {
    let root = Root::new("hlsr-vulkan-");
    let base = Path::new(&root.text);
    let without = vulkan_install(&base.join("a"), false);
    let with = vulkan_install(&base.join("b"), true);
    let ghosting = launch_config_of("ghosting-renewed");
    assert_eq!(
        vulkan_backend::availability(Some(&without), ghosting, Platform::Win32),
        Some(false)
    );
    assert_eq!(
        vulkan_backend::availability(Some(&with), ghosting, Platform::Win32),
        Some(true)
    );
    assert_eq!(
        vulkan_backend::availability(None, ghosting, Platform::Win32),
        Some(false),
        "игра не установлена"
    );
    assert_eq!(
        vulkan_backend::availability(Some(&with), ghosting, Platform::Linux),
        None,
        "под Proton d3d9 и так через DXVK"
    );
    for game_id in ["hl2-4260", "hl2-2707", "hl2-5288", "hl1-won"] {
        assert_eq!(
            vulkan_backend::availability(Some(&with), launch_config_of(game_id), Platform::Win32),
            None,
            "{game_id}"
        );
    }
}

/// «Vulkan: запуск с DXVK — окружение как у Play Vulkan.cmd, прокси без оверлея, аргументы
/// прежние».
#[tokio::test]
async fn vulkan_launch_env_like_play_vulkan_cmd_and_proxy_without_overlay() {
    let root = Root::new("hlsr-vulkan-");
    let base = Path::new(&root.text);
    let install = vulkan_install(base, true);
    let overlay = overlay_service(base, "resources");
    let plain = launch_env(&install, json!({}), Some(overlay.clone())).await;
    let launched = launch_env(&install, json!({"vulkan": true}), Some(overlay.clone())).await;
    let env = &launched.env;
    let path = NodePath::native();
    let install_text = install.to_string_lossy();

    assert_eq!(env["HLSR_D3D9_BACKEND"], "vulkan");
    assert!(
        !env.contains_key("hlsr_d3d9_backend"),
        "переменная пользователя другим регистром не остаётся"
    );
    assert_eq!(
        env["DXVK_LOG_PATH"],
        path.join(&[&install_text, "renderer-logs"])
    );
    assert_eq!(
        env["DXVK_SHADER_CACHE_PATH"],
        path.join(&[&install_text, "renderer-cache"])
    );
    assert_eq!(
        env["DXVK_CONFIG_FILE"],
        path.join(&[&install_text, "dxvk.conf"])
    );
    assert_eq!(env["DXVK_LOG_LEVEL"], "info");
    assert!(
        !env.contains_key("DXVK_HUD"),
        "свой DXVK_HUD пользователя стирается, как в Play Vulkan.cmd"
    );
    assert!(install.join("renderer-logs").is_dir());
    assert!(install.join("renderer-cache").is_dir());
    assert_eq!(
        launched.argv, plain.argv,
        "флаги Play Vulkan.cmd не добавляются"
    );
    for flag in ["-windowed", "-condebug", "-novid"] {
        assert!(!launched.argv.iter().any(|arg| arg == flag), "{flag}");
    }

    // Оверлей выключен, а прокси стоит в обеих папках, и overlay.ini его выключает.
    for folder in [install.clone(), install.join("bin")] {
        assert!(
            LiveSplitOverlay::is_own_proxy(&folder.join("d3d9.dll")),
            "{}",
            folder.display()
        );
        assert!(folder.join(livesplit_overlay::SIDECAR).exists());
    }
    let ini_path = overlay
        .dir_for("ghosting-renewed")
        .unwrap()
        .join("overlay.ini");
    assert!(
        std::fs::read_to_string(&ini_path)
            .unwrap()
            .starts_with("enabled=0\r\n")
    );

    // С включённым оверлеем — тот же прокси, оверлей в overlay.ini включён.
    launch_env(
        &install,
        json!({"vulkan": true, "liveSplitOverlay": {"enabled": true}}),
        Some(overlay),
    )
    .await;
    assert!(
        std::fs::read_to_string(&ini_path)
            .unwrap()
            .starts_with("enabled=1\r\n")
    );
}

/// «Vulkan выключен или DXVK нет: DirectX 9 как раньше, прокси без оверлея убирается».
#[tokio::test]
async fn vulkan_off_or_without_dxvk_is_directx9_and_proxy_is_removed() {
    let root = Root::new("hlsr-vulkan-");
    let base = Path::new(&root.text);
    let overlay = overlay_service(base, "resources");
    let install = vulkan_install(base, true);
    launch_env(&install, json!({"vulkan": true}), Some(overlay.clone())).await;
    assert!(install.join("d3d9.dll").exists());

    let off = launch_env(&install, json!({"vulkan": false}), Some(overlay.clone())).await;
    assert!(
        !has_key(&off.env, "HLSR_D3D9_BACKEND"),
        "без переменной прокси берёт системную d3d9.dll"
    );
    assert_eq!(
        off.env["DXVK_HUD"], "fps",
        "чужие DXVK_* без Vulkan не трогаются"
    );
    assert!(!off.env.contains_key("DXVK_LOG_PATH"));
    assert!(
        !install.join("d3d9.dll").exists(),
        "оверлей выключен — прокси убран"
    );
    assert!(!install.join("bin").join("d3d9.dll").exists());

    let old = vulkan_install(&base.join("old"), false);
    let missing = launch_env(&old, json!({"vulkan": true}), Some(overlay)).await;
    assert!(!has_key(&missing.env, "HLSR_D3D9_BACKEND"));
    assert!(
        missing
            .warnings
            .iter()
            .any(|line| line.contains("backend/dxvk_d3d9.dll: запуск на DirectX 9")),
        "{:?}",
        missing.warnings
    );
    assert!(!old.join("d3d9.dll").exists());
    assert!(!old.join("renderer-logs").exists());
}

/// «Vulkan: прокси не встал (нет файлов оверлея) — DirectX 9 и предупреждение, а не Vulkan на словах».
#[tokio::test]
async fn vulkan_without_placed_proxy_is_directx9() {
    let root = Root::new("hlsr-vulkan-");
    let install = vulkan_install(Path::new(&root.text), true);
    let launched = launch_env(&install, json!({"vulkan": true}), None).await;
    assert!(!has_key(&launched.env, "HLSR_D3D9_BACKEND"));
    assert!(!launched.env.contains_key("DXVK_LOG_PATH"));
    assert!(
        launched
            .warnings
            .iter()
            .any(|line| line.contains("прокси d3d9.dll в папке игры нет: запуск на DirectX 9")),
        "{:?}",
        launched.warnings
    );
}

/// «Vulkan: чужую d3d9.dll (DXVK или ReShade пользователя) лаунчер не трогает».
#[tokio::test]
async fn vulkan_leaves_foreign_d3d9_alone() {
    let root = Root::new("hlsr-vulkan-");
    let base = Path::new(&root.text);
    let overlay = overlay_service(base, "resources");
    let install = vulkan_install(base, true);
    write_file(&install.join("bin").join("d3d9.dll"), "reshade", None);
    launch_env(&install, json!({"vulkan": true}), Some(overlay.clone())).await;
    assert_eq!(
        std::fs::read_to_string(install.join("bin").join("d3d9.dll")).unwrap(),
        "reshade"
    );
    assert!(LiveSplitOverlay::is_own_proxy(&install.join("d3d9.dll")));
    launch_env(&install, json!({}), Some(overlay)).await;
    assert_eq!(
        std::fs::read_to_string(install.join("bin").join("d3d9.dll")).unwrap(),
        "reshade"
    );
}
/// «Vulkan: журналы и кэш DXVK не ломают проверку файлов и не идут в .hlsrpack».
#[test]
fn vulkan_dxvk_logs_and_cache_are_mutable_but_dxvk_files_are_not() {
    use hlsr_delivery::mutable_paths::{ENGINE_CACHES, matches_mutable};
    for file in [
        "renderer-logs/hl2_d3d9.log",
        "renderer-logs/hl2_device_fault.bin",
        "renderer-cache/0123456789abcdef.dxvk.bin",
        "renderer-cache/0123456789abcdef.dxvk.lut",
    ] {
        assert!(matches_mutable(file), "{file}");
    }
    assert!(ENGINE_CACHES.contains(&"renderer-cache/**"));
    // Файлы DXVK из сборки проверяются как обычные файлы игры.
    for file in ["backend/dxvk_d3d9.dll", "dxvk.conf", "licenses/DXVK.txt"] {
        assert!(!matches_mutable(file), "{file}");
    }
}
// ---------------------------------------------------------------- окружение и разбор

/// Окружение игры: без переменных Steam (в любом регистре) и без переменных окна лаунчера
/// (решение 3 спецификации); из моста presence — только непустые `HLSR_*`.
#[test]
fn game_environment_drops_steam_and_launcher_window_variables() {
    let env: Env = [
        ("PATH", "C:\\Windows"),
        ("steamappid", "220"),
        ("STEAMGAMEID", "1"),
        ("SteamOverlayGameId", "2"),
        ("SteamAppUser", "me"),
        ("SLINT_SCALE_FACTOR", "1.25"),
        ("WINIT_X11_SCALE_FACTOR", "1.25"),
        ("SLINT_BACKEND", "winit"),
        ("KEEP", "1"),
    ]
    .iter()
    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
    .collect();
    let extra: Env = [
        ("HLSR_PRESENCE_PIPE", "\\\\.\\pipe\\x"),
        ("HLSR_EMPTY", ""),
        ("hlsr_lower", "x"),
        ("HLSR_", "x"),
        ("OTHER", "y"),
        ("HLSR_A-B", "z"),
    ]
    .iter()
    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
    .collect();
    let service = LaunchService::new(LaunchServiceOptions {
        env: Some(env),
        extra_env_provider: Some(Arc::new(move || Some(extra.clone()))),
        ..base_options()
    });
    let built = service.build_environment();
    let keys: Vec<&str> = built.keys().map(String::as_str).collect();
    assert_eq!(keys, ["HLSR_PRESENCE_PIPE", "KEEP", "PATH"]);
    assert_eq!(STRIPPED_ENV.len(), 4);
}

#[test]
fn steam_app_id_text_is_normalized_like_0_4_7() {
    assert_eq!(normalize_app_id_text("243730\n\0"), "243730");
    assert_eq!(normalize_app_id_text("\u{feff}  70\0\0 \r\n"), "70");
    assert_eq!(normalize_app_id_text("2\u{0}20"), "220");
    assert_eq!(normalize_app_id_text(""), "");
}

/// `SteamAppId` в `gameinfo.txt`: флаги `im` регулярного выражения 0.4.7 — строки по `\n`, `\r`,
/// U+2028, U+2029, без учёта регистра ASCII, комментарий после числа.
#[test]
fn gameinfo_app_id_lines_like_the_0_4_7_regex() {
    let root = Root::new("hlsr-gameinfo-");
    let service = LaunchService::new(base_options());
    let config = launch_config_of("hl2-4260").unwrap();
    let cases: [(&str, Result<(), &str>); 8] = [
        ("\"GameInfo\"\r{\r\tsteamappid\t220\r}", Ok(())),
        ("x\u{2028}  STEAMAPPID 220 // hl2\u{2029}", Ok(())),
        ("SteamAppId 220x\nSteamAppId 0220", Ok(())),
        ("SteamAppId220", Err("В gameinfo.txt не найден SteamAppId")),
        (
            "// SteamAppId 220",
            Err("В gameinfo.txt не найден SteamAppId"),
        ),
        (
            "SteamAppId 220 # hl2",
            Err("В gameinfo.txt не найден SteamAppId"),
        ),
        (
            "SteamAppId 380\nSteamAppId 220",
            Err("gameinfo.txt использует SteamAppId 380, ожидался 220"),
        ),
        ("SteamAppId\t00220", Ok(())),
    ];
    for (index, (text, expected)) in cases.iter().enumerate() {
        let file = Path::new(&root.text).join(format!("gameinfo-{index}.txt"));
        write_file(&file, text, None);
        let message = service
            .assert_game_info_app_id(&file.to_string_lossy(), config)
            .err()
            .map(|error| error.message);
        assert_eq!(message.as_deref(), expected.err(), "{text:?}");
    }
}

// ---------------------------------------------------------------- настоящие процессы (Windows)

#[cfg(windows)]
mod real {
    use super::*;
    use hlsr_platform::launch_service::{CpuAffinity, SystemAffinity, SystemSpawner};
    use hlsr_platform::process_inspect::WindowsInspector;
    use hlsr_platform::spawn::ChildExit;

    fn system32(name: &str) -> PathBuf {
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned());
        PathBuf::from(system).join("System32").join(name)
    }

    /// Копия системной программы под именем игры во временной папке.
    fn copy_as(root: &Root, source: &str, name: &str) -> PathBuf {
        let target = Path::new(&root.text).join(name);
        std::fs::copy(system32(source), &target).unwrap();
        target
    }

    fn kill(pid: u32) {
        let _ = std::process::Command::new(system32("taskkill.exe"))
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    fn affinity_of(pid: u32) -> usize {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetProcessAffinityMask, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: свой дочерний процесс; дескриптор закрывается ниже.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            assert!(!handle.is_null());
            let (mut process, mut system) = (0usize, 0usize);
            assert_ne!(GetProcessAffinityMask(handle, &mut process, &mut system), 0);
            CloseHandle(handle);
            process
        }
    }

    struct ExcludeCpu0;

    impl LaunchSettings for ExcludeCpu0 {
        fn get(&self, _game_id: &str) -> Value {
            json!({"excludeCpu0": true})
        }
        fn save(&self, _game_id: &str, _settings: &Value) -> bool {
            false
        }
    }

    /// Свой процесс игры: `spawn` с флагами libuv, ожидание старта с системным инспектором
    /// (путь и время старта из WinAPI), маска ЦП без нулевого процессора.
    #[tokio::test]
    async fn real_game_process_start_and_affinity() {
        let root = Root::new("hlsr-launch-real-");
        let exe = copy_as(&root, "PING.EXE", "hl2.exe");
        let service = LaunchService::new(LaunchServiceOptions {
            inspector: Some(Arc::new(WindowsInspector::system())),
            settings_store: Some(Arc::new(ExcludeCpu0)),
            early_exit_window: Some(Duration::from_millis(300)),
            ..base_options()
        });
        let before = std::time::SystemTime::now();
        let child = SystemSpawner
            .spawn(
                &exe,
                &["-n".to_owned(), "30".to_owned(), "127.0.0.1".to_owned()],
                SpawnRequest {
                    cwd: PathBuf::from(&root.text),
                    env: Some(hlsr_platform::child_env::process_env()),
                    windows_hide: false,
                    detached: true,
                },
            )
            .unwrap();
        let pid = child.pid().unwrap();
        let started = service.wait_for_start(&child, &exe.to_string_lossy()).await;
        let affinity = service.apply_affinity("hl2-4260", pid);
        let actual_affinity = affinity_of(pid);
        kill(pid);
        let end = tokio::time::timeout(Duration::from_secs(10), child.ended())
            .await
            .unwrap();

        let started = started.unwrap();
        assert_eq!(started.pid, pid);
        assert!(
            hlsr_platform::process_inspect::same_path(
                Some(&started.executable_path),
                Some(&exe.to_string_lossy())
            ),
            "{}",
            started.executable_path
        );
        let at = js::date_parse(&started.started_at).unwrap();
        let since = before
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(
            (at - since).abs() < 5000,
            "{} — {since}",
            started.started_at
        );
        let count = SystemAffinity.cpu_count();
        if (2..=64).contains(&count) {
            let mask = affinity.expect("маска поставлена");
            assert_eq!(mask & 1, 0, "нулевой процессор исключён");
            assert_eq!(actual_affinity as u64, mask);
        }
        assert!(matches!(end, ChildExit::Exit { .. }), "{end:?}");
    }

    /// Окружение игры доходит до процесса без переменных окна лаунчера; `cwd` — папка игры.
    #[tokio::test]
    async fn real_spawn_environment_and_cwd() {
        let root = Root::new("hlsr-launch-env-");
        let cmd = copy_as(&root, "cmd.exe", "hl2.exe");
        let mut env = Env::new();
        for key in ["SystemRoot", "PATH", "TEMP"] {
            if let Ok(value) = std::env::var(key) {
                env.insert(key.to_owned(), value);
            }
        }
        env.insert("SLINT_SCALE_FACTOR".to_owned(), "1.25".to_owned());
        env.insert("WINIT_X11_SCALE_FACTOR".to_owned(), "1.25".to_owned());
        env.insert("HLSR_KEEP".to_owned(), "1".to_owned());
        let child = SystemSpawner
            .spawn(
                &cmd,
                &["/c".to_owned(), "set > env.txt".to_owned()],
                SpawnRequest {
                    cwd: PathBuf::from(&root.text),
                    env: Some(env),
                    windows_hide: true,
                    detached: true,
                },
            )
            .unwrap();
        let end = tokio::time::timeout(Duration::from_secs(20), child.ended())
            .await
            .unwrap();
        assert_eq!(
            end,
            ChildExit::Exit {
                code: Some(0),
                signal: None
            }
        );
        let text = std::fs::read_to_string(Path::new(&root.text).join("env.txt")).unwrap();
        assert!(text.contains("HLSR_KEEP=1"), "{text}");
        assert!(!text.contains("SLINT_"), "{text}");
        assert!(!text.contains("WINIT_"), "{text}");
    }

    /// Ошибки `spawn` как у Node: нет файла — событие `error` (`ENOENT`), не программа —
    /// исключение сразу.
    #[tokio::test]
    async fn real_spawn_errors_like_node() {
        let root = Root::new("hlsr-launch-spawn-");
        let missing = Path::new(&root.text).join("missing.exe");
        let child = SystemSpawner
            .spawn(
                &missing,
                &[],
                SpawnRequest {
                    cwd: PathBuf::from(&root.text),
                    ..SpawnRequest::default()
                },
            )
            .unwrap();
        assert_eq!(child.pid(), None);
        match child.ended().await {
            ChildExit::Error(error) => {
                assert_eq!(error.code.as_deref(), Some("ENOENT"));
                assert_eq!(
                    error.message,
                    format!("spawn {} ENOENT", missing.to_string_lossy())
                );
            }
            other => panic!("{other:?}"),
        }

        let fake = Path::new(&root.text).join("fake.exe");
        write_file(&fake, "MZ", None);
        let error = SystemSpawner
            .spawn(
                &fake,
                &[],
                SpawnRequest {
                    cwd: PathBuf::from(&root.text),
                    ..SpawnRequest::default()
                },
            )
            .expect_err("не программа — исключение сразу");
        assert!(error.message.starts_with("spawn "), "{}", error.message);
        assert_eq!(error.path, None);
    }

    /// Маска ЦП: процессоров не меньше одного, чужой (несуществующий) PID — ошибка, не паника.
    #[test]
    fn affinity_of_missing_process_fails() {
        assert!(SystemAffinity.cpu_count() >= 1);
        assert!(SystemAffinity.set_affinity(u32::MAX - 3, 2).is_err());
    }
}
