//! Самотесты, перенесённые в Rust: `scripts/hl1-selftest.js` 0.4.9 (HL1 с bxt.rs: ставится только
//! закреплённая сборка, ETag главнее даты, другая сборка в источнике не предлагается) и `scripts/diagnostics-selftest.js`
//! (подписанный канал игры принимается, правка после подписи — `SIGNATURE_INVALID`, пустой
//! документ и чужие ответы канал не пропускают). Установка и обновление архивом из того же
//! самотеста HL1 (7-Zip, `GameDeliveryService`) переносятся с этими модулями.
//!
//! Ниже — проверки Rust сверх самотестов: пауза между попытками, общий запуск при отмене
//! первого, атомарная запись, настоящий файловый манифест из `config/games` через кеш,
//! будущие служб — `Send`.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_core::json_text::stringify;
use hlsr_delivery::atomic::{write_file_atomic, write_json_atomic};
use hlsr_delivery::channel_config::ChannelConfig;
use hlsr_delivery::channel_service::{ChannelOptions, ChannelService, FetchStatus};
use hlsr_delivery::environment::Environment;
use hlsr_delivery::errors::Code;
use hlsr_delivery::manifest_signing::{attach, generate_key_pair};
use hlsr_delivery::manifests::manifest_of;
use hlsr_delivery::origin_updates::{OriginOptions, OriginUpdates, default_origins, same_stamp};
use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::timing::{Random, retry_pause_ms};
use hlsr_delivery::update_checker::{CheckOptions, CheckerOptions, MIN_INTERVAL_MS, UpdateChecker};
use serde_json::{Map, Value, json};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::http::{RequestLog, start_server};

// ---------------------------------------------------------------- hl1-selftest.js (0.4.9)

#[test]
fn hl1_pinned_build_stamp_matches_the_bundled_archive() {
    let (_, origin) = &default_origins()[0];
    let base = json!({
        "version": origin.baseline.version,
        "lastModified": origin.baseline.last_modified,
        "etag": origin.baseline.etag,
        "size": origin.baseline.size,
    });
    assert_eq!(
        json!(origin.baseline.size),
        manifest_of("hl1-won").unwrap()["baseArchive"]["size"],
        "отпечаток соответствует встроенному архиву"
    );
    let mut other_date = base.clone();
    other_date["lastModified"] = json!("другая");
    assert!(same_stamp(&base, &other_date), "ETag главнее даты");
    let mut other_etag = base.clone();
    other_etag["etag"] = json!("\"x\"");
    assert!(!same_stamp(&base, &other_etag));
}

const SIZE_V1: u64 = 1000;
const WON_V1: (&str, &str) = ("Mon, 17 Jul 2023 11:23:57 GMT", "\"v1\"");
const WON_V2: (&str, &str) = ("Sun, 20 Sep 2026 00:05:09 GMT", "\"v2\"");

fn stamp_route(stamp: (&str, &str)) -> Value {
    json!([{ "bodySize": SIZE_V1, "headers": [["Last-Modified", stamp.0], ["ETag", stamp.1]] }])
}

