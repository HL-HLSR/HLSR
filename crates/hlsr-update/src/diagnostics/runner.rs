//! `diagnostics/DiagnosticsRunner.js`: проверки по группам `hlsr`, `graphics`, `downloads`,
//! `windows`|`linux`, `library`, `build`, `profile`, `steam`, `network`. Каждая проверка — строка
//! [`Check`]: `{id, group, status, code, detail, detailKey?, detailArgs?}`. Данные других
//! подпроектов приходят через [`DiagnosticsContext`], сеть и DNS — через [`DiagnosticsRequest`]
//! и [`DnsLookup`]. Прогон один за раз ([`RunStatus::AlreadyRunning`]) и отменяется в любой
//! точке ожидания.
//!
//! Отступления от 0.4.7 (причины — в отчёте задачи 7):
//! - строки GPU строятся из состояния рендерера ПП1 ([`GpuStatus`]), а не из данных Chromium
//!   (Р10); `compositing` у Chromium нет;
//! - нет повтора запроса через Chromium при ошибке TLS (см. [`super::net`]);
//! - `latest.yml` читается без YAML-библиотеки: два поля — `version` (строка) и непустой
//!   `files`;
//! - год для проверки часов — по UTC (`getFullYear()` местный): отличие только в новогоднюю ночь
//!   2019/2020 и 2100/2101.

use super::context::{
    ContextData, DiagnosticsContext, GpuStatus, LaunchAvailability, SteamApp, SteamHealth,
    TelemetryInfo,
};
use super::net::{
    DiagnosticsRequest, DnsLookup, NET_TIMEOUT, NetError, Response, classify_http_status,
    classify_network_error, error_detail,
};
use crate::cancel::CancelToken;
use crate::channel::{self, CheckOptions};
use crate::errors::LuCode;
use crate::ports::Clock;
use hlsr_core::js::fs::read_file;
use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::json_text::{js_number, json_parse};
use serde_json::{Map, Value, json};
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Ожидание ответа Steam о владении игрой (`OWNERSHIP_TIMEOUT_MS`).
pub const OWNERSHIP_TIMEOUT: Duration = Duration::from_millis(15_000);

/// Допустимые пределы скорости загрузки, МБ/с (`SPEED_LIMITS` из `shared/download-settings.js`).
const SPEED_LIMITS: [f64; 8] = [0.0, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0];

/// Адреса проверок, не принадлежащие каналу лаунчера (`ENDPOINTS`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Список релизов Proton Wineland (`ProtonManager.RELEASES_API`).
    pub proton_releases: String,
    /// Канал обновлений игры (`endpoints.game.channelUrl("ghosting-renewed")`).
    pub game_channel: String,
    /// `${GITHUB_RELEASE_BASE}/latest`.
    pub github_releases: String,
    pub github_rate_limit: String,
}

impl Endpoints {
    /// Боевые адреса 0.4.7.
    pub fn production() -> Endpoints {
        Endpoints {
            proton_releases:
                "https://api.github.com/repos/kanoyo-git/proton-cachyos/releases?per_page=20"
                    .to_owned(),
            game_channel: "https://hlsr.pro/downloads/ghosting-renewed/channel/stable.json"
                .to_owned(),
            github_releases: "https://github.com/HL-HLSR/HLSR-Releases/releases/latest".to_owned(),
            github_rate_limit: "https://api.github.com/rate_limit".to_owned(),
        }
    }
}

// ------------------------------------------------------------------- строка проверки

/// Строка проверки. Хранится как объект JSON с порядком ключей 0.4.7 — так её видят окно и
/// отчёт: `Object.assign({ id, group, status, code: null, detail: "" }, options)`; новые ключи
/// (`detailKey`, `detailArgs`) идут в порядке вызова методов.
#[derive(Debug, Clone, PartialEq)]
pub struct Check(Map<String, Value>);

impl Check {
    /// `check(id, group, status)`.
    pub fn new(id: &str, group: &str, status: &str) -> Check {
        let mut map = Map::new();
        map.insert("id".to_owned(), json!(id));
        map.insert("group".to_owned(), json!(group));
        map.insert("status".to_owned(), json!(status));
        map.insert("code".to_owned(), Value::Null);
        map.insert("detail".to_owned(), json!(""));
        Check(map)
    }

    /// Код причины; `None` — `null`.
    pub fn code(mut self, code: Option<&str>) -> Check {
        self.0.insert("code".to_owned(), json!(code));
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Check {
        self.0.insert("detail".to_owned(), json!(detail.into()));
        self
    }

    /// `detailKey`: ключ локали (`#L_DGV_…`).
    pub fn detail_key(mut self, key: &str) -> Check {
        self.0.insert("detailKey".to_owned(), json!(key));
        self
    }

    /// `detailKey: null` (в JS — выражение `launch && …` при пустом `launch`).
    pub fn detail_key_null(mut self) -> Check {
        self.0.insert("detailKey".to_owned(), Value::Null);
        self
    }

    pub fn detail_args(mut self, args: Vec<Value>) -> Check {
        self.0.insert("detailArgs".to_owned(), Value::Array(args));
        self
    }

    fn text(&self, key: &str) -> &str {
        self.0.get(key).and_then(Value::as_str).unwrap_or("")
    }

    pub fn id(&self) -> &str {
        self.text("id")
    }

    pub fn group(&self) -> &str {
        self.text("group")
    }

    pub fn status(&self) -> &str {
        self.text("status")
    }

    pub fn code_str(&self) -> Option<&str> {
        self.0.get("code").and_then(Value::as_str)
    }

    pub fn detail_str(&self) -> &str {
        self.text("detail")
    }

    pub fn detail_key_str(&self) -> Option<&str> {
        self.0.get("detailKey").and_then(Value::as_str)
    }

    /// Строка как JSON: то, что уходит окну и в `diagnostics-summary.json`.
    pub fn to_value(&self) -> Value {
        Value::Object(self.0.clone())
    }
}

/// Итог прогона (`run()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Ok,
    Cancelled,
    /// Прогон уже идёт: строк нет.
    AlreadyRunning,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Ok => "ok",
            RunStatus::Cancelled => "cancelled",
            RunStatus::AlreadyRunning => "already-running",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    pub status: RunStatus,
    pub checks: Vec<Check>,
}

