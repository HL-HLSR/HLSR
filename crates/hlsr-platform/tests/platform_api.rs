//! `PlatformApi` вместе с настоящим `DeliveryApi`: служба доставки создаётся фабрикой платформы,
//! процессы игр — настоящий `GameProcessService` с поддельными процессом игры, инспектором и
//! остановщиком. Сценарии повторяют `game-delivery/index.js` 0.4.7: события процесса → ответы
//! каналов, события окна (`game-process-*`, `steam-*`), статистика, онлайн, Discord, трей,
//! дампы и уведомление `GAME_CRASHED`; Steam (опрос, здоровье, профиль, аватары), перезапуск от
//! администратора (поддельные UAC и замок экземпляра) и смена библиотеки.
//!
//! Часы tokio стоят (`start_paused`): паузы трея, опрос Steam, ответ UAC и задержка выхода идут по
//! виртуальному времени в порядке сроков, а не по скорости машины; задачи платформы выполняются на
//! одном потоке теста, поэтому итог (например, последний статус игры после выхода) не зависит от
//! нагрузки. Исключение — ожидание дампа (`wait_for_dump`): срок у него по настоящим часам, и тесты
//! падений крутят опрос до 0,3 с настоящего времени.

mod support;

use hlsr_core::js::Platform;
use hlsr_core::logger::MemoryLogger;
use hlsr_core::notifications::NotificationStore;
use hlsr_delivery::api::{ApiOptions, ApiPorts, DeliveryApi, DeliveryEvent, Parts, ServiceRequest};
use hlsr_delivery::environment::Environment;
use hlsr_delivery::errors::{Code, DeliveryError, Failure};
use hlsr_delivery::game_actions::LauncherIdentity;
use hlsr_delivery::game_delivery_service::{GameDeliveryService, Provided, ServiceOptions};
use hlsr_delivery::manifests;
use hlsr_delivery::ports::{
    Achievements, FakeFreeSpace, FakeRegistry, Logger, NoDialogs, NoShell, Notifier, PresenceSink,
    Shell, ShortcutLink, SilentNotifier, VersionEventInfo,
};
use hlsr_delivery::providers::BoxFuture;
use hlsr_platform::api::{
    DeliveryPorts, ElevationPorts, GameLauncher, LaunchContext, OnlineHooks, PlatformApi,
    PlatformFactory, PlatformOptions, SteamParts, SteamPort, Timings,
};
use hlsr_platform::elevation::{Elevated, Elevator, InstanceLock};
use hlsr_platform::game_process_service::{
    self, ChildEnd, ChildHandle, GameProcessService, StartedProcess,
};
use hlsr_platform::launch_service::LaunchDelivery;
use hlsr_platform::process_inspect::ProcessInfo;
use hlsr_platform::steam_fs::MemoryFs;
use hlsr_platform::tool_runner::{ToolProcessRunner, ToolRunnerOptions};
use hlsr_platform::tray_controller::TrayWindow;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::world::{FakeInspector, FakeStopper, Shared, World, world};
use tokio::sync::{broadcast, oneshot};

const GHOSTING: &str = "ghosting-renewed";

// ---------------------------------------------------------------- подделки

/// Что делали порты: строки по порядку.
#[derive(Default)]
struct Effects(Mutex<Vec<String>>);