/// Сборка HL1 на bxt.rs (0.4.9, A-2): закреплённая сборка — цель с SHA-256 встроенного
/// манифеста; другая сборка в источнике не предлагается, предупреждение одно на отпечаток.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hl1_another_origin_build_without_a_pinned_sha256_is_not_offered() {
    let log = RequestLog::default();
    let server = start_server("a", log.clone()).await;
    server
        .routes
        .lock()
        .unwrap()
        .insert("/won.7z".to_owned(), stamp_route(WON_V1));

    let origins = vec![(
        "hl1-won".to_owned(),
        hlsr_delivery::origin_updates::Origin {
            provider_id: "bxt-rs".to_owned(),
            baseline: hlsr_delivery::origin_updates::Baseline {
                version: "1.0.0".to_owned(),
                last_modified: WON_V1.0.to_owned(),
                etag: WON_V1.1.to_owned(),
                size: SIZE_V1,
            },
        },
    )];
    let mut manifest = manifest_of("hl1-won").unwrap().clone();
    manifest["baseArchive"]["providers"] = json!([{
        "id": "bxt-rs",
        "type": "local-http",
        "status": "permanent",
        "priority": 10,
        "url": format!("http://{}/won.7z", server.authority),
    }]);
    let pinned = manifest["baseArchive"]["sha256"].clone();

    let logger = Arc::new(MemoryLogger::default());
    let origin = Arc::new(OriginUpdates::new(OriginOptions {
        logger: Some(logger.clone()),
        http: None,
        origins: Some(origins),
    }));
    let local = manifest.clone();
    let checker = UpdateChecker::new(CheckerOptions {
        origin_updates: Some(origin.clone()),
        launcher_version: Some("0.3.16".to_owned()),
        min_interval_ms: Some(0),
        installed_version_of: Some(Arc::new(|_| Some("1.0.0".to_owned()))),
        local_manifest_of: Some(Arc::new(move |_| Some(local.clone()))),
        ..CheckerOptions::default()
    });
    let force = CheckOptions {
        force: true,
        ..CheckOptions::default()
    };

    let check = checker.check("hl1-won", &force).await.to_json();
    assert_eq!(
        check["updateAvailable"], false,
        "та же сборка — обновлять нечего"
    );
    let target = origin
        .target("hl1-won", Some(&manifest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(target["source"], "origin");
    assert_eq!(target["fullArchive"]["sha256"], pinned);

    // На bxt.rs лежит другая сборка: её SHA-256 нигде не закреплён.
    server
        .routes
        .lock()
        .unwrap()
        .insert("/won.7z".to_owned(), stamp_route(WON_V2));
    logger.take();
    for _ in 0..3 {
        let check = checker.check("hl1-won", &force).await.to_json();
        assert_eq!(
            check["updateAvailable"], false,
            "незакреплённая сборка не предлагается"
        );
        assert_eq!(check["error"], Value::Null);
    }
    assert_eq!(
        origin.target("hl1-won", Some(&manifest)).await.unwrap(),
        None
    );
    let warnings: Vec<String> = logger
        .take()
        .into_iter()
        .filter(|(level, message)| *level == "warn" && message.contains("другая сборка"))
        .map(|(_, message)| message)
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("\"v2\", 1000 Б"),
        "отпечаток в записи: {}",
        warnings[0]
    );
}
// ---------------------------------------------------------------- diagnostics-selftest.js

fn provider() -> Value {
    json!({ "id": "primary", "type": "https-direct", "status": "permanent", "url": "https://hlsr.pro/downloads/test" })
}

/// Документ канала игры из `signedChannels` самотеста диагностики.
fn game_channel() -> Map<String, Value> {
    let hash = "a".repeat(64);
    let doc = json!({
        "schemaVersion": 1,
        "gameId": "ghosting-renewed",
        "channel": "stable",
        "version": "1.0.0",
        "publishedAt": "2026-10-03T00:00:00Z",
        "minimumLauncherVersion": "0.3.0",
        "enabled": true,
        "updateRequired": false,
        "fullArchive": { "filename": "test.zip", "size": 1024, "sha256": hash, "providers": [provider()] },
        "filesManifest": { "filename": "files.json", "version": "1.0.0", "size": 512, "sha256": hash, "providers": [provider()] },
    });
    doc.as_object().unwrap().clone()
}

fn trusted(public_key: &str) -> ChannelConfig {
    ChannelConfig {
        game_id: "ghosting-renewed".to_owned(),
        url: None,
        channel: "stable".to_owned(),
        require_signature: true,
        public_keys: vec![public_key.to_owned()],
        source: None,
    }
}

#[test]
fn diagnostics_verify_the_correct_signed_game_channel_and_reject_a_tampered_one() {
    let keys = generate_key_pair();
    let service = ChannelService::new(ChannelOptions::default());
    let signed = Value::Object(attach(&game_channel(), &keys.private_key_pem).unwrap());
    let config = trusted(&keys.public_key_pem);
    assert!(
        service
            .validate(&signed, "ghosting-renewed", &config)
            .is_ok()
    );

    let mut tampered = signed.clone();
    tampered["version"] = json!("1.0.1");
    let error = service
        .validate(&tampered, "ghosting-renewed", &config)
        .unwrap_err();
    assert_eq!(error.code, Code::SignatureInvalid);
}

