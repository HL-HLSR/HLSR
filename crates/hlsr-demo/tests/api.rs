//! Фасад «Демо» на фейках: каналы окна — как их зовёт страница G11 (журнал IPC сцен `demo/*`) —
//! отвечают по контракту 0.4.9; хуки рендера (`RENDER_ACTIVE`, сброс mute) и уборка при старте;
//! экспорт и предпросмотр подставным рендером через настоящий FFmpeg (нет FFmpeg — пропуск).

mod common;

use hlsr_core::js::Platform;
use hlsr_core::logger::MemoryLogger;
use hlsr_core::store::Store;
use hlsr_delivery::ports::{Dialogs, OpenDialog, SaveDialog, Shell, ShortcutLink};
use hlsr_delivery::providers::BoxFuture;
use hlsr_demo::api::{self, CHANNELS, Caps, DemoApi, DemoEvent, DemoOptions, RenderGame};
use hlsr_demo::errors::DemoError;
use hlsr_demo::jobs::{Adapter, Material, RenderRequest};
use hlsr_demo::library::Described;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

#[derive(Default)]
struct FakeShell {
    shown: Mutex<Vec<PathBuf>>,
}

impl Shell for FakeShell {
    fn open_path<'a>(&'a self, _path: &'a Path) -> BoxFuture<'a, String> {
        Box::pin(async { String::new() })
    }
    fn open_external(&self, _url: &str) -> Result<(), String> {
        Ok(())
    }
    fn show_item_in_folder(&self, path: &Path) {
        self.shown.lock().unwrap().push(path.to_path_buf());
    }
    fn write_shortcut_link(&self, _path: &Path, _link: &ShortcutLink) -> bool {
        false
    }
    fn read_shortcut_link(&self, _path: &Path) -> Result<ShortcutLink, String> {
        Err("нет".into())
    }
}

/// Диалоги: ответы по очереди (пусто — «Отмена»), запросы запоминаются.
#[derive(Default)]
struct FakeDialogs {
    answers: Mutex<VecDeque<Vec<PathBuf>>>,
    asked: Mutex<Vec<OpenDialog>>,
}

impl Dialogs for FakeDialogs {
    fn show_open_dialog(&self, options: OpenDialog) -> BoxFuture<'_, Vec<PathBuf>> {
        self.asked.lock().unwrap().push(options);
        let answer = self.answers.lock().unwrap().pop_front().unwrap_or_default();
        Box::pin(async move { answer })
    }
    fn show_save_dialog(&self, _options: SaveDialog) -> BoxFuture<'_, Option<PathBuf>> {
        Box::pin(async { None })
    }
}

/// Рендер игры на подделке: активность, флаг mute, сколько раз звали сброс, восстановление.
#[derive(Default)]
struct TestGame {
    active: AtomicBool,
    pending: AtomicBool,
    resets: AtomicUsize,
    reset_ok: AtomicBool,
    recovered: AtomicUsize,
    disposed: AtomicUsize,
}

impl Adapter for TestGame {
    fn render(&self, _request: &RenderRequest) -> Result<Material, DemoError> {
        Err(DemoError::new("RENDER_FAILED"))
    }
}

impl RenderGame for TestGame {
    fn game_id(&self) -> Option<&'static str> {
        Some("hl1-won")
    }
    fn caps(&self) -> Caps {
        Caps {
            max_height: 1080,
            fps: vec![30, 60],
            flags: vec!["keyhud", "audio", "hud"],
        }
    }
    fn can_render(&self, _item: &Described) -> Result<(), DemoError> {
        Err(DemoError::new("BXTRS_MISSING"))
    }
    fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }
    fn mute_reset_blocker(&self) -> Option<String> {
        (!self.pending.load(Ordering::SeqCst)).then(|| "no-flag".to_owned())
    }
    fn reset_mute(&self) -> bool {
        self.resets.fetch_add(1, Ordering::SeqCst);
        self.reset_ok.load(Ordering::SeqCst)
    }
    fn recover(&self) {
        self.recovered.fetch_add(1, Ordering::SeqCst);
    }
    fn dispose(&self) {
        self.disposed.fetch_add(1, Ordering::SeqCst);
    }
}

