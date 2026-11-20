//! Инструменты: установка, удаление и состояние (`ToolService.js`).
//!
//! Установка LiveSplit: до 512 МБ с источников манифеста по очереди (общий ограничитель скорости,
//! sha256 и размер архива), распаковка 7z в `.hlsr/staging/tool-<id>`, обмен каталогов и
//! `Tools/<id>/installed.json` (JSON с двумя пробелами, без перевода строки в конце — как пишет
//! `JSON.stringify(state, null, 2)`). Отмена действует на любом этапе; старая версия остаётся, пока
//! новая не распакована. Удаление — только внутри зоны библиотеки.
//!
//! 0.4.9 (FFmpeg и bxt-rs для «Демо»): `raw` — скачан сам файл, не архив; `extract` — только эти
//! файлы архива, плоско; `extras` — файлы рядом (лицензии) с закреплёнными размером и sha256;
//! `files` — закреплённые sha256 после распаковки (`verifyFiles`).
//!
//! Запуск и остановка (процесс прямо на Windows или через Proton в префиксе игры, слежение за
//! процессом, закрытие) — подпроект 3, за трейтом [`ToolRunner`]. Здесь всё, что запуск берёт из
//! файлов: проверки установленного, сплиты и раскладка игры, TCP-сервер времени из LiveSplit,
//! исправления компонентов под Wine, окружение Proton. Журнал `[tools] … запущен …` пишет тот, кто
//! запускает процесс.
//!
//! Отличие от 0.4.7: идентификатор инструмента ищется только среди своих ключей реестра (в JS
//! `registry["constructor"]` находит свойство `Object.prototype`).

use crate::archive_service::{ArchiveService, CancelHandle, ExtractOptions};
use crate::atomic::fd_error;
use crate::download_rate_limiter::DownloadRateLimiter;
use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::file_ops::hex;
use crate::http_client::{HttpClient, RequestOptions};
use crate::js_value as jv;
use crate::live_split_fixes::{self, LIVESPLIT_FIXES};
use crate::livesplit_splits::LiveSplitSplits;
use crate::node_fs;
use crate::paths::{LibraryPaths, assert_inside, remove_within_zone};
use crate::ports::{
    Logger, RunningTool, StopStatus, ToolLaunch, ToolRunner, TracingLogger, UnavailableToolRunner,
};
use crate::timing::{Clock, iso_from_ms, system_clock};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::json_text::{json_parse, stringify};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// `STATE_FILE`.
pub const STATE_FILE: &str = "installed.json";
/// `STATE_VERSION`.
pub const STATE_VERSION: u32 = 1;
/// `MAX_ARCHIVE_BYTES`: архив инструмента не больше 512 МБ.
pub const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
/// `DOWNLOAD_TIMEOUT_MS`: тайм-аут простоя запроса архива — 10 минут.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// `LIVESPLIT_TOOL_ID`.
pub const LIVESPLIT_TOOL_ID: &str = "livesplit";

/// Событие прогресса загрузки: процент (0–100) и принятые байты.
pub type ProgressFn<'a> = dyn Fn(f64, u64) + Send + Sync + 'a;

/// Исправления компонентов LiveSplit под Wine (`liveSplitFixes`): папка LiveSplit и каталог
/// ресурсов → пути заменённых файлов.
pub type LiveSplitFixesFn =
    Arc<dyn Fn(&Path, &Path) -> Result<Vec<String>, RawError> + Send + Sync>;

/// `toolLaunchEnvironment(toolId, hostEnvironment)`: режим дисплея (`PROTON_ENABLE_WAYLAND`) задаёт
/// запуск игры по префиксу, LiveSplit не должен выбирать его сам, иначе, запущенный первым, он
/// навязал бы свой драйвер игре. `display` — `DISPLAY` окружения лаунчера.
pub fn tool_launch_environment(
    tool_id: &str,
    display: Option<&str>,
) -> Option<Vec<(String, String)>> {
    if tool_id != LIVESPLIT_TOOL_ID || display.is_none_or(str::is_empty) {
        return None;
    }
    Some(vec![(
        "PROTON_USE_X11_EXCLUSIVE".to_owned(),
        "LiveSplit.exe".to_owned(),
    )])
}

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct ToolOptions {
    /// `registry`: идентификатор → манифест; по умолчанию — [`crate::tools::registry`].
    pub registry: Option<Map<String, Value>>,
    /// `archive`: распаковщик; без него установка отвечает `EXTRACT_FAILED`.
    pub archive: Option<ArchiveService>,
    pub logger: Option<Arc<dyn Logger>>,
    /// `rateLimiter`: общий лимит скорости загрузок.
    pub rate_limiter: Option<Arc<DownloadRateLimiter>>,
    /// `httpClient`.
    pub http: Option<HttpClient>,
    /// `platform`.
    pub platform: Option<Platform>,
    /// `hostEnvironment`: по умолчанию — окружение процесса.
    pub host_environment: Option<HashMap<String, String>>,
    /// `resourcesPath`: каталог ресурсов (`livesplit-components`, `livesplit-fixes`).
    pub resources_path: Option<PathBuf>,
    /// `liveSplitFixes`.
    pub live_split_fixes: Option<LiveSplitFixesFn>,
    /// Запуск инструментов; по умолчанию — заглушка подпроекта 3.
    pub runner: Option<Arc<dyn ToolRunner>>,
    /// Часы (`Date`).
    pub clock: Option<Clock>,
}