impl Effects {
    fn push(&self, line: impl Into<String>) {
        self.0.lock().unwrap().push(line.into());
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    fn snapshot(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

struct Online {
    effects: Arc<Effects>,
    server_stats: Mutex<Option<Value>>,
}

impl OnlineHooks for Online {
    fn game_starting(&self, game_id: &str, installed: Option<&Value>) {
        let version = installed
            .and_then(|installed| installed.get("version"))
            .cloned()
            .unwrap_or(Value::Null);
        self.effects
            .push(format!("online.starting {game_id} {version}"));
    }

    fn game_launch_failed(&self, game_id: &str, error: &Value) {
        self.effects
            .push(format!("online.launch-failed {game_id} {}", error["code"]));
    }

    fn game_running(&self, game_id: &str) {
        self.effects.push(format!("online.running {game_id}"));
    }

    fn game_finished(&self, game_id: &str, last_exit: Option<&Value>, crashed: bool) {
        let outcome = last_exit.map_or(Value::Null, |exit| exit["outcome"].clone());
        self.effects.push(format!(
            "online.finished {game_id} {outcome} crashed={crashed}"
        ));
    }

    fn game_stats_from_server(&self, _game_id: &str) -> Option<Value> {
        self.server_stats.lock().unwrap().clone()
    }
}

struct Presence(Arc<Effects>);

impl PresenceSink for Presence {
    fn signal(&self, signal: &Value) -> Result<(), String> {
        self.0.push(format!("presence {signal}"));
        Ok(())
    }
}

struct Achieve(Arc<Effects>);

impl Achievements for Achieve {
    fn achieve(&self, id: &str) {
        self.0.push(format!("achieve {id}"));
    }
}

struct FakeShell(Arc<Effects>);

impl Shell for FakeShell {
    fn open_path<'a>(&'a self, _path: &'a Path) -> BoxFuture<'a, String> {
        Box::pin(async { String::new() })
    }

    fn open_external(&self, url: &str) -> Result<(), String> {
        self.0.push(format!("open-external {url}"));
        Ok(())
    }

    fn show_item_in_folder(&self, path: &Path) {
        self.0.push(format!("show-item {}", path.display()));
    }

    fn write_shortcut_link(&self, path: &Path, link: &ShortcutLink) -> bool {
        NoShell.write_shortcut_link(path, link)
    }

    fn read_shortcut_link(&self, path: &Path) -> Result<ShortcutLink, String> {
        NoShell.read_shortcut_link(path)
    }
}

struct DeliveryNotifier(Arc<Effects>);

impl Notifier for DeliveryNotifier {
    fn notify(&self, code: &str, _detail: &str) {
        self.0.push(format!("notify {code}"));
    }

    fn resolve_update_offers(
        &self,
        target: &str,
        installed_version: Option<&str>,
        game_id: &str,
    ) -> Result<(), String> {
        SilentNotifier.resolve_update_offers(target, installed_version, game_id)
    }

    fn game_update_available(
        &self,
        version: &str,
        current_version: Option<&str>,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String> {
        SilentNotifier.game_update_available(version, current_version, game_id, info)
    }

    fn game_installed(
        &self,
        version: &str,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String> {
        SilentNotifier.game_installed(version, game_id, info)
    }

    fn game_updated(
        &self,
        from_version: Option<&str>,
        to_version: &str,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String> {
        SilentNotifier.game_updated(from_version, to_version, game_id, info)
    }
}

struct Tray(Arc<Effects>);

impl TrayWindow for Tray {
    fn available(&self) -> bool {
        true
    }

    fn hide(&self) -> bool {
        self.0.push("tray.hide");
        true
    }

    fn show(&self) {
        self.0.push("tray.show");
    }

    fn destroy_tray(&self) {
        self.0.push("tray.destroy");
    }
}

#[derive(Default)]
struct FakeSteam {
    account: Mutex<Value>,
    steam_only: Mutex<Value>,
    install: Mutex<Option<String>>,
    account_id: Mutex<Option<u64>>,
    refreshes: AtomicUsize,
}

impl SteamPort for FakeSteam {
    fn refresh_connection(&self) -> String {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        "connected".to_owned()
    }

    fn describe(&self) -> Value {
        self.account.lock().unwrap().clone()
    }

    fn install_path(&self) -> Option<String> {
        self.install.lock().unwrap().clone()
    }

    fn active_account_id(&self) -> Option<u64> {
        *self.account_id.lock().unwrap()
    }

    fn describe_steam_only(&self) -> Value {
        self.steam_only.lock().unwrap().clone()
    }
}

enum Plan {
    Start {
        pid: u32,
        end: oneshot::Receiver<ChildEnd>,
    },
    Fail(DeliveryError),
}

type Plans = Arc<Mutex<VecDeque<Plan>>>;

struct Launcher {
    delivery: Arc<dyn LaunchDelivery>,
    processes: GameProcessService,
    plans: Plans,
    availability: Arc<Mutex<Value>>,
    hint: Arc<Mutex<Option<String>>>,
}

impl GameLauncher for Launcher {
    fn launch<'a>(
        &'a self,
        game_id: &'a str,
        episode: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Value, DeliveryError>> {
        Box::pin(async move {
            let plan = self
                .plans
                .lock()
                .unwrap()
                .pop_front()
                .expect("план запуска");
            match plan {
                Plan::Fail(error) => Err(error),
                Plan::Start { pid, end } => {
                    self.processes.mark_starting(game_id);
                    let installed = self.delivery.installed_of(game_id)?;
                    let install = installed["installPath"].as_str().unwrap().to_owned();
                    let exe = Path::new(&install)
                        .join("hl2.exe")
                        .to_string_lossy()
                        .into_owned();
                    let state = self.delivery.register_started_process(
                        game_id,
                        StartedProcess {
                            pid: Some(i64::from(pid)),
                            executable_path: Some(exe),
                            child: Some(ChildHandle {
                                pid,
                                wait: Box::pin(async move {
                                    end.await.unwrap_or(ChildEnd::Exit {
                                        code: Some(0),
                                        signal: None,
                                    })
                                }),
                            }),
                            episode: episode.map(str::to_owned),
                            ..StartedProcess::default()
                        },
                    )?;
                    Ok(json!({"status": "started", "process": state}))
                }
            }
        })
    }

    fn describe_availability(&self, _game_id: &str) -> Value {
        self.availability.lock().unwrap().clone()
    }

    fn dep_crash_hint(&self, _exit_code: Option<i64>) -> Option<String> {
        self.hint.lock().unwrap().clone()
    }
}

struct Factory {
    steam: Arc<FakeSteam>,
    world: Shared,
    plans: Plans,
    availability: Arc<Mutex<Value>>,
    hint: Arc<Mutex<Option<String>>>,
    logger: Arc<dyn Logger>,
    created: Arc<Mutex<Vec<GameProcessService>>>,
    /// Служба доставки не создаётся (библиотека недоступна): `delivery` отвечает ошибкой.
    fail_delivery: Arc<AtomicBool>,
}

impl PlatformFactory for Factory {
    fn steam(&self) -> SteamParts {
        SteamParts {
            account: Some(self.steam.clone()),
            ..SteamParts::default()
        }
    }

    fn processes(&self, state_dir: &Path) -> GameProcessService {
        let service = GameProcessService::new(game_process_service::ServiceOptions {
            state_dir: state_dir.to_path_buf(),
            logger: Some(self.logger.clone()),
            inspector: Some(Arc::new(FakeInspector(self.world.clone()))),
            stopper: Some(Arc::new(FakeStopper(self.world.clone()))),
            poll_interval_ms: Some(60 * 60 * 1000),
            ..game_process_service::ServiceOptions::default()
        });
        self.created.lock().unwrap().push(service.clone());
        service
    }

    fn tool_runner(&self) -> ToolProcessRunner {
        ToolProcessRunner::new(ToolRunnerOptions::default())
    }

    fn delivery(&self, request: ServiceRequest, ports: DeliveryPorts) -> Result<Parts, Failure> {
        if self.fail_delivery.load(Ordering::SeqCst) {
            return Err(hlsr_delivery::errors::RawError::new("библиотека недоступна").into());
        }
        let (probe, stopper) = ports.process_probe();
        let service = GameDeliveryService::new(ServiceOptions {
            library_path: request.library_path,
            logger: Some(self.logger.clone()),
            env: Some(Environment::new(Vec::<(String, String)>::new(), false)),
            launcher_version: Some("0.5.0".to_owned()),
            registry: request.registry,
            processes: Some(probe),
            process_stopper: Some(stopper),
            channel_service: Provided::Off,
            origin_updates: Provided::Off,
            notifier: Some(Arc::new(SilentNotifier)),
            release_notes: request.release_notes,
            free_space: Some(Arc::new(FakeFreeSpace::default())),
            ..ServiceOptions::default()
        })?;
        Ok(Parts {
            service,
            tools: None,
            practice_saves: None,
        })
    }

    fn launcher(&self, context: LaunchContext) -> Arc<dyn GameLauncher> {
        Arc::new(Launcher {
            delivery: context.delivery,
            processes: context.processes,
            plans: self.plans.clone(),
            availability: self.availability.clone(),
            hint: self.hint.clone(),
        })
    }
}

/// Запросы аватаров: сети в тестах нет — каждый запрос пуст и записан.
struct NoNet(Arc<Effects>);

impl hlsr_platform::profile::steam_avatars::HttpGet for NoNet {
    fn get<'a>(
        &'a self,
        url: &'a str,
        _options: hlsr_platform::profile::steam_avatars::GetOptions,
    ) -> BoxFuture<'a, Option<Vec<u8>>> {
        self.0.push(format!("get {url}"));
        Box::pin(async { None })
    }
}

struct FakeElevator {
    result: Mutex<Option<Result<Elevated, String>>>,
    delay: Duration,
    effects: Arc<Effects>,
}

impl Elevator for FakeElevator {
    fn run_as_admin(&self) -> BoxFuture<'static, Result<Elevated, String>> {
        self.effects.push("uac");
        let result = self.result.lock().unwrap().take().expect("ответ UAC");
        let delay = self.delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            result
        })
    }
}

struct FakeLock(Arc<Effects>);

impl InstanceLock for FakeLock {
    fn release(&self) -> Result<(), String> {
        self.0.push("lock.release");
        Ok(())
    }

    fn reacquire(&self) {
        self.0.push("lock.reacquire");
    }
}

// ---------------------------------------------------------------- сборка

struct Harness {
    _root: tempfile::TempDir,
    root: PathBuf,
    lib: PathBuf,
    delivery: Arc<DeliveryApi>,
    platform: Arc<PlatformApi>,
    events: broadcast::Receiver<DeliveryEvent>,
    /// Только для ожидания выхода игры.
    exits: Mutex<broadcast::Receiver<DeliveryEvent>>,
    delivery_events: broadcast::Receiver<DeliveryEvent>,
    effects: Arc<Effects>,
    log: Arc<MemoryLogger>,
    steam: Arc<FakeSteam>,
    world: Shared,
    plans: Plans,
    availability: Arc<Mutex<Value>>,
    hint: Arc<Mutex<Option<String>>>,
    notifications: Arc<NotificationStore>,
    online: Arc<Online>,
    created: Arc<Mutex<Vec<GameProcessService>>>,
    steam_fs: Arc<MemoryFs>,
    fail_delivery: Arc<AtomicBool>,
}

#[derive(Default)]
struct Setup {
    timings: Option<Timings>,
    elevation: Option<(Option<Result<Elevated, String>>, Duration)>,
    with_tray: bool,
}

/// Установленная игра: `<библиотека>/games/<id>/installed.json` (`schemaVersion` 1, `ready`).
fn install_game(lib: &Path, game_id: &str) -> PathBuf {
    let dir = game_dir(lib, game_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("installed.json"),
        json!({"schemaVersion": 1, "gameId": game_id, "version": "1.2.3", "state": "ready"})
            .to_string(),
    )
    .unwrap();
    dir
}

/// Папка игры в библиотеке (`<библиотека>/Games/<id>` на Windows).
fn game_dir(lib: &Path, game_id: &str) -> PathBuf {
    hlsr_delivery::paths::resolve_library(&lib.to_string_lossy())
        .unwrap()
        .games
        .join(game_id)
}

/// Папка записей процессов (`.hlsr/state`).
fn state_dir(lib: &Path) -> PathBuf {
    hlsr_delivery::paths::resolve_library(&lib.to_string_lossy())
        .unwrap()
        .state
}

fn quiet_timings() -> Timings {
    Timings {
        // Опрос Steam — только там, где его проверяют.
        steam_first: Duration::from_secs(3600),
        dump_settle_ms: Some(300),
        dump_poll_ms: Some(30),
        tray_settle: Some(Duration::from_millis(50)),
        ..Timings::default()
    }
}

fn harness(setup: Setup) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let lib = root.join("lib");
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    // В библиотеке есть игра: иначе фасад искал бы игры в папке по умолчанию.
    install_game(&lib, GHOSTING);

