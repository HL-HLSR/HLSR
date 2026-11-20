//! Сессия обновления (`updater/core/session.js`): стадии `download → verify → wait-exit → install →
//! relaunch → done` (или `error`, `cancelled`). Если в канале есть годный блок `delta`, версия
//! сначала собирается из чанков в `work\tree` (совпавшее берётся из установленной, недостающие
//! куски качаются) и ставится своим же `tree\HLSR.exe`; при любом сбое — полный установщик.
//! Отменить можно только на стадии загрузки.
//!
//! Внешний мир — трейт [`SessionDeps`] (зависимости `deps` 0.4.7): тесты подставляют свои, в
//! приложении — [`SystemDeps`]. События — обратный вызов [`SessionEvent`] (`emit` 0.4.7).

use super::download::{self, DownloadOptions, DownloadProgress, FreeSpace};
use super::install::{self, InstallOptions, Installed};
use super::plan::Plan;
use super::process_wait::{self, ProcessList, WaitInfo, WaitOptions};
use super::processes::ProcessLauncher;
use super::receipt::ReceiptStore;
use crate::cancel::CancelToken;
use crate::channel::{self, VerifyOptions, VerifyOutcome};
use crate::chunks::{self, AssembleOptions, Assembled, ChunkError, ChunkFetch, Delta, Manifest};
use crate::errors::{UpdCode, UpdaterCode, UpdaterError};
use crate::ports::{BoxFuture, Clock, Logger};
use hlsr_core::js::Platform;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// `STAGES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Download,
    Verify,
    WaitExit,
    Install,
    Relaunch,
    Done,
    Error,
    Cancelled,
}

impl Stage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Stage::Download => "download",
            Stage::Verify => "verify",
            Stage::WaitExit => "wait-exit",
            Stage::Install => "install",
            Stage::Relaunch => "relaunch",
            Stage::Done => "done",
            Stage::Error => "error",
            Stage::Cancelled => "cancelled",
        }
    }
}

/// `CANCELLABLE_STAGES`.
pub const CANCELLABLE_STAGES: [Stage; 1] = [Stage::Download];

/// Событие сессии (`emit`): `stage` и `progress` уходят в окно апдейтера.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// `{stage, cancellable, …}`.
    Stage(Value),
    /// `{done, total, bytesPerSecond}`.
    Progress(Value),
    /// `{pids, remainingMs}`.
    Waiting(Value),
    Cancelled,
    /// `{code, message}`.
    Failed(Value),
    /// `{version}`.
    Done(Value),
}

/// Итог `run()`.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionResult {
    Done {
        version: String,
        receipt_written: bool,
    },
    Cancelled,
    Failed(UpdaterError),
}

impl SessionResult {
    /// Форма 0.4.7: `{ok, version, receiptWritten}`, `{ok:false, cancelled:true}`,
    /// `{ok:false, error:{code, message}}`.
    pub fn to_json(&self) -> Value {
        match self {
            SessionResult::Done {
                version,
                receipt_written,
            } => json!({"ok": true, "version": version, "receiptWritten": receipt_written}),
            SessionResult::Cancelled => json!({"ok": false, "cancelled": true}),
            SessionResult::Failed(error) => json!({"ok": false, "error": error.to_public()}),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, SessionResult::Done { .. })
    }
}

/// Что собрать из чанков (`options` `assemble` в сессии).
#[derive(Clone)]
pub struct AssembleRequest {
    pub manifest: Manifest,
    /// Установленная версия (`plan.installDir`).
    pub source_root: String,
    pub target_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub chunk_base_urls: Vec<String>,
    pub cancel: CancelToken,
}

/// Что скачать (`options` `download` в сессии).
#[derive(Clone)]
pub struct DownloadRequest {
    pub urls: Vec<String>,
    pub dir: PathBuf,
    pub filename: String,
    pub size: u64,
    /// `isCancelled`: отмена сессии.
    pub cancel: CancelToken,
}

