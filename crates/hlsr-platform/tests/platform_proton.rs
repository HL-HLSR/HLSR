//! Proton в фасаде платформы (`AppManager.js:643-692` 0.4.7): каналы `proton-*` (только при
//! менеджере Proton), события менеджера → уведомления `PROTON_*` (одно приглашение обновиться на
//! версию, ошибка — один раз до следующего успеха) и событие окна `proton-state`; `umu-available`.

use hlsr_core::notifications::NotificationStore;
use hlsr_delivery::api::{Parts, ServiceRequest};
use hlsr_delivery::errors::Failure;
use hlsr_delivery::ports::{Dialogs, OpenDialog, SaveDialog};
use hlsr_delivery::providers::BoxFuture;
use hlsr_platform::api::{
    DeliveryPorts, GameLauncher, LaunchContext, PlatformApi, PlatformFactory, PlatformOptions,
    ProtonPorts, SteamParts,
};
use hlsr_platform::game_process_service::GameProcessService;
use hlsr_platform::linux::proton_manager::{ProtonEvent, ProtonManager, ProtonManagerOptions};
use hlsr_platform::linux::proton_runner::{ProtonRunner, ProtonRunnerOptions, ProtonSelection};
use hlsr_platform::tool_runner::ToolProcessRunner;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Платформа без библиотеки: Proton проверяется без службы доставки.
struct NoLibrary;

impl PlatformFactory for NoLibrary {
    fn steam(&self) -> SteamParts {
        SteamParts::default()
    }
    fn processes(&self, _state_dir: &Path) -> GameProcessService {
        unreachable!("библиотеки нет")
    }
    fn tool_runner(&self) -> ToolProcessRunner {
        unreachable!("библиотеки нет")
    }
    fn delivery(&self, _request: ServiceRequest, _ports: DeliveryPorts) -> Result<Parts, Failure> {
        unreachable!("библиотеки нет")
    }
    fn launcher(&self, _context: LaunchContext) -> Arc<dyn GameLauncher> {
        unreachable!("библиотеки нет")
    }
}

/// Выбор папки: один ответ.
struct PickFolder(Mutex<Vec<PathBuf>>);

impl Dialogs for PickFolder {
    fn show_open_dialog(&self, options: OpenDialog) -> BoxFuture<'_, Vec<PathBuf>> {
        assert_eq!(options.title.as_deref(), Some("Выберите папку Proton"));
        assert_eq!(options.properties, ["openDirectory"]);
        let answer = std::mem::take(&mut *self.0.lock().unwrap());
        Box::pin(async move { answer })
    }

    fn show_save_dialog(&self, _options: SaveDialog) -> BoxFuture<'_, Option<PathBuf>> {
        Box::pin(async { None })
    }
}

struct Setup {
    _dir: tempfile::TempDir,
    root: PathBuf,
    platform: Arc<PlatformApi>,
    notifications: Arc<NotificationStore>,
    dialogs: Arc<PickFolder>,
    events: tokio::sync::broadcast::Receiver<hlsr_delivery::api::DeliveryEvent>,
}

fn setup(umu: Option<&str>) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let manager = ProtonManager::new(ProtonManagerOptions {
        data_dir: data.clone(),
        home: Some(root.join("home")),
        ..ProtonManagerOptions::default()
    });
    let umu = umu.map(str::to_owned);
    let runner = ProtonRunner::new(ProtonRunnerOptions {
        manager: Some(Arc::new(manager.clone()) as Arc<dyn ProtonSelection>),
        data_dir: data.clone(),
        resources_path: root.join("resources"),
        umu_path: Some(Arc::new(move || umu.clone())),
        ..ProtonRunnerOptions::default()
    });
    let notifications = Arc::new(NotificationStore::new(data.join("notifications.json")));
    let dialogs = Arc::new(PickFolder(Mutex::new(Vec::new())));
    let mut options = PlatformOptions::new(Arc::new(NoLibrary));
    options.notifications = Some(notifications.clone());
    options.dialogs = dialogs.clone();
    options.proton = Some(ProtonPorts { manager, runner });
    let platform = PlatformApi::new(options);
    Setup {
        _dir: dir,
        root,
        events: platform.subscribe(),
        platform,
        notifications,
        dialogs,
    }
}

fn event(name: &'static str, payload: Value) -> ProtonEvent {
    ProtonEvent { name, payload }
}

/// Уведомления: `[code, level, detail, meta]` от старых к новым.
fn notices(store: &NotificationStore) -> Vec<Value> {
    let (mut items, _) = store.list();
    items.reverse();
    items
        .into_iter()
        .map(|item| {
            json!([
                item["code"],
                item["level"],
                item["detail"],
                item.get("meta").cloned().unwrap_or(Value::Null)
            ])
        })
        .collect()
}

