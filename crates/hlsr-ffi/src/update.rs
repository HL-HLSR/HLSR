//! Обновление и диагностика в приложении — то, что `AppManager.js` 0.4.7 делал с `UpdateManager`
//! (`:213-216`, `:356`, `:577`, `:846-987`, `:1044-1047`, `:1107`): служба обновления этой ОС за
//! фасадом [`UpdateApi`] с портами онлайна (уведомления `LAUNCHER_UPDATE_*`, события версий и
//! квитанция, телеметрия `launcher_update_*`), окно (снимки — события фасада, прогресс — кнопка
//! панели задач), выход и перезапуск приложения, диагностика с контекстом доставки, платформы,
//! онлайна и рендерера, выгрузка ZIP; после подтверждённого перехода — уборка следов Electron
//! 0.4.7–0.4.9 (Р12).

use crate::online::Online;
use hlsr_core::js;
use hlsr_core::notifications::NotificationStore;
use hlsr_core::paths::{Paths, Resources};
use hlsr_delivery::api::DeliveryApi;
use hlsr_online::api::OnlineApi;
use hlsr_online::notifications::version_events::{Receipt, VersionEvents};
use hlsr_online::telemetry::launcher_update_events::Transfer;
use hlsr_platform::api::PlatformApi;
use hlsr_update::api::system::{self, BuildInfo, ServiceLinks};
use hlsr_update::api::{DiagnosticsOptions, UpdateApi, UpdateApiOptions, UpdateService};
use hlsr_update::diagnostics::context::{
    BuildRuntime, ContextData, DiagnosticsContext, GameChannelError, GpuStatus, LaunchAvailability,
    ProfileSnapshot, SteamApp, SteamHealth, TelemetryInfo, TelemetryStatus,
};
use hlsr_update::diagnostics::export::ManifestInput;
use hlsr_update::diagnostics::runner::Check;
use hlsr_update::diagnostics::zip::{CrashInfo, SaveDialog, ZipData, ZipExporter};
use hlsr_update::legacy_traces::{LegacyDirs, SETUP_MARK, cleanup_on_launch};
use hlsr_update::linux::update_manager::Relauncher;
use hlsr_update::ports::{BoxFuture, Logger, SystemClock, TracingLogger};
use hlsr_update::update_manager::{
    AppControl, LauncherVersionEvents, UpdateNotifier, UpdateTelemetry, UpdateWindow,
};
use hlsr_update::updater::download::{FreeSpace, SystemFreeSpace};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime};

/// Сколько ждать выхода, отложенного службой (Linux: загрузка снимается перед выходом).
const DEFERRED_QUIT_WAIT: Duration = Duration::from_secs(30);

// ------------------------------------------------------------------ порты онлайна (ПП4)

/// `updateManager.setNotifications(this.notifications)`.
struct Notifications(Arc<NotificationStore>);

impl UpdateNotifier for Notifications {
    fn add(&self, level: &str, code: &str, detail: &str) -> Result<(), String> {
        self.0
            .add(level, code, Some(detail), None)
            .map(drop)
            .map_err(|error| error.to_string())
    }
}

/// `updateManager.setUpdateTelemetry(this.launcherUpdateEvents)`.
struct LauncherUpdates(Arc<OnlineApi>);

impl UpdateTelemetry for LauncherUpdates {
    fn available(&self, current: &str, next: Option<&str>) -> Result<(), String> {
        self.0.launcher_update_events().available(current, next);
        Ok(())
    }

    fn download_started(&self, current: &str, next: Option<&str>) -> Result<(), String> {
        self.0
            .launcher_update_events()
            .download_started(current, next);
        Ok(())
    }

    fn download_completed(
        &self,
        current: &str,
        next: &str,
        bytes: Option<f64>,
        duration_ms: Option<i64>,
    ) -> Result<(), String> {
        self.0.launcher_update_events().download_completed(
            current,
            Some(next),
            Some(Transfer {
                bytes,
                duration_ms: duration_ms.map(|ms| ms as f64),
            }),
        );
        Ok(())
    }

    fn failed(&self, current: &str, next: Option<&str>, code: Option<&str>) -> Result<(), String> {
        self.0.launcher_update_events().failed(current, next, code);
        Ok(())
    }
}

/// `updateManager.setVersionEvents(this.versionEvents)`.
struct VersionEventsLink(Arc<VersionEvents>);