impl RunResult {
    /// `{status, checks}`.
    pub fn to_value(&self) -> Value {
        json!({
            "status": self.status.as_str(),
            "checks": self.checks.iter().map(Check::to_value).collect::<Vec<_>>(),
        })
    }
}

/// Подписчик на строки по мере готовности (`emit`); его сбой не ломает прогон.
pub type Emit = dyn Fn(&Check) + Send + Sync;

struct Sink<'a> {
    checks: Mutex<Vec<Check>>,
    emit: Option<&'a Emit>,
}

impl Sink<'_> {
    fn push(&self, item: Check) {
        self.checks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(item.clone());
        if let Some(emit) = self.emit {
            // Подписчик не роняет диагностику.
            let _ = catch_unwind(AssertUnwindSafe(|| emit(&item)));
        }
    }

    fn into_checks(self) -> Vec<Check> {
        self.checks
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Прогон отменён (`error.code === "CANCELLED"`).
struct Cancelled;

/// `awaitCancelable(value)`: ждёт `fut`, но не дольше отмены.
async fn cancelable<T>(token: &CancelToken, fut: impl Future<Output = T>) -> Result<T, Cancelled> {
    tokio::select! {
        biased;
        () = token.cancelled() => Err(Cancelled),
        value = fut => Ok(value),
    }
}

// ------------------------------------------------------------------- раннер

/// `DiagnosticsRunner`.
pub struct DiagnosticsRunner {
    context: Arc<dyn DiagnosticsContext>,
    request: Arc<dyn DiagnosticsRequest>,
    dns: Arc<dyn DnsLookup>,
    clock: Arc<dyn Clock>,
    launcher_public_keys: Vec<String>,
    endpoints: Endpoints,
    net_timeout: Duration,
    running: AtomicBool,
    token: Mutex<CancelToken>,
}

struct RunningGuard<'a>(&'a AtomicBool);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl DiagnosticsRunner {
    pub fn new(
        context: Arc<dyn DiagnosticsContext>,
        request: Arc<dyn DiagnosticsRequest>,
        dns: Arc<dyn DnsLookup>,
        clock: Arc<dyn Clock>,
    ) -> DiagnosticsRunner {
        DiagnosticsRunner {
            context,
            request,
            dns,
            clock,
            launcher_public_keys: channel::default_public_keys(),
            endpoints: Endpoints::production(),
            net_timeout: NET_TIMEOUT,
            running: AtomicBool::new(false),
            token: Mutex::new(CancelToken::new()),
        }
    }

    /// `launcherPublicKeys`: доверенные ключи канала лаунчера (по умолчанию боевые).
    pub fn with_launcher_public_keys(mut self, keys: Vec<String>) -> DiagnosticsRunner {
        self.launcher_public_keys = keys;
        self
    }

    pub fn with_endpoints(mut self, endpoints: Endpoints) -> DiagnosticsRunner {
        self.endpoints = endpoints;
        self
    }

