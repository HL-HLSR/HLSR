//! Самотесты 0.4.7 для `GameDownloadManager`, перенесённые в Rust:
//! `scripts/download-settings-selftest.js` (лимит скорости на настоящем HTTP: темп, снятие и
//! смена лимита, пауза и `Range` под лимитом, отмена во время ожидания очереди, обрыв под
//! лимитом) и `scripts/cancellation-selftest.js` (отмена в паузе между попытками, удаление
//! остановленной загрузки), зеркало ремонта под тем же лимитом. Время — с тем же допуском, что в
//! самотестах. Случай самотеста с `ToolService` (общий лимит игры и LiveSplit) — здесь же.
//!
//! Сервер с `Range`, как в самотесте, — свой (`range_server`): сценарный сервер отвечает
//! заранее заданным списком, а здесь ответ зависит от заголовка запроса. Сценарии по
//! фикстуре 0.4.7 — в `tests/download.rs`. Ниже же — то, что в 0.4.7 держится на устройстве
//! кода и снимается только так: сброшенный `start`, число попыток по умолчанию, разброс и
//! предел паузы между попытками.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_delivery::download_rate_limiter::DownloadRateLimiter;
use hlsr_delivery::errors::{Code, Failure};
use hlsr_delivery::game_download_manager::{
    DownloadEvent, DownloadOptions, DownloadOutcome, DownloadRequest, GameDownloadManager,
    StateFile, StateName,
};
use hlsr_delivery::paths::resolve_library;
use hlsr_delivery::providers::base_provider::BaseProvider;
use hlsr_delivery::providers::{self, BoxFuture, Provider, ProviderOptions, Resolved};
use hlsr_delivery::repair_mirror::{RepairMirrorClient, RepairMirrorOptions};
use hlsr_delivery::tool_service::{ToolOptions, ToolService};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use support::http::{RequestLog, serve_one, start_server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Верхний предел для сценариев лимита скорости: без исправления они шли бы минуты или часы
/// (лимит 1 Б/с — 1 КБ/с на сотни килобайт), поэтому запас на нагрузку машины большой.
const SLOW_LIMIT_BOUND: Duration = Duration::from_secs(30);

fn payload(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Сервер архива, как в самотесте 0.4.7: `Range: bytes=N-` — `206` с `Content-Range`;
/// `broken` — отдать 1024 байта и оборвать.
struct RangeServer {
    url: String,
    ranges: Arc<Mutex<Vec<u64>>>,
    task: JoinHandle<()>,
}

impl Drop for RangeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn range_server(payload: Arc<Vec<u8>>, broken: bool) -> RangeServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let ranges: Arc<Mutex<Vec<u64>>> = Arc::default();
    let log = ranges.clone();
    let task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (payload, log) = (payload.clone(), log.clone());
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut chunk = [0u8; 2048];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&chunk[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&head).to_lowercase();
                let offset = text
                    .split("range: bytes=")
                    .nth(1)
                    .and_then(|rest| rest.split('-').next())
                    .and_then(|n| n.trim().parse::<u64>().ok())
                    .unwrap_or(0);
                if offset != 0 {
                    log.lock().unwrap().push(offset);
                }
                let total = payload.len() as u64;
                let body = &payload[offset as usize..];
                let mut response = if offset != 0 {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\ncontent-length: {}\r\ncontent-range: \
                         bytes {offset}-{}/{total}\r\n\r\n",
                        body.len(),
                        total - 1
                    )
                } else {
                    format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len())
                }
                .into_bytes();
                if broken {
                    response.extend_from_slice(&body[..1024]);
                    let _ = stream.write_all(&response).await;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    return;
                }
                response.extend_from_slice(body);
                let _ = stream.write_all(&response).await;
                let _ = stream.flush().await;
                // Дать клиенту дочитать, не закрывая раньше времени.
                tokio::time::sleep(Duration::from_secs(5)).await;
            });
        }
    });
    RangeServer {
        url: format!("http://127.0.0.1:{port}/archive"),
        ranges,
        task,
    }
}

struct Free(Option<u64>);

impl hlsr_delivery::ports::FreeSpace for Free {
    fn free_bytes(&self, _dir: &std::path::Path) -> Option<u64> {
        self.0
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    downloads: PathBuf,
    payload: Arc<Vec<u8>>,
    server: RangeServer,
}

impl Fixture {
    async fn new(size: usize, broken: bool) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let payload = Arc::new(payload(size));
        let server = range_server(payload.clone(), broken).await;
        Fixture {
            downloads: root.path().join("Downloads"),
            _root: root,
            payload,
            server,
        }
    }