#[test]
fn empty_or_foreign_metadata_cannot_pass_as_a_healthy_channel() {
    let keys = generate_key_pair();
    let service = ChannelService::new(ChannelOptions::default());
    let config = trusted(&keys.public_key_pem);
    for body in [json!({}), json!(null), json!([]), json!("{}")] {
        assert!(
            service
                .validate(&body, "ghosting-renewed", &config)
                .is_err(),
            "{body} не канал"
        );
    }
    let unsigned = Value::Object(game_channel());
    let error = service
        .validate(&unsigned, "ghosting-renewed", &config)
        .unwrap_err();
    assert_eq!(error.code, Code::SignatureInvalid);
}

/// Канал по сети: ответы, которыми диагностика проверяет канал (самотест подменяет `request`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_channel_over_http_accepts_only_a_correctly_signed_document() {
    let keys = generate_key_pair();
    let signed = Value::Object(attach(&game_channel(), &keys.private_key_pem).unwrap());
    let mut tampered = signed.clone();
    tampered["version"] = json!("1.0.1");
    let log = RequestLog::default();
    let server = start_server("a", log).await;
    *server.routes.lock().unwrap() = json!({
        "/ok.json": [{ "body": signed.to_string() }],
        "/tampered.json": [{ "body": tampered.to_string() }],
        "/empty.json": [{ "body": "{}" }],
        "/truncated.json": [{ "bodySize": 4000, "cutAfter": 100 }],
        "/redirect.json": [{ "status": 302 }],
        "/accepted.json": [{ "status": 202, "body": signed.to_string() }],
    })
    .as_object()
    .unwrap()
    .clone();

    let fetch = |route: &str| {
        let env = Environment::new(
            [
                ("GAME_DELIVERY_ALLOW_LOCAL_HTTP".to_owned(), "1".to_owned()),
                (
                    "HLSR_GAME_DELIVERY_CHANNEL_URL".to_owned(),
                    format!("http://{}{route}", server.authority),
                ),
                (
                    "HLSR_GAME_DELIVERY_CHANNEL_KEY".to_owned(),
                    keys.public_key_pem.replace('\n', "\\n"),
                ),
            ],
            false,
        );
        let service = ChannelService::new(ChannelOptions {
            env: Some(env),
            max_attempts: Some(1),
            ..ChannelOptions::default()
        });
        async move { service.fetch("ghosting-renewed", false).await }
    };

    let fetched = fetch("/ok.json").await.unwrap();
    assert_eq!(fetched.status, FetchStatus::Fresh);
    assert_eq!(
        fetch("/tampered.json").await.unwrap_err().code,
        Code::SignatureInvalid
    );
    for route in [
        "/empty.json",
        "/truncated.json",
        "/redirect.json",
        "/accepted.json",
    ] {
        assert!(fetch(route).await.is_err(), "{route} не здоровый канал");
    }
}

// ---------------------------------------------------------------- сверх самотестов

