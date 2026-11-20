//! `DiagnosticsRunner` против 0.4.7: фикстура `diagnostics.json` из `tools/reference/update/
//! diagnostics.js` (сценарии «Т» — модуль 0.4.7 на заглушках контекста и запроса; строки проверок,
//! их порядок и список запрошенных адресов сверяются по сценариям). Самотесты `diagnostics-
//! selftest.js` перенесены: GPU по состоянию рендерера (Р10), обновлятор и запуск игры, проверки
//! записи, лимит загрузок, наигранное время, отмена на любом ожидании, подписанные каналы,
//! пустые/усечённые/перенаправленные метаданные, лимит GitHub, локализация имён строк.

mod support;

use hlsr_core::js::Platform;
use hlsr_update::channel::launcher_for;
use hlsr_update::diagnostics::context::{ContextData, DiagnosticsContext, GpuStatus};
use hlsr_update::diagnostics::net::NetError;
use hlsr_update::diagnostics::runner::{
    Check, DiagnosticsRunner, Endpoints, RunResult, RunStatus, latest_yml_is_valid,
};
use hlsr_update::ports::{BoxFuture, FakeClock};
use serde_json::{Value, json};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use support::diagnostics::{
    StubContext, StubDns, StubRequest, Tokens, merge, platform_of, rows, write_files,
};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("diagnostics.json"));

struct Outcome {
    result: RunResult,
    tokens: Tokens,
    request: Arc<StubRequest>,
    platform: String,
    _dir: tempfile::TempDir,
}

impl Outcome {
    fn row(&self, id: &str) -> &Check {
        self.result
            .checks
            .iter()
            .find(|check| check.id() == id)
            .unwrap_or_else(|| panic!("нет строки {id}"))
    }

    fn has(&self, id: &str) -> bool {
        self.result.checks.iter().any(|check| check.id() == id)
    }
}

/// Выполнить сценарий. `cancel_after` — отменить прогон через это время (зависший DNS или запрос).
async fn run_spec(spec: Value, cancel_after: Option<Duration>) -> Outcome {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let tokens = Tokens::new(&root);
    write_files(&root, &tokens.fill_deep(&spec["files"]));
    let platform = str_of(&spec["platform"]).to_owned();

    let context = Arc::new(StubContext::new(spec.clone(), root, &FIXTURE));
    let request = Arc::new(StubRequest::new(&FIXTURE, &platform, &spec["net"]));
    let dns = Arc::new(StubDns(spec["dns"].clone()));
    let clock = Arc::new(FakeClock::new(spec["nowMs"].as_i64().unwrap()));
    let runner = Arc::new(
        DiagnosticsRunner::new(context, request.clone(), dns, clock)
            .with_launcher_public_keys(vec![str_of(&FIXTURE["keys"]["publicKeyPem"]).to_owned()]),
    );
    if let Some(after) = cancel_after {
        let canceller = runner.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            canceller.cancel();
        });
    }
    let result = runner.run(None).await;
    Outcome {
        result,
        tokens,
        request,
        platform,
        _dir: dir,
    }
}

fn hangs(spec: &Value) -> bool {
    spec["dns"] == "hang"
        || spec["net"]
            .as_object()
            .is_some_and(|net| net.values().any(|answer| answer["hang"] == true))
}

fn healthy(platform: &str, patch: Value) -> Value {
    merge(&FIXTURE, platform, &patch)
}

// ------------------------------------------------------------------ сверка с 0.4.7

#[tokio::test]
async fn scenarios_match_the_reference() {
    let table = items(&FIXTURE["rowTable"]);
    let mut mismatches = Mismatches::default();
    for scenario in items(&FIXTURE["scenarios"]) {
        let platform = str_of(&scenario["platform"]);
        let spec = merge(&FIXTURE, platform, &scenario["patch"]);
        let after = hangs(&spec).then_some(Duration::from_millis(30));
        let outcome = run_spec(spec, after).await;
        let id = str_of(&scenario["id"]);

        mismatches.check_eq(
            outcome.result.status.as_str(),
            str_of(&scenario["status"]),
            format!("{id}: итог прогона"),
        );
        let expected: Vec<Value> = items(&scenario["rows"])
            .iter()
            .map(|index| table[index.as_u64().unwrap() as usize].clone())
            .collect();
        let actual = rows(&outcome.result.checks, &outcome.tokens);
        mismatches.check(
            &Value::Array(actual),
            &Value::Array(expected),
            format!("{id}: строки проверок"),
        );
        let expected_urls: Vec<String> = items(&scenario["urls"])
            .iter()
            .map(|key| str_of(key).to_owned())
            .collect();
        mismatches.check_eq(
            outcome.request.keys(&FIXTURE, &outcome.platform),
            expected_urls,
            format!("{id}: запрошенные адреса"),
        );
    }
    mismatches.finish("диагностика");
}