    /// Ожидание DNS (по умолчанию 6 с). Простой запроса задаёт сам [`DiagnosticsRequest`].
    pub fn with_net_timeout(mut self, timeout: Duration) -> DiagnosticsRunner {
        self.net_timeout = timeout;
        self
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// `cancel()`: прерывает ожидание текущего прогона.
    pub fn cancel(&self) {
        self.token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cancel();
    }

    /// `run(emit)`.
    pub async fn run(&self, emit: Option<&Emit>) -> RunResult {
        if self.running.swap(true, Ordering::SeqCst) {
            return RunResult {
                status: RunStatus::AlreadyRunning,
                checks: Vec::new(),
            };
        }
        let _running = RunningGuard(&self.running);
        let token = CancelToken::new();
        *self
            .token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = token.clone();
        let sink = Sink {
            checks: Mutex::new(Vec::new()),
            emit,
        };

        // Отмена на любом шаге заканчивает прогон; статус берётся из признака.
        let _ = self.run_steps(&token, &sink).await;

        RunResult {
            status: if token.is_cancelled() {
                RunStatus::Cancelled
            } else {
                RunStatus::Ok
            },
            checks: sink.into_checks(),
        }
    }

    async fn run_steps(&self, token: &CancelToken, sink: &Sink<'_>) -> Result<(), Cancelled> {
        let data = self.context.data();
        let linux = data.platform == Platform::Linux;

        self.run_hlsr_checks(&data, sink);
        self.run_gpu_checks(token, sink).await?;
        self.run_download_checks(&data, sink);
        if linux {
            self.run_linux_checks(&data, sink);
        } else {
            self.run_windows_checks(&data, sink);
        }
        self.run_library_checks(&data, sink);
        self.run_build_metadata_checks(&data, sink);
        self.run_profile_checks(sink);
        self.run_steam_checks(&data, token, sink).await?;
        self.run_network_checks(&data, token, sink).await
    }

    // --------------------------------------------------------------- hlsr

    fn run_hlsr_checks(&self, data: &ContextData, sink: &Sink<'_>) {
        let version = if data.app_version.is_empty() {
            "?"
        } else {
            &data.app_version
        };
        sink.push(Check::new("hlsr-version", "hlsr", "ok").detail(format!(
            "{version} ({})",
            if data.packaged { "packaged" } else { "dev" }
        )));
        sink.push(
            Check::new(
                "hlsr-arch",
                "hlsr",
                if data.arch == "x64" { "ok" } else { "warn" },
            )
            .detail(&data.arch),
        );
        if data.platform == Platform::Linux {
            sink.push(
                Check::new(
                    "hlsr-umu",
                    "hlsr",
                    if data.umu_available { "ok" } else { "warn" },
                )
                .code((!data.umu_available).then_some("UMU_NOT_FOUND")),
            );
        } else {
            sink.push(
                Check::new(
                    "hlsr-native",
                    "hlsr",
                    if data.native_available { "ok" } else { "warn" },
                )
                .code((!data.native_available).then_some("NATIVE_UNAVAILABLE"))
                .detail(if data.native_available {
                    String::new()
                } else {
                    js::slice_utf16(&data.native_load_error, 500)
                }),
            );
        }

        let writable = data.user_data_path.as_deref().is_some_and(write_probe);
        sink.push(
            Check::new(
                "hlsr-userdata",
                "hlsr",
                if writable { "ok" } else { "fail" },
            )
            .code((!writable).then_some("USERDATA_READONLY")),
        );

        let update_failed = data.update_state == "error";
        let disabled = data.update_enabled == Some(false);
        sink.push(
            Check::new(
                "hlsr-updater",
                "hlsr",
                if update_failed {
                    "fail"
                } else if disabled {
                    "warn"
                } else {
                    "ok"
                },
            )
            .detail(if data.update_state.is_empty() {
                "idle"
            } else {
                &data.update_state
            })
            .code(if update_failed {
                Some("UPDATER_ERROR")
            } else if disabled {
                Some("UPDATER_DISABLED")
            } else {
                None
            }),
        );
    }

    // --------------------------------------------------------------- graphics

    async fn run_gpu_checks(&self, token: &CancelToken, sink: &Sink<'_>) -> Result<(), Cancelled> {
        let gpu: Option<GpuStatus> = cancelable(token, self.context.gpu_status()).await?;
        let Some(gpu) = gpu else {
            sink.push(
                Check::new("gpu-runtime", "graphics", "warn").code(Some("GPU_STATUS_UNKNOWN")),
            );
            return Ok(());
        };
        let enabled_key = |enabled: bool| {
            if enabled {
                "#L_DGV_ENABLED"
            } else {
                "#L_DGV_DISABLED"
            }
        };
        sink.push(
            Check::new("gpu-configured", "graphics", "ok")
                .detail_key(enabled_key(gpu.configured_enabled)),
        );
        let pending = gpu
            .applied_enabled
            .is_some_and(|applied| applied != gpu.configured_enabled);
        sink.push(
            Check::new(
                "gpu-applied",
                "graphics",
                if gpu.applied_enabled.is_none() || pending {
                    "warn"
                } else {
                    "ok"
                },
            )
            .code(if pending {
                Some("GPU_RESTART_REQUIRED")
            } else if gpu.applied_enabled.is_none() {
                Some("GPU_STATUS_UNKNOWN")
            } else {
                None
            })
            .detail_key(match gpu.applied_enabled {
                None => "#L_DGV_UNKNOWN",
                Some(applied) => enabled_key(applied),
            }),
        );
        if !gpu.available {
            sink.push(
                Check::new("gpu-runtime", "graphics", "warn").code(Some("GPU_STATUS_UNKNOWN")),
            );
            return Ok(());
        }
        let hardware = gpu.hardware_enabled && !gpu.software_renderer;
        let disable_failed = gpu.applied_enabled == Some(false) && gpu.hardware_enabled;
        let fallback = gpu.applied_enabled == Some(true) && !hardware;
        sink.push(
            Check::new(
                "gpu-runtime",
                "graphics",
                if disable_failed {
                    "fail"
                } else if fallback {
                    "warn"
                } else {
                    "ok"
                },
            )
            .code(if disable_failed {
                Some("GPU_DISABLE_FAILED")
            } else if fallback {
                Some("GPU_SOFTWARE_FALLBACK")
            } else {
                None
            })
            .detail_key(if hardware {
                "#L_DGV_GPU_HARDWARE"
            } else {
                "#L_DGV_GPU_SOFTWARE"
            }),
        );
        if !gpu.device.is_empty() {
            sink.push(Check::new("gpu-device", "graphics", "ok").detail(&gpu.device));
        }
        if !gpu.driver.is_empty() {
            sink.push(Check::new("gpu-driver", "graphics", "ok").detail(&gpu.driver));
        }
        Ok(())
    }

    // --------------------------------------------------------------- downloads

    fn run_download_checks(&self, data: &ContextData, sink: &Sink<'_>) {
        let Some(user_data) = data.user_data_path.as_deref() else {
            sink.push(
                Check::new("downloads-limit", "downloads", "warn")
                    .code(Some("DOWNLOAD_SETTINGS_INVALID")),
            );
            return;
        };
        let saved = read_json_state(&user_data.join("download-settings.json"));
        let limit = saved
            .data
            .as_ref()
            .filter(|data| saved.valid && js::truthy(data))
            .and_then(|data| data.get("speedLimit"));
        let valid_limit =
            limit.is_some_and(|limit| matches!(limit, Value::Number(n) if n.as_f64().is_some_and(|x| SPEED_LIMITS.contains(&x))));
        if saved.found && (!saved.valid || !valid_limit) {
            sink.push(
                Check::new("downloads-limit", "downloads", "fail")
                    .code(Some("DOWNLOAD_SETTINGS_INVALID")),
            );
            return;
        }
        let speed_limit = if saved.found {
            limit.and_then(Value::as_f64).unwrap_or(0.0)
        } else {
            0.0
        };
        let limit_text = js::to_string(&js_number(speed_limit));
        sink.push(
            Check::new("downloads-limit", "downloads", "ok")
                .detail_key(if speed_limit != 0.0 {
                    "#L_S_MBPS"
                } else {
                    "#L_S_NO_LIMIT"
                })
                .detail_args(if speed_limit != 0.0 {
                    vec![json!(limit_text)]
                } else {
                    Vec::new()
                }),
        );
        if let Some(effective) = self.context.download_limit_bps().filter(|x| x.is_finite()) {
            let applied = effective == speed_limit * 1024.0 * 1024.0;
            sink.push(
                Check::new(
                    "downloads-applied",
                    "downloads",
                    if applied { "ok" } else { "warn" },
                )
                .code((!applied).then_some("DOWNLOAD_LIMIT_NOT_APPLIED")),
            );
        }
    }

    // --------------------------------------------------------------- ОС

    fn clock_check(&self, id: &str, group: &str) -> Check {
        let now = iso_from_ms(self.clock.now_ms());
        let year: i64 = now.get(..4).and_then(|y| y.parse().ok()).unwrap_or(0);
        let sane = (2020..=2100).contains(&year);
        Check::new(id, group, if sane { "ok" } else { "warn" })
            .code((!sane).then_some("CLOCK_SUSPECT"))
            .detail(now.get(..10).unwrap_or(&now))
    }

    fn run_windows_checks(&self, data: &ContextData, sink: &Sink<'_>) {
        sink.push(
            Check::new("win-version", "windows", "ok")
                .detail(format!("Windows {}", data.os_release)),
        );
        sink.push(
            Check::new(
                "win-arch",
                "windows",
                if data.arch == "x64" { "ok" } else { "warn" },
            )
            .detail(&data.arch),
        );
        sink.push(self.clock_check("win-clock", "windows"));
    }

    fn run_linux_checks(&self, data: &ContextData, sink: &Sink<'_>) {
        sink.push(Check::new("linux-kernel", "linux", "ok").detail(&data.os_release));
        sink.push(
            Check::new(
                "linux-arch",
                "linux",
                if data.arch == "x64" { "ok" } else { "warn" },
            )
            .detail(&data.arch),
        );
        sink.push(
            Check::new(
                "linux-proton",
                "linux",
                if data.proton_selected.is_some() {
                    "ok"
                } else {
                    "warn"
                },
            )
            .code(
                data.proton_selected
                    .is_none()
                    .then_some("PROTON_NOT_INSTALLED"),
            )
            .detail(data.proton_selected.as_deref().unwrap_or("")),
        );
        sink.push(self.clock_check("linux-clock", "linux"));
    }

    // --------------------------------------------------------------- библиотека

    fn run_library_checks(&self, data: &ContextData, sink: &Sink<'_>) {
        let root = data.library_path.as_deref();
        let Some(paths) = resolve_library(root) else {
            sink.push(
                Check::new("lib-root", "library", "fail")
                    .code(Some("LIBRARY_UNAVAILABLE"))
                    .detail(root.unwrap_or("undefined")),
            );
            return;
        };
        sink.push(Check::new("lib-root", "library", "ok").detail(root.unwrap_or("")));

        let exists = Path::new(&paths.root).exists();
        sink.push(
            Check::new("lib-exists", "library", if exists { "ok" } else { "warn" })
                .code((!exists).then_some("LIBRARY_MISSING")),
        );

        if exists {
            let writable = write_probe(Path::new(&paths.root));
            sink.push(
                Check::new(
                    "lib-writable",
                    "library",
                    if writable { "ok" } else { "fail" },
                )
                .code((!writable).then_some("LIBRARY_READ_ONLY")),
            );
            for (key, dir) in [
                ("games", &paths.games),
                ("downloads", &paths.downloads),
                ("internal", &paths.internal),
            ] {
                sink.push(
                    Check::new(
                        &format!("lib-{key}"),
                        "library",
                        if Path::new(dir).exists() {
                            "ok"
                        } else {
                            "warn"
                        },
                    )
                    .detail(NodePath::native().basename(dir)),
                );
            }
        }

        match data.free_space_mb.filter(|mb| mb.is_finite()) {
            None => sink
                .push(Check::new("lib-space", "library", "warn").code(Some("DISK_SPACE_UNKNOWN"))),
            Some(mb) => sink.push(
                Check::new(
                    "lib-space",
                    "library",
                    if mb > 6000.0 { "ok" } else { "warn" },
                )
                .code((mb <= 6000.0).then_some("LOW_DISK_SPACE"))
                .detail(format!("{} GB", js::to_fixed(mb / 1024.0, 1))),
            ),
        }

        let installed_file = installed_file_of(&paths);
        if Path::new(&installed_file).exists() {
            let installed = read_installed(&installed_file);
            let ok = installed.as_ref().is_some_and(|data| {
                data.get("gameId").and_then(Value::as_str) == Some("ghosting-renewed")
                    && data.get("state").and_then(Value::as_str) == Some("ready")
            });
            let version = installed
                .as_ref()
                .and_then(|data| data.get("version"))
                .filter(|version| js::truthy(version))
                .map(js::to_string)
                .unwrap_or_default();
            sink.push(
                Check::new("lib-installed", "library", if ok { "ok" } else { "fail" })
                    .code((!ok).then_some("INSTALLED_JSON_INVALID"))
                    .detail(version),
            );
        } else {
            sink.push(
                Check::new("lib-installed", "library", "warn").code(Some("GAME_NOT_INSTALLED")),
            );
        }
    }

    // --------------------------------------------------------------- сведения о сборке

    fn run_build_metadata_checks(&self, data: &ContextData, sink: &Sink<'_>) {
        let Some(paths) = resolve_library(data.library_path.as_deref()) else {
            return;
        };
        let installed_file = installed_file_of(&paths);
        if !Path::new(&installed_file).exists() {
            sink.push(
                Check::new("build-meta-present", "build", "warn").code(Some("GAME_NOT_INSTALLED")),
            );
            return;
        }
        let Some(installed) = read_installed(&installed_file) else {
            sink.push(
                Check::new("build-meta-present", "build", "fail")
                    .code(Some("INSTALLED_JSON_INVALID")),
            );
            return;
        };
        let field = |name: &str| installed.get(name);
        let published_at = field("publishedAt").filter(|v| js::truthy(v));

        let snapshot = field("buildMetadata").filter(|v| js::truthy(v));
        let Some(meta) = snapshot else {
            sink.push(
                Check::new("build-meta-present", "build", "warn")
                    .code(Some("BUILD_METADATA_LEGACY"))
                    .detail("legacy"),
            );
            sink.push(
                Check::new(
                    "build-meta-date",
                    "build",
                    if published_at.is_some() { "ok" } else { "warn" },
                )
                .detail(match published_at {
                    Some(at) => format!("publishedAt {}", js::slice_utf16(&js::to_string(at), 10)),
                    None => "Нет данных".to_owned(),
                }),
            );
            sink.push(
                Check::new("build-meta-trust", "build", "ok")
                    .code(Some("BUILD_METADATA_LEGACY"))
                    .detail("legacy"),
            );
            return;
        };
        let meta_text = |name: &str, default: &str| -> String {
            match meta.get(name).filter(|v| js::truthy(v)) {
                Some(value) => js::to_string(value),
                None => default.to_owned(),
            }
        };

        let runtime = self.context.build_runtime(&installed, &data.app_version);
        let reason = runtime.reason.as_deref().filter(|r| !r.is_empty());
        let trusted = runtime.state == "trusted";
        let only = |wanted: &'static str| (reason == Some(wanted)).then_some(wanted);

        sink.push(
            Check::new(
                "build-meta-present",
                "build",
                if reason == Some("BUILD_METADATA_MISSING") {
                    "fail"
                } else {
                    "ok"
                },
            )
            .code(only("BUILD_METADATA_MISSING"))
            .detail(meta_text("buildName", "")),
        );
        sink.push(
            Check::new(
                "build-meta-schema",
                "build",
                if reason == Some("BUILD_METADATA_UNSUPPORTED") {
                    "fail"
                } else {
                    "ok"
                },
            )
            .code(only("BUILD_METADATA_UNSUPPORTED"))
            .detail(format!("schema {}", meta_text("schemaVersion", "1"))),
        );
        sink.push(
            Check::new(
                "build-meta-identity",
                "build",
                if trusted {
                    "ok"
                } else if reason == Some("BUILD_METADATA_MISMATCH") {
                    "fail"
                } else {
                    "warn"
                },
            )
            .code(only("BUILD_METADATA_MISMATCH"))
            .detail(meta_text("buildId", "")),
        );
        sink.push(
            Check::new("build-meta-version", "build", "ok").detail(format!(
                "{} ({})",
                meta_text("version", "?"),
                meta_text("architecture", "?")
            )),
        );

        if let Some(expected) = field("buildMetadataSha256")
            .filter(|v| js::truthy(v))
            .map(js::to_string)
        {
            let hash_ok = match installed_install_path(&installed) {
                Some(install_path) => matches!(
                    self.context.build_metadata_sha256(&install_path),
                    Ok(Some(actual)) if actual == expected
                ),
                None => false,
            };
            sink.push(
                Check::new(
                    "build-meta-hash",
                    "build",
                    if hash_ok { "ok" } else { "fail" },
                )
                .code((!hash_ok).then_some("BUILD_METADATA_MISMATCH"))
                .detail(if hash_ok {
                    format!("sha {}…", js::slice_utf16(&expected, 12))
                } else {
                    "не совпал".to_owned()
                }),
            );
        }

        let built_at = meta.get("builtAt").filter(|v| js::truthy(v));
        let date = if let Some(at) = built_at {
            Some(format!(
                "builtAt {}",
                js::slice_utf16(&js::to_string(at), 10)
            ))
        } else {
            published_at
                .map(|at| format!("publishedAt {}", js::slice_utf16(&js::to_string(at), 10)))
        };
        sink.push(
            Check::new(
                "build-meta-date",
                "build",
                if date.is_some() { "ok" } else { "warn" },
            )
            .detail(date.unwrap_or_else(|| "Нет данных".to_owned())),
        );

        sink.push(
            Check::new(
                "build-meta-launcher",
                "build",
                if reason == Some("BUILD_REQUIRES_NEWER_LAUNCHER") {
                    "fail"
                } else {
                    "ok"
                },
            )
            .code(only("BUILD_REQUIRES_NEWER_LAUNCHER"))
            .detail(format!(">= {}", meta_text("minimumLauncherVersion", "?"))),
        );

        sink.push(
            Check::new(
                "build-meta-trust",
                "build",
                if trusted {
                    "ok"
                } else if runtime.state == "launcher-update-required" {
                    "warn"
                } else {
                    "fail"
                },
            )
            .code(reason)
            .detail(&runtime.state),
        );
    }

    // --------------------------------------------------------------- профиль

    fn run_profile_checks(&self, sink: &Sink<'_>) {
        let Some(snapshot) = self.context.profile() else {
            sink.push(
                Check::new("profile-state", "profile", "warn").code(Some("PROFILE_STATE_UNKNOWN")),
            );
            return;
        };
        sink.push(
            Check::new(
                "profile-steam",
                "profile",
                if snapshot.signed_in { "ok" } else { "warn" },
            )
            .code((!snapshot.signed_in).then_some("STEAM_SIGNED_OUT")),
        );

        let seconds = number_or_zero(snapshot.play_seconds).floor().max(0.0);
        let hours = (seconds / 3600.0).floor();
        let minutes = (seconds / 60.0).floor() % 60.0;
        let secs = seconds % 60.0;
        sink.push(
            Check::new("profile-playtime", "profile", "ok")
                .detail(format!(
                    "{}h {}m {}s",
                    js::to_string(&js_number(hours)),
                    js::to_string(&js_number(minutes)),
                    js::to_string(&js_number(secs))
                ))
                .detail_key("#L_DGV_PLAYTIME")
                .detail_args(vec![
                    js_number(hours),
                    js_number(minutes),
                    js_number(secs),
                    snapshot.launch_count.map_or(Value::Null, js_number),
                    snapshot.games_played.map_or(Value::Null, js_number),
                ]),
        );

        if snapshot.launch_count.is_some_and(|count| count > 0.0) && snapshot.play_seconds <= 0.0 {
            sink.push(
                Check::new("profile-playtime-stuck", "profile", "warn")
                    .code(Some("PLAYTIME_NOT_COUNTED")),
            );
        }

        let top = snapshot.top_game_id.as_deref().filter(|id| !id.is_empty());
        sink.push(
            Check::new(
                "profile-top-game",
                "profile",
                if top.is_some() { "ok" } else { "warn" },
            )
            .code(top.is_none().then_some("NO_TOP_GAME"))
            .detail(top.unwrap_or("")),
        );

        if let Some((unlocked, total)) = snapshot.achievements {
            sink.push(
                Check::new("profile-achievements", "profile", "ok").detail(format!(
                    "{}/{}",
                    js::to_string(&js_number(unlocked)),
                    js::to_string(&js_number(total))
                )),
            );
        }

        let since = snapshot.installed_at.as_deref().filter(|at| !at.is_empty());
        sink.push(
            Check::new(
                "profile-since",
                "profile",
                if since.is_some() { "ok" } else { "warn" },
            )
            .code(since.is_none().then_some("INSTALL_DATE_UNKNOWN"))
            .detail(since.map_or_else(String::new, |at| js::slice_utf16(at, 10))),
        );

        match self.context.telemetry() {
            TelemetryInfo::Status(telemetry) => {
                let check = match telemetry {
                    None => Check::new("profile-telemetry", "profile", "warn")
                        .code(Some("PROFILE_STATE_UNKNOWN"))
                        .detail_key("#L_DGV_UNKNOWN"),
                    Some(t) if !t.backend_configured => {
                        Check::new("profile-telemetry", "profile", "skip")
                            .code(Some("TELEMETRY_NOT_CONFIGURED"))
                            .detail_key("#L_DGV_TELEMETRY_OFFLINE")
                    }
                    Some(t) => {
                        Check::new("profile-telemetry", "profile", "ok").detail_key(if t.enabled {
                            "#L_DGV_ENABLED"
                        } else {
                            "#L_DGV_DISABLED"
                        })
                    }
                };
                sink.push(check);
            }
            TelemetryInfo::Enabled(enabled) => sink.push(
                Check::new("profile-telemetry", "profile", "ok").detail(if enabled {
                    "включена"
                } else {
                    "выключена"
                }),
            ),
            TelemetryInfo::Unavailable => {}
        }
    }

    // --------------------------------------------------------------- Steam

    async fn run_steam_checks(
        &self,
        data: &ContextData,
        token: &CancelToken,
        sink: &Sink<'_>,
    ) -> Result<(), Cancelled> {
        let health = data.steam_health.clone().unwrap_or(SteamHealth {
            status: "unknown".to_owned(),
            code: Some("STEAM_CHECK_FAILED".to_owned()),
            detail: None,
        });
        sink.push(
            Check::new(
                "steam-running",
                "steam",
                match health.status.as_str() {
                    "available" => "ok",
                    "unknown" => "warn",
                    _ => "fail",
                },
            )
            .code(health.code.as_deref().filter(|c| !c.is_empty()))
            .detail(health.detail.as_deref().unwrap_or("")),
        );

        let launch: Option<LaunchAvailability> =
            cancelable(token, self.context.launch_availability()).await?;
        sink.push(match &launch {
            None => Check::new("steam-launch", "steam", "warn")
                .detail_key_null()
                .code(Some("STEAM_CHECK_FAILED")),
            Some(launch) if launch.ok => Check::new("steam-launch", "steam", "ok")
                .detail_key("#L_DGV_LAUNCH_READY")
                .code(None),
            Some(launch) => {
                let code = launch
                    .code
                    .as_deref()
                    .filter(|c| !c.is_empty())
                    .unwrap_or("LAUNCH_CONFIG_INVALID");
                let check = Check::new("steam-launch", "steam", "warn");
                let check = match launch_detail_key(launch.code.as_deref().unwrap_or("")) {
                    Some(key) => check.detail_key(key),
                    None => check,
                };
                check.code(Some(code))
            }
        });

        self.run_ownership_checks(token, sink).await
    }

    async fn run_ownership_checks(
        &self,
        token: &CancelToken,
        sink: &Sink<'_>,
    ) -> Result<(), Cancelled> {
        let apps: Vec<SteamApp> = self.context.required_steam_apps();
        if apps.is_empty() {
            return Ok(());
        }
        tokio::task::yield_now().await;
        if token.is_cancelled() {
            return Ok(());
        }
        for app in apps {
            if token.is_cancelled() {
                return Ok(());
            }
            let owned = cancelable(
                token,
                self.context.steam_ownership(app.app_id, OWNERSHIP_TIMEOUT),
            )
            .await?;
            let suffix = format!("{} (AppID {})", app.title, app.app_id);
            sink.push(
                Check::new(
                    &format!("steam-owns-{}", app.app_id),
                    "steam",
                    match owned {
                        Some(true) => "ok",
                        Some(false) => "fail",
                        None => "warn",
                    },
                )
                .code(match owned {
                    Some(true) => None,
                    Some(false) => Some("STEAM_APP_NOT_OWNED"),
                    None => Some("LICENSE_CHECK_UNAVAILABLE"),
                })
                .detail(suffix),
            );
        }
        Ok(())
    }

    // --------------------------------------------------------------- сеть

    async fn run_network_checks(
        &self,
        data: &ContextData,
        token: &CancelToken,
        sink: &Sink<'_>,
    ) -> Result<(), Cancelled> {
        if token.is_cancelled() {
            return Ok(());
        }
        let dns = tokio::time::timeout(
            self.net_timeout,
            cancelable(token, self.dns.lookup("github.com")),
        )
        .await;
        match dns {
            Err(_) => sink.push(
                Check::new("net-dns", "network", "fail")
                    .code(Some(classify_network_error(&NetError::timeout()))),
            ),
            Ok(Err(Cancelled)) => return Ok(()),
            Ok(Ok(Ok(()))) => {
                sink.push(Check::new("net-dns", "network", "ok").detail("github.com"));
            }
            Ok(Ok(Err(error))) => {
                if error.is_cancelled() {
                    return Ok(());
                }
                sink.push(
                    Check::new("net-dns", "network", "fail")
                        .code(Some(classify_network_error(&error))),
                );
            }
        }
        if token.is_cancelled() {
            return Ok(());
        }

        let forbidden = AtomicBool::new(false);
        let platform = data.platform;
        let feed = channel::launcher_for(platform);
        let keys = &self.launcher_public_keys;

        let validate_launcher = |response: &Response| -> Option<&'static str> {
            let body = match (&response.body, response.truncated) {
                (Some(body), false) => body,
                _ => return Some("CHECKSUM_METADATA_INVALID"),
            };
            let Ok(manifest) = json_parse(&js::utf8_string(body)) else {
                return Some("CHECKSUM_METADATA_INVALID");
            };
            let verdict = channel::check(
                &manifest,
                &CheckOptions {
                    current_version: None,
                    public_keys: Some(keys),
                    platform: Some(platform),
                },
            );
            match verdict {
                Ok(verdict) if verdict.ok => None,
                Ok(verdict) if verdict.code == Some(LuCode::SignatureInvalid) => {
                    Some("SIGNATURE_INVALID")
                }
                _ => Some("CHECKSUM_METADATA_INVALID"),
            }
        };
        let validate_proton = |response: &Response| -> Option<&'static str> {
            let Some(list) = json_body(response) else {
                return Some("CHECKSUM_METADATA_INVALID");
            };
            let ok = list
                .as_array()
                .is_some_and(|list| list.iter().any(|r| self.context.proton_release_ok(r)));
            (!ok).then_some("CHECKSUM_METADATA_INVALID")
        };
        let validate_latest_yml = |response: &Response| -> Option<&'static str> {
            match (&response.body, response.truncated) {
                (Some(body), false) if latest_yml_is_valid(&js::utf8_string(body)) => None,
                _ => Some("CHECKSUM_METADATA_INVALID"),
            }
        };
        let validate_game = |response: &Response| -> Option<&'static str> {
            let Some(data) = json_body(response) else {
                return Some("CHECKSUM_METADATA_INVALID");
            };
            match self.context.validate_game_channel(&data) {
                Ok(()) => None,
                Err(error) if error.code.as_deref() == Some("SIGNATURE_INVALID") => {
                    Some("SIGNATURE_INVALID")
                }
                Err(_) => Some("CHECKSUM_METADATA_INVALID"),
            }
        };

        let forbidden = &forbidden;
        let third = async {
            if platform == Platform::Linux {
                self.probe(
                    token,
                    sink,
                    forbidden,
                    "net-proton-releases",
                    &self.endpoints.proton_releases,
                    Some(&validate_proton),
                )
                .await;
            } else {
                self.probe(
                    token,
                    sink,
                    forbidden,
                    "net-latest-yml",
                    &feed.latest_yml_urls[0],
                    Some(&validate_latest_yml),
                )
                .await;
            }
        };
        tokio::join!(
            self.probe(
                token,
                sink,
                forbidden,
                "net-launcher-channel",
                &feed.channel_urls[0],
                Some(&validate_launcher),
            ),
            self.probe(
                token,
                sink,
                forbidden,
                "net-launcher-fallback",
                &feed.channel_urls[1],
                Some(&validate_launcher),
            ),
            third,
            self.probe(
                token,
                sink,
                forbidden,
                "net-game-channel",
                &self.endpoints.game_channel,
                Some(&validate_game),
            ),
            self.probe(
                token,
                sink,
                forbidden,
                "net-github",
                &self.endpoints.github_releases,
                None,
            ),
        );

        if forbidden.load(Ordering::SeqCst) && !token.is_cancelled() {
            self.rate_limit_check(token, sink).await;
        }
        Ok(())
    }

    /// `probe(id, url, validate)`: один адрес — одна строка `network`.
    async fn probe(
        &self,
        token: &CancelToken,
        sink: &Sink<'_>,
        forbidden: &AtomicBool,
        id: &'static str,
        url: &str,
        validate: Option<&Validate<'_>>,
    ) {
        if token.is_cancelled() {
            return;
        }
        match cancelable(token, self.request.get(url)).await {
            Err(Cancelled) => {}
            Ok(Err(error)) => {
                if error.is_cancelled() {
                    return;
                }
                sink.push(
                    Check::new(id, "network", "fail")
                        .code(Some(classify_network_error(&error)))
                        .detail(error_detail(&error)),
                );
            }
            Ok(Ok(response)) => {
                let status = response.status;
                let http_code =
                    classify_http_status(status).or(if validate.is_some() && status != 200 {
                        Some("UNKNOWN_NETWORK_ERROR")
                    } else {
                        None
                    });
                if let Some(code) = http_code {
                    if status == 403 {
                        forbidden.store(true, Ordering::SeqCst);
                    }
                    sink.push(
                        Check::new(id, "network", "fail")
                            .code(Some(code))
                            .detail(format!("HTTP {status}")),
                    );
                    return;
                }
                if let Some(validate) = validate
                    && let Some(verdict) = validate(&response)
                {
                    sink.push(Check::new(id, "network", "fail").code(Some(verdict)));
                    return;
                }
                sink.push(Check::new(id, "network", "ok").detail(format!("HTTP {status}")));
            }
        }
    }

    /// После 403 от GitHub: сколько осталось запросов в API (`net-rate-limit`).
    async fn rate_limit_check(&self, token: &CancelToken, sink: &Sink<'_>) {
        let response = cancelable(token, self.request.get(&self.endpoints.github_rate_limit)).await;
        let response = match response {
            Err(Cancelled) => return,
            Ok(Err(error)) => {
                if !error.is_cancelled() {
                    sink.push(
                        Check::new("net-rate-limit", "network", "warn")
                            .code(Some(classify_network_error(&error))),
                    );
                }
                return;
            }
            Ok(Ok(response)) => response,
        };
        if let Some(code) = classify_http_status(response.status) {
            sink.push(Check::new("net-rate-limit", "network", "warn").code(Some(code)));
            return;
        }
        let remaining = json_body(&response)
            .and_then(|data| {
                data.get("resources")?
                    .get("core")?
                    .get("remaining")?
                    .as_f64()
            })
            .filter(|remaining| remaining.is_finite());
        let Some(remaining) = remaining else {
            sink.push(
                Check::new("net-rate-limit", "network", "warn")
                    .code(Some("CHECKSUM_METADATA_INVALID")),
            );
            return;
        };
        let limited = remaining == 0.0;
        sink.push(
            Check::new(
                "net-rate-limit",
                "network",
                if limited { "fail" } else { "ok" },
            )
            .code(limited.then_some("GITHUB_API_RATE_LIMIT")),
        );
    }
}