#[test]
fn the_retry_pause_doubles_and_gets_a_spread_below_one_base() {
    let at = |value: f64| -> Random { Arc::new(move || value) };
    assert_eq!(retry_pause_ms(500, 1, &at(0.0)), 500);
    assert_eq!(retry_pause_ms(500, 2, &at(0.0)), 1000);
    assert_eq!(retry_pause_ms(500, 3, &at(0.5)), 2250);
    assert_eq!(retry_pause_ms(500, 1, &at(0.999_999)), 999);
    // Основа 0: пауза нулевая, разброс — floor(random · 1).
    assert_eq!(retry_pause_ms(0, 1, &at(0.9)), 0);
    assert_eq!(retry_pause_ms(7, 2, &at(0.5)), 17);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_channel_pauses_between_attempts_by_the_real_formula() {
    let log = RequestLog::default();
    let server = start_server("a", log).await;
    *server.routes.lock().unwrap() = json!({ "/c.json": [{ "status": 500 }] })
        .as_object()
        .unwrap()
        .clone();
    let logger = Arc::new(MemoryLogger::default());
    let env = Environment::new(
        [
            ("GAME_DELIVERY_ALLOW_LOCAL_HTTP".to_owned(), "1".to_owned()),
            (
                "HLSR_GAME_DELIVERY_CHANNEL_URL".to_owned(),
                format!("http://{}/c.json", server.authority),
            ),
        ],
        false,
    );
    let service = ChannelService::new(ChannelOptions {
        logger: Some(logger.clone()),
        env: Some(env),
        backoff_base_ms: Some(20),
        ..ChannelOptions::default()
    });
    let started = std::time::Instant::now();
    let error = service.fetch("ghosting-renewed", false).await.unwrap_err();
    assert_eq!(error.code, Code::ChannelUnavailable);
    // Паузы — 20…39 мс и 40…59 мс.
    assert!(started.elapsed() >= Duration::from_millis(60));
    let pauses: Vec<u64> = logger
        .take()
        .iter()
        .map(|(_, line)| {
            let tail = line.split("повтор через ").nth(1).unwrap();
            tail.trim_end_matches(" мс").parse().unwrap()
        })
        .collect();
    assert_eq!(pauses.len(), 2);
    assert!((20..40).contains(&pauses[0]), "{pauses:?}");
    assert!((40..60).contains(&pauses[1]), "{pauses:?}");
}

/// Если первый вызов отменён (будущее сброшено), ожидающие не зависают: проверку берёт на себя
/// следующий. В 0.4.7 промис нельзя отменить, поэтому там такого случая нет.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_check_does_not_strand_the_waiting_ones() {
    let log = RequestLog::default();
    let server = start_server("a", log.clone()).await;
    let keys = generate_key_pair();
    let signed = Value::Object(attach(&game_channel(), &keys.private_key_pem).unwrap());
    // Первый запрос молчит намного дольше теста, второй (за отменённым) отвечает сразу.
    *server.routes.lock().unwrap() = json!({ "/c.json": [
        { "delayMs": 30_000, "body": signed.to_string() },
        { "body": signed.to_string() },
    ] })
    .as_object()
    .unwrap()
    .clone();
    let env = Environment::new(
        [
            ("GAME_DELIVERY_ALLOW_LOCAL_HTTP".to_owned(), "1".to_owned()),
            (
                "HLSR_GAME_DELIVERY_CHANNEL_URL".to_owned(),
                format!("http://{}/c.json", server.authority),
            ),
            (
                "HLSR_GAME_DELIVERY_CHANNEL_KEY".to_owned(),
                keys.public_key_pem.replace('\n', "\\n"),
            ),
        ],
        false,
    );
    let checker = Arc::new(UpdateChecker::new(CheckerOptions {
        channel_service: Some(Arc::new(ChannelService::new(ChannelOptions {
            env: Some(env),
            ..ChannelOptions::default()
        }))),
        installed_version_of: Some(Arc::new(|_| Some("0.9.0".to_owned()))),
        ..CheckerOptions::default()
    }));
    let options = CheckOptions {
        force: true,
        ..CheckOptions::default()
    };
    let first = {
        let (checker, options) = (checker.clone(), options.clone());
        tokio::spawn(async move { checker.check("ghosting-renewed", &options).await })
    };
    // Проверка началась: ждём этого как условие, а не времени.
    for _ in 0..3000 {
        if checker.is_checking("ghosting-renewed") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(checker.is_checking("ghosting-renewed"));
    let waiting = {
        let (checker, options) = (checker.clone(), options.clone());
        tokio::spawn(async move { checker.check("ghosting-renewed", &options).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    first.abort();
    let result = tokio::time::timeout(Duration::from_secs(25), waiting)
        .await
        .expect("ожидающий не завис")
        .unwrap();
    assert_eq!(result.to_json()["availableVersion"], "1.0.0");
    assert!(!checker.is_checking("ghosting-renewed"));
}

/// `run` записывает время проверки в начале. Сброшенная проверка не должна отодвигать
/// следующую на интервал: иначе вызов без `force` вернул бы прежний результат (`fromCache`) и
/// сети не коснулся бы 10 минут. Время возвращается к прежнему.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_check_does_not_hold_back_the_next_one() {
    let game = "ghosting-renewed";
    let log = RequestLog::default();
    let server = start_server("a", log.clone()).await;
    let keys = generate_key_pair();
    let signed = Value::Object(attach(&game_channel(), &keys.private_key_pem).unwrap());
    // Первый запрос молчит, второй отвечает сразу.
    *server.routes.lock().unwrap() = json!({ "/c.json": [
        { "delayMs": 5000, "body": signed.to_string() },
        { "body": signed.to_string() },
    ] })
    .as_object()
    .unwrap()
    .clone();
    let env = Environment::new(
        [
            ("GAME_DELIVERY_ALLOW_LOCAL_HTTP".to_owned(), "1".to_owned()),
            (
                "HLSR_GAME_DELIVERY_CHANNEL_URL".to_owned(),
                format!("http://{}/c.json", server.authority),
            ),
            (
                "HLSR_GAME_DELIVERY_CHANNEL_KEY".to_owned(),
                keys.public_key_pem.replace('\n', "\\n"),
            ),
        ],
        false,
    );
    let now = Arc::new(AtomicI64::new(1_000_000));
    let installed = Arc::new(Mutex::new(None::<String>));
    let checker = Arc::new(UpdateChecker::new(CheckerOptions {
        channel_service: Some(Arc::new(ChannelService::new(ChannelOptions {
            env: Some(env),
            ..ChannelOptions::default()
        }))),
        installed_version_of: Some({
            let installed = installed.clone();
            Arc::new(move |_| installed.lock().unwrap().clone())
        }),
        clock: Some({
            let now = now.clone();
            Arc::new(move || now.load(Ordering::SeqCst))
        }),
        ..CheckerOptions::default()
    }));

    // Прежний результат: игра не установлена, сети нет.
    let before = checker
        .check(game, &CheckOptions::default())
        .await
        .to_json();
    assert_eq!(before["installed"], false);
    *installed.lock().unwrap() = Some("0.9.0".to_owned());
    now.fetch_add(MIN_INTERVAL_MS + 1, Ordering::SeqCst);
    assert!(checker.is_due(game));

    // Ведущая проверка уходит в молчащий сервер и сбрасывается.
    let leader = {
        let checker = checker.clone();
        let options = CheckOptions {
            force: true,
            ..CheckOptions::default()
        };
        tokio::spawn(async move { checker.check(game, &options).await })
    };
    // Запрос ушёл на сервер: ждём этого как условие, а не времени.
    for _ in 0..3000 {
        if checker.is_checking(game) && log.lock().unwrap().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(checker.is_checking(game));
    assert_eq!(log.lock().unwrap().len(), 1);
    leader.abort();
    let _ = leader.await;
    assert!(!checker.is_checking(game));
    assert!(
        checker.is_due(game),
        "время начала сброшенной проверки не считается проверкой"
    );

    // Вызов без `force` идёт в сеть, а не отдаёт прежний результат.
    let after = checker
        .check(game, &CheckOptions::default())
        .await
        .to_json();
    assert!(after.get("fromCache").is_none(), "{after}");
    assert_eq!(after["installed"], true);
    assert_eq!(after["availableVersion"], "1.0.0");
    assert_eq!(log.lock().unwrap().len(), 2);
    // Следующий вызов в пределах интервала — уже из кеша.
    let cached = checker
        .check(game, &CheckOptions::default())
        .await
        .to_json();
    assert_eq!(cached["fromCache"], true);
    assert_eq!(log.lock().unwrap().len(), 2);
}

#[test]
fn the_atomic_write_replaces_the_file_and_leaves_no_temporary_one() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a").join("b.json");
    write_json_atomic(&file, &json!({ "a": [1, 2] })).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        stringify(&json!({ "a": [1, 2] }), "  ") + "\n"
    );
    write_file_atomic(&file, "новое".as_bytes()).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "новое");
    let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["b.json"]);
}