#[test]
fn endpoints_and_launcher_urls_match_the_reference() {
    let endpoints = Endpoints::production();
    let reference = &FIXTURE["endpoints"];
    assert_eq!(
        endpoints.proton_releases,
        str_of(&reference["protonReleases"])
    );
    assert_eq!(endpoints.game_channel, str_of(&reference["gameChannel"]));
    assert_eq!(
        endpoints.github_releases,
        str_of(&reference["githubReleases"])
    );
    assert_eq!(
        endpoints.github_rate_limit,
        str_of(&reference["githubRateLimit"])
    );
    for platform in ["win32", "linux"] {
        let feed = launcher_for(platform_of(platform));
        let urls = &FIXTURE["urlsByKey"][platform];
        assert_eq!(feed.channel_urls[0], str_of(&urls["launcher"]));
        assert_eq!(feed.channel_urls[1], str_of(&urls["launcherFallback"]));
        if platform == "win32" {
            assert_eq!(feed.latest_yml_urls[0], str_of(&urls["third"]));
        } else {
            assert_eq!(endpoints.proton_releases, str_of(&urls["third"]));
        }
        assert_eq!(endpoints.game_channel, str_of(&urls["game"]));
        assert_eq!(endpoints.github_releases, str_of(&urls["github"]));
        assert_eq!(endpoints.github_rate_limit, str_of(&urls["rateLimit"]));
    }
}

#[test]
fn latest_yml_check_matches_js_yaml() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["yaml"]) {
        mismatches.check_eq(
            latest_yml_is_valid(str_of(&case["text"])),
            case["valid"].as_bool().unwrap(),
            format!("latest.yml {:?}", case["text"]),
        );
    }
    mismatches.finish("проверка latest.yml");
}

// ------------------------------------------------------------------ самотесты diagnostics-selftest.js

#[tokio::test]
async fn gpu_status_distinguishes_pending_settings_driver_fallback_and_failed_disable() {
    let gpu = |patch: Value| {
        let mut base = json!({"configuredEnabled": true, "appliedEnabled": true, "available": true,
            "hardwareEnabled": true, "softwareRenderer": false, "device": "", "driver": ""});
        base.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        healthy("linux", json!({"gpu": {"value": base}}))
    };
    let ok = run_spec(gpu(json!({})), None).await;
    assert_eq!(ok.row("gpu-runtime").status(), "ok");

    let pending = run_spec(gpu(json!({"configuredEnabled": false})), None).await;
    assert_eq!(
        pending.row("gpu-applied").code_str(),
        Some("GPU_RESTART_REQUIRED")
    );

    let fallback = run_spec(gpu(json!({"softwareRenderer": true})), None).await;
    assert_eq!(
        fallback.row("gpu-runtime").code_str(),
        Some("GPU_SOFTWARE_FALLBACK")
    );

    let failed = run_spec(
        gpu(json!({"configuredEnabled": false, "appliedEnabled": false})),
        None,
    )
    .await;
    assert_eq!(
        failed.row("gpu-runtime").code_str(),
        Some("GPU_DISABLE_FAILED")
    );

    let software = run_spec(
        gpu(json!({"configuredEnabled": false, "appliedEnabled": false, "hardwareEnabled": false})),
        None,
    )
    .await;
    assert_eq!(software.row("gpu-runtime").status(), "ok");
    assert_eq!(
        software.row("gpu-runtime").detail_key_str(),
        Some("#L_DGV_GPU_SOFTWARE")
    );
}

#[tokio::test]
async fn unknown_gpu_information_is_reported_as_unknown_not_successful_acceleration() {
    let spec = healthy(
        "linux",
        json!({"gpu": {"value": {"configuredEnabled": true, "appliedEnabled": true, "available": false}}}),
    );
    let outcome = run_spec(spec, None).await;
    assert_eq!(
        outcome.row("gpu-runtime").code_str(),
        Some("GPU_STATUS_UNKNOWN")
    );
    assert_eq!(outcome.row("gpu-runtime").status(), "warn");
}