impl LauncherVersionEvents for VersionEventsLink {
    fn launcher_update_available(
        &self,
        version: &str,
        current: &str,
        has_release_notes: bool,
    ) -> Result<(), String> {
        self.0
            .launcher_update_available(Some(version), Some(current), has_release_notes)
            .map(drop)
    }

    fn write_pending_receipt(&self, from: &str, to: &str) -> Result<(), String> {
        self.0.write_pending_receipt(Some(from), Some(to));
        Ok(())
    }
}

// ------------------------------------------------------------------ окно, выход, перезапуск

/// `app.quit()`: цикл событий окна заканчивается; после него — отметка для отложенного выхода.
#[derive(Default)]
pub struct AppQuit {
    requested: AtomicBool,
    notify: tokio::sync::Notify,
}

impl AppControl for AppQuit {
    fn quit(&self) {
        self.requested.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
        crate::host::quit();
    }
}

impl AppQuit {
    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.requested.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

/// `app.relaunch({execPath, args})` (Linux): запуск новой версии после выхода этой.
#[derive(Default)]
pub struct PendingRelaunch(Mutex<Option<(PathBuf, Vec<String>)>>);

impl Relauncher for PendingRelaunch {
    fn relaunch(&self, exec_path: &Path, args: &[String]) -> Result<(), String> {
        *self.0.lock().map_err(|error| error.to_string())? =
            Some((exec_path.to_owned(), args.to_vec()));
        Ok(())
    }
}

impl PendingRelaunch {
    /// Запрошен ли перезапуск.
    pub fn pending(&self) -> bool {
        self.0.lock().is_ok_and(|pending| pending.is_some())
    }

    /// Запустить новую версию (замок экземпляра уже отпущен): окружение без переменных окна и
    /// AppImage, как у дочерних процессов 0.4.7.
    pub fn spawn(&self) {
        let Some((exe, args)) = self.0.lock().ok().and_then(|mut pending| pending.take()) else {
            return;
        };
        let spawned = std::process::Command::new(&exe)
            .args(&args)
            .env_clear()
            .envs(hlsr_update::updater::processes::process_child_env())
            .stdin(std::process::Stdio::null())
            .spawn();
        match spawned {
            Ok(child) => tracing::info!(
                "[updater] перезапуск {} (pid {})",
                exe.display(),
                child.id()
            ),
            Err(error) => tracing::error!("[updater] перезапуск не удался: {error}"),
        }
    }
}

/// Окно ОС для службы: прогресс на кнопке панели задач (Windows); снимки окну лаунчера идут
/// событиями фасада.
struct TaskbarWindow;

impl UpdateWindow for TaskbarWindow {
    fn send_state(&self, _payload: Value) {}

    fn set_progress_bar(&self, value: f64) {
        #[cfg(windows)]
        {
            {
                let Some(hwnd) = crate::host::hwnd() else {
                    return;
                };
                if let Err(error) = hlsr_update::windows::taskbar::set_progress(hwnd, value) {
                    tracing::warn!("[updater] прогресс на панели задач: {error}");
                }
            }
        }
        #[cfg(not(windows))]
        let _ = value;
    }
}

// ------------------------------------------------------------------ диагностика

/// Рендерер для строк «Отрисовка интерфейса» (Р10).
#[derive(Debug, Clone)]
pub struct RendererInfo {
    /// Режим в `settings.json` при запуске.
    pub configured: String,
    /// С чем запущен процесс.
    pub started_with: String,
    /// Что работает.
    pub active: String,
}

/// Диалог сохранения отчёта (`dialog.showSaveDialog`, фильтр ZIP).
struct ZipDialog;

impl SaveDialog for ZipDialog {
    fn pick<'a>(&'a self, default_path: &'a Path) -> BoxFuture<'a, Option<PathBuf>> {
        let default_path = default_path.to_path_buf();
        Box::pin(async move {
            let owner = crate::host::dialog_owner().await;
            tokio::task::spawn_blocking(move || {
                crate::platform::file_dialog(owner, None, Some(&default_path), &[])
                    .add_filter("ZIP", &["zip"])
                    .save_file()
            })
            .await
            .ok()
            .flatten()
        })
    }
}

/// `app.getPath("documents")`: известная папка Windows; в Linux — `XDG_DOCUMENTS_DIR` из
/// `user-dirs.dirs`, иначе `~/Documents`.
fn documents_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(dir) = win::documents() {
        return dir;
    }
    let home = std::env::home_dir().unwrap_or_default();
    #[cfg(not(windows))]
    {
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        if let Some(dir) = hlsr_update::linux::xdg_user_dir(&home, &config_home, "DOCUMENTS") {
            return dir;
        }
    }
    home.join("Documents")
}