/// Целью переименования оказался каталог: ошибка Node с обоими путями, временный файл убран.
/// Только Linux: в Windows это `EPERM`.
#[cfg(target_os = "linux")]
#[test]
fn the_atomic_write_reports_a_directory_in_the_way_like_node() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("record.json");
    std::fs::create_dir(&target).unwrap();
    let error = write_file_atomic(&target, b"x").unwrap_err();
    assert_eq!(error.code.as_deref(), Some("EISDIR"));
    assert_eq!(error.syscall.as_deref(), Some("rename"));
    assert!(
        error
            .message
            .starts_with("EISDIR: illegal operation on a directory, rename '"),
        "{}",
        error.message
    );
    assert!(
        error
            .message
            .ends_with(&format!("' -> '{}'", target.display()))
    );
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["record.json"]);
}

/// Этап, на котором запись сорвалась, виден в тексте ошибки, как у Node 24: `mkdir '<каталог>'`,
/// `open '<временный файл>'`, `rename '<временный файл>' -> '<файл>'` (тексты сняты с `atomic.js`).
/// Только Linux: коды и пути Windows другие.
#[cfg(target_os = "linux")]
#[test]
fn the_atomic_write_names_the_failed_stage_like_node() {
    let dir = tempfile::tempdir().unwrap();

    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let error = write_file_atomic(&blocker.join("x.cfg"), b"y").unwrap_err();
    assert_eq!(error.syscall.as_deref(), Some("mkdir"));
    assert_eq!(
        error.message,
        format!("EEXIST: file already exists, mkdir '{}'", blocker.display())
    );

    if !rustix::process::geteuid().is_root() {
        use std::os::unix::fs::PermissionsExt;
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let error = write_file_atomic(&locked.join("a.cfg"), b"y").unwrap_err();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(error.syscall.as_deref(), Some("open"));
        assert!(
            temp_open_message(&error.message, "a.cfg"),
            "{}",
            error.message
        );
    }
}

