//! Самотесты 0.4.7, перенесённые в Rust: `scripts/download-settings-selftest.js` (лимит
//! скорости на настоящем HTTP, настройка `download-settings.json`) и
//! `scripts/cancellation-selftest.js` (отмена не переключает зеркало). Время — с тем же
//! допуском, что в самотесте. Случаи с `GameDownloadManager` (пауза, `Range`, `.part`),
//! `ToolService` и зеркалом ремонта переносятся вместе с этими модулями; здесь тот же
//! поток байтов идёт через `read_body` с ограничителем.

mod support;

use hlsr_delivery::download_rate_limiter::DownloadRateLimiter;
use hlsr_delivery::download_settings::DownloadSettings;
use hlsr_delivery::errors::{Code, DeliveryError, Failure};
use hlsr_delivery::http_client::{HttpClient, RequestOptions, read_body};
use hlsr_delivery::mirror_runner::{Attempt, MirrorRequest, MirrorRunner};
use hlsr_delivery::ports::MemoryLogger;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::http::serve_one;

/// Верхний предел для сценариев лимита скорости: без исправления они шли бы минуты или часы
/// (лимит 1 Б/с — 1 КБ/с на сотни килобайт), поэтому запас на нагрузку машины большой.
const SLOW_LIMIT_BOUND: Duration = Duration::from_secs(30);