/// `os.release()`: версия ядра Linux или Windows (`10.0.<сборка>`).
fn os_release() -> String {
    #[cfg(windows)]
    {
        win::release()
    }
    #[cfg(not(windows))]
    {
        // То же, что `uname().release` у libuv.
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|text| text.trim().to_owned())
            .unwrap_or_default()
    }
}

/// `getFreeSpaceMb(target)`: свободные МБ тома ближайшего существующего каталога.
fn free_space_mb(target: &str) -> Option<f64> {
    let mut current = std::path::absolute(target).ok()?;
    while !current.exists() {
        current = current.parent()?.to_path_buf();
    }
    SystemFreeSpace
        .free_bytes(&current)
        .map(|bytes| (bytes / (1024 * 1024)) as f64)
}

/// Игра, чей запуск и чьи требования к Steam проверяет диагностика (`AppManager.js:870`).
const DIAGNOSTICS_GAME: &str = "ghosting-renewed";

/// Ответ `describeAvailability` (`{ok, code}`) → строка проверки запуска.
fn launch_availability_of(status: &Value) -> LaunchAvailability {
    LaunchAvailability {
        ok: js::truthy(&status["ok"]),
        code: status
            .get("code")
            .filter(|code| js::truthy(code))
            .map(|code| js::to_template_string(Some(code))),
    }
}

/// Ответ `steamOwnership` (`{appId, owned, code}`) → куплена ли игра; `owned: null` — не удалось
/// проверить.
fn ownership_of(answer: &Value) -> Option<bool> {
    answer["owned"].as_bool()
}

/// `diagnosticsProfile()` (`game-delivery/index.js:258`): числа те же, что в окне профиля.
/// `stats` — содержимое `game-stats.json` (`{games}`), `state` — снимок профиля (`profile-get`).
fn profile_snapshot(signed_in: bool, stats: &Value, state: &Value) -> ProfileSnapshot {
    let games = stats.get("games").filter(|games| games.is_object());
    let mut play_seconds = 0.0;
    let mut launch_count = 0.0;
    let mut games_played = 0.0;
    for entry in games
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|games| games.values())
    {
        let seconds = js::value::to_number(entry.get("totalPlaySeconds"));
        let launches = js::value::to_number(entry.get("launchCount"));
        if seconds.is_finite() && seconds > 0.0 {
            play_seconds += seconds;
            games_played += 1.0;
        }
        if launches.is_finite() && launches > 0.0 {
            launch_count += launches;
        }
    }
    let top_game_id = games
        .and_then(hlsr_online::profile::achievements::best_game)
        .map(|(game_id, _)| game_id)
        .unwrap_or_default();
    let achievements = state
        .get("summary")
        .filter(|summary| js::truthy(summary))
        .map(|summary| {
            (
                js::value::to_number(summary.get("unlocked")),
                js::value::to_number(summary.get("total")),
            )
        });
    ProfileSnapshot {
        signed_in,
        play_seconds: play_seconds.floor(),
        launch_count: Some(launch_count),
        games_played: Some(games_played),
        top_game_id: Some(top_game_id),
        achievements,
        installed_at: state
            .get("firstRunAt")
            .filter(|at| js::truthy(at))
            .map(|at| js::to_template_string(Some(at))),
    }
}

/// `ProtonManager.updateCandidate(release) || release.tag_name === RELEASE_TAG`: релиз из списка
/// GitHub подходит лаунчеру (строка сети «Proton Wineland»).
fn proton_release_ok(release: &Value) -> bool {
    hlsr_platform::linux::proton_manager::update_candidate(release).is_some()
        || release["tag_name"].as_str() == Some(hlsr_platform::linux::proton_manager::RELEASE_TAG)
}

/// Контекст диагностики (`ctx` в `AppManager.js:848-875`): доставка и платформа подключаются
/// позже ([`Update::bind_platform`]).
struct AppDiagnostics {
    paths: Paths,
    online: Arc<OnlineApi>,
    renderer: RendererInfo,
    update: OnceLock<Weak<UpdateApi>>,
    delivery: OnceLock<Weak<DeliveryApi>>,
    platform: OnceLock<Weak<PlatformApi>>,
}

impl AppDiagnostics {
    fn updates(&self) -> Option<Arc<UpdateApi>> {
        self.update.get()?.upgrade()
    }