/// Зависимости сессии (`deps` 0.4.7).
pub trait SessionDeps: Send + Sync {
    /// `fetchManifest(delta, {version, platform: "win32", signal})`.
    fn fetch_manifest<'a>(
        &'a self,
        delta: &'a Delta,
        version: &'a str,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Manifest, ChunkError>>;
    /// `assemble({...})` с прогрессом.
    fn assemble<'a>(
        &'a self,
        request: AssembleRequest,
        on_progress: chunks::OnProgress,
    ) -> BoxFuture<'a, Result<Assembled, ChunkError>>;
    /// `download({...})` с прогрессом; путь к установщику.
    fn download<'a>(
        &'a self,
        request: DownloadRequest,
        on_progress: download::OnProgress,
    ) -> BoxFuture<'a, Result<PathBuf, UpdaterError>>;
    /// `verifyInstaller(path, {filename, size, sha256, sha512})`.
    fn verify_installer<'a>(
        &'a self,
        path: &'a Path,
        installer: &'a Value,
    ) -> BoxFuture<'a, VerifyOutcome>;
    /// `waitForExit({imageName: "HLSR.exe", onWait})`.
    fn wait_for_exit<'a>(
        &'a self,
        on_wait: &'a (dyn Fn(&WaitInfo) + Send + Sync),
    ) -> BoxFuture<'a, Result<(), UpdaterError>>;
    /// `runInstaller(installer, options)`.
    fn run_installer<'a>(
        &'a self,
        installer: &'a Path,
        options: InstallOptions,
    ) -> BoxFuture<'a, Result<(), UpdaterError>>;
    /// `verifyInstalled({launcherExe, targetVersion})`.
    fn verify_installed(&self, launcher_exe: &str, target_version: &str) -> Installed;
    /// `writeReceipt(receiptFile, currentVersion, targetVersion)`.
    fn write_receipt(&self, file: &str, from: Option<&str>, to: &str) -> bool;
    /// `relaunch(launcherExe, {})`.
    fn relaunch(&self, launcher_exe: &str) -> Result<(), UpdaterError>;
    /// Окружение установщика (окружение апдейтера без переменных окна и распаковщика).
    fn installer_env(&self) -> Vec<(String, String)>;
}

type EventSink = Arc<dyn Fn(SessionEvent) + Send + Sync>;

#[derive(Default)]
struct State {
    stage: Option<Stage>,
    installer_path: Option<PathBuf>,
    abort: Option<CancelToken>,
}

/// `UpdateSession`.
pub struct UpdateSession {
    plan: Plan,
    work_dir: PathBuf,
    logger: Arc<dyn Logger>,
    deps: Arc<dyn SessionDeps>,
    on_event: EventSink,
    state: Mutex<State>,
    cancel_requested: AtomicBool,
    finished: AtomicBool,
    /// Отмена загрузки установщика: будит её сразу, а не на следующем куске ответа.
    download_cancel: CancelToken,
}

/// Отмена во время загрузки: `UPD_CANCELLED`.
fn cancelled() -> UpdaterError {
    UpdaterError::new(UpdCode::Cancelled, "отменено")
}