/// Р10: состояние рендерера ПП1 → строки «Отрисовка интерфейса».
#[test]
fn renderer_state_maps_to_gpu_status() {
    // Видеокарта просили и она работает.
    let working = GpuStatus::from_renderer("auto", "auto", Some("opengl"), "GPU", "1.0");
    assert!(working.configured_enabled && working.hardware_enabled && working.available);
    assert_eq!(working.applied_enabled, Some(true));
    assert!(!working.software_renderer);
    assert_eq!(
        (working.device.as_str(), working.driver.as_str()),
        ("GPU", "1.0")
    );

    // Настройку сменили на программный режим уже после запуска: нужен перезапуск.
    let pending = GpuStatus::from_renderer("software", "vulkan", Some("vulkan"), "", "");
    assert!(!pending.configured_enabled);
    assert_eq!(pending.applied_enabled, Some(true));

    // Видеокарту просили, после отказа GPU работает программный рендерер.
    let failed = GpuStatus::from_renderer("dx12", "dx12", Some("software"), "", "");
    assert!(failed.software_renderer && !failed.hardware_enabled);
    assert_eq!(failed.applied_enabled, Some(true));

    // Программный режим выбран.
    let software = GpuStatus::from_renderer("software", "software", Some("software"), "", "");
    assert!(!software.configured_enabled);
    assert_eq!(software.applied_enabled, Some(false));
    assert!(software.software_renderer);

    // Рендерер неизвестен.
    let unknown = GpuStatus::from_renderer("auto", "auto", None, "", "");
    assert!(!unknown.available && !unknown.hardware_enabled);
}

struct RendererContext(GpuStatus);

impl DiagnosticsContext for RendererContext {
    fn data(&self) -> ContextData {
        ContextData {
            platform: Platform::Linux,
            arch: "x64".to_owned(),
            ..ContextData::default()
        }
    }

    fn gpu_status(&self) -> BoxFuture<'_, Option<GpuStatus>> {
        let gpu = self.0.clone();
        Box::pin(async move { Some(gpu) })
    }
}

#[tokio::test]
async fn renderer_state_rows_use_the_codes_of_0_4_7() {
    let run = |gpu: GpuStatus| async move {
        let runner = DiagnosticsRunner::new(
            Arc::new(RendererContext(gpu)),
            Arc::new(StubRequest::new(&FIXTURE, "linux", &json!({}))),
            Arc::new(StubDns(json!("hang"))),
            Arc::new(FakeClock::new(0)),
        );
        // Сеть не нужна: DNS не отвечает, прогон останавливает отмена.
        let runner = Arc::new(runner);
        let canceller = runner.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            canceller.cancel();
        });
        runner.run(None).await
    };
    let row = |result: &RunResult, id: &str| {
        result
            .checks
            .iter()
            .find(|check| check.id() == id)
            .cloned()
            .unwrap_or_else(|| panic!("нет строки {id}"))
    };

    let fallback = run(GpuStatus::from_renderer(
        "opengl",
        "opengl",
        Some("software"),
        "",
        "",
    ))
    .await;
    assert_eq!(
        row(&fallback, "gpu-runtime").code_str(),
        Some("GPU_SOFTWARE_FALLBACK")
    );
    assert_eq!(row(&fallback, "gpu-runtime").status(), "warn");

    let restart = run(GpuStatus::from_renderer(
        "software",
        "auto",
        Some("opengl"),
        "",
        "",
    ))
    .await;
    assert_eq!(
        row(&restart, "gpu-applied").code_str(),
        Some("GPU_RESTART_REQUIRED")
    );

    let software = run(GpuStatus::from_renderer(
        "software",
        "software",
        Some("software"),
        "",
        "",
    ))
    .await;
    assert_eq!(row(&software, "gpu-runtime").status(), "ok");
    assert_eq!(
        row(&software, "gpu-runtime").detail_key_str(),
        Some("#L_DGV_GPU_SOFTWARE")
    );
    // Известные признаки сохраняются, а название видеокарты и драйвер не выдумываются.
    assert!(
        !software
            .checks
            .iter()
            .any(|check| matches!(check.id(), "gpu-device" | "gpu-driver"))
    );

    let unknown = run(GpuStatus::from_renderer("auto", "auto", None, "", "")).await;
    assert_eq!(
        row(&unknown, "gpu-runtime").code_str(),
        Some("GPU_STATUS_UNKNOWN")
    );
}