struct Fx {
    dir: tempfile::TempDir,
    api: Arc<DemoApi>,
    shell: Arc<FakeShell>,
    dialogs: Arc<FakeDialogs>,
    trashed: Arc<Mutex<Vec<PathBuf>>>,
    achieved: Arc<Mutex<Vec<String>>>,
    store: Arc<Mutex<Store>>,
}

impl Fx {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    async fn call(&self, channel: &str, args: Value) -> Value {
        let args = args.as_array().cloned().unwrap_or_default();
        self.api
            .call(channel, &args)
            .await
            .unwrap_or_else(|| panic!("канал {channel} не отвечает"))
            .unwrap_or_else(|error| panic!("{channel}: отказ {error}"))
    }
}

fn fx(ffmpeg: Option<PathBuf>, fake_render: bool, renderers: Vec<Arc<dyn RenderGame>>) -> Fx {
    fx_in(tempfile::tempdir().unwrap(), ffmpeg, fake_render, renderers)
}

fn fx_in(
    dir: tempfile::TempDir,
    ffmpeg: Option<PathBuf>,
    fake_render: bool,
    renderers: Vec<Arc<dyn RenderGame>>,
) -> Fx {
    let root = dir.path();
    std::fs::create_dir_all(root.join("demos")).unwrap();
    let store = Arc::new(Mutex::new(Store::open(
        root.join(api::STORE_FILE),
        api::store_defaults(),
    )));
    let shell = Arc::new(FakeShell::default());
    let dialogs = Arc::new(FakeDialogs::default());
    let trashed = Arc::new(Mutex::new(Vec::new()));
    let achieved = Arc::new(Mutex::new(Vec::new()));
    let api = DemoApi::new(DemoOptions {
        store: store.clone(),
        games: Box::new(Vec::new),
        tools: Box::new(|| None),
        achieve: {
            let achieved = achieved.clone();
            Box::new(move |id: &str| achieved.lock().unwrap().push(id.to_owned()))
        },
        dialogs: dialogs.clone(),
        shell: shell.clone(),
        trash: {
            let trashed = trashed.clone();
            Box::new(move |file: &Path| {
                trashed.lock().unwrap().push(file.to_path_buf());
                std::fs::remove_file(file).map_err(|error| error.to_string())
            })
        },
        fullscreen: Box::new(|on| Box::pin(async move { on })),
        videos_dir: root.join("Videos"),
        jobs_root: Some(root.join("demo-jobs")),
        platform: Platform::Win32,
        ffmpeg_override: ffmpeg,
        bxtrs_override: None,
        fake_render,
        renderers,
        logger: Arc::new(MemoryLogger::default()),
        limit_bytes: None,
    });
    Fx {
        dir,
        api,
        shell,
        dialogs,
        trashed,
        achieved,
        store,
    }
}

/// Демка HL1 WON (протокол 47) в папке `demos`.
fn hl1_demo(fx: &Fx, name: &str) -> String {
    let file = fx.path("demos").join(name);
    std::fs::write(&file, common::goldsrc(&json!({"netProtocol": 47}))).unwrap();
    file.to_string_lossy().into_owned()
}