    let effects = Arc::new(Effects::default());
    let log = Arc::new(MemoryLogger::default());
    let logger: Arc<dyn Logger> = log.clone();
    let steam = Arc::new(FakeSteam::default());
    *steam.account.lock().unwrap() = json!({
        "signedIn": true, "steamId64": "76561198000000001", "steamId2": "STEAM_0:1:19867136",
        "personaName": "Игрок", "avatarDataUri": null, "connection": "connected",
    });
    let world: Shared = Arc::new(Mutex::new(World::default()));
    let steam_fs = Arc::new(MemoryFs::new(Platform::Win32));
    let plans: Plans = Arc::default();
    let availability = Arc::new(Mutex::new(json!({"ok": true, "code": null})));
    let hint = Arc::new(Mutex::new(None));
    let created = Arc::new(Mutex::new(Vec::new()));
    let fail_delivery = Arc::new(AtomicBool::new(false));
    let notifications = Arc::new(NotificationStore::new(data.join("notifications.json")));
    let online = Arc::new(Online {
        effects: effects.clone(),
        server_stats: Mutex::new(None),
    });
    let env = Environment::new(
        [
            (
                "LOCALAPPDATA",
                root.join("local").to_string_lossy().into_owned(),
            ),
            (
                "XDG_DATA_HOME",
                root.join("xdg").to_string_lossy().into_owned(),
            ),
        ],
        false,
    );

    let factory = Arc::new(Factory {
        steam: steam.clone(),
        world: world.clone(),
        plans: plans.clone(),
        availability: availability.clone(),
        hint: hint.clone(),
        logger: logger.clone(),
        created: created.clone(),
        fail_delivery: fail_delivery.clone(),
    });
    let mut options = PlatformOptions::new(factory);
    options.logger = logger.clone();
    options.user_data_path = Some(data.clone());
    options.env = env.clone();
    options.platform = Platform::Win32;
    options.notifications = Some(notifications.clone());
    options.online = online.clone();
    options.achievements = Some(Arc::new(Achieve(effects.clone())));
    options.presence = Some(Arc::new(Presence(effects.clone())));
    options.shell = Arc::new(FakeShell(effects.clone()));
    options.tray_window = setup
        .with_tray
        .then(|| Arc::new(Tray(effects.clone())) as Arc<dyn TrayWindow>);
    options.timings = setup.timings.unwrap_or_else(quiet_timings);
    options.avatar_get = Some(Arc::new(NoNet(effects.clone())));
    options.steam_fs = steam_fs.clone();
    if let Some((result, delay)) = setup.elevation {
        options.elevation = Some(ElevationPorts {
            elevator: Arc::new(FakeElevator {
                result: Mutex::new(result),
                delay,
                effects: effects.clone(),
            }),
            lock: Arc::new(FakeLock(effects.clone())),
            quit: {
                let effects = effects.clone();
                Arc::new(move || effects.push("quit"))
            },
        });
    }
    let platform = PlatformApi::new(options);

    let delivery = DeliveryApi::new(ApiOptions {
        library_path: lib.to_string_lossy().into_owned(),
        launcher_version: "0.5.0".to_owned(),
        user_data_path: Some(data.clone()),
        env,
        desktop: root.join("Desktop"),
        launcher: LauncherIdentity {
            exec_path: root.join("HLSR.exe"),
            app_image: None,
            packaged: true,
            app_path: None,
        },
        ports: ApiPorts {
            logger: logger.clone(),
            notifier: Arc::new(DeliveryNotifier(effects.clone())),
            telemetry: None,
            achievements: None,
            presence: None,
            overlay: None,
            dialogs: Arc::new(NoDialogs),
            shell: Arc::new(NoShell),
            registry: Arc::new(FakeRegistry::default()),
            free_space: Arc::new(FakeFreeSpace::default()),
            proton_prefix: None,
        },
        clock: None,
        factory: Some(platform.service_factory()),
        download_settings: None,
        library_store: None,
        schedule_updates: false,
    });
    platform.bind_delivery(&delivery);
    Harness {
        _root: dir,
        root,
        lib,
        events: platform.subscribe(),
        exits: Mutex::new(platform.subscribe()),
        delivery_events: delivery.subscribe(),
        delivery,
        platform,
        effects,
        log,
        steam,
        world,
        plans,
        availability,
        hint,
        notifications,
        online,
        created,
        steam_fs,
        fail_delivery,
    }
}

fn drain(events: &mut broadcast::Receiver<DeliveryEvent>) -> Vec<DeliveryEvent> {
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        out.push(event);
    }
    out
}

fn channels_of(events: &[DeliveryEvent]) -> Vec<&'static str> {
    events.iter().map(|event| event.channel).collect()
}

/// Дождаться условия, а не времени: шаг — 10 мс часов tokio (в тестах они на паузе и стоят,
/// пока все задачи не упрутся в ожидание), предел — 30 с, чтобы сорванный тест не завис.
async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..3000 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("не дождались: {what}");
}