#[test]
fn manager_events_become_notifications_like_0_4_7() {
    let mut s = setup(None);
    let tag = "wineland-11.0-20261003-hlsr3";
    let next = "wineland-11.1-20261101-hlsr1";
    let later = "wineland-11.2-20261201-hlsr1";
    s.platform
        .on_proton_event(&event("install-started", json!({"version": tag})));
    s.platform
        .on_proton_event(&event("installed", json!({"version": tag, "path": "/p"})));
    s.platform
        .on_proton_event(&event("update-available", json!({"from": tag, "to": next})));
    // То же приглашение — без повтора.
    s.platform
        .on_proton_event(&event("update-available", json!({"from": tag, "to": next})));
    assert_eq!(
        notices(&s.notifications),
        [
            json!(["PROTON_INSTALLING", "info", "11.0-20261003-hlsr3", null]),
            json!(["PROTON_INSTALLED", "info", "11.0-20261003-hlsr3", null]),
            json!(["PROTON_UPDATE_AVAILABLE", "info", "11.0-20261003-hlsr3 → 11.1-20261101-hlsr1",
                   {"newVersion": next, "target": "proton"}]),
        ]
    );
    // Новая версия: прежнее приглашение снимается.
    s.platform.on_proton_event(&event(
        "update-available",
        json!({"from": tag, "to": later}),
    ));
    let codes: Vec<Value> = notices(&s.notifications)
        .into_iter()
        .map(|item| item[2].clone())
        .collect();
    assert_eq!(
        codes,
        [
            json!("11.0-20261003-hlsr3"),
            json!("11.0-20261003-hlsr3"),
            json!("11.0-20261003-hlsr3 → 11.2-20261201-hlsr1"),
        ]
    );
    s.platform.on_proton_event(&event(
        "updated",
        json!({"from": tag, "to": later, "path": "/q"}),
    ));
    let last = notices(&s.notifications);
    assert_eq!(last.len(), 3);
    assert_eq!(
        last[2],
        json!([
            "PROTON_UPDATED",
            "info",
            "11.0-20261003-hlsr3 → 11.2-20261201-hlsr1",
            null
        ])
    );

    // Ошибка загрузки — одно уведомление, пока не было успешного состояния.
    for (index, phase) in ["error", "error", "idle", "error"].into_iter().enumerate() {
        s.platform.on_proton_event(&event(
            "state",
            json!({"phase": phase, "error": format!("сбой {index}")}),
        ));
    }
    let failures: Vec<Value> = notices(&s.notifications)
        .into_iter()
        .filter(|item| item[0] == "PROTON_INSTALL_FAILED")
        .collect();
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert_eq!(failures[0][1], "error");
    assert_eq!(failures[0][2], "сбой 0");
    assert_eq!(failures[1][2], "сбой 3");

    let mut states = Vec::new();
    while let Ok(event) = s.events.try_recv() {
        states.push((event.channel, event.payload["phase"].clone()));
    }
    assert_eq!(
        states,
        [
            ("proton-state", json!("error")),
            ("proton-state", json!("error")),
            ("proton-state", json!("idle")),
            ("proton-state", json!("error")),
        ]
    );
}

#[tokio::test]
async fn proton_channels_like_0_4_7() {
    let mut s = setup(Some("/usr/bin/umu-run"));
    assert_eq!(
        s.platform
            .call("umu-available", &[])
            .await
            .unwrap()
            .unwrap(),
        json!(true)
    );
    let status = s
        .platform
        .call("proton-status", &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status["installed"], json!([]));
    assert_eq!(status["selected"], Value::Null);
    // Пока ничего не выбрано, диагностика имени не получает.
    assert_eq!(s.platform.proton_selected_name(), None);
    assert_eq!(
        s.platform
            .call("proton-cancel", &[])
            .await
            .unwrap()
            .unwrap(),
        json!(false)
    );

    // Диалог отменён — `null`.
    assert_eq!(
        s.platform.call("proton-add", &[]).await.unwrap().unwrap(),
        Value::Null
    );
    // Своя папка с `proton`: сборка добавлена, окно узнаёт о новом состоянии.
    let custom = s.root.join("custom-proton");
    std::fs::create_dir_all(&custom).unwrap();
    std::fs::write(custom.join("proton"), "#!/bin/sh\n").unwrap();
    *s.dialogs.0.lock().unwrap() = vec![custom.clone()];
    let added = s.platform.call("proton-add", &[]).await.unwrap().unwrap();
    assert_eq!(added["name"], "custom-proton", "{added}");
    let event = s.events.try_recv().unwrap();
    assert_eq!(event.channel, "proton-state");
    // Добавленная сборка выбирается сама: диагностика получает её имя.
    assert_eq!(
        s.platform.proton_selected_name().as_deref(),
        Some("custom-proton")
    );

    // Выбор несуществующей сборки — отклонённый промис с текстом менеджера.
    assert_eq!(
        s.platform
            .call("proton-select", &[json!("/nope")])
            .await
            .unwrap(),
        Err("Выбранный Proton не найден. Обновите список версий.".to_owned())
    );
    let selected = s
        .platform
        .call("proton-select", &[added["path"].clone()])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected["path"], added["path"]);
    assert_eq!(
        s.platform
            .call("proton-status", &[])
            .await
            .unwrap()
            .unwrap()["selected"],
        added["path"]
    );

    // Без umu — недоступно.
    let s = setup(None);
    assert!(!s.platform.umu_available());
}