#[tokio::test]
async fn updater_errors_and_launch_failures_are_visible_native_module_is_checked_once() {
    let spec = healthy(
        "win32",
        json!({"nativeAvailable": false, "nativeLoadError": "ABI mismatch", "updateState": "error",
            "launch": {"value": {"ok": false, "code": "SOURCE_SDK_NOT_INSTALLED"}}}),
    );
    let outcome = run_spec(spec, None).await;
    assert_eq!(outcome.row("hlsr-updater").status(), "fail");
    assert_eq!(outcome.row("hlsr-native").detail_str(), "ABI mismatch");
    assert_eq!(
        outcome.row("steam-launch").code_str(),
        Some("SOURCE_SDK_NOT_INSTALLED")
    );
    assert!(!outcome.has("steam-native"));
    assert_eq!(
        outcome
            .result
            .checks
            .iter()
            .filter(|check| check.id() == "hlsr-native")
            .count(),
        1
    );
}

#[tokio::test]
async fn write_probes_leave_no_files_and_download_checks_detect_a_limit_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("download-settings.json"), r#"{"speedLimit": 5}"#).unwrap();
    let names = |root: &std::path::Path| {
        let mut names: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before = names(root);

    let run = |bps: Value| {
        let mut spec = healthy("linux", json!({"files": []}));
        spec["downloadLimitBps"] = bps;
        let context = Arc::new(StubContext::new(spec.clone(), root.to_path_buf(), &FIXTURE));
        let runner = DiagnosticsRunner::new(
            context,
            Arc::new(StubRequest::new(&FIXTURE, "linux", &spec["net"])),
            Arc::new(StubDns(json!("ok"))),
            Arc::new(FakeClock::new(spec["nowMs"].as_i64().unwrap())),
        );
        async move { runner.run(None).await }
    };
    let find = |result: &RunResult, id: &str| {
        result
            .checks
            .iter()
            .find(|check| check.id() == id)
            .cloned()
            .unwrap()
    };

    let mismatch = run(json!({"fn": 1024 * 1024})).await;
    assert_eq!(find(&mismatch, "hlsr-userdata").status(), "ok");
    assert_eq!(find(&mismatch, "lib-writable").status(), "ok");
    assert_eq!(names(root), before, "проверки записи не оставляют файлов");
    assert_eq!(
        find(&mismatch, "downloads-applied").code_str(),
        Some("DOWNLOAD_LIMIT_NOT_APPLIED")
    );

    let applied = run(json!({"fn": 5 * 1024 * 1024})).await;
    assert_eq!(find(&applied, "downloads-applied").status(), "ok");

    std::fs::write(root.join("download-settings.json"), "{broken").unwrap();
    let broken = run(json!({"fn": 0})).await;
    assert_eq!(find(&broken, "downloads-limit").status(), "fail");
}

#[tokio::test]
async fn diagnostics_show_precise_playtime_and_do_not_label_an_offline_statistics_server_active() {
    let spec = healthy(
        "linux",
        json!({"profile": {"value": {"signedIn": true, "playSeconds": 127, "launchCount": 3, "gamesPlayed": 1}},
            "telemetryStatus": {"value": {"enabled": true, "backendConfigured": false}}}),
    );
    let outcome = run_spec(spec, None).await;
    let args = outcome.row("profile-playtime").to_value()["detailArgs"].clone();
    assert_eq!(args, json!([0, 2, 7, 3, 1]));
    assert_eq!(outcome.row("profile-telemetry").status(), "skip");
}