impl UpdateSession {
    pub fn new(
        plan: Plan,
        work_dir: PathBuf,
        logger: Arc<dyn Logger>,
        deps: Arc<dyn SessionDeps>,
        on_event: impl Fn(SessionEvent) + Send + Sync + 'static,
    ) -> UpdateSession {
        UpdateSession {
            plan,
            work_dir,
            logger,
            deps,
            on_event: Arc::new(on_event),
            state: Mutex::new(State::default()),
            cancel_requested: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            download_cancel: CancelToken::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// Текущая стадия.
    pub fn stage(&self) -> Option<Stage> {
        self.lock().stage
    }

    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// `isCancellable()`.
    pub fn is_cancellable(&self) -> bool {
        self.lock()
            .stage
            .is_some_and(|stage| CANCELLABLE_STAGES.contains(&stage))
            && !self.finished()
    }

    fn set_stage(&self, stage: Stage, extra: Option<Map<String, Value>>) {
        self.lock().stage = Some(stage);
        let mut payload = Map::new();
        payload.insert("stage".into(), json!(stage.as_str()));
        payload.insert("cancellable".into(), json!(self.is_cancellable()));
        for (key, value) in extra.into_iter().flatten() {
            payload.insert(key, value);
        }
        (self.on_event)(SessionEvent::Stage(Value::Object(payload)));
    }

    /// `cancel()`: только на стадии загрузки; `true` — отмена принята.
    pub fn cancel(&self) -> bool {
        if !self.is_cancellable() {
            return false;
        }
        self.cancel_requested.store(true, Ordering::SeqCst);
        self.download_cancel.cancel();
        if let Some(abort) = &self.lock().abort {
            abort.cancel();
        }
        self.logger
            .info("[updater] запрошена отмена на стадии загрузки");
        true
    }

    fn cancel_requested(&self) -> bool {
        self.cancel_requested.load(Ordering::SeqCst)
    }

    /// `run()`.
    pub async fn run(&self) -> SessionResult {
        let result = self.execute().await;
        self.finished.store(true, Ordering::SeqCst);
        match result {
            Ok((version, receipt_written)) => SessionResult::Done {
                version,
                receipt_written,
            },
            Err(error) if error.code == UpdCode::Cancelled => {
                self.set_stage(Stage::Cancelled, None);
                (self.on_event)(SessionEvent::Cancelled);
                SessionResult::Cancelled
            }
            Err(error) => {
                self.logger
                    .error(&format!("[updater] {}: {}", error.code, error.message));
                let public = error.to_public();
                let mut extra = Map::new();
                extra.insert("error".into(), public.clone());
                self.set_stage(Stage::Error, Some(extra));
                (self.on_event)(SessionEvent::Failed(public));
                SessionResult::Failed(error)
            }
        }
    }

    fn progress_extra(total: u64) -> Map<String, Value> {
        let mut extra = Map::new();
        extra.insert("done".into(), json!(0));
        extra.insert("total".into(), json!(total));
        extra.insert("bytesPerSecond".into(), json!(0));
        extra
    }

    async fn execute(&self) -> Result<(String, bool), UpdaterError> {
        let plan = &self.plan;
        if let Some(delta) = channel::delta_of(&plan.manifest)
            .and_then(|delta| Delta::from_value(delta, channel::is_allowed_url).ok())
            && let Some(tree) = self.prepare_tree(&delta).await?
        {
            let options = install::tree_setup_options(
                &tree,
                &plan.install_dir,
                &plan.target_version,
                self.deps.installer_env(),
            );
            match self.install_from(&tree.join("HLSR.exe"), options).await {
                Ok(done) => return Ok(done),
                Err(error) if error.code == UpdCode::Cancelled => return Err(error),
                Err(error) => self.logger.warn(&format!(
                    "[updater] установка собранной версии не удалась, качаю установщик: {}",
                    error.message
                )),
            }
        }

        let size = plan.installer.size.as_u64().unwrap_or(0);
        self.set_stage(Stage::Download, Some(Self::progress_extra(size)));

        let events = self.on_event.clone();
        let installer_path = self
            .deps
            .download(
                DownloadRequest {
                    urls: plan.installer.urls.clone(),
                    dir: self.work_dir.clone(),
                    filename: plan.installer.filename.clone(),
                    size,
                    cancel: self.download_cancel.clone(),
                },
                Arc::new(move |progress: DownloadProgress| {
                    events(SessionEvent::Progress(progress.to_json()));
                }),
            )
            .await?;
        self.lock().installer_path = Some(installer_path.clone());

        if self.cancel_requested() {
            return Err(cancelled());
        }
        self.logger.info(&format!(
            "[updater] установщик загружен: {}",
            plan.installer.filename
        ));

        self.set_stage(Stage::Verify, None);
        let installer = json!({
            "filename": plan.installer.filename,
            "size": plan.installer.size,
            "sha256": plan.installer.sha256,
            "sha512": plan.installer.sha512,
        });
        let verdict = self
            .deps
            .verify_installer(&installer_path, &installer)
            .await;
        if !verdict.ok {
            self.discard_installer();
            let code: UpdaterCode = verdict
                .code
                .map_or(UpdCode::SignatureInvalid.into(), UpdaterCode::from);
            let joined = verdict.problems.join("; ");
            let message = if joined.is_empty() {
                "установщик не прошёл проверку".to_owned()
            } else {
                joined
            };
            return Err(UpdaterError::new(code, message));
        }
        self.logger
            .info("[updater] установщик подтверждён подписанными метаданными");

        self.install_from(
            &installer_path,
            InstallOptions::installer(&plan.install_dir, self.deps.installer_env()),
        )
        .await
    }

    /// `prepareTree(delta)`: путь к собранной версии или `None` — нужен полный установщик.
    async fn prepare_tree(&self, delta: &Delta) -> Result<Option<PathBuf>, UpdaterError> {
        let plan = &self.plan;
        let tree = self.work_dir.join("tree");
        let abort = CancelToken::new();
        self.lock().abort = Some(abort.clone());
        let result = async {
            self.set_stage(Stage::Download, Some(Self::progress_extra(0)));
            remove_tree(&tree)?;
            let manifest = self
                .deps
                .fetch_manifest(delta, &plan.target_version, &abort)
                .await?;
            std::fs::create_dir_all(&tree)?;
            let events = self.on_event.clone();
            let assembled = self
                .deps
                .assemble(
                    AssembleRequest {
                        manifest,
                        source_root: plan.install_dir.clone(),
                        target_dir: tree.clone(),
                        cache_dir: self.work_dir.join("chunks"),
                        chunk_base_urls: delta.chunk_base_urls.clone(),
                        cancel: abort.clone(),
                    },
                    Arc::new(move |progress: chunks::Progress| {
                        events(SessionEvent::Progress(json!({
                            "done": progress.done,
                            "total": progress.total,
                            "bytesPerSecond": progress.bytes_per_second,
                        })));
                    }),
                )
                .await?;
            Ok::<Assembled, ChunkError>(assembled)
        }
        .await;
        self.lock().abort = None;

        match result {
            Ok(assembled) => {
                if self.cancel_requested() {
                    return Err(cancelled());
                }
                self.logger.info(&format!(
                    "[updater] {} собрана из чанков: скачано {} байт, с диска {}",
                    plan.target_version, assembled.downloaded_bytes, assembled.reused_bytes
                ));
                Ok(Some(tree))
            }
            Err(error) => {
                if self.cancel_requested() {
                    return Err(cancelled());
                }
                self.logger.warn(&format!(
                    "[updater] чанки не сработали, качаю установщик целиком: {error}"
                ));
                let _ = remove_tree(&tree);
                Ok(None)
            }
        }
    }

    /// `installFrom(installer, installOptions)`.
    async fn install_from(
        &self,
        installer: &Path,
        options: InstallOptions,
    ) -> Result<(String, bool), UpdaterError> {
        let plan = &self.plan;
        self.set_stage(Stage::WaitExit, None);
        let events = self.on_event.clone();
        self.deps
            .wait_for_exit(&move |info: &WaitInfo| events(SessionEvent::Waiting(info.to_json())))
            .await?;
        self.logger.info("[updater] все процессы HLSR завершены");

        self.set_stage(Stage::Install, None);
        self.deps.run_installer(installer, options).await?;

        let applied = self
            .deps
            .verify_installed(&plan.launcher_exe, &plan.target_version);
        if !applied.ok {
            return Err(UpdaterError::new(
                applied.code.unwrap_or(UpdCode::InstallNotApplied),
                applied
                    .detail
                    .filter(|detail| !detail.is_empty())
                    .unwrap_or_else(|| "установка не применилась".to_owned()),
            ));
        }
        self.logger.info(&format!(
            "[updater] установлена версия {}",
            plan.target_version
        ));

        let receipt_written = self.deps.write_receipt(
            &plan.receipt_file,
            plan.current_version_str(),
            &plan.target_version,
        );
        if !receipt_written {
            self.logger
                .warn("[updater] receipt не записан: уведомление не появится");
        }

        self.set_stage(Stage::Relaunch, None);
        self.deps.relaunch(&plan.launcher_exe)?;

        let mut extra = Map::new();
        extra.insert("version".into(), json!(plan.target_version));
        self.set_stage(Stage::Done, Some(extra));
        (self.on_event)(SessionEvent::Done(json!({"version": plan.target_version})));
        Ok((plan.target_version.clone(), receipt_written))
    }

    /// `discardInstaller()`.
    fn discard_installer(&self) {
        if let Some(path) = &self.lock().installer_path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `fs.rmSync(dir, {recursive: true, force: true})`.
fn remove_tree(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

// ------------------------------------------------------------------- зависимости в приложении

/// Зависимости сессии в приложении: чанки по HTTPS, загрузка, проверка файла, ожидание выхода
/// лаунчера (Toolhelp32), установщик и запуск лаунчера ([`ProcessLauncher`]), квитанция
/// ([`ReceiptStore`]).
pub struct SystemDeps {
    pub fetch: Arc<dyn ChunkFetch>,
    pub launcher: Arc<dyn ProcessLauncher>,
    pub processes: Arc<dyn ProcessList>,
    pub receipts: Arc<dyn ReceiptStore>,
    pub free_space: Arc<dyn FreeSpace>,
    pub clock: Arc<dyn Clock>,
    pub logger: Arc<dyn Logger>,
    /// Окружение установщика и лаунчера.
    pub env: Vec<(String, String)>,
}

impl SessionDeps for SystemDeps {
    fn fetch_manifest<'a>(
        &'a self,
        delta: &'a Delta,
        version: &'a str,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Manifest, ChunkError>> {
        Box::pin(async move {
            chunks::fetch_manifest(
                delta,
                &chunks::FetchManifestOptions {
                    fetch: self.fetch.as_ref(),
                    is_allowed: chunks::production_allow(),
                    expected: chunks::Expected {
                        version: Some(version),
                        platform: Some(Platform::Win32.as_str()),
                    },
                    cancel: Some(cancel.clone()),
                },
            )
            .await
        })
    }

    fn assemble<'a>(
        &'a self,
        request: AssembleRequest,
        on_progress: chunks::OnProgress,
    ) -> BoxFuture<'a, Result<Assembled, ChunkError>> {
        Box::pin(async move {
            let mut options = AssembleOptions::new(
                request.manifest,
                request.target_dir,
                request.cache_dir,
                request.chunk_base_urls,
                self.fetch.clone(),
                chunks::production_allow(),
            );
            options.source_root = Some(PathBuf::from(request.source_root));
            options.cancel = Some(request.cancel);
            options.on_progress = Some(on_progress);
            options.log = Some(self.logger.clone());
            options.clock = self.clock.clone();
            chunks::assemble(options).await
        })
    }

    fn download<'a>(
        &'a self,
        request: DownloadRequest,
        on_progress: download::OnProgress,
    ) -> BoxFuture<'a, Result<PathBuf, UpdaterError>> {
        Box::pin(async move {
            let mut options = DownloadOptions::new(
                request.urls,
                request.dir,
                request.filename,
                request.size,
                Arc::new(channel::is_allowed_url),
            );
            options.cancel = request.cancel;
            options.on_progress = Some(on_progress);
            options.free_space = self.free_space.clone();
            options.clock = self.clock.clone();
            download::download(options).await
        })
    }

