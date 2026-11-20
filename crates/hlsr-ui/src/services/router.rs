//! Сервисы окна в приложении: `invoke` — по фасадам подпроектов по очереди ([`ChannelService`]:
//! первый, кто знает канал, отвечает; никто — отказ, как `ipcMain.handle` без обработчика),
//! `sendSync` — таблица ответов, события — общий `broadcast` ([`ChannelRouter::emit`]:
//! приложение пересылает туда события фасадов; событие с ожиданием готовности окна —
//! [`ChannelRouter::with_pending`]). Вызовы идут задачами tokio: поток окна только ждёт
//! ответа. Хранилище окна (`store.read/write/quarantine`, `AppManager.js:554-571`) — файлы
//! `library.json` и `settings.json` папки данных ([`ChannelRouter::with_store`]) по правилам
//! `Store.js` (`readState`, `writeState`, `quarantineState`).

use super::{BoxFuture, ChannelService, MainProcess, Reply, ServiceEvent, Standing, StoreRead};
use hlsr_core::json_store::{ReadState, quarantine, read_state, write_state};
use hlsr_core::json_text::JsonFormat;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, mpsc};

/// Сколько событий окно может не забрать, прежде чем начнёт их терять.
const EVENT_CAPACITY: usize = 4096;
/// Файлы, которые окно читает и пишет само (`RENDERER_STORE_NAMES`).
const STORE_NAMES: [&str; 2] = ["library", "settings"];
/// Предел записи окна (`MAX_RENDERER_STORE_BYTES`).
const MAX_STORE_BYTES: usize = 1024 * 1024;

/// Главный процесс окна поверх фасадов по каналам.
pub struct ChannelRouter {
    runtime: Handle,
    channels: Vec<Arc<dyn ChannelService>>,
    sync: Map<String, Value>,
    events: broadcast::Sender<ServiceEvent>,
    /// Папка `userData` для хранилища окна; нет — хранилища нет (файла «нет», запись не идёт).
    store_dir: Option<PathBuf>,
    pending: Vec<PendingEvent>,
    /// Очередь `send` фасадам: одна задача разбирает их по порядку окна.
    sends: mpsc::UnboundedSender<(String, Vec<Value>)>,
    /// Официальна ли сборка ([`ChannelRouter::with_build`]); нет — не официальная.
    build: Option<Arc<dyn Fn() -> Standing + Send + Sync>>,
}

/// Событие для окна, которое могло ещё не подписаться (`pending-event.js` 0.4.9): пока окно не
/// прислало `ready`, первое событие ждёт (сервер отдаёт новые первыми); после отправки не
/// повторяется. Готовность не снимается (окно 0.4.9 шлёт её и после перезагрузки).
struct PendingEvent {
    channel: String,
    ready: String,
    state: Mutex<Pending>,
}

#[derive(Default)]
struct Pending {
    ready: bool,
    held: Option<Value>,
}

/// Текст отказа `invoke` канала без обработчика (Electron: `No handler registered for '…'`).
pub fn no_handler(channel: &str) -> String {
    format!("No handler registered for '{channel}'")
}

impl ChannelRouter {
    /// `channels` — фасады в порядке опроса; `sync` — ответы `sendSync` по каналам.
    pub fn new(
        runtime: Handle,
        channels: Vec<Arc<dyn ChannelService>>,
        sync: Map<String, Value>,
    ) -> ChannelRouter {
        let (sends, mut queue) = mpsc::unbounded_channel::<(String, Vec<Value>)>();
        let services = channels.clone();
        // `ipcMain.on` идёт на одном потоке по порядку: «Главная» → «Демо» не придут наоборот.
        runtime.spawn(async move {
            while let Some((channel, args)) = queue.recv().await {
                let mut known = false;
                for service in &services {
                    if service.call(&channel, &args).await.is_some() {
                        known = true;
                        break;
                    }
                }
                if !known {
                    tracing::debug!("[services] send {channel}: получателя нет");
                }
            }
        });
        ChannelRouter {
            runtime,
            channels,
            sync,
            events: broadcast::channel(EVENT_CAPACITY).0,
            store_dir: None,
            pending: Vec::new(),
            sends,
            build: None,
        }
    }