/// Параметры запуска (`options` `launch`).
#[derive(Debug, Clone, Default)]
pub struct LaunchOptions {
    pub game_id: Option<String>,
    pub splits: Option<String>,
    /// LiveSplit открыт для другой игры или с другими сплитами и игрок согласился его перезапустить.
    pub restart: bool,
    /// Порт TCP-сервера LiveSplit (время из LiveSplit в Ghosting).
    pub server_port: Option<i64>,
}

/// Отмена установки (`AbortController`): сигнал для ожиданий и признак для итога.
#[derive(Clone)]
struct Abort {
    flag: Arc<watch::Sender<bool>>,
}

impl Abort {
    fn new() -> Abort {
        Abort {
            flag: Arc::new(watch::channel(false).0),
        }
    }

    fn abort(&self) {
        self.flag.send_replace(true);
    }

    fn is_aborted(&self) -> bool {
        *self.flag.borrow()
    }

    /// Готово, когда сигнал подан (поданный раньше — сразу).
    async fn aborted(&self) {
        let mut receiver = self.flag.subscribe();
        let _ = receiver.wait_for(|aborted| *aborted).await;
    }

    /// `signal.throwIfAborted()`.
    fn throw_if_aborted(&self) -> Result<(), Failure> {
        if self.is_aborted() {
            Err(abort_failure())
        } else {
            Ok(())
        }
    }
}

/// `AbortError`: на него итог не смотрит — установка отвечает `CANCELLED`, пока сигнал подан.
fn abort_failure() -> Failure {
    RawError {
        code: Some("ABORT_ERR".to_owned()),
        message: "This operation was aborted".to_owned(),
        ..RawError::default()
    }
    .into()
}

/// Идущая установка (`installJob`).
struct InstallJob {
    tool_id: String,
    abort: Abort,
    extract: Mutex<Option<CancelHandle>>,
}

impl InstallJob {
    /// Распаковка начата: отмена, поданная раньше, останавливает её сразу.
    fn set_extract(&self, handle: CancelHandle) {
        let mut slot = lock(&self.extract);
        if self.abort.is_aborted() {
            handle.cancel();
        }
        *slot = Some(handle);
    }
}

/// Ошибка загрузки до перевода в код: запись в `.part` (`writeError`) или любая другая.
enum Fault {
    Write(RawError),
    Other(Failure),
}

impl From<Failure> for Fault {
    fn from(failure: Failure) -> Fault {
        Fault::Other(failure)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `path.join(...)` по правилам этой ОС.
fn join(parts: &[&str]) -> PathBuf {
    PathBuf::from(NodePath::native().join(parts))
}

/// `/^[a-z0-9]+([._-][a-z0-9]+)*$/`.
fn is_tool_id(tool_id: &str) -> bool {
    !tool_id.is_empty()
        && tool_id.split(['.', '_', '-']).all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Аргумент `path.join`, который обязан быть строкой: иначе `TypeError [ERR_INVALID_ARG_TYPE]`.
fn path_arg(value: Option<&Value>) -> Result<String, RawError> {
    let received = match value {
        Some(Value::String(path)) => return Ok(path.clone()),
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Bool(b)) => format!("type boolean ({b})"),
        Some(number @ Value::Number(_)) => format!(
            "type number ({})",
            js::number::number_to_string(jv::number(number).unwrap_or(f64::NAN))
        ),
        Some(Value::Array(_)) => "an instance of Array".to_owned(),
        Some(Value::Object(_)) => "an instance of Object".to_owned(),
    };
    Err(RawError {
        code: Some("ERR_INVALID_ARG_TYPE".to_owned()),
        message: format!("The \"path\" argument must be of type string. Received {received}"),
        ..RawError::default()
    })
}

/// `a === b` для значений JSON: объекты и массивы — разные.
fn strict_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) | (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x == y,
        (Some(Value::String(x)), Some(Value::String(y))) => x == y,
        (Some(x @ Value::Number(_)), Some(y @ Value::Number(_))) => jv::number(x) == jv::number(y),
        _ => false,
    }
}

/// `ToolService`.
pub struct ToolService {
    paths: LibraryPaths,
    registry: Map<String, Value>,
    archive: Option<ArchiveService>,
    logger: Arc<dyn Logger>,
    http: HttpClient,
    rate_limiter: Option<Arc<DownloadRateLimiter>>,
    platform: Platform,
    host_display: Option<String>,
    resources_path: PathBuf,
    live_split_fixes: LiveSplitFixesFn,
    live_split_splits: LiveSplitSplits,
    runner: Arc<dyn ToolRunner>,
    clock: Clock,
    busy_with: Mutex<Option<String>>,
    job: Mutex<Option<Arc<InstallJob>>>,
}