    fn library_path(&self) -> Option<String> {
        Some(self.delivery.get()?.upgrade()?.current_library_path())
    }

    fn platform(&self) -> Option<Arc<PlatformApi>> {
        self.platform.get()?.upgrade()
    }

    /// `updateState`: состояние службы.
    fn update_state(&self) -> String {
        self.updates()
            .map_or("idle", |api| api.updates().state().as_str())
            .to_owned()
    }

    /// Данные отчёта ZIP (`AppManager.js:939-978`).
    fn zip_data(&self, checks: Vec<Check>) -> Result<ZipData, String> {
        let library = self.library_path().unwrap_or_default();
        let installed_file = hlsr_delivery::paths::resolve_library(&library)
            .ok()
            .map(|paths| paths.games.join("ghosting-renewed").join("installed.json"));
        let platform = self.platform();
        let crash_info = platform.as_ref().map(|platform| {
            let info = platform.game_crash_info(None);
            let text = |key: &str| {
                info.get(key)
                    .filter(|value| js::truthy(value))
                    .map(|value| js::to_template_string(Some(value)))
            };
            let dump_path = text("dumpPath");
            let dump_allowed = dump_path.as_deref().is_some_and(|dump| {
                hlsr_platform::crash_dumps::is_allowed_dump_path(
                    dump,
                    text("installPath").as_deref(),
                    &hlsr_delivery::environment::Environment::process(),
                )
            });
            CrashInfo {
                dump_path: dump_path.map(PathBuf::from),
                exit_code: info.get("exitCode").and_then(Value::as_i64),
                hint: text("hint"),
                dump_allowed,
            }
        });
        let stats_summary = platform
            .as_ref()
            .and_then(|platform| platform.game_stats_summary("ghosting-renewed"));
        let state = self.updates().map(|api| api.updates().get_state());
        let user_name = std::env::home_dir().and_then(|home| {
            home.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });
        Ok(ZipData {
            logs_dir: Some(self.paths.logs_dir.clone()),
            checks,
            stats_summary,
            notifications_file: Some(self.paths.file("notifications.json")),
            installed_file,
            crash_info,
            discord_rpc: hlsr_online::discord_rpc::diagnostics_model::normalize(
                &self.online.discord().diagnostics(),
            )
            .to_json(),
            context: ManifestInput {
                now: js::iso_string(SystemTime::now()),
                app_name: self.paths.app_name.to_owned(),
                app_version: env!("CARGO_PKG_VERSION").to_owned(),
                build_channel: option_env!("HLSR_BUILD_CHANNEL")
                    .unwrap_or_default()
                    .to_owned(),
                arch: hlsr_update::diagnostics::context::node_arch(std::env::consts::ARCH)
                    .to_owned(),
                platform: js::Platform::CURRENT.as_str().to_owned(),
                os_release: os_release(),
                library_exists: Path::new(&library).exists(),
                library_path: library,
                update_state: self.update_state(),
                update_error_code: state
                    .as_ref()
                    .and_then(|state| state["errorCode"].as_str())
                    .map(str::to_owned),
                env: std::env::vars().collect(),
                user_name,
                ..ManifestInput::default()
            },
        })
    }
}

impl DiagnosticsContext for AppDiagnostics {
    fn data(&self) -> ContextData {
        let library = self.library_path();
        let platform = self.platform();
        let steam_health = platform
            .as_ref()
            .map(|platform| platform.steam_health_check(true))
            .map(|health| {
                let text = |key: &str| health[key].as_str().map(str::to_owned);
                SteamHealth {
                    status: text("status").unwrap_or_default(),
                    code: text("code"),
                    detail: text("detail"),
                }
            });
        let updates = self.updates();
        ContextData {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            packaged: cfg!(not(debug_assertions)),
            os_release: os_release(),
            user_data_path: Some(self.paths.data_dir.clone()),
            update_state: self.update_state(),
            update_enabled: Some(updates.is_some_and(|api| api.updates().enabled())),
            free_space_mb: library.as_deref().and_then(free_space_mb),
            library_path: library,
            steam_health,
            // Без платформы (она подключается после старта) остаются значения по умолчанию.
            native_available: platform
                .as_ref()
                .is_none_or(|platform| platform.native_available()),
            umu_available: platform
                .as_ref()
                .is_some_and(|platform| platform.umu_available()),
            proton_selected: platform
                .as_ref()
                .and_then(|platform| platform.proton_selected_name()),
            ..ContextData::default()
        }
    }