    /// Официальность сборки — у онлайна (`OnlineApi::standing`).
    pub fn with_build(
        mut self,
        build: impl Fn() -> Standing + Send + Sync + 'static,
    ) -> ChannelRouter {
        self.build = Some(Arc::new(build));
        self
    }

    /// Хранилище окна в папке `dir` (`app.getPath("userData")`).
    pub fn with_store(mut self, dir: impl Into<PathBuf>) -> ChannelRouter {
        self.store_dir = Some(dir.into());
        self
    }

    /// `rendererStorePath(configName)`: только `library` и `settings`.
    fn store_path(&self, name: &str) -> Option<PathBuf> {
        let dir = self.store_dir.as_ref()?;
        STORE_NAMES
            .contains(&name)
            .then(|| dir.join(format!("{name}.json")))
    }

    /// Событие `channel` ждёт сигнала окна `ready` (`send`), как `createPendingEvent` 0.4.9.
    pub fn with_pending(mut self, channel: &str, ready: &str) -> ChannelRouter {
        self.pending.push(PendingEvent {
            channel: channel.to_owned(),
            ready: ready.to_owned(),
            state: Mutex::default(),
        });
        self
    }

    /// Событие окну (`webContents.send`); событие с ожиданием готовности до `ready` держится.
    pub fn emit(&self, event: ServiceEvent) {
        if let Some(pending) = self.pending.iter().find(|p| p.channel == event.channel)
            && let Ok(mut state) = pending.state.lock()
            && !state.ready
        {
            if state.held.is_none() {
                state.held = Some(event.payload);
            }
            return;
        }
        let _ = self.events.send(event);
    }
}

impl MainProcess for ChannelRouter {
    fn invoke(&self, channel: &str, args: Vec<Value>) -> BoxFuture<'static, Reply> {
        let channels = self.channels.clone();
        let channel = channel.to_owned();
        let task = self.runtime.spawn(async move {
            for service in &channels {
                if let Some(reply) = service.call(&channel, &args).await {
                    return reply;
                }
            }
            tracing::debug!("[services] invoke {channel}: обработчика нет");
            Err(no_handler(&channel))
        });
        Box::pin(async move { task.await.unwrap_or_else(|error| Err(error.to_string())) })
    }

    fn send(&self, channel: &str, args: Vec<Value>) {
        let Some(pending) = self.pending.iter().find(|p| p.ready == channel) else {
            // `ipcMain.on` (`discord-rpc-activity`): тот же опрос фасадов, ответ не нужен.
            let _ = self.sends.send((channel.to_owned(), args));
            return;
        };
        let Ok(mut state) = pending.state.lock() else {
            return;
        };
        state.ready = true;
        // Под замком: `emit`, увидевший готовность, не обгонит отложенное событие.
        if let Some(payload) = state.held.take() {
            let _ = self.events.send(ServiceEvent {
                channel: pending.channel.clone(),
                payload,
            });
        }
    }

    fn send_sync(&self, channel: &str, _args: Vec<Value>) -> Value {
        self.sync.get(channel).cloned().unwrap_or(Value::Null)
    }

    /// `renderer-store-read`: чужое имя — `{found: true, valid: false, reason: "io", code: "EACCES"}`.
    fn store_read(&self, name: &str) -> StoreRead {
        let Some(path) = self.store_path(name) else {
            return StoreRead::Invalid;
        };
        match read_state(&path) {
            ReadState::Missing => StoreRead::Missing,
            ReadState::Valid(data) => StoreRead::Found(data),
            ReadState::Unparsable | ReadState::Unreadable { .. } => StoreRead::Invalid,
        }
    }

    /// `renderer-store-write`: объект не больше 1 МиБ в JSON, запись `writeState` (табуляция,
    /// `<файл>.tmp`, переименование).
    fn store_write(&self, name: &str, data: Value) -> bool {
        let Some(path) = self.store_path(name) else {
            return false;
        };
        if !serializable(&data) {
            tracing::warn!("[services] store.write {name}: не объект или больше 1 МиБ");
            return false;
        }
        write_state(&path, &data, JsonFormat::Store)
            .inspect_err(|error| tracing::warn!("[services] store.write {name}: {error}"))
            .is_ok()
    }

    /// `renderer-store-quarantine`: `{ok: true, name}` или `{ok: false, error}`.
    fn store_quarantine(&self, name: &str) -> Value {
        let Some(path) = self.store_path(name) else {
            return json!({"ok": false, "error": "EACCES"});
        };
        quarantine_json(&path)
    }

    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
        self.events.subscribe()
    }

    fn build(&self) -> Standing {
        self.build
            .as_ref()
            .map_or(Standing::Unofficial, |build| build())
    }
}

