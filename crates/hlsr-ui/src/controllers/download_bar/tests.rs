//! Нижняя панель по статусу доставки: главная кнопка (все состояния `mainButton`), метки
//! операций, байты и скорость, цепочка обновлений, Steam-депо, «готово» ещё секунду.

use super::*;
use crate::controllers::game_demo::{Download, GameDemo};
use serde_json::json;

fn status(state: &str, extra: Value) -> Value {
    let mut base = json!({
        "gameId": "hl2-4260",
        "launcherId": "220",
        "state": state,
        "percent": 0,
        "bytesDownloaded": 0,
        "totalBytes": 0,
        "speedBps": 0,
        "etaSeconds": null,
        "installedVersion": null,
        "installable": true,
        "error": null,
        "integrity": null,
        "source": "http",
        "operation": null,
        "chain": null,
        "mainButton": {"state": "not-installed", "action": "install", "enabled": true},
    });
    for (key, value) in extra.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

fn bar_with(lang: Language, delivery: Option<&Value>, hold: &FinishHold) -> BarView {
    view(&Input {
        lang,
        demo: &GameDemo::default(),
        build: "",
        delivery,
        hold,
    })
}

fn bar(delivery: &Value) -> BarView {
    bar_with(Language::En, Some(delivery), &FinishHold::default())
}

fn button(state: &str, action: &str, enabled: bool) -> Value {
    status(
        "ready",
        json!({"mainButton": {"state": state, "action": action, "enabled": enabled}}),
    )
}

#[test]
fn every_main_button_state_has_its_icon_label_and_tone() {
    // (состояние, значок, надпись en, надпись ru, тон)
    let table = [
        ("ready", "fas fa-play", "Play", "Играть", Tone::Ready),
        (
            "not-installed",
            "fas fa-download",
            "Install",
            "Установить",
            Tone::Progress,
        ),
        (
            "episode-not-installed",
            "fas fa-download",
            "Download",
            "Скачать",
            Tone::Progress,
        ),
        (
            "episode-paused",
            "fas fa-play",
            "Resume",
            "Продолжить",
            Tone::Progress,
        ),
        (
            "unavailable",
            "fas fa-ban",
            "Unavailable",
            "Недоступно",
            Tone::Muted,
        ),
        (
            "checking-updates",
            SPIN,
            "Checking...",
            "Проверка...",
            Tone::Progress,
        ),
        (
            "update-available",
            "fas fa-sync-alt",
            "Update",
            "Обновить",
            Tone::Progress,
        ),
        (
            "downloading-update",
            "fas fa-pause",
            "Pause",
            "Пауза",
            Tone::Progress,
        ),
        (
            "update-paused",
            "fas fa-play",
            "Resume",
            "Продолжить",
            Tone::Progress,
        ),
        (
            "verifying-update",
            SPIN,
            "Checking...",
            "Проверка...",
            Tone::Progress,
        ),
        (
            "applying-update",
            SPIN,
            "Updating...",
            "Обновление...",
            Tone::Progress,
        ),
        (
            "update-error",
            "fas fa-redo",
            "Retry",
            "Повторить",
            Tone::Danger,
        ),
        (
            "starting-game",
            SPIN,
            "Starting...",
            "Запуск...",
            Tone::Neutral,
        ),
        (
            "game-running",
            "fas fa-times",
            "Close",
            "Закрыть",
            Tone::Neutral,
        ),
        (
            "stopping-game",
            SPIN,
            "Closing...",
            "Закрытие...",
            Tone::Neutral,
        ),
        (
            "downloading",
            "fas fa-pause",
            "Pause",
            "Пауза",
            Tone::Progress,
        ),
        (
            "download-paused",
            "fas fa-play",
            "Resume",
            "Продолжить",
            Tone::Progress,
        ),
        (
            "installing",
            SPIN,
            "Installing...",
            "Установка...",
            Tone::Progress,
        ),
        (
            "busy",
            SPIN,
            "Please wait...",
            "Подождите...",
            Tone::Progress,
        ),
        ("error", "fas fa-redo", "Retry", "Повторить", Tone::Danger),
        (
            "repair-required",
            "fas fa-wrench",
            "REPAIR",
            "ВОССТАНОВИТЬ",
            Tone::Repair,
        ),
        (
            "update-launcher-required",
            "fas fa-arrow-up",
            "UPDATE LAUNCHER",
            "ОБНОВИТЬ ЛАУНЧЕР",
            Tone::Muted,
        ),
    ];
    assert_eq!(table.len(), BUTTONS.len());
    for (state, icon, en, ru, tone) in table {
        let delivery = button(state, "none", true);
        let english = button_view(Language::En, Some(&delivery), None);
        assert_eq!(english.state, state);
        assert_eq!(
            (english.icon, english.label.as_str(), english.tone),
            (icon, en, tone),
            "{state}"
        );
        let russian = button_view(Language::Ru, Some(&delivery), None);
        assert_eq!(russian.label, ru, "{state}");
    }
    // Незнакомое состояние рисуется как «Играть» (BUTTON.ready, TONES → ready).
    let unknown = button_view(Language::En, Some(&button("brand-new", "play", true)), None);
    assert_eq!(unknown.state, "brand-new");
    assert_eq!(
        (unknown.icon, unknown.label.as_str(), unknown.tone),
        ("fas fa-play", "Play", Tone::Ready)
    );
}

#[test]
fn spinner_icons_belong_to_the_states_mainbutton_marks_as_spinning() {
    use hlsr_delivery::main_button;
    // Кольцо у кнопки — те состояния, где main-button.js ставит spinner: true.
    let probes = [
        (
            "starting-game",
            json!({"state": "downloading"}),
            Some(json!({"state": "starting"})),
            None,
            false,
        ),
        (
            "stopping-game",
            json!({"state": "ready"}),
            Some(json!({"state": "stopping"})),
            None,
            false,
        ),
        (
            "verifying-update",
            json!({"state": "verifying", "operation": "update"}),
            None,
            None,
            false,
        ),
        (
            "applying-update",
            json!({"state": "extracting", "operation": "update"}),
            None,
            None,
            false,
        ),
        (
            "installing",
            json!({"state": "extracting"}),
            None,
            None,
            false,
        ),
        ("busy", json!({"state": "importing"}), None, None, false),
        (
            "checking-updates",
            json!({"state": "ready", "installedVersion": "1"}),
            None,
            None,
            true,
        ),
    ];
    for (state, delivery, process, update, checking) in probes {
        let resolved = main_button::resolve(&main_button::Input {
            delivery: Some(&delivery),
            process: process.as_ref(),
            update: update.as_ref(),
            checking_updates: checking,
        });
        assert_eq!(resolved["state"], state);
        assert_eq!(resolved["spinner"], true, "{state}");
        let style = BUTTONS.iter().find(|style| style.state == state).unwrap();
        assert_eq!(style.icon, SPIN, "{state}");
    }
    // И обратно: кольцо только у этих семи.
    let spinning = BUTTONS.iter().filter(|style| style.icon == SPIN).count();
    assert_eq!(spinning, 7);
}

#[test]
fn the_button_presses_only_known_actions_when_enabled() {
    let none = button_view(Language::En, None, None);
    assert_eq!(
        (none.state.as_str(), none.action.as_str(), none.enabled),
        ("ready", "play", true)
    );
    assert_eq!(none.pressed(), Some("play"));

    for action in ACTIONS {
        let view = button_view(Language::En, Some(&button("ready", action, true)), None);
        assert_eq!(view.pressed(), Some(action));
    }
    // `none` (кнопка крутится) и незнакомое действие не нажимаются.
    for action in ["none", "dance", ""] {
        let view = button_view(Language::En, Some(&button("busy", action, true)), None);
        assert_eq!(view.pressed(), None);
    }
    // enabled === false выключает кнопку; отсутствие поля — включено.
    let off = button_view(Language::En, Some(&button("ready", "play", false)), None);
    assert!(!off.enabled);
    assert_eq!(off.pressed(), None);
    let missing = json!({"mainButton": {"state": "ready", "action": "play"}});
    assert!(button_view(Language::En, Some(&missing), None).enabled);
    // Статус без mainButton — «Играть».
    let bare = json!({"state": "downloading"});
    assert_eq!(button_view(Language::En, Some(&bare), None).label, "Play");
}

#[test]
fn status_labels_follow_the_delivery_state() {
    let labels = [
        ("not-installed", "Build is not installed"),
        ("preparing", "Preparing..."),
        ("downloading", "Downloading build"),
        ("paused", "Download paused"),
        ("verifying", "Verifying the archive"),
        ("extracting", "Extracting files"),
        ("finalizing", "Finishing installation"),
        ("applying-update", "Applying update"),
        ("checking-files", "Verifying files"),
        ("files-valid", "All files are valid"),
        ("files-damaged", "Damaged files found"),
        ("repairing", "Repairing files"),
        ("repair-complete", "Files repaired"),
        ("exporting", "Exporting build"),
        ("importing", "Importing build"),
        ("ready", "Build installed"),
        ("cancelled", "Installation cancelled"),
        ("error", "Installation failed"),
        // Незнакомое состояние — как «не установлена».
        ("teleporting", "Build is not installed"),
    ];
    for (state, text) in labels {
        assert_eq!(bar(&status(state, json!({}))).status_text, text, "{state}");
    }
    let downloading = status("downloading", json!({}));
    let ru = bar_with(Language::Ru, Some(&downloading), &FinishHold::default());
    assert_eq!(ru.status_text, "Загрузка сборки");
    // Сборка недоступна для установки.
    let closed = status("not-installed", json!({"installable": false}));
    assert_eq!(
        bar(&closed).status_text,
        "Build installation is not available yet"
    );
}

#[test]
fn errors_show_their_code_text_or_the_general_one() {
    let with = |code: Value| status("error", json!({"error": {"code": code}}));
    assert_eq!(
        bar(&with(json!("NO_SPACE"))).status_text,
        "Not enough free disk space"
    );
    let no_space = with(json!("NO_SPACE"));
    assert_eq!(
        bar_with(Language::Ru, Some(&no_space), &FinishHold::default()).status_text,
        "Недостаточно места на диске"
    );
    assert_eq!(
        bar(&with(json!("SOMETHING_NEW"))).status_text,
        "Installation failed"
    );
    assert_eq!(bar(&with(json!(null))).status_text, "Installation failed");
    // Нет объекта ошибки — метка состояния.
    assert_eq!(
        bar(&status("error", json!({}))).status_text,
        "Installation failed"
    );
}

#[test]
fn damaged_files_count_and_show_the_bar() {
    let damaged = status(
        "files-damaged",
        json!({"integrity": {"damaged": 3}, "checked": 120, "totalChecked": 450}),
    );
    let view = bar(&damaged);
    assert!(view.show_status);
    assert_eq!(view.status_text, "Damaged files: 3");
    assert_eq!(
        (view.done_text.as_str(), view.total_text.as_str()),
        ("120", "450 files")
    );
    // Без сведений о повреждениях — общая метка.
    assert_eq!(
        bar(&status("files-damaged", json!({}))).status_text,
        "Damaged files found"
    );

    let checking = status(
        "checking-files",
        json!({"checked": 5, "totalChecked": 10, "percent": 50}),
    );
    let view = bar(&checking);
    assert_eq!(
        (view.done_text.as_str(), view.total_text.as_str()),
        ("5", "10 files")
    );
    assert_eq!(view.right_meta, "50%");
    let repairing = status("repairing", json!({"checked": 0, "totalChecked": 7}));
    assert_eq!(bar(&repairing).total_text, "7 files");
}

#[test]
fn an_update_chain_names_its_step_and_version() {
    let chain = |step: i64, steps: i64| {
        json!({
            "step": step,
            "steps": steps,
            "toVersion": "1.0.9",
            "doneBytes": 100 * 1024 * 1024,
            "totalBytes": 300 * 1024 * 1024,
        })
    };
    let update = |state: &str, chain: Value| {
        status(
            state,
            json!({
                "operation": "update",
                "chain": chain,
                "bytesDownloaded": 50 * 1024 * 1024,
                "installedVersion": "1.0.0",
            }),
        )
    };
    let text = |state: &str, chain: Value| bar(&update(state, chain)).status_text;
    assert_eq!(text("downloading", chain(2, 3)), "Updating, step 2 of 3");
    assert_eq!(text("downloading", chain(1, 1)), "Downloading update 1.0.9");
    assert_eq!(text("paused", chain(1, 3)), "Downloading update 1.0.9");
    assert_eq!(text("preparing", chain(1, 1)), "Verifying update");
    assert_eq!(text("verifying", chain(1, 1)), "Verifying update");
    assert_eq!(text("extracting", chain(1, 1)), "Applying update");
    assert_eq!(text("applying-update", Value::Null), "Applying update");
    // Версия цепочки пуста, когда цепочки нет.
    assert_eq!(text("downloading", Value::Null), "Downloading update ");
    let chained = update("downloading", chain(2, 3));
    assert_eq!(
        bar_with(Language::Ru, Some(&chained), &FinishHold::default()).status_text,
        "Обновление, шаг 2 из 3"
    );
    // Байты цепочки: сделано + скачано в этом шаге из общего объёма цепочки.
    let view = bar(&chained);
    assert_eq!(
        (view.done_text.as_str(), view.total_text.as_str()),
        ("150 MB", "300 MB")
    );
}

#[test]
fn steam_depot_counts_files_and_cannot_be_cancelled() {
    let depot = status(
        "downloading",
        json!({"source": "steam-depot", "filesDone": 12, "filesTotal": 340, "percent": 4}),
    );
    let view = bar(&depot);
    assert_eq!(view.status_text, "Downloading from Steam…");
    assert_eq!(
        (view.done_text.as_str(), view.total_text.as_str()),
        ("12", "340 files")
    );
    assert!(!view.can_cancel);
    assert!(view.show_status);
    // Подготовка тоже считается загрузкой Steam; проверка — уже нет.
    let preparing = status("preparing", json!({"source": "steam-depot"}));
    assert_eq!(bar(&preparing).status_text, "Downloading from Steam…");
    let verifying = status("verifying", json!({"source": "steam-depot"}));
    assert_eq!(bar(&verifying).status_text, "Verifying the archive");
    // Пустые счётчики — нули.
    let empty = status("downloading", json!({"source": "steam-depot"}));
    assert_eq!(bar(&empty).total_text, "0 files");
}

#[test]
fn bytes_speed_and_time_left() {
    let mb = 1024.0 * 1024.0;
    let downloading = status(
        "downloading",
        json!({
            "percent": 33.5,
            "bytesDownloaded": 500.0 * mb,
            "totalBytes": 1_945_918_366u64,
            "speedBps": 5.0 * mb,
            "etaSeconds": 600,
        }),
    );
    let view = bar(&downloading);
    assert_eq!(view.percent, 34.0);
    assert_eq!(
        (view.done_text.as_str(), view.total_text.as_str()),
        ("500 MB", "1.86 GB")
    );
    assert_eq!(view.right_meta, "34% · 5.0 MB/s · 10 min left");
    assert!(!view.fill_done);

    let seconds = status(
        "downloading",
        json!({"percent": 10, "speedBps": 1.5 * mb, "etaSeconds": 45}),
    );
    assert_eq!(bar(&seconds).right_meta, "10% · 1.5 MB/s · 45 s left");
    assert_eq!(
        bar_with(Language::Ru, Some(&seconds), &FinishHold::default()).right_meta,
        "10% · 1,5 МБ/с · осталось 45 с"
    );
    // Время неизвестно: хвост пустой, разделитель остаётся (как в 0.4.7).
    let unknown = status("downloading", json!({"percent": 10, "speedBps": mb}));
    assert_eq!(bar(&unknown).right_meta, "10% · 1.0 MB/s · ");
    // Нет скорости (пауза, другое состояние) — только проценты.
    let silent = status("downloading", json!({"percent": 10}));
    assert_eq!(bar(&silent).right_meta, "10%");
    let paused = status(
        "paused",
        json!({"percent": 10, "speedBps": mb, "etaSeconds": 5}),
    );
    assert_eq!(bar(&paused).right_meta, "10%");
    // Нет процента — 0; дробные секунды печатаются как число JS.
    assert_eq!(bar(&status("verifying", json!({}))).percent, 0.0);
    let fractional = status(
        "downloading",
        json!({"percent": 1, "speedBps": mb, "etaSeconds": 12.5}),
    );
    assert_eq!(bar(&fractional).right_meta, "1% · 1.0 MB/s · 12.5 s left");
}

#[test]
fn the_status_block_shows_while_there_is_progress_to_show() {
    for state in [
        "preparing",
        "downloading",
        "verifying",
        "extracting",
        "finalizing",
        "applying-update",
        "checking-files",
        "repairing",
        "exporting",
        "importing",
        "paused",
        "files-damaged",
    ] {
        assert!(bar(&status(state, json!({}))).show_status, "{state}");
    }
    for state in [
        "not-installed",
        "ready",
        "files-valid",
        "repair-complete",
        "error",
        "cancelled",
    ] {
        assert!(!bar(&status(state, json!({}))).show_status, "{state}");
    }
}

#[test]
fn the_cancel_button_shows_for_cancellable_operations_of_ordinary_sources() {
    for state in [
        "preparing",
        "downloading",
        "paused",
        "verifying",
        "extracting",
        "exporting",
        "importing",
    ] {
        assert!(bar(&status(state, json!({}))).can_cancel, "{state}");
        let depot = status(state, json!({"source": "steam-depot"}));
        assert!(!bar(&depot).can_cancel, "{state} steam");
    }
    for state in [
        "finalizing",
        "applying-update",
        "checking-files",
        "repairing",
        "ready",
        "error",
    ] {
        assert!(!bar(&status(state, json!({}))).can_cancel, "{state}");
    }
    assert!(!bar_with(Language::En, None, &FinishHold::default()).can_cancel);
}

#[test]
fn before_the_status_arrives_the_demo_download_shows() {
    let view = |download: Option<Download>| {
        let demo = GameDemo {
            download,
            ..GameDemo::default()
        };
        super::view(&Input {
            lang: Language::En,
            demo: &demo,
            build: "",
            delivery: None,
            hold: &FinishHold::default(),
        })
    };
    let nothing = view(None);
    assert!(!nothing.show_status);
    assert_eq!(nothing.percent, 0.0);
    assert_eq!(nothing.button.label, "Play");
    assert_eq!(nothing.game_id, None);

    let installed = view(Some(Download {
        done: 513.0,
        total: 513.0,
    }));
    assert!(installed.show_status);
    assert_eq!(installed.status_text, "Build installed");
    assert_eq!(
        (installed.done_text.as_str(), installed.total_text.as_str()),
        ("513 MB", "513 MB")
    );
    assert_eq!(installed.percent, 100.0);
    assert!(installed.fill_done);

    let loading = view(Some(Download {
        done: 1234.0,
        total: 2000.0,
    }));
    assert_eq!(loading.status_text, "Downloading files...");
    assert_eq!(
        (loading.done_text.as_str(), loading.total_text.as_str()),
        ("1.23 GB", "2.00 GB")
    );
    assert_eq!(loading.percent, 62.0);
    assert!(!loading.fill_done);
    assert_eq!(loading.right_meta, "62%");
}

#[test]
fn the_menu_asks_about_the_game_of_the_current_status() {
    assert_eq!(
        bar(&status("ready", json!({}))).game_id.as_deref(),
        Some("hl2-4260")
    );
    assert_eq!(
        bar_with(Language::En, None, &FinishHold::default()).game_id,
        None
    );
}

#[test]
fn success_holds_the_full_bar_for_a_second_then_lets_go() {
    let mb = 1024.0 * 1024.0;
    let downloading = status(
        "downloading",
        json!({
            "percent": 99,
            "bytesDownloaded": 700.0 * mb,
            "totalBytes": 700.0 * mb,
            "operation": "update",
        }),
    );
    // Панель создана при готовой игре; операция началась позже (так её и видит `watched`).
    let idle = status("ready", json!({}));
    let mut hold = FinishHold::new(Some(&idle));
    assert_eq!(hold.observe(Some(&idle)), HoldTimer::Keep);
    assert_eq!(hold.observe(Some(&downloading)), HoldTimer::Clear);
    assert!(!hold.active());
    assert_eq!(hold.observe(Some(&downloading)), HoldTimer::Keep);

    // main уже снял операцию с карточки: «Обновление завершено» — по прошлому состоянию.
    let ready = status(
        "ready",
        json!({
            "installedVersion": "1.0.9",
            "totalBytes": 700.0 * mb,
            "mainButton": {"state": "ready", "action": "play", "enabled": true},
        }),
    );
    assert_eq!(hold.observe(Some(&ready)), HoldTimer::Start);
    assert!(hold.active());
    let held = bar_with(Language::En, Some(&ready), &hold);
    assert!(held.show_status);
    assert_eq!(held.percent, 100.0);
    assert_eq!(held.status_text, "Update complete");
    assert_eq!(held.right_meta, "100%");
    assert_eq!(
        (held.done_text.as_str(), held.total_text.as_str()),
        ("700 MB", "700 MB")
    );
    // Операция кончилась: полоса на 100 %, но уже бледная (`done`).
    assert!(held.fill_done);

    // Тот же статус повторно — ничего не меняется, таймер не перезапускается.
    assert_eq!(hold.observe(Some(&ready)), HoldTimer::Keep);
    assert!(hold.active());

    hold.expire();
    assert!(!hold.active());
    let after = bar_with(Language::En, Some(&ready), &hold);
    assert!(!after.show_status);
    assert_eq!(after.status_text, "Build installed");
    assert_eq!(FINISH_HOLD, Duration::from_millis(1000));
}

#[test]
fn the_finish_text_depends_on_how_the_operation_ended() {
    let finished = |before: Value, after: Value| {
        let idle = status("not-installed", json!({}));
        let mut hold = FinishHold::new(Some(&idle));
        hold.observe(Some(&before));
        assert_eq!(hold.observe(Some(&after)), HoldTimer::Start);
        bar_with(Language::En, Some(&after), &hold).status_text
    };
    let busy = |state: &str, operation: Value| status(state, json!({"operation": operation}));
    assert_eq!(
        finished(
            busy("checking-files", json!(null)),
            status("files-valid", json!({}))
        ),
        "All files are valid"
    );
    assert_eq!(
        finished(
            busy("repairing", json!(null)),
            status("repair-complete", json!({}))
        ),
        "Files repaired"
    );
    assert_eq!(
        finished(
            busy("exporting", json!("export")),
            status("ready", json!({}))
        ),
        "Export complete"
    );
    assert_eq!(
        finished(
            busy("importing", json!("import")),
            status("ready", json!({}))
        ),
        "Import complete"
    );
    assert_eq!(
        finished(busy("downloading", json!(null)), status("ready", json!({}))),
        "Installation complete"
    );
    // Операция берётся и из нового статуса, если main её оставил.
    assert_eq!(
        finished(
            busy("extracting", json!(null)),
            status("ready", json!({"operation": "update"}))
        ),
        "Update complete"
    );
}

#[test]
fn only_progress_to_success_of_the_same_game_holds() {
    let downloading = status("downloading", json!({}));
    let ready = status("ready", json!({}));

    // Первый статус «готово» (до него статуса не было) ничего не удерживает.
    let mut hold = FinishHold::new(None);
    assert_eq!(hold.observe(Some(&ready)), HoldTimer::Clear);
    assert!(!hold.active());

    // Из паузы в успех — удерживает.
    let paused = status("paused", json!({}));
    let mut hold = FinishHold::new(Some(&paused));
    assert_eq!(hold.observe(Some(&ready)), HoldTimer::Start);

    // Другая игра: статус «готово» после чужой загрузки не удерживается.
    let mut hold = FinishHold::new(Some(&downloading));
    let other = status("ready", json!({"gameId": "hl1-won"}));
    assert_eq!(hold.observe(Some(&other)), HoldTimer::Clear);
    assert!(!hold.active());

    // Удержание снимается, когда игра сменилась или началась новая операция.
    let mut hold = FinishHold::new(Some(&downloading));
    assert_eq!(hold.observe(Some(&ready)), HoldTimer::Start);
    assert_eq!(hold.observe(Some(&other)), HoldTimer::Clear);
    assert!(!hold.active());
    let mut hold = FinishHold::new(Some(&downloading));
    hold.observe(Some(&ready));
    assert_eq!(hold.observe(Some(&downloading)), HoldTimer::Clear);
    assert!(!hold.active());

    // Статус пропал (игра не выбрана): удержание снимается.
    let mut hold = FinishHold::new(Some(&downloading));
    hold.observe(Some(&ready));
    assert_eq!(hold.observe(None), HoldTimer::Clear);
    assert!(!hold.active());

    // Уход панели (beforeUnmount).
    let mut hold = FinishHold::new(Some(&downloading));
    hold.observe(Some(&ready));
    hold.reset();
    assert!(!hold.active());
}

/// Ghosting с эпизодами в «Версии сборки» (`launcher-data.js` 0.4.9). Эпизоды заданы явно:
/// на Linux `GHOSTING_EPISODES` пуст, а логика панели общая и проверяется на обеих системах.
fn ghosting_view(build: &str, delivery: &Value, lang: Language) -> BarView {
    let demo = GameDemo {
        episodes: &[("Episode One", "ep1"), ("Episode Two", "ep2")],
        ..GameDemo::default()
    };
    view(&Input {
        lang,
        demo: &demo,
        build,
        delivery: Some(delivery),
        hold: &FinishHold::default(),
    })
}

fn ghosting(state: &str, main: Value, episodes: Value) -> Value {
    status(
        state,
        json!({"gameId": "ghosting-renewed", "launcherId": "218", "mainButton": main, "episodes": episodes}),
    )
}

#[test]
fn a_selected_episode_swaps_only_the_play_button() {
    let ready = json!({"state": "ready", "action": "play", "enabled": true});
    let gb = 1024.0 * 1024.0 * 1024.0;
    let none = json!({
        "ep1": {"installed": false, "size": 2.0 * gb, "paused": false},
        "ep2": {"installed": false, "size": 0.7 * gb, "paused": false},
    });
    let delivery = ghosting("ready", ready.clone(), none.clone());
    // Сама Ghosting: «Играть», бейджи у обоих эпизодов — их общий размер.
    let base = ghosting_view("HL1 MOVEMENT", &delivery, Language::Ru);
    assert_eq!(base.button.state, "ready");
    assert_eq!(base.menu_episode, None);
    assert_eq!(
        base.build_badges,
        [
            ("Episode One", "2,7\u{a0}ГБ".to_owned()),
            ("Episode Two", "2,7\u{a0}ГБ".to_owned())
        ]
    );
    // Эпизод не скачан: «Скачать» тоном progress, нажатие — свой канал.
    let one = ghosting_view("Episode One", &delivery, Language::Ru);
    assert_eq!(
        (one.button.state.as_str(), one.button.label.as_str()),
        ("episode-not-installed", "Скачать")
    );
    assert_eq!(one.button.tone, Tone::Progress);
    assert_eq!(one.button.pressed(), Some(INSTALL_EPISODE));
    assert_eq!(one.menu_episode, Some(false));
    assert!(!one.can_cancel && one.paused_episode.is_none());

    // Эпизод на паузе: «Продолжить» и отмена только его архива.
    let paused = json!({
        "ep1": {"installed": true, "size": 2.0 * gb},
        "ep2": {"installed": false, "size": 0.7 * gb, "paused": true},
    });
    let two = ghosting_view(
        "Episode Two",
        &ghosting("ready", ready.clone(), paused),
        Language::En,
    );
    assert_eq!(
        (two.button.state.as_str(), two.button.label.as_str()),
        ("episode-paused", "Resume")
    );
    assert_eq!(two.button.pressed(), Some(INSTALL_EPISODE));
    assert!(two.can_cancel);
    assert_eq!(two.paused_episode, Some("ep2"));
    // Скачан только Episode One — бейдж только у второго, размер — его.
    assert_eq!(
        two.build_badges,
        [("Episode Two", "716.8\u{a0}MB".to_owned())]
    );

    // Скачан: «Играть» Ghosting, меню удаляет эпизод.
    let installed = json!({"ep1": {"installed": true, "size": 2.0 * gb}});
    let one = ghosting_view(
        "Episode One",
        &ghosting("ready", ready, installed),
        Language::En,
    );
    assert_eq!(one.button.pressed(), Some("play"));
    assert_eq!(one.menu_episode, Some(true));
    assert!(one.build_badges.is_empty());

    // Всё, что не «Играть» (обновление, загрузка, ошибка), — кнопка Ghosting.
    let update = json!({"state": "update-available", "action": "update", "enabled": true});
    let one = ghosting_view(
        "Episode One",
        &ghosting("ready", update, none.clone()),
        Language::En,
    );
    assert_eq!(one.button.pressed(), Some("update"));
    // Без размера хоть одного недостающего эпизода (манифеста нет) — бейджей нет.
    let unknown = json!({
        "ep1": {"installed": false, "size": 2.0 * gb},
        "ep2": {"installed": false, "size": null},
    });
    let base = ghosting_view(
        "HL1 MOVEMENT",
        &ghosting("ready", json!({"state": "ready"}), unknown),
        Language::En,
    );
    assert!(base.build_badges.is_empty());
    // Без статуса эпизодов — тоже, а кнопка эпизода считает его нескачанным.
    let bare = ghosting(
        "ready",
        json!({"state": "ready", "action": "play"}),
        Value::Null,
    );
    let one = ghosting_view("Episode One", &bare, Language::En);
    assert!(one.build_badges.is_empty());
    assert_eq!(one.button.state, "episode-not-installed");
}

#[test]
fn small_downloads_count_kilobytes_and_numbers_follow_the_language() {
    let mb = 1024.0 * 1024.0;
    // Докачка пары cfg: всего меньше мегабайта — КБ вверх.
    let cfg = status(
        "downloading",
        json!({"bytesDownloaded": 1500, "totalBytes": 40_000}),
    );
    let view = bar(&cfg);
    assert_eq!(
        (view.done_text.as_str(), view.total_text.as_str()),
        ("2 KB", "40 KB")
    );
    let ru = bar_with(Language::Ru, Some(&cfg), &FinishHold::default());
    assert_eq!(ru.total_text, "40 КБ");
    // Десятичный знак языка: точка в en, запятая в ru.
    let big = status(
        "downloading",
        json!({
            "bytesDownloaded": 500.0 * mb,
            "totalBytes": 1_945_918_366u64,
            "speedBps": 5.0 * mb,
            "etaSeconds": 30,
        }),
    );
    assert_eq!(bar(&big).total_text, "1.86 GB");
    assert_eq!(bar(&big).right_meta, "0% · 5.0 MB/s · 30 s left");
    let ru = bar_with(Language::Ru, Some(&big), &FinishHold::default());
    assert_eq!(ru.total_text, "1,86 ГБ");
    // «Загрузка файлов» при докачке ремонта, пока она готовится.
    let repair = status("preparing", json!({"operation": "repair-download"}));
    assert_eq!(bar(&repair).status_text, "Downloading files...");
    let plain = status("preparing", json!({}));
    assert_ne!(bar(&plain).status_text, "Downloading files...");
}