    /// `downloadLimitBps: () => this.downloadSettings.rateLimiter.bytesPerSecond`.
    fn download_limit_bps(&self) -> Option<f64> {
        self.delivery.get()?.upgrade()?.download_limit_bps()
    }

    /// `launchAvailability`: без службы запуска — `LAUNCH_CONFIG_INVALID`, как у 0.4.7.
    fn launch_availability(&self) -> BoxFuture<'_, Option<LaunchAvailability>> {
        let platform = self.platform();
        Box::pin(async move {
            let status = match platform {
                // Поиск Steam и чтение файлов — не на потоке выполнения.
                Some(platform) => tokio::task::spawn_blocking(move || {
                    platform.game_launch_steam_status(&Value::from(DIAGNOSTICS_GAME))
                })
                .await
                .ok()?,
                None => json!({"ok": false, "code": "LAUNCH_CONFIG_INVALID"}),
            };
            Some(launch_availability_of(&status))
        })
    }

    /// `profile: () => this.gameDelivery.diagnosticsProfile()`: нет платформы (старт ещё идёт) — нет
    /// и снимка, строка получит `PROFILE_STATE_UNKNOWN`.
    fn profile(&self) -> Option<ProfileSnapshot> {
        let platform = self.platform()?;
        let signed_in = js::truthy(&platform.steam_account_state()["signedIn"]);
        // Нет файла или он не читается — как пустой `readAll()`: нули.
        let stats = std::fs::read_to_string(
            self.paths
                .file(hlsr_platform::game_stats::game_stats_store::FILE_NAME),
        )
        .ok()
        .and_then(|text| hlsr_core::json_text::json_parse(&text).ok())
        .unwrap_or(Value::Null);
        Some(profile_snapshot(
            signed_in,
            &stats,
            &self.online.profiles().snapshot(),
        ))
    }

    /// `requiredSteamApps: () => this.gameDelivery.requiredSteamApps()`.
    fn required_steam_apps(&self) -> Vec<SteamApp> {
        hlsr_delivery::manifests::required_owned_apps()
            .into_iter()
            .map(|app| SteamApp {
                app_id: u64::from(app.app_id),
                title: app.title,
            })
            .collect()
    }

    /// `steamOwnership: (appId, options) => this.gameDelivery.steamOwnership(appId, options)`.
    fn steam_ownership(&self, app_id: u64, timeout: Duration) -> BoxFuture<'_, Option<bool>> {
        let platform = self.platform();
        Box::pin(async move {
            let options =
                json!({"timeoutMs": u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)});
            let answer = platform?
                .steam_ownership(&json!(app_id), Some(&options))
                .await;
            ownership_of(&answer)
        })
    }

    fn proton_release_ok(&self, release: &Value) -> bool {
        proton_release_ok(release)
    }

    fn gpu_status(&self) -> BoxFuture<'_, Option<GpuStatus>> {
        let status = GpuStatus::from_renderer(
            &self.renderer.configured,
            &self.renderer.started_with,
            Some(&self.renderer.active),
            "",
            "",
        );
        Box::pin(async move { Some(status) })
    }

    /// `buildMetadataTrust.verifyAgainstSnapshot({installRoot, snapshot, launcherVersion})` (ПП2).
    fn build_runtime(&self, installed: &Value, launcher_version: &str) -> BuildRuntime {
        use hlsr_delivery::build_metadata_trust::{VerifyAgainstSnapshot, verify_against_snapshot};
        let Some(root) = installed["installPath"].as_str() else {
            return BuildRuntime {
                state: "repair-required".to_owned(),
                reason: Some("BUILD_METADATA_INVALID".to_owned()),
            };
        };
        let verdict = verify_against_snapshot(&VerifyAgainstSnapshot {
            install_root: Path::new(root),
            snapshot: Some(installed),
            launcher_version: Some(launcher_version),
            expected: None,
            now: None,
        });
        BuildRuntime {
            state: verdict.state.as_str().to_owned(),
            reason: verdict.reason.map(|reason| reason.as_str().to_owned()),
        }
    }

    /// `buildMetadataReader.read(installPath).sha256` (ПП2).
    fn build_metadata_sha256(&self, install_path: &str) -> Result<Option<String>, String> {
        use hlsr_delivery::build_metadata_reader::{ReadOptions, Reading, read};
        match read(Path::new(install_path), &ReadOptions::default()) {
            Ok(Reading::Present(found)) => Ok(Some(found.sha256)),
            Ok(Reading::Absent) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    /// `channelService.validate(data, "ghosting-renewed", configFor("ghosting-renewed"))` (ПП2).
    fn validate_game_channel(&self, data: &Value) -> Result<(), GameChannelError> {
        use hlsr_delivery::channel_service::{ChannelOptions, ChannelService};
        const GAME: &str = "ghosting-renewed";
        let service = ChannelService::new(ChannelOptions::default());
        let Some(config) = service.config_for(GAME) else {
            return Err(GameChannelError { code: None });
        };
        service
            .validate(data, GAME, &config)
            .map(drop)
            .map_err(|error| GameChannelError {
                code: Some(error.code.as_str().to_owned()),
            })
    }

    fn telemetry(&self) -> TelemetryInfo {
        let snapshot = self.online.telemetry().snapshot();
        TelemetryInfo::Status(Some(TelemetryStatus {
            backend_configured: snapshot["backendConfigured"] == Value::Bool(true),
            enabled: snapshot["enabled"] == Value::Bool(true),
        }))
    }
}

// ------------------------------------------------------------------ сборка

/// Обновление запущенного лаунчера.
#[derive(Clone)]
pub struct Update {
    api: Arc<UpdateApi>,
    quit: Arc<AppQuit>,
    relaunch: Arc<PendingRelaunch>,
    diagnostics: Arc<AppDiagnostics>,
}

/// Окружение дочерних процессов (установщик, апдейтер, рантайм AppImage).
fn child_env() -> Vec<(String, String)> {
    hlsr_update::updater::processes::process_child_env()
}

/// Служба обновления этой ОС.
fn service(
    paths: &Paths,
    links: ServiceLinks,
    relaunch: Arc<PendingRelaunch>,
) -> Arc<dyn UpdateService> {
    let build = BuildInfo::current(env!("CARGO_PKG_VERSION"));
    let exe = std::env::current_exe().unwrap_or_default();
    if cfg!(windows) {
        let _ = relaunch;
        let env = child_env();
        let (temp_dir, download_dir) = windows_dirs(paths, &crate::modes::mode_env());
        Arc::new(system::windows_service(system::WindowsServiceOptions {
            build,
            resources: Resources::detect().dir().to_path_buf(),
            exe,
            user_data_dir: paths.data_dir.clone(),
            temp_dir,
            download_dir,
            registry: crate::modes::registry(),
            launcher: crate::modes::launcher(),
            env,
            links,
        }))
    } else {
        Arc::new(system::linux_service(system::LinuxServiceOptions {
            build,
            exe,
            argv: std::env::args().collect(),
            env: child_env(),
            relauncher: relaunch,
            links,
        }))
    }
}

/// Кеш electron-updater в `%LOCALAPPDATA%`: `<имя>-updater`. `updaterCacheDirName` из
/// `app-update.yml` electron-builder выводил из `name` упакованного `package.json`
/// (`appInfo.js`): `hlsr-updater`, у QA-сборки 0.4.7 — `hlsr-qa-updater`.
fn updater_cache(local_app_data: &Path, paths: &Paths) -> PathBuf {
    local_app_data.join(format!("{}-updater", paths.app_name))
}

/// Папки службы Windows: `os.tmpdir()` (копия `HLSRUpdater.exe`) и
/// `%LOCALAPPDATA%\<имя>-updater\pending` (установщик пути А, как electron-updater,
/// [`updater_cache`]). Окружение — как у режимов ([`crate::modes::mode_env`],
/// [`crate::modes::temp_dir`]): под `dev-hooks` папки пользователя перенаправлены
/// (`HLSR_KNOWN_FOLDERS`, `HLSR_TEMP_DIR`), выпуск берёт окружение процесса.
fn windows_dirs(paths: &Paths, env: &hlsr_update::windows::processes::Env) -> (PathBuf, PathBuf) {
    let temp_dir = crate::modes::temp_dir(&env.lookup());
    let local_app_data = env
        .get("LOCALAPPDATA")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| temp_dir.clone());
    let download_dir = updater_cache(&local_app_data, paths).join("pending");
    (temp_dir, download_dir)
}