/// Идущая установка: при выходе из `install` (и при брошенной задаче) — `finally` 0.4.7: сервис
/// свободен, архив и staging убраны.
struct Finish<'a> {
    service: &'a ToolService,
    archive_path: PathBuf,
    staging: PathBuf,
}

impl Drop for Finish<'_> {
    fn drop(&mut self) {
        *lock(&self.service.busy_with) = None;
        *lock(&self.service.job) = None;
        let _ = std::fs::remove_file(&self.archive_path);
        let _ = remove_within_zone(&self.service.paths.root, &self.staging);
    }
}

impl ToolService {
    /// `new ToolService({ paths, … })`.
    pub fn new(paths: LibraryPaths, options: ToolOptions) -> ToolService {
        let logger = options
            .logger
            .unwrap_or_else(|| Arc::new(TracingLogger) as Arc<dyn Logger>);
        let resources_path = options
            .resources_path
            .unwrap_or_else(|| hlsr_core::paths::Resources::detect().dir().to_path_buf());
        let host_display = match &options.host_environment {
            Some(environment) => environment.get("DISPLAY").cloned(),
            None => std::env::var("DISPLAY").ok(),
        };
        ToolService {
            registry: options.registry.unwrap_or_else(crate::tools::registry),
            archive: options.archive,
            http: options.http.unwrap_or_default(),
            rate_limiter: options.rate_limiter,
            platform: options.platform.unwrap_or(Platform::CURRENT),
            host_display,
            live_split_fixes: options.live_split_fixes.unwrap_or_else(|| {
                Arc::new(|root: &Path, resources: &Path| {
                    live_split_fixes::apply_live_split_fixes(root, resources, &LIVESPLIT_FIXES)
                })
            }),
            live_split_splits: LiveSplitSplits::new(resources_path.clone(), logger.clone()),
            resources_path,
            runner: options
                .runner
                .unwrap_or_else(|| Arc::new(UnavailableToolRunner)),
            clock: options.clock.unwrap_or_else(system_clock),
            logger,
            busy_with: Mutex::new(None),
            job: Mutex::new(None),
            paths,
        }
    }

    /// Подставить распаковщик (`service.archive = …`).
    pub fn set_archive(&mut self, archive: Option<ArchiveService>) {
        self.archive = archive;
    }

    /// `busyWith`: инструмент, который сейчас ставится.
    pub fn busy_with(&self) -> Option<String> {
        lock(&self.busy_with).clone()
    }

    /// `toolsRoot()`.
    fn tools_root(&self) -> Result<&Path, DeliveryError> {
        if self.paths.tools.as_os_str().is_empty() {
            return Err(DeliveryError::new(
                Code::PathInvalid,
                "Каталог инструментов не задан",
            ));
        }
        Ok(&self.paths.tools)
    }

    /// `installPathOf(toolId)`: `Tools/<toolId>`.
    pub fn install_path_of(&self, tool_id: &str) -> Result<PathBuf, DeliveryError> {
        if !is_tool_id(tool_id) {
            return Err(DeliveryError::new(
                Code::ToolUnknown,
                format!("Недопустимый идентификатор инструмента: {tool_id}"),
            ));
        }
        let root = self.tools_root()?;
        let target = join(&[&text(root), tool_id]);
        assert_inside(root, &target, None)?;
        Ok(target)
    }

    /// `manifestOf(toolId)`.
    pub fn manifest_of(&self, tool_id: &str) -> Result<&Value, DeliveryError> {
        self.registry.get(tool_id).ok_or_else(|| {
            DeliveryError::new(
                Code::ToolUnknown,
                format!("Нет манифеста инструмента {tool_id}"),
            )
        })
    }