/// `open '<каталог>/.<имя>.<pid>.<время36>.tmp'`.
#[cfg(target_os = "linux")]
fn temp_open_message(message: &str, name: &str) -> bool {
    let pattern = format!(
        r"^EACCES: permission denied, open '.+/\.{}\.\d+\.[0-9a-z]+\.tmp'$",
        regex::escape(name)
    );
    regex::Regex::new(&pattern).unwrap().is_match(message)
}

/// Редактор cfg и перенос сборки пишут через `writeFileAtomic`: сбой записи в каталог без
/// прав называет временный файл (`open '<каталог>/.a.cfg.<pid>.<время36>.tmp'`), а не сам cfg.
#[cfg(target_os = "linux")]
#[test]
fn cfg_files_and_build_transfer_report_the_stage_of_the_atomic_write() {
    use hlsr_delivery::build_transfer::{PickedFile, TransferFailure, apply_files};
    use hlsr_delivery::cfg_files::{self, CfgFilesError};
    use std::os::unix::fs::PermissionsExt;

    if rustix::process::geteuid().is_root() {
        return; // root пишет и в каталог без прав
    }
    let install = tempfile::tempdir().unwrap();
    let sub = install.path().join("valve").join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("a.cfg"), "old\n").unwrap();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();

    let written = cfg_files::write(
        install.path(),
        "valve/sub/a.cfg",
        "new\n",
        &cfg_files::WriteOptions::default(),
    );
    let transfer = apply_files(
        install.path(),
        &[PickedFile {
            path: "valve/sub/a.cfg".to_owned(),
            data: b"new\n".to_vec(),
        }],
    );
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let Err(CfgFilesError::Raw(error)) = written else {
        panic!("ожидалась ошибка файловой системы: {written:?}");
    };
    assert!(
        temp_open_message(&error.message, "a.cfg"),
        "{}",
        error.message
    );
    let Err(TransferFailure::Raw(error)) = transfer else {
        panic!("ожидалась ошибка файловой системы: {transfer:?}");
    };
    assert!(
        temp_open_message(&error.message, "a.cfg"),
        "{}",
        error.message
    );
    assert_eq!(std::fs::read_to_string(sub.join("a.cfg")).unwrap(), "old\n");
}