    fn request(&self) -> DownloadRequest {
        let options = ProviderOptions::with_allowed_provider_types(
            hlsr_delivery::http_client::HttpClient::new(),
            ["local-http"],
        );
        let provider = providers::create(
            &json!({ "id": "local", "type": "local-http", "url": self.server.url }),
            &options,
        )
        .unwrap();
        DownloadRequest {
            game_id: "test-game".to_owned(),
            provider: Arc::from(provider),
            filename: "test.zip".to_owned(),
            expected_size: Some(self.payload.len() as u64),
            expected_sha256: Some(sha256(&self.payload)),
        }
    }

    fn manager(&self, limiter: Option<Arc<DownloadRateLimiter>>) -> Arc<GameDownloadManager> {
        Arc::new(GameDownloadManager::new(
            &self.downloads,
            DownloadOptions {
                max_attempts: Some(1),
                free_space: Some(Arc::new(Free(Some(u64::MAX)))),
                rate_limiter: limiter,
                ..DownloadOptions::default()
            },
        ))
    }

    fn part(&self) -> PathBuf {
        self.downloads.join("test-game").join("test.zip.part")
    }
}

fn completed_file(outcome: &DownloadOutcome) -> PathBuf {
    match outcome {
        DownloadOutcome::Completed { file_path, .. } => file_path.clone(),
        other => panic!("ожидалась завершённая загрузка, а не {other:?}"),
    }
}