    fn verify_installer<'a>(
        &'a self,
        path: &'a Path,
        installer: &'a Value,
    ) -> BoxFuture<'a, VerifyOutcome> {
        Box::pin(channel::verify_installer_stream(
            path,
            installer,
            &VERIFY_DEFAULTS,
        ))
    }

    fn wait_for_exit<'a>(
        &'a self,
        on_wait: &'a (dyn Fn(&WaitInfo) + Send + Sync),
    ) -> BoxFuture<'a, Result<(), UpdaterError>> {
        Box::pin(async move {
            process_wait::wait_for_exit(WaitOptions {
                image_name: process_wait::LAUNCHER_IMAGE,
                timeout: process_wait::DEFAULT_TIMEOUT,
                poll: process_wait::POLL_INTERVAL,
                list: self.processes.as_ref(),
                clock: self.clock.as_ref(),
                sleep: &process_wait::tokio_sleep,
                on_wait,
            })
            .await
        })
    }

    fn run_installer<'a>(
        &'a self,
        installer: &'a Path,
        options: InstallOptions,
    ) -> BoxFuture<'a, Result<(), UpdaterError>> {
        Box::pin(async move {
            install::run_installer(self.launcher.as_ref(), installer, &options).await
        })
    }

    fn verify_installed(&self, launcher_exe: &str, target_version: &str) -> Installed {
        install::verify_installed(
            Path::new(launcher_exe),
            Some(target_version),
            &install::read_exe_product_version,
        )
    }

    fn write_receipt(&self, file: &str, from: Option<&str>, to: &str) -> bool {
        self.receipts.write_pending_receipt(file, from, to)
    }

    fn relaunch(&self, launcher_exe: &str) -> Result<(), UpdaterError> {
        install::relaunch(
            self.launcher.as_ref(),
            Path::new(launcher_exe),
            self.env.clone(),
        )
        .map(drop)
    }

    fn installer_env(&self) -> Vec<(String, String)> {
        self.env.clone()
    }
}

static VERIFY_DEFAULTS: VerifyOptions = VerifyOptions {
    max_bytes: None,
    cancel: None,
};