/// `serializableStoreData`: объект (не массив), `JSON.stringify` не больше 1 МиБ.
fn serializable(data: &Value) -> bool {
    data.is_object() && serde_json::to_string(data).is_ok_and(|text| text.len() <= MAX_STORE_BYTES)
}

fn quarantine_json(path: &Path) -> Value {
    match quarantine(path, SystemTime::now()) {
        Ok(target) => json!({
            "ok": true,
            "name": target.file_name().map(|name| name.to_string_lossy().into_owned()),
        }),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo(&'static str);

    impl ChannelService for Echo {
        fn call<'a>(&'a self, channel: &'a str, args: &'a [Value]) -> BoxFuture<'a, Option<Reply>> {
            Box::pin(async move {
                (channel == self.0).then(|| Ok(json!({"from": self.0, "args": args})))
            })
        }
    }

    #[test]
    fn the_first_service_that_knows_the_channel_answers() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let mut sync = Map::new();
        sync.insert("app-version".into(), json!("0.5.0"));
        let router = ChannelRouter::new(
            runtime.handle().clone(),
            vec![Arc::new(Echo("a")), Arc::new(Echo("b"))],
            sync,
        );
        let answer = runtime.block_on(router.invoke("b", vec![json!(1)]));
        assert_eq!(answer, Ok(json!({"from": "b", "args": [1]})));
        let missing = runtime.block_on(router.invoke("c", Vec::new()));
        assert_eq!(missing, Err(no_handler("c")));
        assert_eq!(router.send_sync("app-version", Vec::new()), json!("0.5.0"));
        assert_eq!(router.send_sync("other", Vec::new()), Value::Null);

        let mut events = router.subscribe();
        router.emit(ServiceEvent {
            channel: "game-delivery-status".into(),
            payload: json!({"launcherId": "70"}),
        });
        assert_eq!(events.try_recv().unwrap().channel, "game-delivery-status");
    }

    /// Пишет каждый вызов в канал теста.
    struct Recorder(Mutex<std::sync::mpsc::Sender<(String, Vec<Value>)>>);

    impl ChannelService for Recorder {
        fn call<'a>(&'a self, channel: &'a str, args: &'a [Value]) -> BoxFuture<'a, Option<Reply>> {
            let _ = self
                .0
                .lock()
                .unwrap()
                .send((channel.to_owned(), args.to_vec()));
            Box::pin(async { Some(Ok(Value::Null)) })
        }
    }

    #[test]
    fn a_plain_send_reaches_the_services_like_ipc_main_on() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .build()
            .unwrap();
        let (sender, calls) = std::sync::mpsc::channel();
        let router = ChannelRouter::new(
            runtime.handle().clone(),
            vec![Arc::new(Recorder(Mutex::new(sender)))],
            Map::new(),
        )
        .with_pending("support-thanks", "support-thanks-ready");
        // Готовность окна — только флаг событий, фасадам не идёт.
        router.send("support-thanks-ready", Vec::new());
        // Порядок окна сохраняется и на нескольких потоках.
        for n in 0..50 {
            router.send("discord-rpc-activity", vec![json!(n)]);
        }
        for n in 0..50 {
            let call = calls.recv_timeout(std::time::Duration::from_secs(5));
            assert_eq!(
                call,
                Ok(("discord-rpc-activity".to_owned(), vec![json!(n)]))
            );
        }
    }

    #[test]
    fn a_pending_event_waits_for_the_window_like_pending_event_js() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let router = ChannelRouter::new(runtime.handle().clone(), Vec::new(), Map::new())
            .with_pending("support-thanks", "support-thanks-ready");
        let mut events = router.subscribe();
        let thanks = |n: u32| ServiceEvent {
            channel: "support-thanks".into(),
            payload: json!({"id": n}),
        };
        // До готовности: держится только первое, прочие каналы идут сразу.
        router.emit(thanks(1));
        router.emit(thanks(2));
        router.emit(ServiceEvent {
            channel: "game-delivery-status".into(),
            payload: json!({}),
        });
        assert_eq!(events.try_recv().unwrap().channel, "game-delivery-status");
        assert!(events.try_recv().is_err());
        router.send("support-thanks-ready", Vec::new());
        assert_eq!(events.try_recv().unwrap(), thanks(1));
        assert!(events.try_recv().is_err());
        // После готовности — сразу; повторная готовность ничего не шлёт.
        router.emit(thanks(3));
        assert_eq!(events.try_recv().unwrap(), thanks(3));
        router.send("support-thanks-ready", Vec::new());
        assert!(events.try_recv().is_err());
    }

    /// Своя временная папка теста (у `hlsr-ui` нет `tempfile`).
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hlsr-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_window_store_reads_and_writes_only_library_and_settings_like_store_js() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let dir = temp_dir("store");
        let router =
            ChannelRouter::new(runtime.handle().clone(), Vec::new(), Map::new()).with_store(&dir);
        assert_eq!(router.store_read("settings"), StoreRead::Missing);
        assert!(router.store_write("settings", json!({"config": {"language": 1}})));
        // writeState: табуляция, без перевода строки в конце.
        assert_eq!(
            std::fs::read_to_string(dir.join("settings.json")).unwrap(),
            "{
	\"config\": {
		\"language\": 1
	}
}"
        );
        assert_eq!(
            router.store_read("settings"),
            StoreRead::Found(json!({"config": {"language": 1}}))
        );
        // Чужие имена и не-объекты: ни чтения, ни записи.
        assert_eq!(router.store_read("launcher-prefs"), StoreRead::Invalid);
        assert!(!router.store_write("launcher-prefs", json!({})));
        assert!(!router.store_write("library", json!([1, 2])));
        let huge = "x".repeat(MAX_STORE_BYTES);
        assert!(!router.store_write("library", json!({"big": huge})));
        assert!(!dir.join("library.json").exists());
        assert_eq!(
            router.store_quarantine("launcher-prefs"),
            json!({"ok": false, "error": "EACCES"})
        );
        // Битый файл: «есть, но не прочитан»; карантин переносит его в `.broken-<время>`.
        std::fs::write(dir.join("library.json"), "{not json").unwrap();
        assert_eq!(router.store_read("library"), StoreRead::Invalid);
        let moved = router.store_quarantine("library");
        assert_eq!(moved["ok"], json!(true));
        let name = moved["name"].as_str().unwrap();
        assert!(name.starts_with("library.json.broken-"), "{name}");
        assert!(dir.join(name).exists());
        assert_eq!(router.store_read("library"), StoreRead::Missing);
        // Без папки данных хранилища нет.
        let bare = ChannelRouter::new(runtime.handle().clone(), Vec::new(), Map::new());
        assert_eq!(bare.store_read("settings"), StoreRead::Invalid);
        assert!(!bare.store_write("settings", json!({})));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