#[tokio::test]
async fn cancellation_interrupts_a_pending_check() {
    struct Pending(&'static str);

    impl DiagnosticsContext for Pending {
        fn data(&self) -> ContextData {
            ContextData {
                platform: Platform::Linux,
                arch: "x64".to_owned(),
                ..ContextData::default()
            }
        }

        fn gpu_status(&self) -> BoxFuture<'_, Option<GpuStatus>> {
            if self.0 == "gpu" {
                Box::pin(std::future::pending())
            } else {
                Box::pin(async { None })
            }
        }

        fn required_steam_apps(&self) -> Vec<hlsr_update::diagnostics::context::SteamApp> {
            if self.0 == "ownership" {
                vec![hlsr_update::diagnostics::context::SteamApp {
                    app_id: 220,
                    title: "Half-Life 2".to_owned(),
                }]
            } else {
                Vec::new()
            }
        }

        fn steam_ownership(&self, _app_id: u64, _timeout: Duration) -> BoxFuture<'_, Option<bool>> {
            Box::pin(std::future::pending())
        }
    }

    struct PendingRequest;

    impl hlsr_update::diagnostics::net::DiagnosticsRequest for PendingRequest {
        fn get<'a>(
            &'a self,
            _url: &'a str,
        ) -> BoxFuture<'a, Result<hlsr_update::diagnostics::net::Response, NetError>> {
            Box::pin(std::future::pending())
        }
    }

    for waiting in ["gpu", "dns", "request", "ownership"] {
        let dns: Arc<dyn hlsr_update::diagnostics::net::DnsLookup> = if waiting == "dns" {
            Arc::new(StubDns(json!("hang")))
        } else {
            Arc::new(StubDns(json!("ok")))
        };
        let runner = Arc::new(DiagnosticsRunner::new(
            Arc::new(Pending(waiting)),
            Arc::new(PendingRequest),
            dns,
            Arc::new(FakeClock::new(1_792_143_005_000)),
        ));
        let canceller = runner.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(15)).await;
            canceller.cancel();
        });
        let started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(30), runner.run(None))
            .await
            .unwrap_or_else(|_| panic!("прогон завис на проверке {waiting}"));
        assert_eq!(result.status, RunStatus::Cancelled, "{waiting}");
        // Ожидание проверки без отмены — 6 с (`NET_TIMEOUT`) и больше: отмена шла бы не быстрее.
        assert!(started.elapsed() < Duration::from_secs(4), "{waiting}");
        assert!(!runner.is_running(), "{waiting}");
    }
}

#[tokio::test]
async fn only_one_run_at_a_time() {
    let spec = healthy("linux", json!({"dns": "hang"}));
    let dir = tempfile::tempdir().unwrap();
    let context = Arc::new(StubContext::new(
        spec.clone(),
        dir.path().to_path_buf(),
        &FIXTURE,
    ));
    let runner = Arc::new(DiagnosticsRunner::new(
        context,
        Arc::new(StubRequest::new(&FIXTURE, "linux", &spec["net"])),
        Arc::new(StubDns(json!("hang"))),
        Arc::new(FakeClock::new(spec["nowMs"].as_i64().unwrap())),
    ));
    let first = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run(None).await })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(runner.is_running());
    let second = runner.run(None).await;
    assert_eq!(second.status, RunStatus::AlreadyRunning);
    assert!(second.checks.is_empty());
    runner.cancel();
    assert_eq!(first.await.unwrap().status, RunStatus::Cancelled);
    assert!(!runner.is_running());
    // После отмены прогон снова можно начать.
    let third = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run(None).await })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    runner.cancel();
    assert_eq!(third.await.unwrap().status, RunStatus::Cancelled);
}

#[tokio::test(start_paused = true)]
async fn dns_that_never_answers_times_out_after_six_seconds() {
    let spec = healthy("linux", json!({"dns": "hang"}));
    let dir = tempfile::tempdir().unwrap();
    let context = Arc::new(StubContext::new(
        spec.clone(),
        dir.path().to_path_buf(),
        &FIXTURE,
    ));
    let request = Arc::new(StubRequest::new(&FIXTURE, "linux", &spec["net"]));
    let runner = DiagnosticsRunner::new(
        context,
        request.clone(),
        Arc::new(StubDns(json!("hang"))),
        Arc::new(FakeClock::new(spec["nowMs"].as_i64().unwrap())),
    );
    let result = runner.run(None).await;
    assert_eq!(result.status, RunStatus::Ok);
    let dns = result.checks.iter().find(|c| c.id() == "net-dns").unwrap();
    assert_eq!(dns.status(), "fail");
    assert_eq!(dns.code_str(), Some("CONNECT_TIMEOUT"));
    // После таймаута DNS остальные сетевые проверки идут своим чередом.
    assert!(
        result
            .checks
            .iter()
            .any(|c| c.id() == "net-launcher-channel")
    );
}