type Validate<'a> = dyn Fn(&Response) -> Option<&'static str> + Sync + 'a;

// ------------------------------------------------------------------- помощники

/// `launchDetails[code]`: ключ локали для кода среды запуска.
fn launch_detail_key(code: &str) -> Option<&'static str> {
    Some(match code {
        "STEAM_NOT_INSTALLED" => "#L_S_LAUNCH_STEAM_NOT_INSTALLED",
        "STEAM_NOT_RUNNING" | "STEAM_PROCESS_MISMATCH" => "#L_S_LAUNCH_STEAM_NOT_RUNNING",
        "SOURCE_SDK_NOT_INSTALLED" | "SOURCE_SDK_INCOMPLETE" => "#L_S_LAUNCH_SDK_MISSING",
        "PROTON_NOT_INSTALLED" => "#L_S_PROTON_EMPTY",
        "UMU_NOT_FOUND" => "#L_DGV_UMU_MISSING",
        _ => return None,
    })
}

/// `Number(x) || 0`.
fn number_or_zero(x: f64) -> f64 {
    if x.is_nan() { 0.0 } else { x }
}

/// Тело ответа как JSON (`JSON.parse(body.toString("utf8"))`); усечённое или пустое — `None`.
fn json_body(response: &Response) -> Option<Value> {
    if response.truncated {
        return None;
    }
    json_parse(&js::utf8_string(response.body.as_deref()?)).ok()
}