/// Создать службу обновления и диагностику. Вызывать в среде tokio, после онлайна и до доставки
/// (порядок конструктора `AppManager`).
pub fn start_update(paths: &Paths, online: &Online, renderer: RendererInfo) -> Update {
    let logger: Arc<dyn Logger> = Arc::new(TracingLogger);
    let online_api = online.api().clone();
    let notifier: Arc<dyn UpdateNotifier> =
        Arc::new(Notifications(online_api.notifications().clone()));
    let quit = Arc::new(AppQuit::default());
    let relaunch = Arc::new(PendingRelaunch::default());
    let links = ServiceLinks {
        app: quit.clone(),
        logger: logger.clone(),
        notifier: Some(notifier.clone()),
        telemetry: Some(Arc::new(LauncherUpdates(online_api.clone()))),
        version_events: Some(Arc::new(VersionEventsLink(
            online_api.version_events().clone(),
        ))),
    };
    let updates = service(paths, links, relaunch.clone());

    let diagnostics = Arc::new(AppDiagnostics {
        paths: paths.clone(),
        online: online_api,
        renderer,
        update: OnceLock::new(),
        delivery: OnceLock::new(),
        platform: OnceLock::new(),
    });
    let zip_data = {
        let diagnostics = Arc::downgrade(&diagnostics);
        Arc::new(move |checks: Vec<Check>| {
            diagnostics
                .upgrade()
                .ok_or_else(|| "лаунчер закрывается".to_owned())?
                .zip_data(checks)
        })
    };
    let lookup = |name: &str| std::env::var(name).ok();
    let api = UpdateApi::new(UpdateApiOptions {
        updates,
        window: Some(Arc::new(TaskbarWindow)),
        notifier: Some(notifier),
        diagnostics: Some(DiagnosticsOptions {
            runner: system::diagnostics_runner(diagnostics.clone()),
            exporter: ZipExporter {
                dialog: Arc::new(ZipDialog),
                logger: logger.clone(),
                clock: Arc::new(SystemClock),
                sevenzip: Resources::detect().sevenzip(),
                temp_dir: crate::modes::temp_dir(&lookup),
                documents_dir: documents_dir(),
            },
            zip_data,
        }),
        logger,
    });
    let _ = diagnostics.update.set(Arc::downgrade(&api));
    Update {
        api,
        quit,
        relaunch,
        diagnostics,
    }
}