fn timeline(source_id: &str) -> Value {
    json!({
        "version": 1,
        "video": {"duration": 20, "fps": 60, "width": 1920, "height": 1080, "speed": 1,
            "range": {"from": 0, "to": 20}},
        "source": {"id": source_id, "game": "hl1", "map": "c1a0", "files": ["a.dem"]},
        "markers": [{"t": 0, "type": "map", "label": "c1a0", "sourceTime": 0}],
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_of_the_demo_page_answer_like_0_4_9() {
    let fx = fx(None, true, Vec::new());
    let unknown = fx.api.call("demo-nope", &[]).await;
    assert!(unknown.is_none());

    // activated(): списки и статусы.
    assert_eq!(
        fx.call("demo-list", json!([])).await,
        json!({"status": "ok", "items": []})
    );
    let ffmpeg = fx.call("demo-ffmpeg-status", json!([])).await;
    assert_eq!(ffmpeg["ffmpeg"]["state"], "not-installed", "{ffmpeg}");
    assert_eq!(ffmpeg["ffmpeg"]["installable"], true);
    assert_eq!(ffmpeg["ffmpeg"]["pinnedVersion"], "9.0.2");
    assert_eq!(
        ffmpeg["ffmpeg"]["origins"][0],
        json!({"site": "github.com/GyanD/codexffmpeg", "release": "9.0.2"})
    );
    let bxtrs = fx.call("demo-bxtrs-status", json!([])).await;
    assert_eq!(bxtrs["bxtrs"]["state"], "not-installed", "{bxtrs}");
    assert_eq!(bxtrs["bxtrs"]["sourceLink"], "bxtrsSource");
    let settings = fx.call("demo-settings-get", json!([])).await;
    let videos = fx
        .path("Videos")
        .join("HLSR")
        .to_string_lossy()
        .into_owned();
    assert_eq!(settings["outputDir"], json!(videos));
    assert_eq!(settings["defaultOutputDir"], json!(videos));
    assert_eq!(
        settings["preview"],
        json!({"keyhud": true, "hud": true, "ownConfig": true, "ownHud": true})
    );
    assert_eq!(settings["settings"]["preset"], "quality");
    assert_eq!(
        fx.call("demo-rendered-list", json!([])).await,
        json!({"status": "ok", "outputDir": videos, "items": []})
    );

    // «Добавить»: две демки HL1 — один набор; бросание файлов — то же.
    let (a, b) = (hl1_demo(&fx, "a.dem"), hl1_demo(&fx, "b.dem"));
    let added = fx
        .call("demo-add-paths", json!([[a, b, "C:\\nope\\x.txt"]]))
        .await;
    assert_eq!(added["status"], "ok", "{added}");
    assert_eq!(added["added"].as_array().unwrap().len(), 1);
    assert_eq!(added["rejected"][0]["code"], "FILE_TYPE_UNSUPPORTED");
    let item = added["items"][0].clone();
    assert_eq!(item["kind"], "segments");
    let id = item["id"].as_str().unwrap().to_owned();
    assert_eq!(
        fx.call("demo-add-paths", json!(["x"])).await,
        json!({"status": "error", "code": "PATH_INVALID"})
    );

    let detail = fx.call("demo-inspect", json!([id])).await;
    assert_eq!(detail["status"], "ok", "{detail}");
    assert_eq!(detail["item"]["id"], json!(id));
    assert_eq!(detail["segments"][0]["name"], "a.dem");
    assert!(
        detail["segments"][0].get("path").is_none(),
        "путь в окно не уходит"
    );
    assert_eq!(detail["render"]["ok"], true, "подставной рендер готов");
    assert_eq!(
        detail["render"]["view"],
        json!({"keyhud": false, "hud": false, "ownConfig": false, "ownHud": false})
    );
    assert!(detail["markers"].is_array());
    assert_eq!(
        fx.call("demo-inspect", json!(["nope"])).await,
        json!({"status": "error", "code": "DEMO_NOT_FOUND"})
    );

    // Порядок набора и «Убрать».
    let swapped = fx.call("demo-reorder", json!([id, [1, 0]])).await;
    assert_eq!(swapped["segments"][0]["name"], "b.dem", "{swapped}");
    assert_eq!(
        fx.call("demo-reorder", json!([id, [0, 0]])).await,
        json!({"status": "error", "code": "ORDER_INVALID"})
    );

    // Без FFmpeg: оценка есть, план и задания — FFMPEG_MISSING, слот задания свободен.
    let estimate = fx
        .call(
            "demo-estimate",
            json!([id, [{"preset": "preview", "range": null}]]),
        )
        .await;
    assert_eq!(estimate["status"], "ok", "{estimate}");
    assert_eq!(estimate["threshold"], 120);
    assert!(estimate["items"][0]["seconds"].is_number());
    assert_eq!(estimate["items"][0]["segments"], 2);
    assert_eq!(
        fx.call("demo-estimate", json!([id, []])).await,
        json!({"status": "error", "code": "SETTINGS_INVALID", "field": "estimate"})
    );
    let options = fx.call("demo-options", json!([id])).await;
    assert_eq!(
        options["ffmpeg"],
        json!({"state": "not-installed", "code": null})
    );
    assert_eq!(options["modes"], Value::Null);
    assert_eq!(options["capabilities"]["maxHeight"], 2160);
    assert_eq!(
        options["keyhud"],
        json!({"available": false, "code": "KEYHUD_NOT_AVAILABLE"})
    );
    assert_eq!(options["speed"], json!({"min": 0.25, "max": 4}));
    assert_eq!(options["resolutions"].as_array().unwrap().len(), 4);
    let missing = json!({"status": "error", "code": "FFMPEG_MISSING"});
    assert_eq!(fx.call("demo-plan", json!([id, {}])).await, missing);
    assert_eq!(fx.call("demo-encoders", json!([])).await, missing);
    assert_eq!(
        fx.call("demo-export-start", json!([id, {"preset": "discord"}]))
            .await,
        missing
    );
    let view =
        json!({"range": null, "keyhud": false, "hud": true, "ownConfig": true, "ownHud": true});
    assert_eq!(
        fx.call("demo-preview-start", json!([id, view])).await,
        missing
    );
    // Вид предпросмотра запомнен.
    assert_eq!(
        fx.call("demo-settings-get", json!([])).await["preview"]["keyhud"],
        false
    );
    assert_eq!(
        fx.call("demo-cancel", json!(["p1"])).await,
        json!({"status": "ok", "cancelled": false})
    );
    assert_eq!(
        fx.call("demo-preview-clear", json!([])).await,
        json!({"status": "ok", "cleared": false})
    );

    // Инструменты без службы доставки.
    let no_tools = json!({"status": "error", "code": "TOOLS_UNAVAILABLE"});
    assert_eq!(fx.call("demo-ffmpeg-install", json!([])).await, no_tools);
    assert_eq!(fx.call("demo-bxtrs-install", json!([])).await, no_tools);
    let not_cancelled = json!({"status": "ok", "cancelled": false});
    assert_eq!(
        fx.call("demo-ffmpeg-cancel", json!([])).await,
        not_cancelled
    );
    assert_eq!(fx.call("demo-bxtrs-cancel", json!([])).await, not_cancelled);

    // Итоги заданий по id — только свои.
    let not_found = json!({"status": "error", "code": "RESULT_NOT_FOUND"});
    for channel in ["demo-reveal", "demo-media-url", "demo-timeline-read"] {
        assert_eq!(
            fx.call(channel, json!(["e1"])).await,
            not_found,
            "{channel}"
        );
    }

    // Папка вывода: выбор и отмена.
    let out = fx.path("out");
    fx.dialogs
        .answers
        .lock()
        .unwrap()
        .push_back(vec![out.clone()]);
    let picked = fx.call("demo-output-pick", json!([])).await;
    assert_eq!(
        picked,
        json!({"status": "ok", "outputDir": out.to_string_lossy()})
    );
    assert_eq!(
        fx.call("demo-output-pick", json!([])).await,
        json!({"status": "cancelled"})
    );
    assert_eq!(
        fx.dialogs.asked.lock().unwrap()[1].default_path.as_deref(),
        Some(&*out.to_string_lossy())
    );

    // «Отрендеренные»: только .mp4 со своим timeline.json.
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("ours.mp4"), b"mp4").unwrap();
    std::fs::write(out.join("ours.timeline.json"), timeline(&id).to_string()).unwrap();
    std::fs::write(out.join("foreign.mp4"), b"mp4").unwrap();
    std::fs::write(out.join("broken.mp4"), b"mp4").unwrap();
    std::fs::write(out.join("broken.timeline.json"), b"{").unwrap();
    let list = fx.call("demo-rendered-list", json!([])).await;
    let names: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["ours.mp4"], "{list}");
    assert_eq!(list["items"][0]["markerCount"], 1);
    assert_eq!(
        list["items"][0]["source"],
        json!({"game": "hl1", "map": "c1a0", "files": 1})
    );

    let open = fx.call("demo-rendered-open", json!(["ours.mp4"])).await;
    assert_eq!(open["status"], "ok", "{open}");
    assert_eq!(
        open["sourceId"],
        json!(id),
        "исходная демка по id из timeline.json"
    );
    let url = open["url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("hlsr-media://media/"), "{url}");
    assert_eq!(fx.api.media_file(&url), Some(out.join("ours.mp4")));
    assert_eq!(open["video"]["range"], json!({"from": 0, "to": 20}));
    for bad in ["../ours.mp4", "a.txt", "con:x.mp4", "a\\b.mp4"] {
        assert_eq!(
            fx.call("demo-rendered-open", json!([bad])).await,
            json!({"status": "error", "code": "PATH_INVALID"}),
            "{bad}"
        );
    }
    assert_eq!(
        fx.call("demo-rendered-open", json!(["broken.mp4"])).await,
        json!({"status": "error", "code": "TIMELINE_INVALID"})
    );

    // Строже 0.4.9: «Показать» и удаление — только у видео со своим timeline.json.
    assert_eq!(
        fx.call("demo-rendered-reveal", json!(["foreign.mp4"]))
            .await,
        not_found
    );
    assert_eq!(
        fx.call("demo-rendered-delete", json!(["foreign.mp4"]))
            .await,
        not_found
    );
    assert!(out.join("foreign.mp4").exists());
    assert!(fx.shell.shown.lock().unwrap().is_empty());
    assert_eq!(
        fx.call("demo-rendered-reveal", json!(["ours.mp4"])).await,
        json!({"status": "ok"})
    );
    assert_eq!(*fx.shell.shown.lock().unwrap(), [out.join("ours.mp4")]);
    let deleted = fx.call("demo-rendered-delete", json!(["ours.mp4"])).await;
    assert_eq!(deleted["status"], "ok", "{deleted}");
    assert_eq!(deleted["items"], json!([]));
    assert_eq!(
        deleted["deleted"],
        json!(out.join("ours.mp4").to_string_lossy())
    );
    assert_eq!(
        *fx.trashed.lock().unwrap(),
        [out.join("ours.mp4"), out.join("ours.timeline.json")]
    );
    assert_eq!(fx.api.media_file(&url), None, "открытое видео забыто");

    // Полный экран плеера: ответ — состояние.
    assert_eq!(
        fx.call("demo-player-fullscreen", json!([true])).await,
        json!(true)
    );
    assert_eq!(
        fx.call("demo-player-fullscreen", json!(["yes"])).await,
        json!(false)
    );

    // «Выбрать файлы…» и «Убрать».
    let ghosting = fx.path("demos").join("run.dem");
    std::fs::write(&ghosting, common::source(&json!({}), None)).unwrap();
    fx.dialogs.answers.lock().unwrap().push_back(vec![ghosting]);
    let picked = fx.call("demo-pick-files", json!([])).await;
    assert_eq!(picked["items"].as_array().unwrap().len(), 2, "{picked}");
    assert_eq!(
        fx.dialogs.asked.lock().unwrap()[2].properties,
        ["openFile", "multiSelections"]
    );
    assert_eq!(
        fx.call("demo-pick-files", json!([])).await,
        json!({"status": "cancelled"})
    );
    let removed = fx.call("demo-remove", json!([id])).await;
    assert_eq!(removed["items"].as_array().unwrap().len(), 1, "{removed}");
    assert_eq!(
        fx.store
            .lock()
            .unwrap()
            .get("manual")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Все 30 каналов отвечают и с пустыми аргументами.
    for channel in CHANNELS {
        if matches!(channel, "demo-pick-files" | "demo-output-pick") {
            continue;
        }
        let reply = fx.call(channel, json!([])).await;
        assert!(
            reply.get("status").is_some() || reply.is_boolean(),
            "{channel}: {reply}"
        );
    }
    assert!(fx.achieved.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_hooks_mute_resets_and_start_cleanup() {
    let game = Arc::new(TestGame::default());
    // Остатки прошлого запуска в корне заданий убираются при старте, после восстановления.
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("demo-jobs").join("old");
    std::fs::create_dir_all(&old).unwrap();
    let fx = fx_in(dir, None, false, vec![game.clone()]);
    assert!(!old.exists());
    assert_eq!(game.recovered.load(Ordering::SeqCst), 1);

    // RENDER_ACTIVE для запуска из лаунчера.
    assert!(!fx.api.render_active("hl1-won"));
    game.active.store(true, Ordering::SeqCst);
    assert!(fx.api.render_active("hl1-won"));
    assert!(!fx.api.render_active("ghosting-renewed"));

    // Фоновый сброс: не во время рендера, только при флаге, неудачный — до перезапуска не повторяется.
    game.pending.store(true, Ordering::SeqCst);
    fx.api.reset_pending_mutes();
    assert_eq!(game.resets.load(Ordering::SeqCst), 0, "идёт рендер");
    game.active.store(false, Ordering::SeqCst);
    fx.api.reset_pending_mutes();
    fx.api.reset_pending_mutes();
    assert_eq!(
        game.resets.load(Ordering::SeqCst),
        1,
        "не удался — больше не пробуем"
    );
    // Перед «Играть» — пробуем всё равно.
    game.reset_ok.store(true, Ordering::SeqCst);
    assert!(fx.api.reset_mute_before_launch("hl1-won"));
    assert!(!fx.api.reset_mute_before_launch("ghosting-renewed"));
    game.pending.store(false, Ordering::SeqCst);
    assert!(!fx.api.reset_mute_before_launch("hl1-won"), "флага нет");
    assert_eq!(game.resets.load(Ordering::SeqCst), 2);

    // Отказ адаптера виден окну, его возможности — в «Дополнительных настройках».
    let a = hl1_demo(&fx, "a.dem");
    let added = fx.call("demo-add-paths", json!([[a]])).await;
    let id = added["items"][0]["id"].clone();
    let detail = fx.call("demo-inspect", json!([id])).await;
    assert_eq!(
        detail["render"],
        json!({"ok": false, "code": "BXTRS_MISSING",
            "view": {"keyhud": true, "hud": true, "ownConfig": false, "ownHud": false}})
    );

    fx.api.dispose();
    assert_eq!(game.disposed.load(Ordering::SeqCst), 1);
}

/// Папка с ffmpeg и ffprobe: подмена или PATH.
fn ffmpeg_dir() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    if let Some(given) = std::env::var_os("HLSR_DEMO_FFMPEG").map(PathBuf::from) {
        return Some(if given.is_dir() {
            given
        } else {
            given.parent()?.to_path_buf()
        });
    }
    std::env::split_paths(&std::env::var_os("PATH")?).find(|dir| dir.join(exe).is_file())
}

async fn job_end(events: &mut broadcast::Receiver<DemoEvent>) -> DemoEvent {
    tokio::time::timeout(Duration::from_secs(240), async {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(error) => panic!("{error}"),
            };
            if event.channel == api::JOB_DONE || event.channel == api::JOB_ERROR {
                return event;
            }
        }
    })
    .await
    .expect("задание не закончилось за 4 минуты")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_and_preview_through_real_ffmpeg() {
    let Some(ffmpeg) = ffmpeg_dir() else {
        eprintln!("FFmpeg не найден — тест пропущен");
        return;
    };
    let fx = fx(Some(ffmpeg), true, Vec::new());
    let a = hl1_demo(&fx, "a.dem");
    let added = fx.call("demo-add-paths", json!([[a]])).await;
    let id = added["items"][0]["id"].clone();
    let mut events = fx.api.subscribe();

    let settings = json!({"preset": "quality", "height": 720, "fps": 30, "encoder": "cpu",
        "audio": true, "range": {"from": 0, "to": 1}});
    let started = fx.call("demo-export-start", json!([id, settings])).await;
    assert_eq!(started["status"], "ok", "{started}");
    let job_id = started["jobId"].as_str().unwrap().to_owned();
    assert_eq!(job_id.len(), 16);
    assert_eq!(started["plan"]["encoder"], "libx264");
    assert_eq!(
        fx.call("demo-export-start", json!([id, settings])).await,
        json!({"status": "error", "code": "JOB_BUSY"})
    );
    let done = job_end(&mut events).await;
    assert_eq!(done.channel, api::JOB_DONE, "{}", done.payload);
    let result = &done.payload["result"];
    assert_eq!(done.payload["jobId"], json!(job_id));
    assert_eq!(result["kind"], "export");
    let file = PathBuf::from(result["file"].as_str().unwrap());
    assert!(file.is_file(), "{file:?}");
    assert_eq!(
        fx.api.media_file(result["mediaUrl"].as_str().unwrap()),
        Some(file.clone())
    );
    assert_eq!(*fx.achieved.lock().unwrap(), ["video-editor"], "«Монтажёр»");
    assert_eq!(
        fx.store.lock().unwrap().get("export").unwrap()["height"],
        720,
        "настройки экспорта запомнены"
    );
    assert_eq!(
        fx.call("demo-media-url", json!([job_id])).await["url"],
        result["mediaUrl"]
    );
    assert_eq!(
        fx.call("demo-timeline-read", json!([job_id])).await["timeline"]["version"],
        1
    );
    assert_eq!(
        fx.call("demo-reveal", json!([job_id])).await,
        json!({"status": "ok"})
    );

    // Видео — во вкладке «Отрендеренные»; удаление забывает и итог экспорта.
    let list = fx.call("demo-rendered-list", json!([])).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    let name = list["items"][0]["name"].clone();
    let open = fx.call("demo-rendered-open", json!([name])).await;
    assert_eq!(open["sourceId"], id, "{open}");
    let deleted = fx.call("demo-rendered-delete", json!([name])).await;
    assert_eq!(deleted["items"], json!([]), "{deleted}");
    assert_eq!(
        fx.call("demo-media-url", json!([job_id])).await,
        json!({"status": "error", "code": "RESULT_NOT_FOUND"})
    );

    // Предпросмотр живёт в папке задания до «очистки».
    let preview = fx
        .call(
            "demo-preview-start",
            json!([id, {"range": {"from": 0, "to": 1}}]),
        )
        .await;
    assert_eq!(preview["status"], "ok", "{preview}");
    assert_eq!(preview["plan"]["preset"], "preview");
    let done = job_end(&mut events).await;
    assert_eq!(done.channel, api::JOB_DONE, "{}", done.payload);
    assert_eq!(done.payload["result"]["file"], Value::Null);
    let url = done.payload["result"]["mediaUrl"]
        .as_str()
        .unwrap()
        .to_owned();
    let preview_file = fx.api.media_file(&url).expect("файл предпросмотра");
    assert_eq!(
        fx.call("demo-preview-clear", json!([])).await,
        json!({"status": "ok", "cleared": true})
    );
    assert!(!preview_file.exists());
    assert_eq!(fx.api.media_file(&url), None);

    // Отмена задания (отрезок подлиннее — задание не успеет кончиться само).
    let long = json!({"preset": "quality", "height": 720, "fps": 30, "encoder": "cpu",
        "range": {"from": 0, "to": 12}});
    let started = fx.call("demo-export-start", json!([id, long])).await;
    let job_id = started["jobId"].clone();
    assert_eq!(
        fx.call("demo-cancel", json!([job_id])).await,
        json!({"status": "ok", "cancelled": true})
    );
    let end = job_end(&mut events).await;
    assert_eq!(end.channel, api::JOB_ERROR, "{}", end.payload);
    assert_eq!(end.payload["error"], json!({"code": "CANCELLED"}));
    assert_eq!(*fx.achieved.lock().unwrap(), ["video-editor"]);
}

/// Окно ушло, пока задание готовилось (future отменён): место задания освобождается, а не
/// отвечает JOB_BUSY до перезапуска.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_start_frees_the_job_slot() {
    let fx = fx(None, true, Vec::new());
    let args = [json!("nope"), json!({})];
    let dropped = tokio::time::timeout(Duration::ZERO, fx.api.call("demo-export-start", &args));
    let _ = dropped.await;
    assert_eq!(
        fx.call("demo-export-start", json!(args)).await,
        json!({"status": "error", "code": "DEMO_NOT_FOUND"})
    );
}