    /// `status(toolId)`: карточка состояния — `{toolId, state, version, availableVersion,
    /// updateAvailable, installPath, percent, error, running, stopping}`. Поля, которых в 0.4.7
    /// нет (`undefined`), в объект не попадают.
    pub fn status(&self, tool_id: &str) -> Value {
        let busy = self.busy_with().as_deref() == Some(tool_id);
        let running = self.runner.running(tool_id);
        let mut base = Map::new();
        base.insert("toolId".to_owned(), json!(tool_id));
        base.insert(
            "state".to_owned(),
            json!(if busy { "installing" } else { "not-installed" }),
        );
        base.insert("version".to_owned(), Value::Null);
        base.insert("availableVersion".to_owned(), Value::Null);
        base.insert("updateAvailable".to_owned(), json!(false));
        base.insert("installPath".to_owned(), Value::Null);
        base.insert("percent".to_owned(), json!(0));
        base.insert("error".to_owned(), Value::Null);
        base.insert("running".to_owned(), json!(running.is_some()));
        base.insert(
            "stopping".to_owned(),
            json!(running.as_ref().is_some_and(|r| r.stopping)),
        );

        let manifest = match self.manifest_of(tool_id) {
            Ok(manifest) => manifest,
            Err(error) => {
                base.insert("state".to_owned(), json!("unknown"));
                base.insert("error".to_owned(), json!({"code": error.code.as_str()}));
                return Value::Object(base);
            }
        };
        let available = jv::prop(manifest, "version");
        set_or_remove(&mut base, "availableVersion", available);

        let Some(installed) = self.read_installed(tool_id) else {
            return Value::Object(base);
        };
        base.insert(
            "state".to_owned(),
            json!(if busy { "installing" } else { "ready" }),
        );
        let version = jv::prop(&installed, "version");
        set_or_remove(&mut base, "version", version);
        set_or_remove(
            &mut base,
            "installPath",
            jv::prop(&installed, "installPath"),
        );
        base.insert(
            "updateAvailable".to_owned(),
            json!(jv::truthy(available) && !strict_eq(version, available)),
        );
        Value::Object(base)
    }

    /// `readInstalled(toolId)`: `installed.json` инструмента; нет файла, он не разбирается, чужой
    /// (`toolId` другой) или без версии — `None`.
    pub fn read_installed(&self, tool_id: &str) -> Option<Value> {
        let file = join(&[&text(&self.install_path_of(tool_id).ok()?), STATE_FILE]);
        let bytes = js::fs::read_file(&file).ok()?;
        let data = json_parse(&js::utf8_string(&bytes)).ok()?;
        (jv::is_str(jv::prop(&data, "toolId"), tool_id) && jv::truthy(jv::prop(&data, "version")))
            .then_some(data)
    }