/// `writeProbe(dir)`: создать и сразу удалить пустой файл — каталог доступен для записи.
fn write_probe(dir: &Path) -> bool {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let unique = (nanos as u64) ^ (COUNTER.fetch_add(1, Ordering::SeqCst) << 48);
    let probe = dir.join(format!(
        ".hlsr-diag-{}-{:012x}",
        std::process::id(),
        unique & 0xFFFF_FFFF_FFFF
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if options.open(&probe).is_err() {
        return false;
    }
    // Не удалился — файл-проба остаётся мусором: ещё одна попытка и отказ.
    if std::fs::remove_file(&probe).is_ok() {
        return true;
    }
    let _ = std::fs::remove_file(&probe);
    false
}

/// Итог `readJsonState`.
struct JsonState {
    found: bool,
    valid: bool,
    data: Option<Value>,
}

/// `readJsonState(file)`: файла нет, он не читается или не JSON — флаги; BOM срезается.
fn read_json_state(file: &Path) -> JsonState {
    let bytes = match read_file(file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return JsonState {
                found: false,
                valid: false,
                data: None,
            };
        }
        Err(_) => {
            return JsonState {
                found: true,
                valid: false,
                data: None,
            };
        }
    };
    let text = js::utf8_string(&bytes);
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    match json_parse(text) {
        Ok(data) => JsonState {
            found: true,
            valid: true,
            data: Some(data),
        },
        Err(_) => JsonState {
            found: true,
            valid: false,
            data: None,
        },
    }
}

/// `JSON.parse(fs.readFileSync(installedFile, "utf8"))`: BOM — ошибка разбора. Пустое значение
/// (`null`, `0`, `""`) — как «нет данных»: проверки сведений о сборке на нём останавливаются.
fn read_installed(file: &str) -> Option<Value> {
    let bytes = read_file(Path::new(file)).ok()?;
    json_parse(&js::utf8_string(&bytes)).ok().filter(js::truthy)
}

fn installed_install_path(installed: &Value) -> Option<String> {
    installed
        .get("installPath")
        .filter(|v| js::truthy(v))
        .map(js::to_string)
}

struct LibraryPaths {
    root: String,
    games: String,
    downloads: String,
    internal: String,
}

fn installed_file_of(paths: &LibraryPaths) -> String {
    NodePath::native().join(&[&paths.games, "ghosting-renewed", "installed.json"])
}

/// `resolveLibrary(root)` из `game-delivery/paths.js`: абсолютный путь не в корне диска.
/// `None` — `PATH_INVALID`.
fn resolve_library(raw: Option<&str>) -> Option<LibraryPaths> {
    let trimmed = js::trim(raw?);
    if trimmed.is_empty() {
        return None;
    }
    let path = NodePath::native();
    let root = path.resolve(&[trimmed]);
    let absolute = match Platform::CURRENT {
        Platform::Win32 => {
            let bytes = root.as_bytes();
            bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/')
        }
        Platform::Linux => path.is_absolute(trimmed),
    };
    if !absolute || path.dirname(&root) == root {
        return None;
    }
    Some(LibraryPaths {
        games: path.join(&[&root, "Games"]),
        downloads: path.join(&[&root, "Downloads"]),
        internal: path.join(&[&root, ".hlsr"]),
        root,
    })
}

// ------------------------------------------------------------------- latest.yml

/// Проверка `latest.yml` из диагностики: `version` — строка и `files` — непустой список. В 0.4.7 документ
/// грузит js-yaml; здесь читаются только эти два поля верхнего уровня и повтор любого ключа верхнего
/// уровня считается ошибкой (как у js-yaml: «duplicated mapping key»). Отступление: другие ошибки
/// синтаксиса не замечаются.
pub fn latest_yml_is_valid(text: &str) -> bool {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut version_is_string = false;
    let mut files_items = 0usize;
    let mut in_files = false;
    let mut seen: Vec<&str> = Vec::new();

    for line in text.lines() {
        let top_level = !line.starts_with([' ', '\t', '#']) && !line.trim().is_empty();
        let item = line.trim_start().starts_with("- ") || line.trim() == "-";
        if top_level && !item {
            in_files = false;
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            if seen.contains(&key) {
                return false;
            }
            seen.push(key);
            let value = value.trim();
            match key {
                "version" => version_is_string = yaml_scalar_is_string(value),
                "files" => {
                    in_files = true;
                    if value.starts_with('[') && !value.trim_matches(['[', ']', ' ']).is_empty() {
                        files_items += 1;
                    }
                }
                _ => {}
            }
        } else if in_files && item {
            files_items += 1;
        }
    }
    version_is_string && files_items > 0
}

/// Скаляр остаётся строкой в схеме js-yaml 5 (core): не `null`, не булево, не целое и не дробное
/// число. Даты и время строкой остаются.
fn yaml_scalar_is_string(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if value.starts_with(['\'', '"']) {
        return true;
    }
    // Комментарий после значения.
    let value = value.split(" #").next().unwrap_or(value).trim();
    if value.is_empty() {
        return false;
    }
    !(matches!(
        value,
        "~" | "null" | "Null" | "NULL" | "true" | "True" | "TRUE" | "false" | "False" | "FALSE"
    ) || is_yaml_int(value)
        || is_yaml_float(value))
}

/// `^(?:[-+]?[0-9]+|0o[0-7]+|0x[0-9a-fA-F]+)$`.
fn is_yaml_int(value: &str) -> bool {
    let all =
        |text: &str, digit: fn(&u8) -> bool| !text.is_empty() && text.bytes().all(|b| digit(&b));
    if let Some(octal) = value.strip_prefix("0o") {
        return all(octal, |b| (b'0'..=b'7').contains(b));
    }
    if let Some(hex) = value.strip_prefix("0x") {
        return all(hex, u8::is_ascii_hexdigit);
    }
    all(
        value.strip_prefix(['+', '-']).unwrap_or(value),
        u8::is_ascii_digit,
    )
}

/// `^[-+]?(?:\.[0-9]+|[0-9]+(?:\.[0-9]*)?)(?:[eE][-+]?[0-9]+)?$`, `[-+]?\.(inf|Inf|INF)`, `\.(nan|NaN|NAN)`.
fn is_yaml_float(value: &str) -> bool {
    if matches!(value, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") {
        return true;
    }
    let body = unsigned.as_bytes();
    let digits = |from: usize| {
        let mut i = from;
        while i < body.len() && body[i].is_ascii_digit() {
            i += 1;
        }
        i
    };
    let mut i;
    if body.first() == Some(&b'.') {
        i = digits(1);
        if i == 1 {
            return false;
        }
    } else {
        i = digits(0);
        if i == 0 {
            return false;
        }
        if body.get(i) == Some(&b'.') {
            i = digits(i + 1);
        }
    }
    if matches!(body.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(body.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let start = i;
        i = digits(i);
        if i == start {
            return false;
        }
    }
    i == body.len()
}