#[tokio::test]
async fn linux_and_windows_verify_the_correct_signed_launcher_and_game_channels() {
    for platform in ["linux", "win32"] {
        let outcome = run_spec(healthy(platform, json!({})), None).await;
        for id in [
            "net-launcher-channel",
            "net-launcher-fallback",
            "net-game-channel",
        ] {
            assert_eq!(outcome.row(id).status(), "ok", "{platform} {id}");
        }
        let feed = launcher_for(platform_of(platform));
        let urls = outcome.request.urls.lock().unwrap().clone();
        for url in &feed.channel_urls {
            assert!(urls.contains(url), "{platform}: не запрошен {url}");
        }

        let tampered = run_spec(
            healthy(
                platform,
                json!({"net": {"game": {"status": 200, "body": "@game-tampered"}}}),
            ),
            None,
        )
        .await;
        assert_eq!(
            tampered.row("net-game-channel").code_str(),
            Some("SIGNATURE_INVALID")
        );
    }
}

#[tokio::test]
async fn empty_truncated_or_redirected_metadata_cannot_pass_as_a_healthy_channel() {
    let patches = [
        json!({"status": 200, "body": null}),
        json!({"status": 200, "body": "{}"}),
        json!({"status": 200, "body": null, "truncated": true}),
        json!({"status": 302, "body": ""}),
        json!({"status": 202, "body": "@game-ok"}),
    ];
    for patch in patches {
        let outcome = run_spec(
            healthy("linux", json!({"net": {"game": patch.clone()}})),
            None,
        )
        .await;
        assert_eq!(outcome.row("net-game-channel").status(), "fail", "{patch}");
    }
    let unsigned = run_spec(
        healthy(
            "linux",
            json!({"net": {"launcher": {"status": 200, "body": "@launcher-linux-null-signature"}}}),
        ),
        None,
    )
    .await;
    assert_eq!(unsigned.row("net-launcher-channel").status(), "fail");
}

#[tokio::test]
async fn malformed_github_rate_limit_data_is_unknown_rather_than_healthy() {
    let spec = healthy(
        "linux",
        json!({"net": {"github": {"status": 403, "body": ""}, "rateLimit": {"status": 200, "body": "{}"}}}),
    );
    let outcome = run_spec(spec, None).await;
    assert_eq!(outcome.row("net-rate-limit").status(), "warn");
    assert_eq!(
        outcome.row("net-rate-limit").code_str(),
        Some("CHECKSUM_METADATA_INVALID")
    );
}

/// Имена строк и значения локализованы на обоих языках: идентификаторы берутся из исходника
/// раннера, как это делает самотест 0.4.7.
#[test]
fn diagnostic_row_names_and_structured_values_are_localized_in_both_languages() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/diagnostics/runner.rs"
    ))
    .unwrap();
    // Идентификаторы строк: Check::new("id", …) и адреса проверок сети ("net-…").
    let literal_after = |at: usize| -> Option<String> {
        let rest = source[at..].trim_start();
        let rest = rest.strip_prefix('"')?;
        Some(rest[..rest.find('"')?].to_owned())
    };
    let mut ids: Vec<String> = Vec::new();
    let mut add = |id: String| {
        if !ids.contains(&id) {
            ids.push(id);
        }
    };
    for (at, _) in source.match_indices("Check::new(") {
        if let Some(id) = literal_after(at + "Check::new(".len()) {
            add(id);
        }
    }
    for (at, _) in source.match_indices("\"net-") {
        if let Some(id) = literal_after(at) {
            add(id);
        }
    }
    // Идентификаторы, которые раннер собирает из частей.
    for dynamic in [
        "lib-games",
        "lib-downloads",
        "lib-internal",
        "steam-owns-220",
    ] {
        ids.push(dynamic.to_owned());
    }
    assert!(
        ids.len() > 50,
        "нашлось только {} идентификаторов",
        ids.len()
    );

    let mut keys: Vec<String> = ids
        .iter()
        .map(|id| format!("#L_DG_{}", id.to_uppercase().replace('-', "_")))
        .collect();
    // Значения: "#L_DGV_…" и "#L_S_…" в исходнике.
    let mut rest = source.as_str();
    while let Some(at) = rest.find("\"#L_") {
        let after = &rest[at + 1..];
        let end = after.find('"').unwrap();
        keys.push(after[..end].to_owned());
        rest = &after[end..];
    }
    assert!(keys.iter().any(|k| k.starts_with("#L_DGV_")));

    for language in ["ru", "en"] {
        let path = format!(
            "{}/../hlsr-ui/locales/{language}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let locale: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let missing: Vec<&String> = keys
            .iter()
            .filter(|key| {
                locale
                    .get(key.as_str())
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
            })
            .collect();
        assert!(missing.is_empty(), "{language}: нет ключей {missing:?}");
    }
}