    /// `install(toolId, onProgress)`: `{status: "installed", version, installPath}`. Отмена
    /// ([`ToolService::cancel`]) — `CANCELLED`; ошибки файловой системы идут как есть (`Failure::Raw`).
    pub async fn install(
        &self,
        tool_id: &str,
        on_progress: Option<&ProgressFn<'_>>,
    ) -> Result<Value, Failure> {
        if self.runner.running(tool_id).is_some() {
            return Err(DeliveryError::new(
                Code::AlreadyRunning,
                "Закройте инструмент перед обновлением",
            )
            .into());
        }
        if self.busy_with().is_some() {
            return Err(already_installing());
        }

        let manifest = self.manifest_of(tool_id)?.clone();
        let target = self.install_path_of(tool_id)?;
        if jv::prop(&manifest, "enabled") == Some(&Value::Bool(false)) {
            return Err(
                DeliveryError::new(Code::RemoteDisabled, "Инструмент временно недоступен").into(),
            );
        }

        let staging = join(&[&text(&self.paths.staging), &format!("tool-{tool_id}")]);
        let filename = match jv::prop(&manifest, "archive") {
            None => return Err(jv::read_error(None, "filename").into()),
            Some(Value::Null) => return Err(jv::read_error(Some(&Value::Null), "filename").into()),
            Some(archive) => path_arg(jv::prop(archive, "filename"))?,
        };
        let archive_path = join(&[&text(&self.paths.downloads), &filename]);

        let job = Arc::new(InstallJob {
            tool_id: tool_id.to_owned(),
            abort: Abort::new(),
            extract: Mutex::new(None),
        });
        {
            let mut busy = lock(&self.busy_with);
            if busy.is_some() {
                return Err(already_installing());
            }
            *busy = Some(tool_id.to_owned());
            *lock(&self.job) = Some(job.clone());
        }
        let _finish = Finish {
            service: self,
            archive_path: archive_path.clone(),
            staging: staging.clone(),
        };

        let result = self
            .run_install(
                tool_id,
                &manifest,
                &target,
                &staging,
                &archive_path,
                on_progress,
                &job,
            )
            .await;
        match result {
            Err(_) if job.abort.is_aborted() => {
                Err(DeliveryError::new(Code::Cancelled, "Установка инструмента отменена").into())
            }
            other => other,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_install(
        &self,
        tool_id: &str,
        manifest: &Value,
        target: &Path,
        staging: &Path,
        archive_path: &Path,
        on_progress: Option<&ProgressFn<'_>>,
        job: &InstallJob,
    ) -> Result<Value, Failure> {
        node_fs::mkdir_recursive(self.tools_root()?)?;
        node_fs::mkdir_recursive(&self.paths.downloads)?;
        node_fs::mkdir_recursive(&self.paths.staging)?;

        self.download(manifest, archive_path, on_progress, &job.abort)
            .await?;
        job.abort.throw_if_aborted()?;

        remove_within_zone(&self.paths.root, staging)?;
        node_fs::mkdir_recursive(staging)?;

        let executable = jv::prop(manifest, "executable");
        // manifest.raw — скачан сам файл инструмента (bxt_rs.dll), не архив.
        let raw = jv::truthy(jv::prop(manifest, "raw"));
        if raw {
            // Строже 0.4.9: путь проверяется до переноса, а не после.
            let target = join(&[&text(staging), &path_arg(executable)?]);
            assert_inside(staging, &target, None)?;
            std::fs::rename(archive_path, &target).map_err(|error| {
                RawError::fs_with_dest(&error, "rename", archive_path, Some(&target))
            })?;
        } else {
            let Some(archive) = &self.archive else {
                return Err(
                    DeliveryError::new(Code::ExtractFailed, "Распаковщик не подключён").into(),
                );
            };
            // manifest.extract — только эти файлы архива, плоско в корень.
            let only: Vec<String> = jv::array(jv::prop(manifest, "extract"))
                .map(|list| list.iter().map(|file| jv::template(Some(file))).collect())
                .unwrap_or_default();
            let extraction =
                archive.extract_only(archive_path, staging, &only, ExtractOptions::default());
            job.set_extract(extraction.cancel_handle());
            extraction.await?;
            job.abort.throw_if_aborted()?;
        }

        // manifest.extras — файлы рядом с инструментом (лицензия, уведомления) с закреплёнными
        // размером и sha256.
        for extra in jv::array(jv::prop(manifest, "extras")).map_or(&[][..], Vec::as_slice) {
            let field = |key: &str| jv::prop(extra, key).filter(|value| js::truthy(value));
            let (Some(name), Some(url), Some(_), Some(_)) =
                (field("name"), field("url"), field("size"), field("sha256"))
            else {
                return Err(DeliveryError::new(
                    Code::ManifestInvalid,
                    "У файла инструмента нет размера или sha256",
                )
                .into());
            };
            let file = join(&[&text(staging), &jv::template(Some(name))]);
            assert_inside(staging, &file, None)?;
            self.download_with(&jv::template(Some(url)), extra, &file, None, &job.abort)
                .await?;
        }

        let exe = join(&[&text(staging), &path_arg(executable)?]);
        assert_inside(staging, &exe, None)?;
        if !exe.exists() {
            return Err(DeliveryError::new(
                Code::ExtractFailed,
                format!(
                    "В архиве нет {} — распаковано не то, что ожидалось",
                    jv::template(executable)
                ),
            )
            .into());
        }

        self.verify_files(staging, jv::prop(manifest, "files"))?;

        remove_within_zone(&self.paths.root, target)?;
        std::fs::rename(staging, target)
            .map_err(|error| RawError::fs_with_dest(&error, "rename", staging, Some(target)))?;

        self.write_installed(tool_id, manifest, target)?;

        // У raw архив уже перенесён в папку инструмента.
        if !raw && let Err(error) = std::fs::remove_file(archive_path) {
            self.logger.warn(&format!(
                "[tools] архив не удалён: {}",
                RawError::fs(&error, "unlink", archive_path).message
            ));
        }

        let version = jv::prop(manifest, "version");
        self.logger.info(&format!(
            "[tools] {tool_id} {} установлен: {}",
            jv::template(version),
            target.display()
        ));
        let mut done = Map::new();
        done.insert("status".to_owned(), json!("installed"));
        set_or_remove(&mut done, "version", version);
        done.insert("installPath".to_owned(), json!(text(target)));
        Ok(Value::Object(done))
    }

    /// `verifyFiles(root, files)`: закреплённые SHA-256 распакованных файлов (`{имя: sha256}`).
    /// Файл не читается — `EXTRACT_FAILED`, другой хеш — `CHECKSUM_MISMATCH`.
    fn verify_files(&self, root: &Path, files: Option<&Value>) -> Result<(), Failure> {
        let Some(Value::Object(files)) = files else {
            return Ok(());
        };
        for (name, expected) in files {
            let file = join(&[&text(root), name]);
            assert_inside(root, &file, None)?;
            let missing =
                || DeliveryError::new(Code::ExtractFailed, format!("В архиве нет {name}"));
            let mut reader = std::fs::File::open(&file).map_err(|_| missing())?;
            let mut hash = Sha256::new();
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                let count = std::io::Read::read(&mut reader, &mut buffer).map_err(|_| missing())?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
            }
            if hex(&hash.finalize()) != jv::template(Some(expected)).to_lowercase() {
                return Err(DeliveryError::new(
                    Code::ChecksumMismatch,
                    format!("SHA-256 {name} не совпал с закреплённым"),
                )
                .into());
            }
        }
        Ok(())
    }

    /// `cancel(toolId)`: отменить идущую установку этого инструмента; `false` — её нет.
    pub fn cancel(&self, tool_id: &str) -> bool {
        let job = lock(&self.job).clone();
        let Some(job) = job.filter(|job| job.tool_id == tool_id) else {
            return false;
        };
        job.abort.abort();
        if let Some(extract) = lock(&job.extract).as_ref() {
            extract.cancel();
        }
        true
    }

    /// `writeInstalled(toolId, manifest, installPath)`: `installed.json` (`JSON.stringify` с двумя
    /// пробелами, без перевода строки в конце; записывается не атомарно, как в 0.4.7).
    fn write_installed(
        &self,
        tool_id: &str,
        manifest: &Value,
        install_path: &Path,
    ) -> Result<(), RawError> {
        let mut state = Map::new();
        state.insert("schemaVersion".to_owned(), json!(STATE_VERSION));
        state.insert("toolId".to_owned(), json!(tool_id));
        if let Some(version) = jv::prop(manifest, "version") {
            state.insert("version".to_owned(), version.clone());
        }
        if let Some(executable) = jv::prop(manifest, "executable") {
            state.insert("executable".to_owned(), executable.clone());
        }
        state.insert("installPath".to_owned(), json!(text(install_path)));
        state.insert("installedAt".to_owned(), json!(iso_from_ms((self.clock)())));
        node_fs::write_bytes(
            &join(&[&text(install_path), STATE_FILE]),
            stringify(&Value::Object(state), "  ").as_bytes(),
        )
    }

    /// `download(manifest, target, onProgress, signal)`: источники по очереди; первый, что отдал
    /// архив целиком, — конец. Все не сработали — последняя ошибка.
    async fn download(
        &self,
        manifest: &Value,
        target: &Path,
        on_progress: Option<&ProgressFn<'_>>,
        abort: &Abort,
    ) -> Result<(), Failure> {
        let descriptor = jv::prop(manifest, "archive").unwrap_or(&Value::Null);
        let providers: &[Value] =
            jv::array(jv::prop(descriptor, "providers")).map_or(&[], Vec::as_slice);
        if providers.is_empty() {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                "У архива инструмента нет ни одного источника",
            )
            .into());
        }
        let cancelled = || -> Failure {
            DeliveryError::new(Code::Cancelled, "Загрузка инструмента отменена").into()
        };