/// Слушатель, который при первом состоянии `state` запускает `action` в отдельной задаче.
fn on_state(
    manager: &GameDownloadManager,
    state: StateName,
    action: impl Fn() + Send + Sync + 'static,
) {
    let fired = AtomicBool::new(false);
    manager.on_event(move |event| {
        if let DownloadEvent::State(current) = event
            && current.state == state
            && !fired.swap(true, Ordering::SeqCst)
        {
            action();
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_limit_paces_real_http_bytes_without_changing_the_archive() {
    let f = Fixture::new(128 * 1024, false).await;
    let manager = f.manager(Some(Arc::new(DownloadRateLimiter::new(128.0 * 1024.0))));
    let start = Instant::now();
    let outcome = manager.start(f.request()).await.unwrap();
    assert!(
        start.elapsed() >= Duration::from_millis(900),
        "128 КБ при 128 КБ/с — около секунды, а не {:?}",
        start.elapsed()
    );
    assert_eq!(outcome.status(), "completed");
    assert_eq!(std::fs::read(completed_file(&outcome)).unwrap(), *f.payload);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_a_limit_wakes_an_active_http_download_immediately() {
    let f = Fixture::new(512 * 1024, false).await;
    let limiter = Arc::new(DownloadRateLimiter::new(1024.0));
    let manager = f.manager(Some(limiter.clone()));
    on_state(&manager, StateName::Downloading, move || {
        let limiter = limiter.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            limiter.set_limit(0.0);
        });
    });
    let start = Instant::now();
    let outcome = manager.start(f.request()).await.unwrap();
    // Старый таймер при 1 КБ/с держал бы 512 КБ минуты: предел с большим запасом на нагрузку.
    assert!(
        start.elapsed() < SLOW_LIMIT_BOUND,
        "снятый лимит не должен держать старый таймер: {:?}",
        start.elapsed()
    );
    assert_eq!(std::fs::read(completed_file(&outcome)).unwrap(), *f.payload);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_and_range_resume_retain_the_exact_archive_under_a_limit() {
    let f = Fixture::new(512 * 1024, false).await;
    let limiter = Arc::new(DownloadRateLimiter::new(256.0 * 1024.0));
    let manager = f.manager(Some(limiter.clone()));
    // `manager.once("progress", () => manager.pause())`: слушатель держит менеджер слабой ссылкой.
    let weak: Weak<GameDownloadManager> = Arc::downgrade(&manager);
    let paused_once = AtomicBool::new(false);
    manager.on_event(move |event| {
        if matches!(event, DownloadEvent::Progress(_))
            && !paused_once.swap(true, Ordering::SeqCst)
            && let Some(manager) = weak.upgrade()
        {
            assert!(manager.pause());
        }
    });

    let paused = manager.start(f.request()).await.unwrap();
    let DownloadOutcome::Paused { bytes_downloaded } = paused else {
        panic!("ожидалась пауза, а не {paused:?}");
    };
    assert!(bytes_downloaded > 0);
    assert!(!manager.is_busy());
    // Пауза оставляет начало файла и состояние.
    assert_eq!(
        std::fs::metadata(f.part()).unwrap().len(),
        bytes_downloaded,
        ".part — ровно то, что скачано до паузы"
    );
    let resumable = manager
        .find_resumable("test-game", "test.zip")
        .unwrap()
        .expect("остановленная загрузка видна");
    assert_eq!(resumable.bytes_downloaded, bytes_downloaded);

    limiter.set_limit(0.0);
    let outcome = manager.start(f.request()).await.unwrap();
    assert!(
        !f.server.ranges.lock().unwrap().is_empty(),
        "продолжение просит сохранённый диапазон"
    );
    assert_eq!(
        f.server.ranges.lock().unwrap()[0],
        bytes_downloaded,
        "диапазон начинается там, где остановились"
    );
    assert_eq!(std::fs::read(completed_file(&outcome)).unwrap(), *f.payload);
    assert!(!f.part().exists(), "после успеха .part переименован");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_a_positive_limit_takes_effect_on_the_current_stream() {
    let f = Fixture::new(256 * 1024, false).await;
    let limiter = Arc::new(DownloadRateLimiter::new(1.0));
    let manager = f.manager(Some(limiter.clone()));
    on_state(&manager, StateName::Downloading, move || {
        let limiter = limiter.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            limiter.set_limit(256.0 * 1024.0);
        });
    });
    let start = Instant::now();
    let outcome = manager.start(f.request()).await.unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < SLOW_LIMIT_BOUND,
        "ожидалась примерно секунда, а не {elapsed:?}"
    );
    assert_eq!(std::fs::read(completed_file(&outcome)).unwrap(), *f.payload);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_interrupts_a_limiter_wait_and_removes_partial_files() {
    let f = Fixture::new(128 * 1024, false).await;
    let manager = f.manager(Some(Arc::new(DownloadRateLimiter::new(1.0))));
    let weak = Arc::downgrade(&manager);
    on_state(&manager, StateName::Downloading, move || {
        let weak = weak.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            if let Some(manager) = weak.upgrade() {
                manager.cancel();
            }
        });
    });
    let start = Instant::now();
    let outcome = manager.start(f.request()).await.unwrap();
    assert_eq!(outcome.status(), "cancelled");
    assert!(start.elapsed() < SLOW_LIMIT_BOUND);
    let paths = manager.part_paths("test-game", "test.zip").unwrap();
    assert!(!paths.part.exists());
    assert!(!paths.state.exists());
}

/// «repair mirror transfers obey the same limit and validate SHA-256».
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_mirror_transfers_obey_the_same_limit_and_validate_sha256() {
    let f = Fixture::new(64 * 1024, false).await;
    let client = RepairMirrorClient::new(RepairMirrorOptions {
        rate_limiter: Some(Arc::new(DownloadRateLimiter::new(64.0 * 1024.0))),
        max_attempts: Some(1),
        ..RepairMirrorOptions::default()
    });
    let entry = json!({"path": "test.bin", "size": f.payload.len(), "sha256": sha256(&f.payload)});
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("test.bin");
    let start = Instant::now();
    let length = client
        .fetch_one(
            &f.server.url,
            &entry,
            "sha256-2",
            &target,
            None,
            &mut |_| {},
        )
        .await
        .unwrap();
    assert!(
        start.elapsed() >= Duration::from_millis(900),
        "64 КБ при 64 КБ/с — около секунды, а не {:?}",
        start.elapsed()
    );
    assert_eq!(length, f.payload.len() as u64);
    assert_eq!(std::fs::read(&target).unwrap(), *f.payload);
}

/// «game and LiveSplit downloads share one bandwidth limit».
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn game_and_livesplit_downloads_share_one_bandwidth_limit() {
    let f = Fixture::new(256 * 1024, false).await;
    let limiter = Arc::new(DownloadRateLimiter::new(512.0 * 1024.0));
    let manager = f.manager(Some(limiter.clone()));
    let lib = f.downloads.with_file_name("lib");
    let tools = ToolService::new(
        resolve_library(&lib.to_string_lossy()).unwrap(),
        ToolOptions {
            rate_limiter: Some(limiter),
            ..ToolOptions::default()
        },
    );
    let target = f.downloads.with_file_name("livesplit.zip");
    let descriptor = json!({"size": f.payload.len(), "sha256": sha256(&f.payload)});
    let start = Instant::now();
    let (outcome, tool) = tokio::join!(
        manager.start(f.request()),
        tools.download_from(&f.server.url, &descriptor, &target, None)
    );
    assert!(
        start.elapsed() >= Duration::from_millis(900),
        "два файла по 256 КБ делят 512 КБ/с, а не {:?}",
        start.elapsed()
    );
    tool.unwrap();
    assert_eq!(
        std::fs::read(completed_file(&outcome.unwrap())).unwrap(),
        *f.payload
    );
    assert_eq!(std::fs::read(&target).unwrap(), *f.payload);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_limited_connection_remains_a_network_error() {
    let f = Fixture::new(128 * 1024, true).await;
    let manager = f.manager(Some(Arc::new(DownloadRateLimiter::new(1024.0 * 1024.0))));
    let Err(Failure::Delivery(error)) = manager.start(f.request()).await else {
        panic!("обрыв — ошибка загрузки");
    };
    assert_eq!(error.code, Code::NetworkError, "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_during_retry_backoff_is_prompt_and_deletes_a_paused_download() {
    // В 0.4.7 попытка подменена на падающую; здесь сервер отвечает 500, пауза — 30 с.
    let (server, _) = serve_one("/f", json!({ "status": 500 })).await;
    let root = tempfile::tempdir().unwrap();
    let manager = Arc::new(GameDownloadManager::new(
        root.path(),
        DownloadOptions {
            backoff_base_ms: Some(30_000),
            free_space: Some(Arc::new(Free(None))),
            ..DownloadOptions::default()
        },
    ));
    let weak = Arc::downgrade(&manager);
    on_state(&manager, StateName::Retrying, move || {
        weak.upgrade().unwrap().cancel();
    });
    let options = ProviderOptions::with_allowed_provider_types(
        hlsr_delivery::http_client::HttpClient::new(),
        ["local-http"],
    );
    let provider = providers::create(
        &json!({ "id": "one", "type": "local-http", "url": format!("http://{}/f", server.authority) }),
        &options,
    )
    .unwrap();
    let start = Instant::now();
    let outcome = manager
        .start(DownloadRequest {
            game_id: "game".to_owned(),
            provider: Arc::from(provider),
            filename: "game.zip".to_owned(),
            expected_size: Some(100),
            expected_sha256: Some("a".repeat(64)),
        })
        .await
        .unwrap();
    assert_eq!(outcome.status(), "cancelled");
    // Пауза повтора — 30 с: отмена, которая её дожидается, не уложится.
    assert!(start.elapsed() < Duration::from_secs(20));

    let files = manager.part_paths("game", "game.zip").unwrap();
    std::fs::write(&files.part, "partial").unwrap();
    manager
        .write_state(
            &files.state,
            &StateFile {
                game_id: Some("game".to_owned()),
                filename: Some("game.zip".to_owned()),
                ..StateFile::default()
            },
        )
        .unwrap();
    std::fs::write(files.dir.join("keep.zip"), "complete").unwrap();
    assert!(manager.discard_paused("game", None).unwrap());
    let mut left: Vec<_> = std::fs::read_dir(&files.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["keep.zip"]);
}

// ------------------------------------------------------------------ устройство кода

/// Источник с заданным списком ответов `resolve()` и счётчиком вызовов.
struct Scripted {
    base: BaseProvider,
    calls: AtomicUsize,
    outcome: Box<dyn Fn(usize) -> Result<Resolved, Failure> + Send + Sync>,
}

impl Provider for Scripted {
    fn base(&self) -> &BaseProvider {
        &self.base
    }

    fn resolve(&self) -> BoxFuture<'_, Result<Resolved, Failure>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let result = (self.outcome)(call);
        Box::pin(async move { result })
    }
}

fn scripted(
    outcome: impl Fn(usize) -> Result<Resolved, Failure> + Send + Sync + 'static,
) -> Arc<Scripted> {
    Arc::new(Scripted {
        base: BaseProvider::new(&json!({ "id": "scripted" })),
        calls: AtomicUsize::new(0),
        outcome: Box::new(outcome),
    })
}

fn request_of(provider: Arc<dyn Provider>, size: Option<u64>) -> DownloadRequest {
    DownloadRequest {
        game_id: "g".to_owned(),
        provider,
        filename: "f.zip".to_owned(),
        expected_size: size,
        expected_sha256: None,
    }
}

fn manager_in(
    root: &tempfile::TempDir,
    max_attempts: Option<u32>,
    backoff_ms: u64,
    random: f64,
) -> Arc<GameDownloadManager> {
    Arc::new(GameDownloadManager::new(
        root.path(),
        DownloadOptions {
            max_attempts,
            backoff_base_ms: Some(backoff_ms),
            free_space: Some(Arc::new(Free(None))),
            random: Some(Arc::new(move || random)),
            ..DownloadOptions::default()
        },
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_start_releases_the_manager() {
    let log = RequestLog::default();
    let server = start_server("a", log).await;
    *server.routes.lock().unwrap() = json!({
        "/f": [{ "bodySize": 3000, "gates": [{ "at": 1000, "name": "hold" }] }]
    })
    .as_object()
    .unwrap()
    .clone();
    let root = tempfile::tempdir().unwrap();
    let manager = manager_in(&root, Some(1), 1, 0.5);
    let provider = providers::create(
        &json!({ "id": "p", "type": "local-http", "url": format!("http://{}/f", server.authority) }),
        &ProviderOptions::with_allowed_provider_types(
            hlsr_delivery::http_client::HttpClient::new(),
            ["local-http"],
        ),
    )
    .unwrap();
    let provider: Arc<dyn Provider> = Arc::from(provider);

    let dropped = tokio::time::timeout(
        Duration::from_millis(200),
        manager.start(request_of(provider.clone(), Some(3000))),
    )
    .await;
    assert!(dropped.is_err(), "ворота закрыты — загрузка не кончилась");
    assert!(!manager.is_busy(), "сброшенный start освободил менеджер");

    server.release("hold");
    let outcome = manager
        .start(request_of(provider, Some(3000)))
        .await
        .unwrap();
    assert_eq!(outcome.status(), "completed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_link_is_resolved_again_on_every_attempt() {
    let (server, log) = serve_one("/ok", json!({ "bodySize": 3000 })).await;
    let authority = server.authority.clone();
    let root = tempfile::tempdir().unwrap();
    let manager = manager_in(&root, Some(4), 1, 0.5);
    // Две неудачи при получении ссылки, затем ссылка рабочая.
    let provider = scripted(move |call| match call {
        0 => Err(hlsr_delivery::errors::DeliveryError::new(
            Code::ProviderLinkFailed,
            "ссылка не получена",
        )
        .into()),
        1 => Err(
            hlsr_delivery::errors::DeliveryError::new(Code::ProviderForbidden, "API отказал")
                .into(),
        ),
        _ => Ok(Resolved {
            url: format!("http://{authority}/ok"),
            headers: Vec::new(),
            temporary: true,
        }),
    });
    let outcome = manager
        .start(request_of(provider.clone(), Some(3000)))
        .await
        .unwrap();
    assert_eq!(outcome.status(), "completed");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failure_that_cannot_be_retried_stops_after_one_resolve() {
    let root = tempfile::tempdir().unwrap();
    let manager = manager_in(&root, None, 1, 0.5);
    let provider = scripted(|_| {
        Err(hlsr_delivery::errors::DeliveryError::new(Code::ProviderUnsupported, "не умеет").into())
    });
    let Err(Failure::Delivery(error)) = manager.start(request_of(provider.clone(), None)).await
    else {
        panic!("ошибка источника");
    };
    assert_eq!(error.code, Code::ProviderUnsupported);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn five_attempts_by_default_and_a_zero_means_the_default() {
    for max_attempts in [None, Some(0)] {
        let root = tempfile::tempdir().unwrap();
        let manager = manager_in(&root, max_attempts, 1, 0.5);
        let provider = scripted(|_| {
            Err(hlsr_delivery::errors::DeliveryError::new(Code::NetworkTimeout, "тайм-аут").into())
        });
        let Err(Failure::Delivery(error)) = manager.start(request_of(provider.clone(), None)).await
        else {
            panic!("ошибка источника");
        };
        assert_eq!(error.code, Code::NetworkTimeout);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 5, "{max_attempts:?}");
    }
}

/// Пауза перед повтором: `min(30 с, round(основа × 2ⁿ⁻¹ × (0,8 + random × 0,4)))`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_retry_pause_is_jittered_and_capped() {
    // (основа, random, ожидаемая пауза первого повтора)
    let cases = [
        (1000u64, 0.0, 800u64),
        (1000, 0.5, 1000),
        (1000, 0.999_999, 1200),
        (1001, 0.123, 850),
        (1000, 0.25, 900),
        (40_000, 0.5, 30_000),
        (0, 0.5, 0),
    ];
    for (base, random, expected) in cases {
        let root = tempfile::tempdir().unwrap();
        let manager = manager_in(&root, Some(3), base, random);
        let pauses = Arc::new(Mutex::new(Vec::new()));
        let seen = pauses.clone();
        let weak = Arc::downgrade(&manager);
        manager.on_event(move |event| {
            if let DownloadEvent::State(state) = event
                && state.state == StateName::Retrying
            {
                seen.lock().unwrap().push(state.extra.retry_in_ms.unwrap());
                // Дальше ждать не нужно: пауза проверена.
                if let Some(manager) = weak.upgrade() {
                    manager.cancel();
                }
            }
        });
        let provider = scripted(|_| {
            Err(hlsr_delivery::errors::DeliveryError::new(Code::NetworkError, "сеть").into())
        });
        let outcome = manager.start(request_of(provider, None)).await.unwrap();
        assert_eq!(outcome.status(), "cancelled");
        assert_eq!(
            *pauses.lock().unwrap(),
            [expected],
            "основа {base}, random {random}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pause_doubles_with_every_attempt() {
    let root = tempfile::tempdir().unwrap();
    let manager = manager_in(&root, Some(4), 3, 0.5);
    let pauses = Arc::new(Mutex::new(Vec::new()));
    let seen = pauses.clone();
    manager.on_event(move |event| {
        if let DownloadEvent::State(state) = event
            && state.state == StateName::Retrying
        {
            seen.lock().unwrap().push((
                state.extra.attempt.unwrap(),
                state.extra.max_attempts.unwrap(),
                state.extra.retry_in_ms.unwrap(),
            ));
        }
    });
    let provider = scripted(|_| {
        Err(hlsr_delivery::errors::DeliveryError::new(Code::NetworkError, "сеть").into())
    });
    let result = manager.start(request_of(provider, None)).await;
    assert!(result.is_err());
    assert_eq!(*pauses.lock().unwrap(), [(1, 4, 3), (2, 4, 6), (3, 4, 12)]);
}

/// Место спрашивают у каталога загрузок (родитель папки игры), один раз и только если
/// размер известен; нужно размер плюс запас.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn free_space_is_asked_about_the_downloads_folder() {
    use hlsr_delivery::ports::FakeFreeSpace;
    let root = tempfile::tempdir().unwrap();
    let space = Arc::new(FakeFreeSpace::default());
    let manager = GameDownloadManager::new(
        root.path(),
        DownloadOptions {
            free_space: Some(space.clone()),
            free_space_reserve: Some(100),
            max_attempts: Some(1),
            ..DownloadOptions::default()
        },
    );
    let provider = scripted(|_| {
        Err(hlsr_delivery::errors::DeliveryError::new(Code::ProviderUnsupported, "нет").into())
    });

    space.set(root.path(), Some(1099));
    let Err(Failure::Delivery(error)) = manager
        .start(request_of(provider.clone(), Some(1000)))
        .await
    else {
        panic!("места не хватает");
    };
    assert_eq!(error.code, Code::NoSpace);
    assert_eq!(
        error.message,
        "Недостаточно места: нужно 1100, свободно 1099"
    );
    assert_eq!(
        error.details,
        Some(json!({ "requiredBytes": 1100, "freeBytes": 1099 }))
    );
    assert_eq!(space.calls(), 1);
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        0,
        "до источника не дошли"
    );
    assert!(!root.path().join("g").exists(), "папка игры не создана");

    // Ровно столько, сколько нужно: проверка пройдена, дальше — отказ источника.
    space.set(root.path(), Some(1100));
    let Err(Failure::Delivery(error)) = manager
        .start(request_of(provider.clone(), Some(1000)))
        .await
    else {
        panic!("источник отказал");
    };
    assert_eq!(error.code, Code::ProviderUnsupported);
    assert_eq!(space.calls(), 2);

    // Размер неизвестен — место не спрашивают.
    let Err(Failure::Delivery(_)) = manager.start(request_of(provider, None)).await else {
        panic!("источник отказал");
    };
    assert_eq!(space.calls(), 2);
}