/// Будущие служб можно отдать другому потоку (`tokio::spawn` в `DeliveryApi`).
#[test]
fn the_futures_of_the_services_are_send() {
    fn assert_send<T: Send>(_: &T) {}
    let dir = tempfile::tempdir().unwrap();
    let paths = hlsr_delivery::paths::resolve_library(&dir.path().to_string_lossy()).unwrap();
    let channel = ChannelService::new(ChannelOptions::default());
    assert_send(&channel.fetch("ghosting-renewed", false));
    let store = hlsr_delivery::files_manifest_store::FilesManifestStore::new(
        &paths,
        hlsr_delivery::files_manifest_store::StoreOptions::default(),
    );
    let descriptor = json!({});
    let options = hlsr_delivery::files_manifest_store::FetchOptions::default();
    assert_send(&store.fetch("ghosting-renewed", &descriptor, &options));
    let origin = OriginUpdates::new(OriginOptions::default());
    assert_send(&origin.probe("hl1-won", None));
    assert_send(&origin.target("hl1-won", None));
    let checker = UpdateChecker::new(CheckerOptions::default());
    assert_send(&checker.check("ghosting-renewed", &CheckOptions::default()));
    assert_send(&checker.check_if_older_than("ghosting-renewed", 1, None));
}

/// Настоящий файловый манифест из `config/games` (сотни килобайт): канал отдаёт его, хранилище
/// кладёт байты в кеш как есть, `load` отдаёт тот же манифест, `trust.json` ему соответствует.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bundled_files_manifest_survives_the_round_trip_through_the_cache() {
    use hlsr_delivery::files_manifest_store::{FetchOptions, FilesManifestStore, StoreOptions};
    use hlsr_delivery::manifests::files_manifests_of;
    use sha2::{Digest, Sha256};

    let (version, embedded) = files_manifests_of("hl2-2707")[0];
    let text = embedded.text();
    let sha256: String = Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let filename = format!("hl2-2707-{version}.files.json");
    let server = start_server("a", RequestLog::default()).await;
    *server.routes.lock().unwrap() = json!({ format!("/m/{filename}"): [{ "body": text }] })
        .as_object()
        .unwrap()
        .clone();

    let library = tempfile::tempdir().unwrap();
    let paths = hlsr_delivery::paths::resolve_library(&library.path().to_string_lossy()).unwrap();
    let store = FilesManifestStore::new(
        &paths,
        StoreOptions {
            env: Some(Environment::new(
                [("GAME_DELIVERY_ALLOW_LOCAL_HTTP".to_owned(), "1".to_owned())],
                false,
            )),
            ..StoreOptions::default()
        },
    );
    let descriptor = json!({
        "version": version,
        "sha256": sha256,
        "size": text.len(),
        "filename": filename,
        "providers": [{
            "id": "mirror-a",
            "type": "local-http",
            "status": "permanent",
            "url": format!("http://{}/m/base.7z", server.authority),
        }],
    });
    let options = FetchOptions {
        channel_sha256: Some("c".repeat(64)),
        source: None,
    };
    let fetched = store
        .fetch("hl2-2707", &descriptor, &options)
        .await
        .unwrap();
    assert_eq!(&fetched, embedded.value());

    let file = store.manifest_path("hl2-2707", version).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), text.as_bytes());
    assert_eq!(
        store.load("hl2-2707", version).as_ref(),
        Some(embedded.value())
    );
    assert_eq!(store.list("hl2-2707"), [version]);

    let trust =
        hlsr_core::json_store::read_json_safe(&store.meta_path("hl2-2707", version).unwrap());
    assert_eq!(trust["metaVersion"], 1);
    assert_eq!(trust["size"], text.len());
    assert_eq!(trust["sha256"], json!(sha256));
    assert_eq!(trust["channelSha256"], json!("c".repeat(64)));
    assert_eq!(
        trust["source"],
        json!({ "id": "mirror-a", "type": "local-http" })
    );
    assert_eq!(trust["totalFiles"], fetched["totalFiles"]);
    assert_eq!(trust["manifestSha256"], fetched["manifestSha256"]);
}