/// Тело `bodySize` байт, как отдаёт сервер сценария.
fn payload(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

async fn fetch(url: &str, limiter: Option<&DownloadRateLimiter>) -> Result<Vec<u8>, DeliveryError> {
    let response = HttpClient::new()
        .request(url, &RequestOptions::default())
        .await
        .map_err(|failure| match failure {
            Failure::Delivery(error) => error,
            Failure::Raw(raw) => panic!("{raw}"),
        })?;
    read_body(response, Some(64 * 1024 * 1024), limiter).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_limit_paces_real_http_bytes_without_changing_the_archive() {
    let (server, _) = serve_one("/archive", json!({ "bodySize": 128 * 1024 })).await;
    let url = format!("http://{}/archive", server.authority);
    let limiter = DownloadRateLimiter::new(128.0 * 1024.0);
    let start = Instant::now();
    let body = fetch(&url, Some(&limiter)).await.unwrap();
    assert!(
        start.elapsed() >= Duration::from_millis(900),
        "128 КБ при 128 КБ/с — около секунды, а не {:?}",
        start.elapsed()
    );
    assert_eq!(body, payload(128 * 1024));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_downloads_share_one_bandwidth_limit() {
    let (server, _) = serve_one("/archive", json!({ "bodySize": 256 * 1024 })).await;
    let url = format!("http://{}/archive", server.authority);
    let limiter = DownloadRateLimiter::new(512.0 * 1024.0);
    let start = Instant::now();
    let (game, livesplit) = tokio::join!(fetch(&url, Some(&limiter)), fetch(&url, Some(&limiter)));
    assert!(
        start.elapsed() >= Duration::from_millis(900),
        "два файла по 256 КБ делят 512 КБ/с: {:?}",
        start.elapsed()
    );
    assert_eq!(game.unwrap(), payload(256 * 1024));
    assert_eq!(livesplit.unwrap(), payload(256 * 1024));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_a_limit_wakes_an_active_http_download_immediately() {
    let (server, _) = serve_one("/archive", json!({ "bodySize": 512 * 1024 })).await;
    let url = format!("http://{}/archive", server.authority);
    let limiter = Arc::new(DownloadRateLimiter::new(1024.0));
    let start = Instant::now();
    let clear = {
        let limiter = limiter.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            limiter.set_limit(0.0);
        }
    };
    let (body, ()) = tokio::join!(fetch(&url, Some(&limiter)), clear);
    // Старый таймер при 1 КБ/с держал бы 512 КБ минуты: предел с большим запасом на нагрузку.
    assert!(
        start.elapsed() < SLOW_LIMIT_BOUND,
        "снятый лимит не должен держать старый таймер: {:?}",
        start.elapsed()
    );
    assert_eq!(body.unwrap(), payload(512 * 1024));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_a_positive_limit_takes_effect_on_the_current_stream() {
    let (server, _) = serve_one("/archive", json!({ "bodySize": 256 * 1024 })).await;
    let url = format!("http://{}/archive", server.authority);
    let limiter = Arc::new(DownloadRateLimiter::new(1.0));
    let start = Instant::now();
    let raise = {
        let limiter = limiter.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            limiter.set_limit(256.0 * 1024.0);
        }
    };
    let (body, ()) = tokio::join!(fetch(&url, Some(&limiter)), raise);
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < SLOW_LIMIT_BOUND,
        "ожидалась примерно секунда, а не {elapsed:?}"
    );
    assert_eq!(body.unwrap(), payload(256 * 1024));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_interrupts_a_limiter_wait() {
    let (server, _) = serve_one("/archive", json!({ "bodySize": 128 * 1024 })).await;
    let url = format!("http://{}/archive", server.authority);
    let limiter = DownloadRateLimiter::new(1.0);
    let start = Instant::now();
    // Отмена в Rust — сброс будущего: ожидание очереди снимается сразу.
    let cancelled =
        tokio::time::timeout(Duration::from_millis(30), fetch(&url, Some(&limiter))).await;
    assert!(
        cancelled.is_err(),
        "загрузка при 1 Б/с не могла закончиться"
    );
    assert!(start.elapsed() < SLOW_LIMIT_BOUND);
    // Снятое ожидание не держит очередь: следующая загрузка идёт по новому лимиту.
    limiter.set_limit(64.0 * 1024.0 * 1024.0);
    let body = tokio::time::timeout(Duration::from_secs(5), fetch(&url, Some(&limiter)))
        .await
        .expect("очередь ограничителя свободна");
    assert_eq!(body.unwrap(), payload(128 * 1024));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_limited_connection_remains_a_network_error() {
    let (server, _) = serve_one(
        "/archive",
        json!({ "bodySize": 128 * 1024, "cutAfter": 1024 }),
    )
    .await;
    let url = format!("http://{}/archive", server.authority);
    let limiter = DownloadRateLimiter::new(1024.0 * 1024.0);
    let error = fetch(&url, Some(&limiter)).await.unwrap_err();
    assert_eq!(error.code, Code::NetworkError, "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn limited_response_collection_still_rejects_oversized_bodies() {
    let (server, _) = serve_one("/archive", json!({ "bodySize": 64 * 1024 })).await;
    let url = format!("http://{}/archive", server.authority);
    let response = HttpClient::new()
        .request(&url, &RequestOptions::default())
        .await
        .unwrap();
    let limiter = DownloadRateLimiter::new(1024.0 * 1024.0);
    let error = read_body(response, Some(1024), Some(&limiter))
        .await
        .unwrap_err();
    assert_eq!(error.code, Code::HttpError);
}

#[test]
fn download_preference_survives_restart_and_rejects_invalid_ipc_values() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("download-settings.json");
    let mut settings = DownloadSettings::new(&file);
    assert!(!settings.snapshot().initialized);
    assert!(settings.save(&json!(5)).is_ok());
    assert_eq!(DownloadSettings::new(&file).snapshot().speed_limit, 5);
    // -1, Infinity, NaN, "5", null, {}, 3 (Infinity и NaN в JSON не бывает — их нет и в IPC Rust).
    for value in [
        json!(-1),
        json!("5"),
        Value::Null,
        json!({}),
        json!(3),
        json!(5.5),
    ] {
        assert!(settings.save(&value).is_err(), "{value}");
        assert_eq!(DownloadSettings::new(&file).snapshot().speed_limit, 5);
    }
    std::fs::write(&file, "{broken").unwrap();
    let mut broken = DownloadSettings::new(&file);
    assert!(broken.save(&json!(10)).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{broken");
    std::fs::write(&file, r#"{"speedLimit":5}"#).unwrap();
    assert!(
        broken.save(&json!(1)).is_ok(),
        "после починки файла сохранение повторяется"
    );
}

#[test]
fn saved_limit_reaches_the_shared_rate_limiter() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = DownloadSettings::new(root.path().join("download-settings.json"));
    let limiter = settings.rate_limiter().clone();
    assert_eq!(limiter.bytes_per_second(), 0.0);
    settings.save(&json!(2)).unwrap();
    assert_eq!(limiter.bytes_per_second(), 2.0 * 1024.0 * 1024.0);
}

#[tokio::test]
async fn cancelling_a_stream_skips_mirror_retries() {
    let runner = MirrorRunner::new(Arc::new(MemoryLogger::default()));
    let descriptors = [
        json!({ "id": "one", "type": "https-direct", "url": "https://example.test/timer" }),
        json!({ "id": "two", "type": "https-direct", "url": "https://example.test/mirror" }),
    ];
    let attempts = AtomicUsize::new(0);
    let request = MirrorRequest::<Value> {
        artifact: "tool:timer",
        descriptors: &descriptors,
        is_terminal_result: None,
        on_discard_partial: None,
    };
    let error = runner
        .run(request, |_: Attempt| {
            attempts.fetch_add(1, Ordering::SeqCst);
            async {
                Err::<Value, Failure>(
                    DeliveryError::new(Code::Cancelled, "Загрузка остановлена").into(),
                )
            }
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, Code::Cancelled);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_artifact_is_not_downloaded_twice_at_once() {
    let runner = Arc::new(MirrorRunner::new(Arc::new(MemoryLogger::default())));
    let descriptors = vec![json!({ "id": "one", "url": "https://example.test/a" })];
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let wait = Mutex::new(Some(wait));
    let first = {
        let request = MirrorRequest::<Value> {
            artifact: "game:a.7z",
            descriptors: &descriptors,
            is_terminal_result: None,
            on_discard_partial: None,
        };
        runner.run(request, |_: Attempt| {
            let wait = wait.lock().unwrap().take().unwrap();
            async move {
                let _ = wait.await;
                Ok::<Value, Failure>(json!("done"))
            }
        })
    };
    let second = async {
        tokio::task::yield_now().await;
        let request = MirrorRequest::<Value> {
            artifact: "game:a.7z",
            descriptors: &descriptors,
            is_terminal_result: None,
            on_discard_partial: None,
        };
        let result = runner
            .run(request, |_: Attempt| async {
                Ok::<Value, Failure>(json!("second"))
            })
            .await;
        let _ = release.send(());
        result
    };
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.unwrap().value, json!("done"));
    assert_eq!(second.unwrap_err().code, Code::AlreadyRunning);
}

#[tokio::test]
async fn a_dropped_run_releases_its_artifact() {
    let runner = MirrorRunner::new(Arc::new(MemoryLogger::default()));
    let descriptors = vec![json!({ "id": "one", "url": "https://example.test/a" })];
    let request = || MirrorRequest::<Value> {
        artifact: "game:a.7z",
        descriptors: &descriptors,
        is_terminal_result: None,
        on_discard_partial: None,
    };
    let pending = runner.run(request(), |_: Attempt| {
        std::future::pending::<Result<Value, Failure>>()
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), pending)
            .await
            .is_err()
    );
    let done = runner
        .run(request(), |_: Attempt| async {
            Ok::<Value, Failure>(json!(1))
        })
        .await
        .unwrap();
    assert_eq!(done.value, json!(1));
}

#[cfg(not(any(debug_assertions, feature = "dev-hooks")))]
#[test]
fn local_http_is_impossible_in_a_production_build() {
    use hlsr_delivery::providers::{self, ProviderOptions};
    let options = ProviderOptions::with_allowed_provider_types(
        HttpClient::new(),
        ["yandex-public", "https-direct", "local-http"],
    );
    let descriptor =
        json!({ "id": "local", "type": "local-http", "url": "http://127.0.0.1:8080/g.7z" });
    let error = providers::create(&descriptor, &options).err().unwrap();
    assert_eq!(error.code, Code::RemoteDisabled);
}