impl Update {
    /// Фасад (каналы окна).
    pub fn api(&self) -> &Arc<UpdateApi> {
        &self.api
    }

    /// Доставка и платформа созданы: проверка «идёт ли игра» и контекст диагностики.
    pub fn bind_platform(&self, delivery: &Arc<DeliveryApi>, platform: &Arc<PlatformApi>) {
        let weak = Arc::downgrade(platform);
        self.api.set_game_running_check(Arc::new(move || {
            weak.upgrade()
                .is_some_and(|platform| platform.is_any_game_running())
        }));
        let _ = self.diagnostics.delivery.set(Arc::downgrade(delivery));
        let _ = self.diagnostics.platform.set(Arc::downgrade(platform));
    }

    /// Окно загрузилось (`ready`, после квитанции онлайна): фоновая проверка; если квитанция
    /// подтвердила установку этой версии или установщик поставил её поверх 0.4.x (метка
    /// [`SETUP_MARK`]) — уборка следов Electron в фоне.
    pub fn window_loaded(&self, receipt: Option<Receipt>) {
        self.api.start_checking_interval();
        let confirmed = receipt.is_some_and(|receipt| receipt.to == env!("CARGO_PKG_VERSION"));
        let mark = self.diagnostics.paths.file(SETUP_MARK);
        if let Some(dirs) = legacy_dirs(&self.diagnostics.paths) {
            tokio::task::spawn_blocking(move || {
                cleanup_on_launch(&dirs, confirmed, &mark, &TracingLogger);
            });
        }
    }

    /// `before-quit` службы (Linux: снимает загрузку и откладывает выход, затем ставит обновление
    /// при выходе; Windows — ничего). В 0.4.7 это раньше `dispose()`, который на Linux шёл на
    /// `will-quit` (`index.js:194-198`).
    pub fn before_quit(&self, runtime: &tokio::runtime::Runtime) {
        if self.api.before_quit() {
            tracing::info!("[updater] выход отложен: служба обновления заканчивает работу");
            let _ = runtime.block_on(tokio::time::timeout(DEFERRED_QUIT_WAIT, self.quit.wait()));
            // Второй `before-quit` уже ничего не откладывает: установка при выходе.
            let _ = self.api.before_quit();
        }
    }

    /// `updateManager.dispose()`: таймеры проверок.
    pub fn dispose(&self) {
        self.api.dispose();
    }

    /// `quit` с кодом выхода (Windows, electron-updater): установка при выходе — после `dispose()`
    /// и только при коде 0.
    pub fn install_on_quit(&self, exit_code: i32) {
        self.api.on_app_quit(exit_code);
    }