        let mut last: Option<Failure> = None;
        for provider in providers {
            if abort.is_aborted() {
                return Err(cancelled());
            }
            let url = jv::template(jv::prop(provider, "url"));
            match self
                .download_with(&url, descriptor, target, on_progress, abort)
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if abort.is_aborted() {
                        return Err(cancelled());
                    }
                    let message = match &error {
                        Failure::Delivery(error) => error.message.clone(),
                        Failure::Raw(error) => error.message.clone(),
                    };
                    self.logger.warn(&format!(
                        "[tools] источник {} не сработал: {message}",
                        jv::template(jv::prop(provider, "id"))
                    ));
                    last = Some(error);
                }
            }
        }
        Err(last.unwrap_or_else(|| {
            DeliveryError::new(Code::NetworkError, "Скачать архив инструмента не удалось").into()
        }))
    }

    /// `downloadFrom(url, descriptor, target, onProgress)`: один источник, без отмены.
    pub async fn download_from(
        &self,
        url: &str,
        descriptor: &Value,
        target: &Path,
        on_progress: Option<&ProgressFn<'_>>,
    ) -> Result<(), Failure> {
        self.download_with(url, descriptor, target, on_progress, &Abort::new())
            .await
    }

    /// `downloadFrom` с сигналом отмены: архив → `<target>.part` → `target`.
    async fn download_with(
        &self,
        url: &str,
        descriptor: &Value,
        target: &Path,
        on_progress: Option<&ProgressFn<'_>>,
        abort: &Abort,
    ) -> Result<(), Failure> {
        let size = jv::prop(descriptor, "size");
        if size.is_some_and(|size| jv::to_number(size) > MAX_ARCHIVE_BYTES as f64) {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                "Архив инструмента подозрительно велик",
            )
            .into());
        }
        let part = PathBuf::from(format!("{}.part", text(target)));
        match self
            .fetch(url, descriptor, target, &part, on_progress, abort)
            .await
        {
            Ok(()) => Ok(()),
            Err(fault) => {
                let _ = std::fs::remove_file(&part);
                if abort.is_aborted() {
                    return Err(DeliveryError::new(
                        Code::Cancelled,
                        "Загрузка инструмента отменена",
                    )
                    .into());
                }
                Err(match fault {
                    Fault::Write(error) => {
                        DeliveryError::new(Code::PathNotWritable, error.message).into()
                    }
                    Fault::Other(Failure::Delivery(error)) => error.into(),
                    Fault::Other(Failure::Raw(error)) => {
                        DeliveryError::new(Code::NetworkError, error.message).into()
                    }
                })
            }
        }
    }

    /// Тело `try` из `downloadFrom`: ответ → ограничитель скорости → счётчик, хеш, прогресс → `.part`.
    async fn fetch(
        &self,
        url: &str,
        descriptor: &Value,
        target: &Path,
        part: &Path,
        on_progress: Option<&ProgressFn<'_>>,
        abort: &Abort,
    ) -> Result<(), Fault> {
        let options = RequestOptions {
            timeout: Some(DOWNLOAD_TIMEOUT),
            ..RequestOptions::default()
        };
        let mut response = tokio::select! {
            result = self.http.request(url, &options) => result?,
            () = abort.aborted() => return Err(abort_failure().into()),
        };
        if response.status() != 200 {
            let status = response.status();
            drop(response);
            return Err(Failure::from(DeliveryError::new(
                Code::NetworkError,
                format!("Источник ответил {status}"),
            ))
            .into());
        }

        let size = jv::prop(descriptor, "size").filter(|size| js::truthy(size));
        let mut hash = Sha256::new();
        let mut received: u64 = 0;
        let mut file = std::fs::File::create(part)
            .map_err(|error| Fault::Write(RawError::fs(&error, "open", part)))?;
        loop {
            let chunk = tokio::select! {
                chunk = response.chunk() => chunk.map_err(|raw| Fault::Other(raw.into()))?,
                () = abort.aborted() => return Err(abort_failure().into()),
            };
            let Some(chunk) = chunk else {
                break;
            };
            if let Some(limiter) = &self.rate_limiter {
                tokio::select! {
                    () = limiter.acquire(chunk.len()) => {}
                    () = abort.aborted() => return Err(abort_failure().into()),
                }
            }
            received += chunk.len() as u64;
            if received > MAX_ARCHIVE_BYTES {
                return Err(Failure::from(DeliveryError::new(
                    Code::NetworkError,
                    "Ответ длиннее допустимого",
                ))
                .into());
            }
            hash.update(&chunk);
            if let (Some(report), Some(size)) = (on_progress, size) {
                let percent =
                    js::math_round(received as f64 / jv::to_number(size) * 100.0).min(100.0);
                report(percent, received);
            }
            file.write_all(&chunk)
                .map_err(|error| Fault::Write(fd_error(&error, "write")))?;
        }
        drop(file);

        if let Some(size) = size
            && jv::number(size) != Some(received as f64)
        {
            return Err(Failure::from(DeliveryError::new(
                Code::SizeMismatch,
                format!(
                    "Размер архива не совпал: {received} вместо {}",
                    jv::template(Some(size))
                ),
            ))
            .into());
        }
        if let Some(sha256) = jv::prop(descriptor, "sha256").filter(|sha| js::truthy(sha))
            && !jv::is_str(Some(sha256), &hex(&hash.finalize()))
        {
            return Err(Failure::from(DeliveryError::new(
                Code::ChecksumMismatch,
                "SHA-256 скачанного архива не совпал с объявленным",
            ))
            .into());
        }
        std::fs::rename(part, target).map_err(|error| {
            Failure::from(DeliveryError::new(
                Code::PathNotWritable,
                RawError::fs_with_dest(&error, "rename", part, Some(target)).message,
            ))
        })?;
        Ok(())
    }

    /// `launch(toolId, options)`: `{status: "started", pid}`; уже открытый LiveSplit не перечитывает
    /// аргументы, поэтому другие сплиты или другая игра — только через перезапуск (`restart`):
    /// `{status: "already-running", gameId, splits, same}`, не закрылся — `{status: "stop-timeout"}`.
    pub async fn launch(&self, tool_id: &str, options: &LaunchOptions) -> Result<Value, Failure> {
        let game_choice = options.game_id.as_deref().filter(|id| !id.is_empty());
        let splits_choice = options.splits.as_deref().filter(|id| !id.is_empty());
        if let Some(current) = self.runner.running(tool_id) {
            let same = tool_id != LIVESPLIT_TOOL_ID
                || (game_choice.is_none_or(|id| current.game_id.as_deref() == Some(id))
                    && splits_choice.is_none_or(|id| current.splits.as_deref() == Some(id)));
            if same || !options.restart {
                return Ok(json!({
                    "status": "already-running",
                    "gameId": current.game_id,
                    "splits": current.splits,
                    "same": same,
                }));
            }
            let stopped = self.stop(tool_id, false).await?;
            if !matches!(stopped, StopStatus::Stopped | StopStatus::NotRunning) {
                return Ok(json!({"status": "stop-timeout"}));
            }
        }

        let installed = self
            .read_installed(tool_id)
            .ok_or_else(|| DeliveryError::new(Code::NotInstalled, "Инструмент не установлен"))?;
        let manifest = self.manifest_of(tool_id)?;
        let root = self.install_path_of(tool_id)?;
        let installed_executable = jv::prop(&installed, "executable").filter(|v| js::truthy(v));
        let executable = installed_executable.or_else(|| jv::prop(manifest, "executable"));
        let executable_name = path_arg(executable)?;
        let exe = join(&[&text(&root), &executable_name]);
        assert_inside(&root, &exe, None)?;
        if !exe.exists() {
            return Err(DeliveryError::new(
                Code::ToolExecutableMissing,
                format!(
                    "Файл {} не найден — переустановите инструмент",
                    jv::template(executable)
                ),
            )
            .into());
        }

        // На Windows LiveSplit общий, но запоминается, для какой игры он открыт: оверлей LiveSplit
        // показывается только в этой игре.
        let game_id = game_choice
            .map(str::to_owned)
            .or_else(|| (self.platform == Platform::Linux).then(|| "ghosting-renewed".to_owned()));
        let is_livesplit = tool_id == LIVESPLIT_TOOL_ID;
        let (args, preset) = if is_livesplit {
            self.split_arguments(&root, game_id.as_deref(), splits_choice)
        } else {
            (Vec::new(), None)
        };
        if is_livesplit
            && let Some(port) = options.server_port
            && let Err(error) = self.live_split_splits.enable_server(&root, port)
        {
            self.logger.warn(&format!(
                "[tools] livesplit: TCP-сервер не включён: {}",
                error.message
            ));
        }

        let mut request = ToolLaunch {
            tool_id: tool_id.to_owned(),
            exe,
            root: root.clone(),
            args,
            game_id,
            splits: preset,
            executable: NodePath::new(Platform::Win32, "").basename(&executable_name),
            env: None,
            font_profile: None,
        };
        if self.platform == Platform::Linux {
            if is_livesplit {
                request.font_profile = Some("livesplit".to_owned());
                // Исправления компонентов под Wine. Без них LiveSplit всё равно запускается, поэтому
                // ошибка только пишется в журнал.
                match (self.live_split_fixes)(&root, &self.resources_path) {
                    Ok(files) => {
                        for file in files {
                            self.logger.info(&format!(
                                "[tools] livesplit: заменён {file} исправленным под Wine"
                            ));
                        }
                    }
                    Err(error) => self.logger.warn(&format!(
                        "[tools] livesplit: исправления не применены: {}",
                        error.message
                    )),
                }
            }
            request.env = tool_launch_environment(tool_id, self.host_display.as_deref());
        }

        let pid = self.runner.launch(&request).await?;
        Ok(json!({"status": "started", "pid": pid}))
    }

    /// `stop(toolId, options)`: закрыть запущенный инструмент.
    pub async fn stop(&self, tool_id: &str, force: bool) -> Result<StopStatus, Failure> {
        self.runner.stop(tool_id, force).await
    }

    /// `runningInfo(toolId)`: запущен ли инструмент и для какой игры (`None` — нет или закрывается).
    pub fn running_info(&self, tool_id: &str) -> Option<RunningTool> {
        self.runner
            .running(tool_id)
            .filter(|record| !record.stopping)
    }

    /// `splitArguments(root, gameId, presetId)`: сплиты и раскладка игры (`livesplit-splits.js`).
    /// Без них LiveSplit всё равно запускается — с последними открытыми сплитами.
    fn split_arguments(
        &self,
        root: &Path,
        game_id: Option<&str>,
        preset_id: Option<&str>,
    ) -> (Vec<String>, Option<String>) {
        let Some(game_id) = game_id else {
            return (Vec::new(), None);
        };
        match self.live_split_splits.prepare(root, game_id, preset_id) {
            Ok(prepared) => {
                if let Some(preset) = prepared.preset {
                    self.logger.info(&format!(
                        "[tools] livesplit: сплиты {game_id}/{}",
                        preset.id
                    ));
                }
                (
                    prepared.args,
                    prepared.preset.map(|preset| preset.id.to_owned()),
                )
            }
            Err(error) => {
                self.logger.warn(&format!(
                    "[tools] livesplit: сплиты {game_id} не подготовлены: {}",
                    error.message
                ));
                (Vec::new(), None)
            }
        }
    }

    /// `remove(toolId)`: `{status: "removed"}` или `{status: "not-installed"}`.
    pub fn remove(&self, tool_id: &str) -> Result<Value, Failure> {
        if self.busy_with().as_deref() == Some(tool_id) {
            return Err(
                DeliveryError::new(Code::AlreadyRunning, "Установка ещё выполняется").into(),
            );
        }
        if self.runner.running(tool_id).is_some() {
            return Err(DeliveryError::new(
                Code::AlreadyRunning,
                "Закройте инструмент перед удалением",
            )
            .into());
        }
        let target = self.install_path_of(tool_id)?;
        if !target.exists() {
            return Ok(json!({"status": "not-installed"}));
        }
        remove_within_zone(&self.paths.root, &target)?;
        self.logger
            .info(&format!("[tools] {tool_id} удалён: {}", target.display()));
        Ok(json!({"status": "removed"}))
    }
}

fn already_installing() -> Failure {
    DeliveryError::new(Code::AlreadyRunning, "Установка уже идёт").into()
}

/// `object[key] = value` для JSON: `undefined` (`None`) в объект не попадает.
fn set_or_remove(object: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    match value {
        Some(value) => {
            object.insert(key.to_owned(), value.clone());
        }
        None => {
            object.shift_remove(key);
        }
    }
}