/// Статусы игры в доставке до итогового — «не отслеживается» с `lastExit`. Событие `state`
/// обработчика выхода приходит уже после `game-process-exited`, поэтому слить очередь сразу
/// после выхода нельзя: итоговый статус ждём как условие.
async fn statuses_until_not_running(
    events: &mut broadcast::Receiver<DeliveryEvent>,
    game_id: &str,
) -> Vec<Value> {
    let mut statuses: Vec<Value> = Vec::new();
    for _ in 0..3000 {
        statuses.extend(
            drain(events)
                .into_iter()
                .filter(|event| event.channel == "game-delivery-status")
                .map(|event| event.payload)
                .filter(|status| status["gameId"] == game_id),
        );
        if statuses.last().is_some_and(|status| {
            status["process"]["state"] == "not-running" && !status["process"]["lastExit"].is_null()
        }) {
            return statuses;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("нет итогового статуса игры {game_id}: {statuses:?}");
}

impl Harness {
    /// Запуск игры каналом окна; процесс игры ждёт `end`.
    async fn launch(&self, game_id: &str, pid: u32) -> (Value, oneshot::Sender<ChildEnd>) {
        let (end, receiver) = oneshot::channel();
        self.plans
            .lock()
            .unwrap()
            .push_back(Plan::Start { pid, end: receiver });
        let response = self
            .platform
            .call("game-process-launch", &[json!(game_id)])
            .await
            .unwrap()
            .unwrap();
        (response, end)
    }

    /// Дождаться события выхода игры (`game-process-exited`/`-crashed` — последний шаг
    /// обработчика `finished`) и фоновой работы платформы и доставки.
    async fn wait_exit(&self, game_id: &str) {
        for _ in 0..1000 {
            let seen = {
                let mut exits = self.exits.lock().unwrap();
                let mut seen = false;
                while let Ok(event) = exits.try_recv() {
                    if matches!(
                        event.channel,
                        "game-process-exited" | "game-process-crashed"
                    ) && event.payload["gameId"] == game_id
                    {
                        seen = true;
                    }
                }
                seen
            };
            if seen {
                self.platform.settle().await;
                self.delivery.settle().await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("игра {game_id} не завершилась");
    }

    fn crashes(&self) -> Vec<Value> {
        self.notifications
            .list()
            .0
            .into_iter()
            .filter(|item| item["code"] == "GAME_CRASHED")
            .collect()
    }

    fn log_lines(&self) -> Vec<(&'static str, String)> {
        self.log.take()
    }
}

fn exit(code: i64) -> ChildEnd {
    ChildEnd::Exit {
        code: Some(code),
        signal: None,
    }
}

// ---------------------------------------------------------------- сценарии

/// Запуск и штатный выход: `game-process-running` (трей прячет окно, сессия статистики, онлайн,
/// Discord `running`), выход — трей, Steam, Discord `stopped`, статистика, онлайн,
/// `game-process-exited` и окно возвращается.
#[tokio::test(start_paused = true)]
async fn launch_and_normal_exit_like_0_4_7() {
    let h = harness(Setup {
        with_tray: true,
        ..Setup::default()
    });
    let (response, end) = h.launch(GHOSTING, 4242).await;
    assert_eq!(response["status"], "started", "{response}");
    assert_eq!(response["process"]["state"], "running");
    assert_eq!(response["process"]["gameId"], GHOSTING);

    let effects = h.effects.take();
    assert_eq!(
        effects,
        [
            "online.starting ghosting-renewed \"1.2.3\"",
            "online.running ghosting-renewed",
            "tray.hide",
            &format!(
                "presence {}",
                json!({"type": "running", "gameId": GHOSTING, "episode": null})
            ),
        ]
    );
    assert!(h.platform.tray().unwrap().is_hidden());
    assert!(h.platform.is_any_game_running());

    let refreshes_before = h.steam.refreshes.load(Ordering::SeqCst);
    end.send(exit(0)).unwrap();
    h.wait_exit(GHOSTING).await;
    // Окно возвращается после паузы трея: ждём возврата окна, а не времени.
    wait_until("возврат окна из трея", || {
        h.effects.snapshot().iter().any(|line| line == "tray.show")
    })
    .await;

    let effects = h.effects.take();
    assert_eq!(
        effects,
        [
            &format!(
                "presence {}",
                json!({"type": "stopped", "gameId": GHOSTING})
            ),
            "online.finished ghosting-renewed \"exited\" crashed=false",
            "tray.show",
        ]
    );
    assert_eq!(
        h.steam.refreshes.load(Ordering::SeqCst),
        refreshes_before + 1
    );
    assert!(!h.platform.tray().unwrap().is_hidden());
    assert!(h.crashes().is_empty());

    let stats = h.platform.game_stats_get(&json!(GHOSTING));
    assert_eq!(stats["available"], true, "{stats}");
    assert_eq!(stats["launchCount"], 1);
    assert_eq!(stats["sessionActive"], false);
}

/// События окна платформы по порядку: `game-process-running`, `steam-account`,
/// `game-process-exited` с полями `lastExit`.
#[tokio::test(start_paused = true)]
async fn window_events_follow_the_process() {
    let mut h = harness(Setup::default());
    let (_, end) = h.launch(GHOSTING, 77).await;
    end.send(exit(0)).unwrap();
    h.wait_exit(GHOSTING).await;

    let events = drain(&mut h.events);
    assert_eq!(
        channels_of(&events),
        [
            "game-process-running",
            "steam-account",
            "game-process-exited"
        ]
    );
    let running = &events[0].payload;
    assert_eq!(running["gameId"], GHOSTING);
    assert_eq!(running["state"], "running");
    assert_eq!(running["running"], true);
    assert_eq!(events[1].payload["personaName"], "Игрок");
    let exited = &events[2].payload;
    let keys: Vec<&str> = exited
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["gameId", "outcome", "exitCode", "signal", "requested", "at"]
    );
    assert_eq!(exited["outcome"], "exited");
    assert_eq!(exited["exitCode"], 0);
    assert_eq!(exited["signal"], Value::Null);
    assert_eq!(exited["requested"], false);

    // Статус игры в доставке видел процесс: `running` и выход.
    let statuses = statuses_until_not_running(&mut h.delivery_events, GHOSTING).await;
    let states: Vec<Value> = statuses
        .iter()
        .map(|status| status["process"]["state"].clone())
        .collect();
    assert!(states.contains(&json!("running")), "{states:?}");
    assert!(states.contains(&json!("exited")), "{states:?}");
    // Последний статус — игра не отслеживается, `lastExit` — её выход.
    let last = &statuses.last().unwrap()["process"];
    assert_eq!(last["state"], "not-running", "{last}");
    assert_eq!(last["lastExit"]["outcome"], "exited", "{last}");
}

/// Падение без дампа: `game-process-crashed`, уведомление `GAME_CRASHED` с `meta`, журнал,
/// сведения о падении и «Показать дамп» — `NO_DUMP`.
#[tokio::test(start_paused = true)]
async fn crash_without_dump() {
    let mut h = harness(Setup::default());
    let (_, end) = h.launch(GHOSTING, 4243).await;
    h.log_lines();
    end.send(exit(3_221_225_477)).unwrap();
    h.wait_exit(GHOSTING).await;

    let events = drain(&mut h.events);
    let crashed = events
        .iter()
        .find(|event| event.channel == "game-process-crashed")
        .expect("событие падения");
    assert_eq!(crashed.payload["outcome"], "crashed");
    assert_eq!(crashed.payload["exitCode"], 3_221_225_477_i64);
    // Окно о падении 0.4.9 — после ожидания дампа.
    let report = events
        .iter()
        .find(|event| event.channel == "game-crash-report")
        .expect("окно о падении");
    assert_eq!(
        report.payload,
        json!({"gameId": GHOSTING, "launcherId": manifests::launcher_id_of(GHOSTING),
               "exitCode": 3_221_225_477_i64, "hasDump": false, "archive": false,
               "file": null, "hint": null})
    );
    assert!(
        !channels_of(&events).contains(&"game-process-exited"),
        "{:?}",
        channels_of(&events)
    );
    assert!(
        h.effects
            .snapshot()
            .contains(&"online.finished ghosting-renewed \"crashed\" crashed=true".to_owned())
    );

    let crashes = h.crashes();
    assert_eq!(crashes.len(), 1);
    assert_eq!(crashes[0]["level"], "error");
    assert_eq!(crashes[0]["detail"], "код выхода 3221225477");
    assert_eq!(
        crashes[0]["meta"],
        json!({"hasDump": false, "exitCode": 3_221_225_477_i64, "gameId": GHOSTING,
               "launcherId": manifests::launcher_id_of(GHOSTING)})
    );
    let lines = h.log_lines();
    assert!(
        lines.contains(&(
            "warn",
            "[crash] crash dump не найден в разрешённых каталогах".to_owned()
        )),
        "{lines:?}"
    );

    let info = h
        .platform
        .call("game-crash-info", &[json!(GHOSTING)])
        .await
        .unwrap()
        .unwrap();
    let keys: Vec<&str> = info
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "at",
            "exitCode",
            "dumpPath",
            "dumpSize",
            "logPath",
            "archivePath",
            "installPath",
            "hint"
        ]
    );
    assert_eq!(info["exitCode"], 3_221_225_477_i64);
    assert_eq!(info["dumpPath"], Value::Null);
    // Без игры — последнее падение; неизвестная игра — тоже.
    assert_eq!(h.platform.game_crash_info(None), info);
    assert_eq!(h.platform.game_crash_info(Some(&json!(""))), info);
    assert_eq!(h.platform.game_crash_info(Some(&json!("nope"))), info);
    assert_eq!(
        h.platform.game_crash_info(Some(&json!("hl1-won"))),
        Value::Null
    );
    assert_eq!(
        h.platform.game_crash_open_dump(Some(&json!(GHOSTING))),
        json!({"status": "error", "error": {"code": "NO_DUMP"}})
    );
}

/// Игра вышла с кодом 0, но оставила свежий дамп в папке мода (Proton): это падение — событие,
/// `lastExit` процесса, уведомление с дампом; «Показать дамп» открывает его в проводнике.
#[tokio::test(start_paused = true)]
async fn exit_zero_with_fresh_dump_counts_as_crash() {
    let mut h = harness(Setup::default());
    let game_dir = game_dir(&h.lib, GHOSTING);
    let (_, end) = h.launch(GHOSTING, 4244).await;
    h.log_lines();
    std::fs::create_dir_all(game_dir.join("ghostingmod")).unwrap();
    let dump = game_dir.join("ghostingmod").join("crash_1.mdmp");
    std::fs::write(&dump, vec![7u8; 64]).unwrap();
    end.send(exit(0)).unwrap();
    h.wait_exit(GHOSTING).await;

    let lines = h.log_lines();
    assert!(
        lines.contains(&(
            "warn",
            "[game-process] ghosting-renewed вышла с кодом 0, но оставила дамп — считаем падением"
                .to_owned()
        )),
        "{lines:?}"
    );
    let events = drain(&mut h.events);
    let crashed = events
        .iter()
        .find(|event| event.channel == "game-process-crashed")
        .expect("событие падения");
    assert_eq!(crashed.payload["outcome"], "crashed");
    assert_eq!(crashed.payload["exitCode"], 0);
    let report = events
        .iter()
        .find(|event| event.channel == "game-crash-report")
        .expect("окно о падении");
    assert_eq!(report.payload["hasDump"], true);
    assert_eq!(report.payload["file"], json!(dump.to_string_lossy()));
    let process = h.platform.processes().unwrap().public_state(GHOSTING);
    assert_eq!(process["lastExit"]["outcome"], "crashed", "{process}");

    let crashes = h.crashes();
    assert_eq!(crashes.len(), 1);
    assert_eq!(crashes[0]["detail"], "код выхода 0");
    assert_eq!(crashes[0]["meta"]["hasDump"], true);
    let info = h.platform.game_crash_info(Some(&json!(GHOSTING)));
    assert_eq!(info["dumpSize"], 64);
    assert_eq!(info["dumpPath"], json!(dump.to_string_lossy()));

    assert_eq!(
        h.platform.game_crash_open_dump(Some(&json!(GHOSTING))),
        json!({"status": "ok"})
    );
    assert!(
        h.effects
            .take()
            .contains(&format!("show-item {}", dump.display()))
    );
    // Дамп убрали — путь больше не проходит проверку.
    std::fs::remove_file(&dump).unwrap();
    assert_eq!(
        h.platform.game_crash_open_dump(Some(&json!(GHOSTING))),
        json!({"status": "error", "error": {"code": "PATH_INVALID"}})
    );
}

/// Подсказка DEP — только у GoldSrc: у Ghosting её нет, даже если запуск её знает.
#[tokio::test(start_paused = true)]
async fn dep_hint_only_for_goldsrc() {
    let h = harness(Setup::default());
    *h.hint.lock().unwrap() = Some("DEP_ALWAYS_ON".to_owned());
    let (_, end) = h.launch(GHOSTING, 5000).await;
    end.send(exit(1)).unwrap();
    h.wait_exit(GHOSTING).await;
    assert_eq!(
        h.platform.game_crash_info(Some(&json!(GHOSTING)))["hint"],
        Value::Null
    );

    install_game(&h.lib, "hl1-won");
    let (_, end) = h.launch("hl1-won", 5001).await;
    h.log_lines();
    end.send(exit(3_221_225_477)).unwrap();
    h.wait_exit("hl1-won").await;
    let info = h.platform.game_crash_info(Some(&json!("hl1-won")));
    assert_eq!(info["hint"], "DEP_ALWAYS_ON");
    let lines = h.log_lines();
    assert!(lines.contains(&(
        "warn",
        "[crash] hl1-won: вероятная причина — DEP_ALWAYS_ON".to_owned()
    )));
    let crash = h
        .crashes()
        .into_iter()
        .find(|item| item["meta"]["gameId"] == "hl1-won")
        .unwrap();
    assert_eq!(crash["meta"]["hint"], "DEP_ALWAYS_ON");
    assert_eq!(crash["meta"]["launcherId"], "70");
}

/// Ошибки запуска: неизвестная игра, отказ запуска (уведомление, онлайн), игра уже идёт.
#[tokio::test(start_paused = true)]
async fn launch_errors_like_0_4_7() {
    let h = harness(Setup::default());
    let unknown = h
        .platform
        .call("game-process-launch", &[json!("nope")])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unknown["status"], "error");
    assert_eq!(unknown["error"]["code"], "INSTALL_FAILED");
    assert_eq!(unknown["error"]["message"], "Неизвестная игра: nope");

    h.effects.take();
    h.plans
        .lock()
        .unwrap()
        .push_back(Plan::Fail(DeliveryError::new(
            Code::SteamNotRunning,
            "Steam не запущен",
        )));
    let failed = h
        .platform
        .call("game-process-launch", &[json!(GHOSTING)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed["status"], "error");
    assert_eq!(failed["error"]["code"], "STEAM_NOT_RUNNING");
    assert_eq!(
        h.effects.take(),
        [
            "online.starting ghosting-renewed \"1.2.3\"",
            "online.launch-failed ghosting-renewed \"STEAM_NOT_RUNNING\"",
            "notify STEAM_NOT_RUNNING",
        ]
    );

    // По id лаунчера (218 — Ghosting) и уже идущая игра.
    let (started, end) = h.launch("218", 6000).await;
    assert_eq!(started["status"], "started");
    let again = h
        .platform
        .call("game-process-launch", &[json!(GHOSTING)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again, json!({"status": "already-running"}));
    end.send(exit(0)).unwrap();
    h.wait_exit(GHOSTING).await;
}

/// `game-process-stop`: игра не запущена; запущена — остановка по просьбе пользователя
/// (`requested`), не падение.
#[tokio::test(start_paused = true)]
async fn stop_game_like_0_4_7() {
    let mut h = harness(Setup::default());
    let none = h
        .platform
        .call("game-process-stop", &[json!(GHOSTING)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(none, json!({"status": "not-running"}));

    let (started, end) = h.launch(GHOSTING, 7000).await;
    let process = &started["process"];
    {
        let mut world = world(&h.world);
        world.alive.insert(7000);
        world.kills_on_stop = true;
        world.inspect.insert(
            7000,
            Some(ProcessInfo {
                pid: 7000,
                executable_path: Some(
                    game_dir(&h.lib, GHOSTING)
                        .join("hl2.exe")
                        .to_string_lossy()
                        .into_owned(),
                ),
                name: Some("hl2.exe".to_owned()),
                started_at: process["startedAt"].as_str().map(str::to_owned),
                source: Some("winapi".to_owned()),
            }),
        );
    }
    let stopped = h
        .platform
        .call(
            "game-process-stop",
            &[json!(GHOSTING), json!({"force": true})],
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped, json!({"status": "stopped"}));
    assert_eq!(world(&h.world).stop_calls, ["force(7000)"]);
    drop(end);
    h.wait_exit(GHOSTING).await;
    let events = drain(&mut h.events);
    let exited = events
        .iter()
        .find(|event| event.channel == "game-process-exited")
        .expect("выход по просьбе");
    assert_eq!(exited.payload["requested"], true);
    assert!(h.crashes().is_empty());
}

/// Steam: опрос подключения отдаёт аккаунт в окно; здоровье Steam (кеш 60 с, `force`, подробности
/// из `steam`); статус для окна запуска; профиль в браузере; Steam и SDK в клиенте Steam.
#[tokio::test(start_paused = true)]
async fn steam_account_health_and_links() {
    let mut h = harness(Setup {
        timings: Some(Timings {
            steam_first: Duration::from_millis(20),
            steam_tick: Duration::from_secs(3600),
            ..quiet_timings()
        }),
        ..Setup::default()
    });
    // До службы Steam неизвестен.
    let empty = h.platform.steam_account_state();
    assert_eq!(empty["signedIn"], false);
    assert_eq!(empty["connection"], "unknown");

    *h.availability.lock().unwrap() =
        json!({"ok": false, "code": "STEAM_NOT_RUNNING", "message": "Steam не запущен"});
    let health = h
        .platform
        .call("steam-health-get", &[json!({})])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(health["status"], "unavailable");
    assert_eq!(health["code"], "STEAM_NOT_RUNNING");
    assert_eq!(health["detail"], "Steam не запущен");
    let keys: Vec<&str> = health
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["status", "code", "detail", "checkedAt"]);
    assert!(
        h.steam.refreshes.load(Ordering::SeqCst) >= 1,
        "refreshConnection при создании"
    );

    *h.availability.lock().unwrap() = json!({"ok": true, "code": null,
        "steam": {"installPath": "C:\\Steam", "pid": 1234, "source": "registry"}});
    let cached = h.platform.steam_health_get(&json!({}));
    assert_eq!(cached, health, "кеш 60 с");
    let fresh = h.platform.steam_health_get(&json!({"force": true}));
    assert_eq!(fresh["status"], "available");
    assert_eq!(fresh["code"], Value::Null);
    assert_eq!(fresh["detail"], "C:\\Steam, PID 1234, опознан: registry");

    // Первый опрос Steam приходит позже запросов здоровья: ждём его как условие.
    let mut events = Vec::new();
    wait_until("опрос Steam", || {
        events.extend(drain(&mut h.events));
        events.iter().any(|event| event.channel == "steam-account")
    })
    .await;
    let channels = channels_of(&events);
    assert_eq!(
        channels.iter().filter(|c| **c == "steam-health").count(),
        2,
        "{channels:?}"
    );
    let account = events
        .iter()
        .find(|event| event.channel == "steam-account")
        .expect("опрос Steam");
    assert_eq!(account.payload["steamId64"], "76561198000000001");

    let status = h.platform.game_launch_steam_status(&json!("nope"));
    assert_eq!(status, json!({"ok": true, "code": null}));

    assert_eq!(h.platform.steam_open_profile(), json!({"status": "ok"}));
    assert_eq!(h.platform.game_launch_open_steam(), json!({"status": "ok"}));
    assert_eq!(
        h.platform.game_launch_install_sdk(),
        json!({"status": "ok"})
    );
    let opened: Vec<String> = h
        .effects
        .take()
        .into_iter()
        .filter(|line| line.starts_with("open-external"))
        .collect();
    assert_eq!(
        opened,
        [
            "open-external https://steamcommunity.com/profiles/76561198000000001",
            "open-external steam://open/main",
            "open-external steam://install/243730",
        ]
    );
    *h.steam.account.lock().unwrap() = json!({"signedIn": false, "steamId64": null});
    assert_eq!(
        h.platform.steam_open_profile(),
        json!({"status": "unknown-account"})
    );
}

/// Без входа в Steam опрос идёт раз в 3 с первые 10 раз, затем раз в 30 с; окно скрыто — реже.
#[tokio::test(start_paused = true)]
async fn steam_watch_polls_faster_while_signed_out() {
    let mut h = harness(Setup {
        timings: Some(Timings {
            steam_first: Duration::from_millis(5),
            steam_signed_out_tick: Duration::from_millis(20),
            steam_signed_out_fast_ticks: 3,
            steam_tick: Duration::from_secs(3600),
            ..quiet_timings()
        }),
        ..Setup::default()
    });
    *h.steam.account.lock().unwrap() = json!({"signedIn": false});
    h.delivery.ensure_service();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let pushes = drain(&mut h.events)
        .into_iter()
        .filter(|event| event.channel == "steam-account")
        .count();
    // Первый опрос и ещё 3 быстрых, потом пауза на час.
    assert_eq!(pushes, 4);
}

/// Служба доставки не создалась (библиотека недоступна): в 0.4.7 Steam и его опрос поднимаются
/// раньше службы (`ensureService`), поэтому аккаунт остаётся виден окну и опрашивается.
#[tokio::test(start_paused = true)]
async fn steam_account_stays_observed_when_the_service_cannot_be_created() {
    let mut h = harness(Setup {
        timings: Some(Timings {
            steam_first: Duration::from_millis(5),
            steam_tick: Duration::from_millis(20),
            ..quiet_timings()
        }),
        ..Setup::default()
    });
    h.fail_delivery.store(true, Ordering::SeqCst);
    assert!(h.delivery.ensure_service().is_none(), "службы нет");
    assert!(h.platform.processes().is_none());

    let account = h.platform.steam_account_state();
    assert_eq!(account["signedIn"], true);
    assert_eq!(account["steamId64"], "76561198000000001");
    let created_with = h.steam.refreshes.load(Ordering::SeqCst);
    assert!(created_with >= 1, "refreshConnection при создании");

    tokio::time::sleep(Duration::from_millis(400)).await;
    let pushes = drain(&mut h.events)
        .into_iter()
        .filter(|event| event.channel == "steam-account")
        .count();
    assert!(pushes >= 3, "опрос Steam идёт без службы: {pushes}");
    assert!(h.steam.refreshes.load(Ordering::SeqCst) > created_with);
}

/// Без среды tokio опрос Steam не запустить: это строка в журнале, а не молчание.
#[test]
fn steam_watch_without_a_tokio_runtime_says_so() {
    let h = harness(Setup::default());
    assert!(h.delivery.ensure_service().is_some(), "служба создана");
    let lines = h.log_lines();
    assert!(
        lines.iter().any(|(level, message)| *level == "warn"
            && message == "[steam] опрос подключения не запущен: нет среды tokio"),
        "{lines:?}"
    );
    // Аккаунт при этом виден.
    assert_eq!(h.platform.steam_account_state()["signedIn"], true);
}

/// Перезапуск от администратора: отказ UAC — замок обратно; согласие — выход через 600 мс без
/// замка; сбой — ошибка и замок обратно; нет ответа — `pending`, итог применяется позже.
#[tokio::test(start_paused = true)]
async fn restart_elevated_like_0_4_7() {
    let declined = harness(Setup {
        elevation: Some((Some(Ok(Elevated::Declined(1223))), Duration::ZERO)),
        ..Setup::default()
    });
    declined.log_lines();
    let answer = declined
        .platform
        .call("game-launch-restart-elevated", &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer, json!({"status": "declined"}));
    assert_eq!(
        declined.effects.take(),
        ["lock.release", "uac", "lock.reacquire"]
    );
    assert_eq!(
        declined.log_lines(),
        [(
            "info",
            "[launch] повышение прав отклонено, код 1223".to_owned()
        )]
    );

    let accepted = harness(Setup {
        elevation: Some((Some(Ok(Elevated::Started)), Duration::ZERO)),
        timings: Some(Timings {
            elevation_quit_delay: Duration::from_millis(100),
            ..quiet_timings()
        }),
        ..Setup::default()
    });
    let answer = accepted.platform.game_launch_restart_elevated().await;
    assert_eq!(answer, json!({"status": "ok"}));
    assert_eq!(accepted.effects.snapshot(), ["lock.release", "uac"]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(accepted.effects.take(), ["lock.release", "uac", "quit"]);

    let failed = harness(Setup {
        elevation: Some((Some(Err("нет exe".to_owned())), Duration::ZERO)),
        ..Setup::default()
    });
    failed.log_lines();
    assert_eq!(
        failed.platform.game_launch_restart_elevated().await,
        json!({"status": "error"})
    );
    assert_eq!(
        failed.effects.take(),
        ["lock.release", "uac", "lock.reacquire"]
    );
    assert_eq!(
        failed.log_lines(),
        [(
            "warn",
            "[launch] перезапуск от администратора не удался: нет exe".to_owned()
        )]
    );

    let pending = harness(Setup {
        elevation: Some((
            Some(Ok(Elevated::Declined(1223))),
            Duration::from_millis(200),
        )),
        timings: Some(Timings {
            elevation_wait: Duration::from_millis(30),
            ..quiet_timings()
        }),
        ..Setup::default()
    });
    assert_eq!(
        pending.platform.game_launch_restart_elevated().await,
        json!({"status": "pending"})
    );
    assert_eq!(pending.effects.snapshot(), ["lock.release", "uac"]);
    pending.platform.settle().await;
    assert_eq!(
        pending.effects.take(),
        ["lock.release", "uac", "lock.reacquire"]
    );

    // Без перезапуска (Linux или нет портов) — `unsupported`.
    let none = harness(Setup::default());
    assert_eq!(
        none.platform.game_launch_restart_elevated().await,
        json!({"status": "unsupported"})
    );
    assert!(none.effects.take().is_empty());
}

/// Смена библиотеки (0.4.9): пока игра идёт — отказ `busy`, служба процессов прежняя и следит
/// за игрой; после выхода игры — новая служба, своя.
#[tokio::test(start_paused = true)]
async fn library_change_refused_while_game_runs() {
    let mut h = harness(Setup::default());
    let (_, end) = h.launch(GHOSTING, 8000).await;
    let first = h.platform.processes().unwrap();
    assert!(first.is_running(GHOSTING));

    let other = h.root.join("other");
    install_game(&other, "hl1-won");
    let other = other.to_string_lossy().into_owned();
    assert_eq!(
        h.delivery.set_library_path(&other),
        Some(json!({"ok": false, "error": "busy"}))
    );
    assert!(first.is_running(GHOSTING), "прежняя служба следит за игрой");
    assert_eq!(h.created.lock().unwrap().len(), 1);

    end.send(exit(0)).unwrap();
    h.wait_exit(GHOSTING).await;
    assert_eq!(h.delivery.set_library_path(&other), None);
    let second = h.platform.processes().unwrap();
    assert!(!second.is_running(GHOSTING));
    assert_eq!(h.created.lock().unwrap().len(), 2);
    assert!(!h.platform.is_any_game_running());
    drain(&mut h.events);
    h.delivery.dispose();
}

/// Перезапуск лаунчера при идущей игре: процесс из `process.json` опознаётся (PID, путь, старт),
/// и платформа видит запуск — окно прячется, событие окна, сессия статистики.
#[tokio::test(start_paused = true)]
async fn game_surviving_launcher_restart_is_adopted() {
    let mut h = harness(Setup {
        with_tray: true,
        ..Setup::default()
    });
    let exe = game_dir(&h.lib, GHOSTING)
        .join("hl2.exe")
        .to_string_lossy()
        .into_owned();
    let started_at = "2026-10-17T10:00:00.000Z";
    let state = state_dir(&h.lib);
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        state.join("ghosting-renewed.process.json"),
        json!({
            "version": 1, "gameId": GHOSTING, "state": "running", "pid": 9100,
            "executablePath": exe,
            "installPath": game_dir(&h.lib, GHOSTING).to_string_lossy(),
            "startedAt": started_at,
            "processKey": game_process_service::process_key_of(9100.0, Some(&exe), Some(started_at)),
            "launcherPid": 1,
        })
        .to_string(),
    )
    .unwrap();
    {
        let mut world = world(&h.world);
        world.alive.insert(9100);
        world.inspect.insert(
            9100,
            Some(ProcessInfo {
                pid: 9100,
                executable_path: Some(exe.clone()),
                name: Some("hl2.exe".to_owned()),
                started_at: Some(started_at.to_owned()),
                source: Some("winapi".to_owned()),
            }),
        );
    }
    h.delivery.ensure_service();
    let events = drain(&mut h.events);
    assert_eq!(channels_of(&events), ["game-process-running"], "{events:?}");
    assert_eq!(events[0].payload["gameId"], GHOSTING);
    assert!(h.platform.is_any_game_running());
    assert!(h.effects.take().contains(&"tray.hide".to_owned()));
    h.delivery.dispose();
}

/// Аватары друзей (`friends-avatars`): не больше 100 id и только SteamID64; сначала кеш Steam
/// (`config/avatarcache`), недостающие — из профиля Steam (здесь сеть подменена: ответа нет).
#[tokio::test(start_paused = true)]
async fn friends_avatars_from_steam_cache_then_profiles() {
    let h = harness(Setup::default());
    let first = "76561198000000001";
    let second = "76561198000000002";
    assert_eq!(
        h.platform
            .call("friends-avatars", &[json!(vec![first; 101])])
            .await
            .unwrap()
            .unwrap(),
        json!({})
    );
    assert_eq!(h.platform.friends_avatars(&json!(first)).await, json!({}));

    // Службы ещё нет — Steam не найден: всё — запросами (подделка без ответа).
    assert_eq!(
        h.platform
            .friends_avatars(&json!([first, "123", 5, "76561190000000000x", second]))
            .await,
        json!({})
    );
    let mut requested: Vec<String> = h
        .effects
        .take()
        .into_iter()
        .filter(|line| line.starts_with("get "))
        .collect();
    requested.sort();
    assert_eq!(
        requested,
        [
            format!("get https://steamcommunity.com/profiles/{first}?xml=1"),
            format!("get https://steamcommunity.com/profiles/{second}?xml=1"),
        ]
    );

    // Steam найден: аватар из кеша Steam, запрос — только за недостающим.
    *h.steam.install.lock().unwrap() = Some(r"C:\Steam".to_owned());
    let png = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
    h.steam_fs
        .add_file(&format!(r"C:\Steam\config\avatarcache\{first}.png"), png);
    h.delivery.ensure_service();
    let found = h.platform.friends_avatars(&json!([first, second])).await;
    assert_eq!(found.as_object().unwrap().len(), 1, "{found}");
    assert!(
        found[first]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,iVBORw0KGgo")
    );
    let requested: Vec<String> = h
        .effects
        .take()
        .into_iter()
        .filter(|line| line.starts_with("get "))
        .collect();
    assert_eq!(
        requested,
        [format!(
            "get https://steamcommunity.com/profiles/{second}?xml=1"
        )]
    );
}

/// Друзья для онлайн-профиля (`steamFriends`): без Steam или аккаунта — пусто.
#[tokio::test(start_paused = true)]
async fn steam_friends_need_steam_and_account() {
    let h = harness(Setup::default());
    assert!(h.platform.steam_friends().is_empty(), "службы ещё нет");
    h.delivery.ensure_service();
    assert!(h.platform.steam_friends().is_empty(), "Steam не найден");
    *h.steam.install.lock().unwrap() = Some(r"C:\Steam".to_owned());
    *h.steam.account_id.lock().unwrap() = Some(39_734_272);
    h.steam_fs.add_file(
        r"C:\Steam\userdata\39734272\config\localconfig.vdf",
        "\"UserLocalConfigStore\"\n{\n\t\"friends\"\n\t{\n\t\t\"20\"\n\t\t{\n\t\t\t\"name\"\t\t\"Друг\"\n\t\t}\n\t}\n}\n",
    );
    let friends = h.platform.steam_friends();
    assert_eq!(friends.len(), 1, "{friends:?}");
    assert_eq!(friends[0]["steamId64"], "76561197960265748");
    assert_eq!(friends[0]["personaName"], "Друг");
}

/// `game-stats-get`: при входе — снимок сервера, иначе — `game-stats.json`; неизвестная игра —
/// недоступно. `tray-hide-while-playing-set`, `native-available`, `umu-available`.
#[tokio::test(start_paused = true)]
async fn stats_tray_switch_and_sync_channels() {
    let h = harness(Setup {
        with_tray: true,
        ..Setup::default()
    });
    assert_eq!(
        h.platform.game_stats_get(&json!("nope")),
        json!({"available": false})
    );
    assert_eq!(
        h.platform.game_stats_get(&json!(GHOSTING)),
        json!({"available": false, "gameId": GHOSTING})
    );
    *h.online.server_stats.lock().unwrap() =
        Some(json!({"available": true, "gameId": GHOSTING, "source": "server"}));
    assert_eq!(h.platform.game_stats_get(&json!("218"))["source"], "server");

    assert_eq!(
        h.platform
            .call("tray-hide-while-playing-set", &[json!(false)])
            .await
            .unwrap()
            .unwrap(),
        json!({"status": "ok"})
    );
    assert!(!h.platform.tray().unwrap().is_enabled());
    h.platform.tray_hide_while_playing_set(&Value::Null);
    assert!(
        h.platform.tray().unwrap().is_enabled(),
        "не false — включено"
    );

    assert_eq!(
        h.platform
            .call("native-available", &[])
            .await
            .unwrap()
            .unwrap(),
        json!(true)
    );
    assert_eq!(
        h.platform
            .call("umu-available", &[])
            .await
            .unwrap()
            .unwrap(),
        json!(false)
    );
    assert!(h.platform.call("proton-status", &[]).await.is_none());
    assert!(h.platform.call("unknown-channel", &[]).await.is_none());
}

// ---------------------------------------------------------------- 0.4.9: эпизоды и окно о падении

/// `episode-launch-selftest.js`: `game-process-launch(id, {episode})` — чужое значение
/// `LAUNCH_CONFIG_INVALID` до запуска; эпизод доходит до запуска, записи процесса и Discord
/// `running`; на выходе config.cfg эпизода возвращается в Ghosting.
#[tokio::test(start_paused = true)]
async fn episode_reaches_the_launch_presence_and_config_return() {
    let h = harness(Setup::default());
    for episode in [json!(["ep1"]), json!("ep3"), json!({"id": "ep1"})] {
        let answer = h
            .platform
            .call(
                "game-process-launch",
                &[json!(GHOSTING), json!({"episode": episode})],
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(answer["status"], "error", "{episode}");
        assert_eq!(
            answer["error"]["code"], "LAUNCH_CONFIG_INVALID",
            "{episode}"
        );
    }
    let effects = h.effects.take();
    assert!(
        !effects
            .iter()
            .any(|line| line.starts_with("online.starting")),
        "до запуска не дошло: {effects:?}"
    );

    let game = game_dir(&h.lib, GHOSTING);
    std::fs::create_dir_all(game.join("ghostingmod").join("cfg")).unwrap();
    std::fs::create_dir_all(game.join("ep2").join("cfg")).unwrap();
    std::fs::write(game.join("ghostingmod/cfg/config.cfg"), "bind w +forward\n").unwrap();
    hlsr_delivery::episodes::sync_config_in(&game, "ep2", None).unwrap();
    std::fs::write(game.join("ep2/cfg/config.cfg"), "bind e +use\n").unwrap();

    let (end, receiver) = oneshot::channel();
    h.plans.lock().unwrap().push_back(Plan::Start {
        pid: 4300,
        end: receiver,
    });
    let response = h
        .platform
        .call(
            "game-process-launch",
            &[json!(GHOSTING), json!({"episode": "ep2"})],
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response["status"], "started", "{response}");
    assert_eq!(response["process"]["episode"], "ep2");
    assert!(h.effects.take().contains(&format!(
        "presence {}",
        json!({"type": "running", "gameId": GHOSTING, "episode": "ep2"})
    )));
    assert_eq!(
        std::fs::read_to_string(game.join("ghostingmod/cfg/config.cfg")).unwrap(),
        "bind w +forward\n",
        "пока игра идёт — не трогаем"
    );

    end.send(exit(0)).unwrap();
    h.wait_exit(GHOSTING).await;
    assert_eq!(
        std::fs::read_to_string(game.join("ghostingmod/cfg/config.cfg")).unwrap(),
        "bind e +use\n"
    );
    assert!(hlsr_delivery::episodes::pending_sync(Some(game.as_path())).is_empty());
}

/// `crash-report-selftest.js`: Ghosting пакует лог и дамп сбоя в `ghosting_crash-*.zip` — окно о
/// падении просит архив, «Показать» выделяет его; без архива — лог рядом с дампом.
#[tokio::test(start_paused = true)]
async fn crash_report_asks_for_the_ghosting_archive() {
    let mut h = harness(Setup::default());
    let mod_dir = game_dir(&h.lib, GHOSTING).join("ghostingmod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let base = mod_dir.join("ghosting_crash-2026-11-07-12-00-00-2");
    let file = |ext: &str| base.with_extension(ext).to_string_lossy().into_owned();

    let (_, end) = h.launch(GHOSTING, 4310).await;
    for ext in ["txt", "dmp", "zip"] {
        std::fs::write(base.with_extension(ext), format!("data {ext}")).unwrap();
    }
    end.send(exit(3_221_225_477)).unwrap();
    h.wait_exit(GHOSTING).await;
    let events = drain(&mut h.events);
    let report = &events
        .iter()
        .find(|event| event.channel == "game-crash-report")
        .expect("окно о падении")
        .payload;
    assert_eq!(report["archive"], true);
    assert_eq!(report["hasDump"], true);
    assert_eq!(report["file"], json!(file("zip")));
    let info = h.platform.game_crash_info(Some(&json!(GHOSTING)));
    assert_eq!(info["archivePath"], json!(file("zip")));
    assert_eq!(info["logPath"], json!(file("txt")));
    h.effects.take();
    assert_eq!(
        h.platform.game_crash_open_dump(Some(&json!(GHOSTING))),
        json!({"status": "ok"})
    );
    assert!(
        h.effects
            .take()
            .contains(&format!("show-item {}", file("zip")))
    );

    // Без архива (другие игры) — лог рядом с дампом; дамп и лог свежие для нового запуска.
    std::fs::remove_file(base.with_extension("zip")).unwrap();
    let (_, end) = h.launch(GHOSTING, 4311).await;
    for ext in ["txt", "dmp"] {
        std::fs::write(base.with_extension(ext), format!("second {ext}")).unwrap();
    }
    end.send(exit(1)).unwrap();
    h.wait_exit(GHOSTING).await;
    let events = drain(&mut h.events);
    let report = &events
        .iter()
        .find(|event| event.channel == "game-crash-report")
        .expect("окно о падении")
        .payload;
    assert_eq!(report["archive"], false);
    assert_eq!(report["file"], json!(file("txt")));
    h.effects.take();
    assert_eq!(
        h.platform.game_crash_open_dump(Some(&json!(GHOSTING))),
        json!({"status": "ok"})
    );
    assert!(
        h.effects
            .take()
            .contains(&format!("show-item {}", file("txt")))
    );
}

/// «Демо» (0.4.9): пока рендер этой игры идёт — `RENDER_ACTIVE` без запуска; хук перед запуском
/// снимает mute, его сбой игру не останавливает.
#[tokio::test(start_paused = true)]
async fn render_active_refuses_and_before_launch_runs_first() {
    let h = harness(Setup::default());
    h.platform
        .set_render_active(Some(Arc::new(|game_id: &str| game_id == GHOSTING)));
    let answer = h
        .platform
        .call("game-process-launch", &[json!(GHOSTING)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer["error"]["code"], "RENDER_ACTIVE", "{answer}");
    h.platform.set_render_active(None);

    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = calls.clone();
    h.platform
        .set_before_launch(Some(Arc::new(move |game_id: &str| {
            seen.lock().unwrap().push(game_id.to_owned());
            Box::pin(async { Err("mute не снят".to_owned()) }) as BoxFuture<'static, _>
        })));
    h.log_lines();
    let (response, _end) = h.launch(GHOSTING, 4320).await;
    assert_eq!(response["status"], "started", "{response}");
    assert_eq!(*calls.lock().unwrap(), [GHOSTING]);
    assert!(h.log_lines().contains(&(
        "warn",
        format!("[game-delivery] подготовка к запуску {GHOSTING}: mute не снят")
    )));
}