    /// Нужен ли перезапуск новой версии (Linux, `app.relaunch` из `before-quit`).
    pub fn relaunch_pending(&self) -> bool {
        self.relaunch.pending()
    }

    /// Запустить новую версию после выхода (Linux, `app.relaunch`).
    pub fn spawn_relaunch(&self) {
        self.relaunch.spawn();
    }
}

/// Где лежат следы Electron (0.4.7–0.4.9) этой ОС. Отладочная сборка и `dev-hooks` с
/// перенаправленным каталогом данных, но без перенаправленных папок пользователя
/// (`HLSR_KNOWN_FOLDERS`), в Windows ничего не
/// убирают: квитанция из временного профиля не должна чистить настоящий `%LOCALAPPDATA%`.
fn legacy_dirs(paths: &Paths) -> Option<LegacyDirs> {
    if !cfg!(windows) {
        return Some(LegacyDirs::linux(paths.data_dir.clone()));
    }
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if cfg!(any(debug_assertions, feature = "dev-hooks"))
        && set("HLSR_DATA_DIR")
        && !set("HLSR_KNOWN_FOLDERS")
    {
        tracing::info!(
            "[update] следы Electron не убираются: каталог данных перенаправлен, папки пользователя — нет"
        );
        return None;
    }
    let env = crate::modes::mode_env();
    let lookup = env.lookup();
    let env_dir = |name: &str| {
        env.get(name)
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
    };
    Some(LegacyDirs {
        chromium_profile: Some(paths.data_dir.clone()),
        ..LegacyDirs::windows(
            crate::modes::temp_dir(&lookup),
            env_dir("LOCALAPPDATA").map(|local| updater_cache(&local, paths)),
            env_dir("APPDATA"),
        )
    })
}

/// Квитанция на эту версию в `version-state.json` (первый запуск после перехода): чтение без
/// побочных действий, до окна и до онлайна. Битый файл — квитанции нет.
pub fn fresh_receipt(paths: &Paths) -> bool {
    let Ok(text) = std::fs::read_to_string(paths.file("version-state.json")) else {
        return false;
    };
    let Ok(state) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    state["receipt"]["to"].as_str() == Some(env!("CARGO_PKG_VERSION"))
}

/// Папки и версия Windows.
#[cfg(windows)]
mod win {
    use std::path::PathBuf;

    /// `app.getPath("documents")`: известная папка оболочки (бывает в OneDrive).
    pub fn documents() -> Option<PathBuf> {
        use std::os::windows::ffi::OsStringExt as _;
        use windows_sys::Win32::System::Com::CoTaskMemFree;
        use windows_sys::Win32::UI::Shell::{
            FOLDERID_Documents, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
        };

        let mut raw: *mut u16 = std::ptr::null_mut();
        // SAFETY: `FOLDERID_Documents` — константа; без токена — текущий пользователь; строка
        // результата выделена COM и освобождается ниже ровно один раз.
        let result = unsafe {
            SHGetKnownFolderPath(
                &FOLDERID_Documents,
                KF_FLAG_DEFAULT as u32,
                std::ptr::null_mut(),
                &mut raw,
            )
        };
        let dir = (result >= 0 && !raw.is_null()).then(|| {
            // SAFETY: `raw` указывает на строку UTF-16 с нулём на конце.
            let length = (0..).take_while(|&at| unsafe { *raw.add(at) } != 0).count();
            // SAFETY: `length` единиц UTF-16 прочитаны выше.
            let wide = unsafe { std::slice::from_raw_parts(raw, length) };
            PathBuf::from(std::ffi::OsString::from_wide(wide))
        });
        // SAFETY: память выделена `SHGetKnownFolderPath`; `CoTaskMemFree(NULL)` допустим.
        unsafe { CoTaskMemFree(raw.cast()) };
        dir
    }

    /// `os.release()` в Windows: `<major>.<minor>.<сборка>` (`RtlGetVersion`, как libuv).
    pub fn release() -> String {
        use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
        use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
        // SAFETY: структура обнулена, размер задан; RtlGetVersion только заполняет её.
        let info = unsafe {
            let mut info: OSVERSIONINFOW = std::mem::zeroed();
            info.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
            (RtlGetVersion(&mut info) == 0).then_some(info)
        };
        info.map(|info| {
            format!(
                "{}.{}.{}",
                info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
            )
        })
        .unwrap_or_default()
    }
}

