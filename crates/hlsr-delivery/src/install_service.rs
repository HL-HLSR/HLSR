//! Установка и обновление полной сборкой (`InstallService.js`).
//!
//! Одна установка за раз: `.hlsr/state/install.lock` (`{pid, gameId, host, startedAt}`) не даёт
//! начать вторую, пока жив записавший её процесс. Ход операции пишется в журнал
//! `<id>.operation.json` (фаза, пути staging/rollback, архив); при сбое — `<id>.last-failure.json`.
//! Установка: место, sha256 архива, проверка записей, распаковка в staging, обязательные файлы,
//! метаданные сборки, перенос файлов игрока (`preservePaths`, данные игрока, изменённые cfg), обмен
//! каталогов, `installed.json`. Обновление полной сборкой меняет каталоги по фазам
//! `swap-retire` → `swap-place` → `installed-write` → `commit-complete`; старая версия до конца
//! лежит в rollback. Сбой в том же процессе откатывается сразу; сбой процесса (точки
//! `UPDATE_FAULTS`) — при следующем запуске по журналу: [`InstallService::recover_full_update`].
//!
//! Удаление — только внутри своей зоны (`removeWithinZone`, повторы и ошибки как у `fs.rmSync`
//! Node 24, в том числе файлы «только для чтения» в Windows). Тексты журнала и ошибок — как в
//! 0.4.7. Отличия: путь архива и версия, с которой обновляемся, — обязательные строки (в 0.4.7 их
//! отсутствие давало `undefined` в журнале и ошибку размера).

use crate::archive_entries::ArchiveEntry;
use crate::archive_service::{ArchiveService, CancelHandle, ExtractOptions, InspectArgs};
use crate::atomic::{fd_error, write_json_atomic};
use crate::build_metadata_trust::{self, Reason, VerifyForInstall};
use crate::errors::{
    Code, DeliveryError, Failure, RawError, antivirus_error, is_antivirus_block, preserve_error,
    to_delivery_error,
};
use crate::file_ops;
use crate::free_space::FreeSpaceCache;
use crate::js_value as jv;
use crate::manifests;
use crate::mutable_paths;
use crate::node_fs;
use crate::path_match::Matcher;
use crate::paths::{LibraryPaths, assert_inside_with, game_install_path, remove_within_zone};
use crate::ports::{FreeSpace, Logger, SystemFreeSpace, TracingLogger};
use crate::providers::BoxFuture;
use crate::timing::{Clock, iso_from_ms, system_clock};
use crate::user_configs;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::json_store::{JsonState, read_json_safe, read_json_state};
use hlsr_core::json_text::{js_key_order, js_number, json_parse, stringify};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// `INSTALLED_SCHEMA_VERSION`.
pub const INSTALLED_SCHEMA_VERSION: u32 = 1;
/// `OPERATION_SCHEMA_VERSION`: версия журнала `<id>.operation.json`.
pub const OPERATION_SCHEMA_VERSION: u32 = 1;
/// `INSTALLED_FILE`.
pub const INSTALLED_FILE: &str = "installed.json";
/// `LOCK_FILE`.
pub const LOCK_FILE: &str = "install.lock";
/// `SPACE_MARGIN`: запас места сверх `expectedInstalledSize`.
pub const SPACE_MARGIN: u64 = 512 * 1024 * 1024;
/// `ORPHAN_STAGING_MIN_AGE_MS`: забытый staging убирается, если ему больше суток.
pub const ORPHAN_STAGING_MIN_AGE_MS: u64 = 24 * 60 * 60 * 1000;
/// `UPDATE_SWAP_PHASES`: фазы обмена каталогов при обновлении полной сборкой.
pub const UPDATE_SWAP_PHASES: [&str; 4] = [
    "swap-retire",
    "swap-place",
    "installed-write",
    "commit-complete",
];
/// `UPDATE_FAULTS`: точки отказа обмена каталогов (для проверки восстановления).
pub const UPDATE_FAULTS: [&str; 6] = [
    "before-retire",
    "after-retire",
    "before-place",
    "after-place",
    "before-installed-write",
    "after-installed-write",
];

/// Размер блока чтения архива при подсчёте sha256 (`highWaterMark`).
const HASH_BLOCK: usize = 4 * 1024 * 1024;

/// `faultInjector(name, operation)`: точка отказа и журнал операции на этот момент; ошибка —
/// сбой в этой точке.
pub type FaultInjector = Arc<dyn Fn(&str, &Value) -> Result<(), Failure> + Send + Sync>;

/// Слушатель события `state`.
pub type Listener = Arc<dyn Fn(&InstallEvent) + Send + Sync>;

// ---------------------------------------------------------------- события

/// Состояние в событии `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    Preparing,
    Verifying,
    Extracting,
    Finalizing,
    Ready,
    Error,
    Cancelled,
}

impl InstallState {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallState::Preparing => "preparing",
            InstallState::Verifying => "verifying",
            InstallState::Extracting => "extracting",
            InstallState::Finalizing => "finalizing",
            InstallState::Ready => "ready",
            InstallState::Error => "error",
            InstallState::Cancelled => "cancelled",
        }
    }
}

/// Событие `state`: `{gameId, state}` и `percent` (распаковка), `installed` (готово) или
/// `error` (`toPublic()` ошибки).
#[derive(Debug, Clone, PartialEq)]
pub struct InstallEvent {
    pub game_id: String,
    pub state: InstallState,
    pub percent: Option<f64>,
    pub installed: Option<Value>,
    pub error: Option<Value>,
}

impl InstallEvent {
    fn new(game_id: &str, state: InstallState) -> InstallEvent {
        InstallEvent {
            game_id: game_id.to_owned(),
            state,
            percent: None,
            installed: None,
            error: None,
        }
    }

    /// Объект события, как его получает слушатель в 0.4.7.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("gameId".to_owned(), json!(self.game_id));
        out.insert("state".to_owned(), json!(self.state.as_str()));
        if let Some(percent) = self.percent {
            out.insert("percent".to_owned(), js_number(percent));
        }
        if let Some(installed) = &self.installed {
            out.insert("installed".to_owned(), installed.clone());
        }
        if let Some(error) = &self.error {
            out.insert("error".to_owned(), error.clone());
        }
        Value::Object(out)
    }
}

// ---------------------------------------------------------------- проверка распакованной сборки

/// Запрос к сервису проверки файлов (`integrityService.check`): распакованная сборка в staging.
#[derive(Debug, Clone, Copy)]
pub struct IntegrityRequest<'a> {
    pub game_id: &'a str,
    pub install_path: &'a Path,
    pub files_manifest: &'a Value,
    pub preserve_paths: &'a [String],
    /// `stockConfigs`: сверить и стоковые cfg манифеста («Проверить файлы» и ремонт).
    pub stock_configs: bool,
}

/// То, что установка читает из отчёта проверки файлов.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IntegrityReport {
    pub ok: bool,
    pub damaged: usize,
    pub missing: Vec<String>,
    /// Пути повреждённых файлов (`corrupted[].path`).
    pub corrupted: Vec<String>,
    /// Пути файлов другого размера (`sizeMismatch[].path`).
    pub size_mismatch: Vec<String>,
    pub extra: Vec<String>,
}

/// Сервис проверки файлов (`IntegrityService`), которым обновление сверяет распакованную сборку
/// с файловым манифестом.
pub trait IntegrityCheck: Send + Sync {
    fn check<'a>(
        &'a self,
        request: IntegrityRequest<'a>,
    ) -> BoxFuture<'a, Result<IntegrityReport, Failure>>;
}

// ---------------------------------------------------------------- запрос и параметры

/// `request.update`: обновление полной сборкой поверх установленной версии.
#[derive(Clone)]
pub struct FullUpdate {
    /// Версия, которая стоит сейчас (`fromVersion`).
    pub from_version: String,
    /// Файловый манифест целевой версии.
    pub files_manifest: Option<Value>,
    /// Сервис проверки файлов (`integrityService`).
    pub integrity: Option<Arc<dyn IntegrityCheck>>,
}

/// `install(request)`.
#[derive(Clone)]
pub struct InstallRequest {
    /// Манифест игры (как в канале или встроенный).
    pub manifest: Value,
    pub manifest_sha256: Option<String>,
    pub archive_path: PathBuf,
    pub files_manifest: Option<Value>,
    pub published_at: Option<String>,
    pub update: Option<FullUpdate>,
}

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct InstallOptions {
    pub archive_service: Option<ArchiveService>,
    pub logger: Option<Arc<dyn Logger>>,
    /// `spaceMargin`: `None` — [`SPACE_MARGIN`].
    pub space_margin: Option<u64>,
    /// `launcherVersion`: `None` или пустая — `0.0.0`.
    pub launcher_version: Option<String>,
    /// `metadataNow`: «сейчас» для проверки метаданных сборки, мс.
    pub metadata_now: Option<f64>,
    pub fault_injector: Option<FaultInjector>,
    /// Свободное место (`freeSpaceBytes`): по умолчанию — ОС с кешем на 5 с.
    pub free_space: Option<Arc<dyn FreeSpace>>,
    /// Часы (`Date.now()`); для тестов.
    pub clock: Option<Clock>,
}

/// То, что нужно обмену каталогов и записи `installed.json` (`ctx` в 0.4.7): так же их зовёт
/// импорт `.hlsrpack`.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallTarget {
    pub game_id: String,
    /// `manifest.version`.
    pub version: Value,
    pub install_path: PathBuf,
    pub staging_path: PathBuf,
    pub published_at: Option<String>,
    pub manifest_sha256: Option<String>,
}

/// Параметры `cleanupOrphanStaging`.
#[derive(Debug, Clone, Copy, Default)]
pub struct OrphanOptions {
    /// `minAgeMs`: `None` — [`ORPHAN_STAGING_MIN_AGE_MS`].
    pub min_age_ms: Option<f64>,
    /// `now`, мс: `None` или 0 — часы сервиса.
    pub now: Option<f64>,
}

/// Итог `recoverFullUpdate`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recovery {
    pub recovered: bool,
    /// `forward` (новая версия уже записана), `back` (возвращена старая), `kept` (дерево оставлено).
    pub direction: Option<&'static str>,
    pub phase: Option<Value>,
    /// `version`: новая версия при `forward`.
    pub version: Option<Value>,
    /// `restoredVersion` при `back`.
    pub restored_version: Option<Value>,
    pub needs_verify: bool,
}

impl Recovery {
    /// `{recovered, direction, phase, version | restoredVersion | needsVerify}`.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("recovered".to_owned(), json!(self.recovered));
        if let Some(direction) = self.direction {
            out.insert("direction".to_owned(), json!(direction));
        }
        for (key, value) in [
            ("phase", &self.phase),
            ("version", &self.version),
            ("restoredVersion", &self.restored_version),
        ] {
            if let Some(value) = value {
                out.insert(key.to_owned(), value.clone());
            }
        }
        if self.needs_verify {
            out.insert("needsVerify".to_owned(), json!(true));
        }
        Value::Object(out)
    }
}

/// Итог `cleanupPending`.
#[derive(Debug, Clone, PartialEq)]
pub enum Cleanup {
    /// Незавершённое обновление полной сборкой — восстановлено (`recoverFullUpdate`).
    Recovered(Box<Recovery>),
    /// `{removed}`: журнал установки был и убран вместе с её staging.
    Removed(bool),
}

impl Cleanup {
    pub fn to_json(&self) -> Value {
        match self {
            Cleanup::Recovered(recovery) => recovery.to_json(),
            Cleanup::Removed(removed) => json!({ "removed": removed }),
        }
    }
}

// ---------------------------------------------------------------- сервис

/// Идущая установка (`this.current`).
struct Current {
    game_id: String,
    cancel: Arc<AtomicBool>,
    extract: Option<CancelHandle>,
}

/// Идущая установка: при выходе из `install` (и при брошенной задаче) — `finally` 0.4.7:
/// снять `install.lock`, сервис свободен.
struct Running<'a>(&'a InstallService);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.release_lock();
        *self.0.current() = None;
    }
}

/// `ctx` установки.
struct Ctx {
    target: InstallTarget,
    manifest: Value,
    archive_path: PathBuf,
    update: Option<FullUpdate>,
    files_manifest: Option<Value>,
    build_metadata_snapshot: Option<Map<String, Value>>,
    rollback_path: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
}

/// `InstallService`.
pub struct InstallService {
    paths: LibraryPaths,
    archive: ArchiveService,
    logger: Arc<dyn Logger>,
    space_margin: u64,
    launcher_version: String,
    metadata_now: Option<f64>,
    fault_injector: Option<FaultInjector>,
    free_space: Arc<dyn FreeSpace>,
    clock: Clock,
    current: Mutex<Option<Current>>,
    listeners: Arc<Mutex<Vec<Listener>>>,
}

pub(crate) fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `path.join(...)` по правилам этой ОС.
pub(crate) fn join(parts: &[&str]) -> PathBuf {
    PathBuf::from(NodePath::native().join(parts))
}

/// `assertInside(root, candidate, what)`: путь после `resolve`.
pub(crate) fn assert_inside(
    root: &Path,
    candidate: &Path,
    what: &str,
) -> Result<PathBuf, DeliveryError> {
    assert_inside_with(
        &NodePath::native(),
        &text(root),
        &text(candidate),
        Some(what),
    )
    .map(PathBuf::from)
}

/// `path.basename(p)`.
fn basename(path: &str) -> String {
    NodePath::native().basename(path)
}

/// `fs.renameSync(from, to)`.
pub(crate) fn rename(from: &Path, to: &Path) -> Result<(), RawError> {
    std::fs::rename(from, to).map_err(|e| RawError::fs_with_dest(&e, "rename", from, Some(to)))
}

/// `fs.existsSync(value)`: не строка — `false`.
pub(crate) fn exists_value(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|path| !path.is_empty() && Path::new(path).exists())
}

/// `x || null`.
pub(crate) fn or_null(value: Option<&Value>) -> Value {
    value
        .filter(|v| js::truthy(v))
        .cloned()
        .unwrap_or(Value::Null)
}

/// `a === b` для значений JSON (`undefined` — `None`): объекты и массивы — разные.
pub(crate) fn strict_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x == y,
        (Some(Value::String(x)), Some(Value::String(y))) => x == y,
        (Some(x @ Value::Number(_)), Some(y @ Value::Number(_))) => jv::number(x) == jv::number(y),
        _ => false,
    }
}

/// Положить поле, если значение есть (`undefined` в JSON не попадает).
pub(crate) fn put(map: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), value.clone());
    }
}

/// Аргумент `path` Node, который обязан быть строкой (`path.join`, `path.basename`):
/// иначе `TypeError [ERR_INVALID_ARG_TYPE]`.
pub(crate) fn path_arg(value: Option<&Value>) -> Result<String, RawError> {
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

/// Строки списка (`preservePaths` и т. п.); не массив — пусто.
fn strings(value: Option<&Value>) -> Vec<String> {
    jv::array(value)
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Перевод ошибки чтения архива при подсчёте sha256 (`hashFile`, обработчик `error`): блокировка
/// антивирусом в Windows — `ANTIVIRUS_BLOCKED` с именем файла, прочее — `INSTALL_FAILED`.
pub fn hash_read_error(file_path: &Path, error: &RawError, platform: Platform) -> DeliveryError {
    if is_antivirus_block(error, platform) {
        return antivirus_error(Some(&text(file_path)), &error.message);
    }
    DeliveryError::new(
        Code::InstallFailed,
        format!("Не удалось прочитать архив: {}", error.message),
    )
}

/// `manifest.gameId` для `gameInstallPath`: не строка — `PATH_INVALID`, как у
/// `String(gameId || "")`, который не проходит проверку (у не строки, похожей на `gameId`, в
/// 0.4.7 дальше был бы `TypeError` в `path.join`).
fn manifest_game_id(manifest: &Value) -> Result<String, DeliveryError> {
    let game_id = jv::prop(manifest, "gameId");
    match game_id {
        Some(Value::String(id)) => Ok(id.clone()),
        other => Err(DeliveryError::new(
            Code::PathInvalid,
            format!("Недопустимый gameId: {}", jv::template(other)),
        )),
    }
}

/// `process.kill(pid, 0)` без исключения: процесс есть или чужой (`EPERM`).
fn process_alive(pid: Option<&Value>) -> bool {
    let Some(pid) = pid.filter(|pid| js::truthy(pid)) else {
        return false;
    };
    match js::process::kill_zero(jv::to_number(pid)) {
        Ok(()) => true,
        Err(code) => code == "EPERM",
    }
}

impl InstallService {
    /// `new InstallService({ paths, … })`.
    pub fn new(paths: LibraryPaths, options: InstallOptions) -> InstallService {
        let logger = options
            .logger
            .unwrap_or_else(|| Arc::new(TracingLogger) as Arc<dyn Logger>);
        InstallService {
            archive: options
                .archive_service
                .unwrap_or_else(|| ArchiveService::with_default_bin(logger.clone())),
            logger,
            space_margin: options.space_margin.unwrap_or(SPACE_MARGIN),
            launcher_version: options
                .launcher_version
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "0.0.0".to_owned()),
            metadata_now: options
                .metadata_now
                .filter(|now| *now != 0.0 && !now.is_nan()),
            fault_injector: options.fault_injector,
            free_space: options.free_space.unwrap_or_else(|| {
                Arc::new(FreeSpaceCache::new(Arc::new(SystemFreeSpace))) as Arc<dyn FreeSpace>
            }),
            clock: options.clock.unwrap_or_else(system_clock),
            current: Mutex::new(None),
            listeners: Arc::new(Mutex::new(Vec::new())),
            paths,
        }
    }

    pub fn paths(&self) -> &LibraryPaths {
        &self.paths
    }

    /// `on("state", listener)`: слушатель зовётся сразу, в той же точке кода, где рождается событие.
    pub fn on_event(&self, listener: impl Fn(&InstallEvent) + Send + Sync + 'static) {
        self.listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::new(listener));
    }

    fn emit(&self, event: &InstallEvent) {
        emit_to(&self.listeners, event);
    }

    fn now_ms(&self) -> i64 {
        (self.clock)()
    }

    fn now_iso(&self) -> String {
        iso_from_ms(self.now_ms())
    }

    fn current(&self) -> std::sync::MutexGuard<'_, Option<Current>> {
        self.current.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn fault(&self, name: &str, operation: &Map<String, Value>) -> Result<(), Failure> {
        match &self.fault_injector {
            Some(injector) => injector(name, &Value::Object(operation.clone())),
            None => Ok(()),
        }
    }

    /// `isBusy()`.
    pub fn is_busy(&self) -> bool {
        self.current().is_some()
    }

    // ------------------------------------------------------------ блокировка

    /// `lockPath()`: `.hlsr/state/install.lock`.
    pub fn lock_path(&self) -> PathBuf {
        join(&[&text(&self.paths.state), LOCK_FILE])
    }

    /// `acquireLock(gameId)`: чужая живая блокировка — `ALREADY_RUNNING`; устаревшая или своя —
    /// снимается. Файл пишется как есть (`JSON.stringify(…, null, 2)`); ошибка записи — ошибка `fs`.
    pub fn acquire_lock(&self, game_id: &str) -> Result<(), Failure> {
        let file = self.lock_path();
        if file.exists() {
            // JSON.parse(fs.readFileSync(file, "utf8")): BOM не срезается, ошибка — null.
            let existing = js::fs::read_file(&file)
                .ok()
                .and_then(|bytes| json_parse(&js::utf8_string(&bytes)).ok())
                .unwrap_or(Value::Null);
            let pid = jv::prop(&existing, "pid");
            let own = pid.and_then(jv::number) == Some(f64::from(std::process::id()));
            if js::truthy(&existing) && process_alive(pid) && !own {
                let mut details = Map::new();
                put(&mut details, "gameId", jv::prop(&existing, "gameId"));
                put(&mut details, "pid", pid);
                return Err(DeliveryError::new(
                    Code::AlreadyRunning,
                    "Другая установка уже выполняется",
                )
                .with_details(Value::Object(details))
                .into());
            }
            if js::truthy(&existing) {
                self.logger
                    .warn("[game-delivery] найдена устаревшая блокировка установки, снимаю");
            }
        }
        let lock = json!({
            "pid": std::process::id(),
            "gameId": game_id,
            "host": js::process::hostname(),
            "startedAt": self.now_iso(),
        });
        node_fs::write_bytes(&file, stringify(&lock, "  ").as_bytes())?;
        Ok(())
    }

    /// `releaseLock()`.
    pub fn release_lock(&self) {
        let file = self.lock_path();
        if file.exists()
            && let Err(error) = node_fs::unlink(&file)
        {
            self.logger.warn(&format!(
                "[game-delivery] не удалось снять блокировку: {}",
                error.message
            ));
        }
    }

    // ------------------------------------------------------------ журнал операции

    /// `operationPath(gameId)`: `.hlsr/state/<id>.operation.json`.
    pub fn operation_path(&self, game_id: &str) -> PathBuf {
        join(&[
            &text(&self.paths.state),
            &format!("{game_id}.operation.json"),
        ])
    }

    /// `readOperation(gameId)`: журнал версии 1 или `None`.
    pub fn read_operation(&self, game_id: &str) -> Option<Value> {
        let data = read_json_safe(&self.operation_path(game_id));
        let version = jv::prop(&data, "version").and_then(jv::number);
        (js::truthy(&data) && version == Some(f64::from(OPERATION_SCHEMA_VERSION))).then_some(data)
    }

    /// `writeOperation(operation)`: `{version: 1, …}` с отступом 2 без перевода строки, не атомарно.
    fn write_operation(&self, operation: &Map<String, Value>) -> Result<(), RawError> {
        node_fs::mkdir_recursive(&self.paths.state)?;
        let mut out = Map::new();
        out.insert("version".to_owned(), json!(OPERATION_SCHEMA_VERSION));
        for (key, value) in operation {
            out.insert(key.clone(), value.clone());
        }
        let game_id = jv::template(operation.get("gameId"));
        node_fs::write_bytes(
            &self.operation_path(&game_id),
            stringify(&Value::Object(out), "  ").as_bytes(),
        )
    }

    /// `updateOperation(operation, patch)`: поля, `updatedAt`, запись.
    fn update_operation(
        &self,
        operation: &mut Map<String, Value>,
        patch: &[(&str, Value)],
    ) -> Result<(), RawError> {
        for (key, value) in patch {
            operation.insert((*key).to_owned(), value.clone());
        }
        operation.insert("updatedAt".to_owned(), json!(self.now_iso()));
        self.write_operation(operation)
    }

    fn set_phase(&self, operation: &mut Map<String, Value>, phase: &str) -> Result<(), RawError> {
        self.update_operation(operation, &[("phase", json!(phase))])
    }

    /// `clearOperation(gameId)`.
    pub fn clear_operation(&self, game_id: &str) {
        let file = self.operation_path(game_id);
        if file.exists()
            && let Err(error) = node_fs::unlink(&file)
        {
            self.logger.warn(&format!(
                "[game-delivery] не удалось удалить журнал операции: {}",
                error.message
            ));
        }
    }

    /// `listPendingOperations()`: незавершённые операции по журналам в `.hlsr/state` (журнал
    /// `null` в 0.4.7 — `TypeError`, он и возвращается).
    pub fn list_pending_operations(&self) -> Result<Vec<Value>, Failure> {
        let Ok(names) = js::fs::read_dir_names(&self.paths.state) else {
            return Ok(Vec::new());
        };
        let mut pending = Vec::new();
        for name in names.iter().filter(|n| n.ends_with(".operation.json")) {
            let file = join(&[&text(&self.paths.state), name]);
            let Some(data) = js::fs::read_file(&file)
                .ok()
                .and_then(|bytes| json_parse(&js::utf8_string(&bytes)).ok())
            else {
                continue;
            };
            if data.is_null() {
                return Err(jv::read_error(Some(&data), "stagingPath").into());
            }
            let field = |name: &str| jv::prop(&data, name);
            let mut item = Map::new();
            put(&mut item, "gameId", field("gameId"));
            item.insert(
                "kind".to_owned(),
                field("kind")
                    .filter(|v| js::truthy(v))
                    .cloned()
                    .unwrap_or_else(|| json!("install")),
            );
            put(&mut item, "phase", field("phase"));
            item.insert("fromVersion".to_owned(), or_null(field("fromVersion")));
            item.insert("toVersion".to_owned(), or_null(field("toVersion")));
            item.insert("stagingPath".to_owned(), or_null(field("stagingPath")));
            item.insert(
                "stagingExists".to_owned(),
                json!(exists_value(field("stagingPath"))),
            );
            item.insert("archivePath".to_owned(), or_null(field("archivePath")));
            item.insert(
                "archiveExists".to_owned(),
                json!(exists_value(field("archivePath"))),
            );
            put(&mut item, "startedAt", field("startedAt"));
            put(&mut item, "updatedAt", field("updatedAt"));
            pending.push(Value::Object(item));
        }
        Ok(pending)
    }

    /// `cleanupPending(gameId)`: обновление полной сборкой — восстановить, установку — убрать
    /// её staging и журнал.
    pub fn cleanup_pending(&self, game_id: &str) -> Result<Cleanup, Failure> {
        if let Some(raw) = self.read_operation(game_id)
            && jv::is_str(jv::prop(&raw, "kind"), "full-update")
        {
            return self
                .recover_full_update(game_id)
                .map(|recovery| Cleanup::Recovered(Box::new(recovery)));
        }
        let pending = self.list_pending_operations()?;
        let Some(operation) = pending
            .iter()
            .find(|item| jv::is_str(jv::prop(item, "gameId"), game_id))
        else {
            return Ok(Cleanup::Removed(false));
        };
        let staging = jv::prop(operation, "stagingPath");
        if jv::truthy(staging) && exists_value(staging) {
            let staging = staging.and_then(Value::as_str).unwrap_or_default();
            remove_within_zone(&self.paths.staging, Path::new(staging))?;
        }
        self.clear_operation(game_id);
        Ok(Cleanup::Removed(true))
    }

    /// `cleanupOrphanStaging(options)`: убрать из `.hlsr/staging` то, на что не ссылается ни один
    /// журнал и что старше `minAgeMs`; во время установки — ничего. Возвращает число убранного.
    pub fn cleanup_orphan_staging(&self, options: OrphanOptions) -> usize {
        let min_age = options
            .min_age_ms
            .unwrap_or(ORPHAN_STAGING_MIN_AGE_MS as f64);
        let now = options
            .now
            .filter(|now| *now != 0.0 && !now.is_nan())
            .unwrap_or(self.now_ms() as f64);
        if self.is_busy() {
            return 0;
        }
        let Ok(names) = js::fs::read_dir_names(&self.paths.staging) else {
            return 0;
        };
        let known = self.staging_names_in_use();
        let mut removed = 0;
        for name in names {
            if known.contains(&name) {
                continue;
            }
            let target = join(&[&text(&self.paths.staging), &name]);
            let Ok(age) = std::fs::metadata(&target).and_then(|meta| js::fs::mtime_ms(&meta))
            else {
                continue;
            };
            if now - age < min_age {
                continue;
            }
            match remove_within_zone(&self.paths.staging, &target) {
                Ok(()) => removed += 1,
                Err(error) => self.logger.warn(&format!(
                    "[game-delivery] не удалось убрать забытый staging {name}: {error}"
                )),
            }
        }
        if removed > 0 {
            self.logger.info(&format!(
                "[game-delivery] убрано забытых каталогов staging: {removed}"
            ));
        }
        removed
    }

    /// `stagingNamesInUse()`: имена из `stagingPath`, `rollbackPath`, `mirrorPath` всех журналов.
    pub fn staging_names_in_use(&self) -> HashSet<String> {
        let mut known = HashSet::new();
        let Ok(names) = js::fs::read_dir_names(&self.paths.state) else {
            return known;
        };
        for name in names.iter().filter(|n| n.ends_with(".json")) {
            let data = read_json_safe(&join(&[&text(&self.paths.state), name]));
            if !js::truthy(&data) {
                continue;
            }
            for key in ["stagingPath", "rollbackPath", "mirrorPath"] {
                if let Some(value) = jv::prop(&data, key).and_then(Value::as_str)
                    && !value.is_empty()
                {
                    known.insert(basename(value));
                }
            }
        }
        known
    }

    // ------------------------------------------------------------ installed.json

    /// `readInstalled(gameId)`: `installed.json` версии схемы 1 в состоянии `ready`; `installPath` —
    /// папка, где он лежит (библиотеку могли перенести). Недопустимый `gameId` — `PATH_INVALID`.
    pub fn read_installed(&self, game_id: &str) -> Result<Option<Value>, DeliveryError> {
        let install_path = game_install_path(&self.paths, game_id)?;
        let file = install_path.join(INSTALLED_FILE);
        let data = js::fs::read_file(&file)
            .ok()
            .and_then(|bytes| json_parse(&js::utf8_string(&bytes)).ok());
        let Some(mut data) = data.filter(|data| {
            jv::prop(data, "schemaVersion").and_then(jv::number)
                == Some(f64::from(INSTALLED_SCHEMA_VERSION))
                && jv::is_str(jv::prop(data, "state"), "ready")
        }) else {
            return Ok(None);
        };
        data["installPath"] = json!(text(&install_path));
        Ok(Some(data))
    }

    /// `fillInstalled(gameId, patch)`: дописать поля, которых нет или которые `null` (кроме
    /// защищённых). Нечего дописывать — `installed.json` не трогается.
    pub fn fill_installed(&self, game_id: &str, patch: &Value) -> Result<Option<Value>, Failure> {
        const PROTECTED: [&str; 10] = [
            "schemaVersion",
            "gameId",
            "version",
            "state",
            "installPath",
            "manifestSha256",
            "archiveSha256",
            "installedAt",
            "buildMetadata",
            "buildMetadataSha256",
        ];
        let Some(current) = self.read_installed(game_id)? else {
            return Ok(None);
        };
        let mut additions = Vec::new();
        if let Some(map) = patch.as_object() {
            for (key, value) in js_key_order(map) {
                if PROTECTED.contains(&key.as_str()) {
                    continue;
                }
                if current.get(key).is_some_and(|v| !v.is_null()) || value.is_null() {
                    continue;
                }
                additions.push((key.clone(), value.clone()));
            }
        }
        if additions.is_empty() {
            return Ok(Some(current));
        }
        let mut updated = current;
        for (key, value) in additions {
            updated[key] = value;
        }
        self.rewrite_installed(&updated)?;
        Ok(Some(updated))
    }

    /// `replaceBuildMetadataSnapshot(gameId, snapshot)`.
    pub fn replace_build_metadata_snapshot(
        &self,
        game_id: &str,
        snapshot: &Value,
    ) -> Result<Option<Value>, Failure> {
        let Some(current) = self.read_installed(game_id)? else {
            return Ok(None);
        };
        if !js::truthy(snapshot) || !jv::truthy(jv::prop(snapshot, "buildMetadata")) {
            return Ok(None);
        }
        let mut updated = current;
        updated["buildMetadata"] = snapshot["buildMetadata"].clone();
        updated["buildMetadataSha256"] = or_null(jv::prop(snapshot, "buildMetadataSha256"));
        updated["buildMetadataVerifiedAt"] = or_null(jv::prop(snapshot, "buildMetadataVerifiedAt"));
        self.rewrite_installed(&updated)?;
        Ok(Some(updated))
    }

    /// `writeJsonAtomic(path.join(current.installPath, INSTALLED_FILE), updated)`.
    fn rewrite_installed(&self, installed: &Value) -> Result<(), RawError> {
        let dir = jv::template(jv::prop(installed, "installPath"));
        write_json_atomic(&join(&[&dir, INSTALLED_FILE]), installed)
    }

    /// `writeInstalled(ctx, archiveSha256, extra)`: `installed.json` атомарно (два пробела и `\n`).
    pub fn write_installed(
        &self,
        target: &InstallTarget,
        archive_sha256: Value,
        extra: Option<&Map<String, Value>>,
    ) -> Result<Value, RawError> {
        let mut installed = Map::new();
        installed.insert("schemaVersion".to_owned(), json!(INSTALLED_SCHEMA_VERSION));
        installed.insert("gameId".to_owned(), json!(target.game_id));
        installed.insert("version".to_owned(), target.version.clone());
        installed.insert("installedAt".to_owned(), json!(self.now_iso()));
        installed.insert(
            "publishedAt".to_owned(),
            json!(target.published_at.as_deref().filter(|p| !p.is_empty())),
        );
        installed.insert(
            "manifestSha256".to_owned(),
            json!(target.manifest_sha256.as_deref().filter(|s| !s.is_empty())),
        );
        installed.insert("installPath".to_owned(), json!(text(&target.install_path)));
        installed.insert("archiveSha256".to_owned(), archive_sha256);
        for (key, value) in extra.into_iter().flatten() {
            installed.insert(key.clone(), value.clone());
        }
        installed.insert("state".to_owned(), json!("ready"));
        let installed = Value::Object(installed);
        write_json_atomic(&target.install_path.join(INSTALLED_FILE), &installed)?;
        Ok(installed)
    }

    // ------------------------------------------------------------ отмена

    /// `cancel()`: просьба остановить установку; идущая распаковка останавливается сразу.
    pub fn cancel(&self) -> bool {
        let current = self.current();
        let Some(current) = current.as_ref() else {
            return false;
        };
        current.cancel.store(true, Ordering::SeqCst);
        if let Some(extract) = &current.extract {
            extract.cancel();
        }
        true
    }

    fn set_extract(&self, handle: Option<CancelHandle>) {
        if let Some(current) = self.current().as_mut() {
            current.extract = handle;
        }
    }

    fn throw_if_cancelled(ctx: &Ctx) -> Result<(), DeliveryError> {
        if ctx.cancel.load(Ordering::SeqCst) {
            return Err(DeliveryError::new(Code::Cancelled, "Установка отменена"));
        }
        Ok(())
    }

    // ------------------------------------------------------------ установка

    /// `install(request)`: установка или обновление полной сборкой. Итог — объект `installed.json`.
    /// Ошибки — `DeliveryError`; ошибка записи `install.lock` — как пришла из `fs`. Брошенная
    /// задача (future) отпускает блокировку, как `finally` в 0.4.7; staging и журнал остаются —
    /// их уберёт `cleanup_pending` или `cleanup_orphan_staging`, как после сбоя процесса.
    pub async fn install(&self, request: InstallRequest) -> Result<Value, Failure> {
        let (mut ctx, mut operation, running) = self.begin(request)?;
        let result = match self.write_operation(&operation) {
            Ok(()) => self.run_install(&mut ctx, &mut operation).await,
            Err(error) => Err(error.into()),
        };
        let outcome = match result {
            Ok(installed) => {
                self.clear_operation(&ctx.target.game_id);
                Ok(installed)
            }
            Err(error) => Err(self.fail_install(&ctx, &operation, error)),
        };
        // finally: блокировка снята, установка закончена.
        drop(running);
        outcome
    }

    /// Начало `install` до `try`: проверка «уже идёт», пути, журнал, `install.lock`. Проверка и
    /// захват — под одной блокировкой `current`, чтобы две установки из разных потоков не прошли
    /// обе (в 0.4.7 поток один).
    fn begin(
        &self,
        request: InstallRequest,
    ) -> Result<(Ctx, Map<String, Value>, Running<'_>), Failure> {
        let mut current = self.current();
        if let Some(running) = current.as_ref() {
            return Err(
                DeliveryError::new(Code::AlreadyRunning, "Установка уже выполняется")
                    .with_details(json!({ "gameId": running.game_id }))
                    .into(),
            );
        }
        let manifest = request.manifest;
        let game_id = manifest_game_id(&manifest)?;
        let install_path = game_install_path(&self.paths, &game_id)?;
        let staging_name = format!(
            "{game_id}-{}-{}",
            js::base36(u128::try_from(self.now_ms()).unwrap_or(0)),
            random_hex4()
        );
        let staging = text(&self.paths.staging);
        let staging_path = assert_inside(
            &self.paths.staging,
            &join(&[&staging, &staging_name]),
            "Папка staging",
        )?;
        let rollback_path = match &request.update {
            Some(_) => {
                let rollback = text(&self.paths.rollback);
                Some(assert_inside(
                    &self.paths.rollback,
                    &join(&[&rollback, &staging_name]),
                    "Папка rollback",
                )?)
            }
            None => None,
        };
        let published_at = request.published_at.filter(|p| !p.is_empty()).or_else(|| {
            jv::prop(&manifest, "publishedAt")
                .filter(|p| js::truthy(p))
                .map(|p| jv::template(Some(p)))
        });
        let files_manifest = request.files_manifest.filter(js::truthy).or_else(|| {
            request
                .update
                .as_ref()
                .and_then(|u| u.files_manifest.clone())
                .filter(js::truthy)
        });
        let ctx = Ctx {
            target: InstallTarget {
                game_id: game_id.clone(),
                version: manifest.get("version").cloned().unwrap_or(Value::Null),
                install_path: install_path.clone(),
                staging_path: staging_path.clone(),
                published_at,
                manifest_sha256: request.manifest_sha256,
            },
            archive_path: request.archive_path,
            update: request.update,
            files_manifest,
            build_metadata_snapshot: None,
            rollback_path,
            cancel: Arc::new(AtomicBool::new(false)),
            manifest,
        };

        let mut operation = Map::new();
        operation.insert("gameId".to_owned(), json!(game_id));
        let kind = if ctx.update.is_some() {
            "full-update"
        } else {
            "install"
        };
        operation.insert("kind".to_owned(), json!(kind));
        operation.insert(
            "fromVersion".to_owned(),
            json!(ctx.update.as_ref().map(|u| u.from_version.clone())),
        );
        put(&mut operation, "toVersion", ctx.manifest.get("version"));
        operation.insert("phase".to_owned(), json!("starting"));
        operation.insert("stagingPath".to_owned(), json!(text(&staging_path)));
        operation.insert("installPath".to_owned(), json!(text(&install_path)));
        operation.insert(
            "rollbackPath".to_owned(),
            json!(ctx.rollback_path.as_deref().map(text)),
        );
        operation.insert("archivePath".to_owned(), json!(text(&ctx.archive_path)));
        operation.insert("startedAt".to_owned(), json!(self.now_iso()));
        operation.insert("updatedAt".to_owned(), json!(self.now_iso()));

        self.acquire_lock(&game_id)?;
        *current = Some(Current {
            game_id,
            cancel: ctx.cancel.clone(),
            extract: None,
        });
        Ok((ctx, operation, Running(self)))
    }

    /// `catch` установки: диагностика, уборка staging и rollback, событие `error`/`cancelled`.
    fn fail_install(&self, ctx: &Ctx, operation: &Map<String, Value>, error: Failure) -> Failure {
        let delivery = to_delivery_error(error, Some(Code::InstallFailed));
        self.record_failure(ctx, operation, &delivery);
        let game_id = &ctx.target.game_id;
        if delivery.code == Code::RollbackFailed {
            let mut event = InstallEvent::new(game_id, InstallState::Error);
            event.error = Some(delivery.to_public());
            self.emit(&event);
            return delivery.into();
        }
        let cleanup = || -> Result<(), Failure> {
            if ctx.target.staging_path.exists() {
                remove_within_zone(&self.paths.staging, &ctx.target.staging_path)?;
            }
            if let Some(rollback) = &ctx.rollback_path
                && rollback.exists()
            {
                remove_within_zone(&self.paths.rollback, rollback)?;
            }
            self.clear_operation(game_id);
            Ok(())
        };
        if let Err(error) = cleanup() {
            self.logger.error(&format!(
                "[game-delivery] не удалось убрать staging {}: {error}",
                ctx.target.staging_path.display()
            ));
        }
        let state = if delivery.code == Code::Cancelled {
            InstallState::Cancelled
        } else {
            InstallState::Error
        };
        let mut event = InstallEvent::new(game_id, state);
        event.error = Some(delivery.to_public());
        self.emit(&event);
        delivery.into()
    }

    /// `runInstall(ctx, operation)`.
    async fn run_install(
        &self,
        ctx: &mut Ctx,
        operation: &mut Map<String, Value>,
    ) -> Result<Value, Failure> {
        let game_id = ctx.target.game_id.clone();
        let manifest = ctx.manifest.clone();
        let field = |object: &str, name: &str| {
            jv::prop(&manifest, object).and_then(|value| jv::prop(value, name))
        };

        self.set_phase(operation, "checking")?;
        self.emit(&InstallEvent::new(&game_id, InstallState::Preparing));
        self.check_writable()?;
        let expected = jv::prop(&manifest, "expectedInstalledSize")
            .and_then(jv::number)
            .unwrap_or(0.0);
        let user_data = self.user_data_bytes(&game_id, &ctx.target.install_path, &manifest);
        self.check_space_with(expected, user_data)?;
        Self::throw_if_cancelled(ctx)?;

        self.set_phase(operation, "verifying")?;
        self.emit(&InstallEvent::new(&game_id, InstallState::Verifying));
        let archive_sha256 = self.verify_archive(ctx).await?;
        Self::throw_if_cancelled(ctx)?;

        self.set_phase(operation, "inspecting")?;
        let root_prefix = field("baseArchive", "rootPrefix")
            .filter(|p| js::truthy(p))
            .map(|p| jv::template(Some(p)));
        let inspection = self
            .archive
            .inspect(
                &ctx.archive_path,
                &InspectArgs {
                    max_entries: field("filesManifest", "maxEntries").and_then(jv::number),
                    max_uncompressed_size: field("filesManifest", "maxUncompressedSize")
                        .and_then(jv::number),
                    root_prefix: root_prefix.clone(),
                },
            )
            .await?;
        Self::check_staging_containment(ctx, &inspection.listing.entries)?;
        Self::check_expected_size(&manifest, inspection.report.uncompressed_size)?;
        Self::throw_if_cancelled(ctx)?;

        self.set_phase(operation, "extracting")?;
        let mut event = InstallEvent::new(&game_id, InstallState::Extracting);
        event.percent = Some(0.0);
        self.emit(&event);
        node_fs::mkdir_recursive(&ctx.target.staging_path)?;
        self.extract(ctx).await?;
        Self::throw_if_cancelled(ctx)?;

        self.set_phase(operation, "validating")?;
        let content_root = Self::resolve_content_root(ctx, root_prefix.as_deref())?;
        Self::check_required_files(&manifest, &content_root)?;
        Self::throw_if_cancelled(ctx)?;
        self.verify_build_metadata(ctx, &content_root)?;
        Self::throw_if_cancelled(ctx)?;

        if ctx.update.is_some() {
            Self::assert_still_installed(ctx)?;
            Self::assert_game_not_running(ctx)?;
            self.strip_library_internals(&content_root)?;
            self.verify_staging_tree(ctx, &content_root).await?;
            Self::throw_if_cancelled(ctx)?;
        }

        self.set_phase(operation, "finalizing")?;
        self.emit(&InstallEvent::new(&game_id, InstallState::Finalizing));
        self.preserve_user_data(ctx, &content_root)?;

        let installed = if ctx.update.is_some() {
            self.commit_update(ctx, operation, &content_root, &archive_sha256)?
        } else {
            self.swap_into_library(&ctx.target, &content_root)?;
            self.write_installed(
                &ctx.target,
                json!(archive_sha256),
                ctx.build_metadata_snapshot.as_ref(),
            )?
        };
        let mut event = InstallEvent::new(&game_id, InstallState::Ready);
        event.installed = Some(installed.clone());
        self.emit(&event);
        Ok(installed)
    }

    /// `verifyBuildMetadata(ctx, contentRoot)`: метаданные сборки сверены с манифестом — снимок
    /// для `installed.json`; необязательных и отсутствующих — ничего.
    fn verify_build_metadata(
        &self,
        ctx: &mut Ctx,
        content_root: &Path,
    ) -> Result<(), DeliveryError> {
        let Some(descriptor) = jv::prop(&ctx.manifest, "buildMetadata").filter(|d| js::truthy(d))
        else {
            return Ok(());
        };
        let mut manifest = ctx.manifest.clone();
        manifest["publishedAt"] = json!(ctx.target.published_at);
        let now = self.metadata_now.unwrap_or(self.now_ms() as f64);
        let options = VerifyForInstall {
            install_root: content_root,
            descriptor,
            manifest: Some(&manifest),
            files_manifest: ctx.files_manifest.as_ref(),
            launcher_version: Some(&self.launcher_version),
            expected: None,
            now: Some(now),
        };
        match build_metadata_trust::verify_for_install(&options) {
            Ok(verified) => {
                let snapshot = build_metadata_trust::snapshot_from_verified(&verified).to_value();
                ctx.build_metadata_snapshot = snapshot.as_object().cloned();
                Ok(())
            }
            Err(error) => {
                let optional = jv::prop(descriptor, "required") == Some(&Value::Bool(false));
                if optional && error.code == Reason::Missing {
                    return Ok(());
                }
                Err(
                    DeliveryError::new(Code::coerce(error.code.as_str()), error.message)
                        .with_details(error.detail.unwrap_or(Value::Null)),
                )
            }
        }
    }

    /// `assertStillInstalled(ctx)`: обновлять есть что, и стоит та версия, для которой готовились.
    fn assert_still_installed(ctx: &Ctx) -> Result<(), DeliveryError> {
        let from = ctx
            .update
            .as_ref()
            .map(|u| u.from_version.clone())
            .unwrap_or_default();
        let data = read_json_safe(&ctx.target.install_path.join(INSTALLED_FILE));
        if !js::truthy(&data) || !jv::is_str(jv::prop(&data, "state"), "ready") {
            return Err(DeliveryError::new(
                Code::NotInstalled,
                "Игра не установлена — обновлять нечего",
            )
            .with_details(json!({ "gameId": ctx.target.game_id })));
        }
        let version = jv::prop(&data, "version");
        if !jv::is_str(version, &from) {
            let mut details = Map::new();
            details.insert("expected".to_owned(), json!(from));
            put(&mut details, "installed", version);
            return Err(DeliveryError::new(
                Code::VersionMismatch,
                format!(
                    "Обновление готовилось для версии {from}, установлена {}",
                    jv::template(version)
                ),
            )
            .with_details(Value::Object(details)));
        }
        Ok(())
    }

    /// `assertGameNotRunning(ctx)`.
    fn assert_game_not_running(ctx: &Ctx) -> Result<(), DeliveryError> {
        match file_ops::is_game_running(&ctx.target.install_path, &["hl2.exe"]) {
            Some(path) => Err(DeliveryError::new(
                Code::GameRunning,
                format!("Игра запущена ({path}). Закройте её и повторите обновление."),
            )
            .with_details(json!({ "path": path }))),
            None => Ok(()),
        }
    }

    /// `stripLibraryInternals(contentRoot)`: `installed.json*` из архива — не часть сборки.
    fn strip_library_internals(&self, content_root: &Path) -> Result<(), RawError> {
        let Ok(names) = js::fs::read_dir_names(content_root) else {
            return Ok(());
        };
        for name in names {
            if name != INSTALLED_FILE && !name.starts_with(&format!("{INSTALLED_FILE}.")) {
                continue;
            }
            self.logger.warn(&format!(
                "[game-delivery] в архиве оказался служебный файл {name} — удаляю из staging"
            ));
            file_ops::rm_sync(
                &join(&[&text(content_root), &name]),
                false,
                0,
                Duration::ZERO,
            )?;
        }
        Ok(())
    }

    /// `verifyStagingTree(ctx, contentRoot)`: распакованная сборка совпала с файловым манифестом
    /// целевой версии, лишних файлов нет.
    async fn verify_staging_tree(&self, ctx: &Ctx, content_root: &Path) -> Result<(), Failure> {
        let update = ctx.update.as_ref().expect("обновление");
        let Some(files_manifest) = update.files_manifest.as_ref().filter(|m| js::truthy(m)) else {
            return Err(DeliveryError::new(
                Code::FilesManifestMissing,
                "Нет файлового манифеста целевой версии — сверить распакованную сборку нечем",
            )
            .into());
        };
        let Some(integrity) = &update.integrity else {
            return Err(DeliveryError::new(
                Code::VerifyFailed,
                "Не передан сервис проверки файлов",
            )
            .into());
        };
        self.emit(&InstallEvent::new(
            &ctx.target.game_id,
            InstallState::Verifying,
        ));
        let preserve = strings(jv::prop(&ctx.manifest, "preservePaths"));
        let result = integrity
            .check(IntegrityRequest {
                game_id: &ctx.target.game_id,
                install_path: content_root,
                files_manifest,
                preserve_paths: &preserve,
                stock_configs: false,
            })
            .await?;
        let version = jv::template(jv::prop(files_manifest, "version"));
        if !result.ok {
            let samples: Vec<&String> = result
                .missing
                .iter()
                .chain(&result.corrupted)
                .chain(&result.size_mismatch)
                .take(10)
                .collect();
            return Err(DeliveryError::new(
                Code::VerifyFailed,
                format!(
                    "Распакованная сборка не совпала с манифестом версии {version}: \
                     повреждено или отсутствует файлов — {}",
                    result.damaged
                ),
            )
            .with_details(json!({ "damaged": result.damaged, "samples": samples }))
            .into());
        }
        if !result.extra.is_empty() {
            let shown: Vec<&str> = result.extra.iter().take(5).map(String::as_str).collect();
            let listed: Vec<&String> = result.extra.iter().take(10).collect();
            return Err(DeliveryError::new(
                Code::VerifyFailed,
                format!(
                    "В архиве есть файлы, которых нет в манифесте версии {version}: {}",
                    shown.join(", ")
                ),
            )
            .with_details(json!({ "extra": listed }))
            .into());
        }
        Ok(())
    }

    /// `commitUpdate(ctx, operation, contentRoot, archiveSha256)`: обмен каталогов по фазам;
    /// сбой в процессе — откат на старую версию, неудачный откат — `ROLLBACK_FAILED`.
    fn commit_update(
        &self,
        ctx: &Ctx,
        operation: &mut Map<String, Value>,
        content_root: &Path,
        archive_sha256: &str,
    ) -> Result<Value, Failure> {
        let rollback_path = ctx.rollback_path.as_deref().expect("rollback обновления");
        node_fs::mkdir_recursive(&self.paths.games)?;
        node_fs::mkdir_recursive(rollback_path)?;
        let retired = assert_inside(
            &self.paths.rollback,
            &join(&[&text(rollback_path), "previous"]),
            "Старая версия",
        )?;
        self.update_operation(
            operation,
            &[
                ("phase", json!("swap-retire")),
                ("retiredPath", json!(text(&retired))),
            ],
        )?;
        self.fault("before-retire", operation)?;
        let game_id = &ctx.target.game_id;
        self.park_episodes(game_id, &ctx.target.install_path)?;
        if let Err(error) = rename(&ctx.target.install_path, &retired) {
            self.unpark_episodes(game_id, &ctx.target.install_path);
            return Err(error.into());
        }

        match self.commit_after_retire(ctx, operation, content_root, archive_sha256) {
            Ok(installed) => Ok(installed),
            Err(error) => {
                let delivery = to_delivery_error(error, Some(Code::InstallFailed));
                let install_path = &ctx.target.install_path;
                if !install_path.exists()
                    && retired.exists()
                    && let Err(restore) = rename(&retired, install_path)
                {
                    self.logger.error(&format!(
                        "[game-delivery] откат обновления не удался: {}. Старая версия сохранена в {}",
                        restore.message,
                        retired.display()
                    ));
                    return Err(DeliveryError::new(
                        Code::RollbackFailed,
                        format!(
                            "Обновление не применено и откат не удался: {}. \
                             Файлы прежней версии сохранены в {}",
                            restore.message,
                            retired.display()
                        ),
                    )
                    .with_details(json!({
                        "rollbackPath": text(rollback_path),
                        "original": delivery.to_public(),
                    }))
                    .into());
                }
                if install_path.join(INSTALLED_FILE).exists() {
                    self.logger.warn(&format!(
                        "[game-delivery] обновление {} прервано на фазе {}, старая версия возвращена на место",
                        ctx.target.game_id,
                        jv::template(operation.get("phase"))
                    ));
                }
                self.unpark_episodes(game_id, install_path);
                Err(delivery.into())
            }
        }
    }

    /// Старая версия уже в rollback: новая — на её место.
    fn commit_after_retire(
        &self,
        ctx: &Ctx,
        operation: &mut Map<String, Value>,
        content_root: &Path,
        archive_sha256: &str,
    ) -> Result<Value, Failure> {
        self.fault("after-retire", operation)?;
        self.set_phase(operation, "swap-place")?;
        self.fault("before-place", operation)?;
        rename(content_root, &ctx.target.install_path)?;
        match self.commit_after_place(ctx, operation, archive_sha256) {
            Ok(installed) => Ok(installed),
            Err(error) => {
                self.discard_placed_tree(
                    &ctx.target.install_path,
                    &text(&ctx.target.staging_path),
                )?;
                Err(error)
            }
        }
    }

    /// Новая версия на месте: `installed.json`, конец транзакции, уборка rollback и staging.
    fn commit_after_place(
        &self,
        ctx: &Ctx,
        operation: &mut Map<String, Value>,
        archive_sha256: &str,
    ) -> Result<Value, Failure> {
        self.fault("after-place", operation)?;
        self.set_phase(operation, "installed-write")?;
        self.fault("before-installed-write", operation)?;
        let update = ctx.update.as_ref().expect("обновление");
        let files = update.files_manifest.as_ref().unwrap_or(&Value::Null);
        let mut extra = Map::new();
        extra.insert("migratedFrom".to_owned(), json!(update.from_version));
        extra.insert(
            "filesManifestSha256".to_owned(),
            or_null(jv::prop(files, "manifestSha256")),
        );
        put(&mut extra, "totalFiles", jv::prop(files, "totalFiles"));
        put(&mut extra, "totalSize", jv::prop(files, "totalSize"));
        for (key, value) in ctx.build_metadata_snapshot.iter().flatten() {
            extra.insert(key.clone(), value.clone());
        }
        let installed = self.write_installed(&ctx.target, json!(archive_sha256), Some(&extra))?;
        self.fault("after-installed-write", operation)?;
        self.set_phase(operation, "commit-complete")?;
        self.cleanup_update_zones(ctx);
        self.unpark_episodes(&ctx.target.game_id, &ctx.target.install_path);
        Ok(installed)
    }

    /// `discardPlacedTree(installPath, stagingPath)`: новое дерево — в `<staging>-discard` и удалить.
    fn discard_placed_tree(&self, install_path: &Path, staging_path: &str) -> Result<(), Failure> {
        let discard = assert_inside(
            &self.paths.staging,
            &join(&[
                &text(&self.paths.staging),
                &format!("{}-discard", basename(staging_path)),
            ]),
            "Отменённая новая версия",
        )?;
        rename(install_path, &discard)?;
        remove_within_zone(&self.paths.staging, &discard)?;
        Ok(())
    }

    /// `episodesParkPath(gameId)`: эпизоды (`EPISODE_DIRS`, ~6 ГБ) на время подмены сборки
    /// откладываются сюда — эту папку не чистят ни откат, ни уборка staging — и возвращаются
    /// rename в ту сборку, что стоит после подмены или отката.
    pub fn episodes_park_path(&self, game_id: &str) -> PathBuf {
        join(&[&text(&self.paths.state), "episodes-park", game_id])
    }

    fn aside(&self, path: &Path, kind: &str) -> PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(
            ".{kind}-{}",
            js::base36(self.now_ms().max(0) as u128)
        ));
        PathBuf::from(name)
    }

    /// `parkEpisodes(gameId, installPath)`: эпизоды игры — в park до подмены сборки.
    pub fn park_episodes(&self, game_id: &str, install_path: &Path) -> Result<(), DeliveryError> {
        self.unpark_episodes(game_id, install_path); // хвост прошлой подмены, прерванной крахом
        let park = self.episodes_park_path(game_id);
        let mut moved: Vec<&str> = Vec::new();
        let result = (|| -> Result<(), RawError> {
            for dir in mutable_paths::EPISODE_DIRS {
                let source = install_path.join(dir);
                if !source.exists() {
                    continue;
                }
                let target = park.join(dir);
                // Отложенный раньше не вернулся (в игре уже был свой): он отходит в сторону, а не
                // стопорит обновления.
                if target.exists() {
                    let aside = self.aside(&target, "stale");
                    rename(&target, &aside)?;
                    self.logger.warn(&format!(
                        "[game-delivery] прежняя отложенная копия эпизода {dir} отложена в {}",
                        aside.display()
                    ));
                }
                node_fs::mkdir_recursive(&park)?;
                rename(&source, &target)?;
                moved.push(dir);
            }
            Ok(())
        })();
        if let Err(error) = result {
            for dir in moved {
                if let Err(back) = rename(&park.join(dir), &install_path.join(dir)) {
                    self.logger.error(&format!(
                        "[game-delivery] эпизод {dir} остался в {}: {}",
                        park.display(),
                        back.message
                    ));
                }
            }
            return Err(to_delivery_error(error.into(), Some(Code::InstallFailed)));
        }
        Ok(())
    }

    /// `unparkEpisodes(gameId, installPath)`: отложенные эпизоды — обратно в игру. Не вернулся (в
    /// сборке своя папка, файл занят) — остаётся отложенным, повтор на старте лаунчера. Не бросает.
    pub fn unpark_episodes(&self, game_id: &str, install_path: &Path) {
        let park = self.episodes_park_path(game_id);
        if !park.exists() {
            return;
        }
        for dir in mutable_paths::EPISODE_DIRS {
            let source = park.join(dir);
            if !source.exists() {
                continue;
            }
            let target = install_path.join(dir);
            // Встал эпизод, поставленный лаунчером (маркер внутри), — отложенная копия прежняя: из неё
            // нужны только данные игрока. Чужая папка (своя episodic в сборке) — копия ждёт целиком.
            if target.join(crate::episodes::MARKER).exists() {
                self.merge_parked_episode(&source, &target);
                continue;
            }
            if !install_path.exists() || target.exists() {
                self.logger.warn(&format!(
                    "[game-delivery] эпизод {dir} пока отложен в {}: в {} его некуда вернуть",
                    source.display(),
                    install_path.display()
                ));
                continue;
            }
            if let Err(error) = rename(&source, &target) {
                self.logger.error(&format!(
                    "[game-delivery] эпизод {dir} не вернулся из {}: {}",
                    source.display(),
                    error.message
                ));
            }
        }
        if js::fs::read_dir_names(&park).is_ok_and(|names| names.is_empty()) {
            let _ = std::fs::remove_dir(&park);
        }
    }

    /// `mergeParkedEpisode(source, target)`: папка эпизода уже стоит (подмена эпизода прервалась
    /// после того, как встала новая): данные игрока (`PLAYER_ENTRIES`, демо в корне) переезжают в
    /// неё rename, остальное — контент игры — удаляется. Одноимённое в новой уже есть — копия игрока
    /// уходит в `<dir>.player-<время>`, а не в корзину. Не перенеслось — отложенная копия остаётся
    /// целиком до следующего старта.
    fn merge_parked_episode(&self, source: &Path, target: &Path) {
        let result = (|| -> Result<(), RawError> {
            let scandir = |dir: &Path| {
                js::fs::read_dir_names(dir).map_err(|error| RawError::fs(&error, "scandir", dir))
            };
            let existing: HashSet<String> = scandir(target)?
                .into_iter()
                .map(|name| name.to_lowercase())
                .collect();
            let mut aside: Option<PathBuf> = None;
            for name in scandir(source)? {
                if !crate::episodes::is_player_entry(&name) {
                    continue;
                }
                if !existing.contains(&name.to_lowercase()) {
                    rename(&source.join(&name), &target.join(&name))?;
                    continue;
                }
                let dir = aside
                    .get_or_insert_with(|| self.aside(source, "player"))
                    .clone();
                node_fs::mkdir_recursive(&dir)?;
                rename(&source.join(&name), &dir.join(&name))?;
                self.logger.warn(&format!(
                    "[game-delivery] {name} игрока уже есть в {}: отложенная копия — в {}",
                    target.display(),
                    dir.display()
                ));
            }
            file_ops::rm_sync(source, true, 0, Duration::ZERO)
        })();
        if let Err(error) = result {
            self.logger.error(&format!(
                "[game-delivery] данные игрока из {} не перенесены в {}: {}",
                source.display(),
                target.display(),
                error.message
            ));
        }
    }

    /// `cleanupUpdateZones(ctx)`.
    fn cleanup_update_zones(&self, ctx: &Ctx) {
        let zones = [
            (&self.paths.rollback, ctx.rollback_path.as_deref()),
            (&self.paths.staging, Some(ctx.target.staging_path.as_path())),
        ];
        for (zone, target) in zones {
            let Some(target) = target.filter(|t| t.exists()) else {
                continue;
            };
            if let Err(error) = remove_within_zone(zone, target) {
                self.logger.warn(&format!(
                    "[game-delivery] не удалось убрать {}: {error}",
                    target.display()
                ));
            }
        }
    }

    /// `checkWritable()`: в `Games`, `staging`, `state`, `rollback` можно писать.
    pub fn check_writable(&self) -> Result<(), DeliveryError> {
        for dir in [
            &self.paths.games,
            &self.paths.staging,
            &self.paths.state,
            &self.paths.rollback,
        ] {
            let probe = join(&[
                &text(dir),
                &format!(".hlsr-write-test-{}-{}", std::process::id(), self.now_ms()),
            ]);
            let result = node_fs::mkdir_recursive(dir)
                .and_then(|()| node_fs::write_bytes(&probe, b""))
                .and_then(|()| node_fs::unlink(&probe));
            if let Err(error) = result {
                return Err(DeliveryError::new(
                    Code::PathNotWritable,
                    format!("Нет прав на запись в {}: {}", dir.display(), error.message),
                ));
            }
        }
        Ok(())
    }

    /// `checkSpace(expectedInstalledSize)`: свободного места хватает на сборку и запас; узнать
    /// место не вышло — не проверяется.
    pub fn check_space(&self, expected_installed_size: f64) -> Result<(), DeliveryError> {
        self.check_space_with(expected_installed_size, 0)
    }

    /// `checkSpace(expectedInstalledSize, userDataBytes)`: `user_data_bytes` — копии файлов игрока,
    /// которые лягут в новую сборку рядом со старой.
    pub fn check_space_with(
        &self,
        expected_installed_size: f64,
        user_data_bytes: u64,
    ) -> Result<(), DeliveryError> {
        if expected_installed_size == 0.0 || expected_installed_size.is_nan() {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                "В манифесте не указан expectedInstalledSize — проверка места невозможна",
            ));
        }
        let Some(free) = self.free_space.free_bytes(&self.paths.staging) else {
            return Ok(());
        };
        let required = expected_installed_size + user_data_bytes as f64 + self.space_margin as f64;
        if (free as f64) < required {
            return Err(DeliveryError::new(
                Code::NoSpace,
                format!(
                    "Недостаточно места: нужно {}, свободно {free}",
                    js::number::number_to_string(required)
                ),
            )
            .with_details(json!({ "requiredBytes": js_number(required), "freeBytes": free })));
        }
        Ok(())
    }

    /// `verifyArchive(ctx)`: размер и sha256 архива — как в манифесте.
    async fn verify_archive(&self, ctx: &Ctx) -> Result<String, DeliveryError> {
        let archive = &ctx.archive_path;
        let Ok(meta) = std::fs::metadata(archive) else {
            return Err(DeliveryError::new(
                Code::SizeMismatch,
                format!("Архив не найден: {}", archive.display()),
            ));
        };
        let base = jv::prop(&ctx.manifest, "baseArchive");
        let expected_size = base.and_then(|b| jv::prop(b, "size"));
        let size = meta.len();
        if expected_size.and_then(jv::number) != Some(size as f64)
            || !matches!(expected_size, Some(Value::Number(_)))
        {
            let mut details = Map::new();
            put(&mut details, "expected", expected_size);
            details.insert("actual".to_owned(), json!(size));
            return Err(DeliveryError::new(
                Code::SizeMismatch,
                format!(
                    "Размер архива {size} не совпадает с манифестом {}",
                    jv::template(expected_size)
                ),
            )
            .with_details(Value::Object(details)));
        }
        let digest = hash_file(&ctx.cancel, archive).await?;
        let expected_sha = base.and_then(|b| jv::prop(b, "sha256"));
        if !jv::is_str(expected_sha, &digest) {
            let mut details = Map::new();
            put(&mut details, "expected", expected_sha);
            details.insert("actual".to_owned(), json!(digest));
            return Err(DeliveryError::new(
                Code::ChecksumMismatch,
                "SHA-256 архива не совпадает с манифестом",
            )
            .with_details(Value::Object(details)));
        }
        Ok(digest)
    }

    /// `checkStagingContainment(ctx, entries)`: ни одна запись не выходит из staging.
    fn check_staging_containment(ctx: &Ctx, entries: &[ArchiveEntry]) -> Result<(), DeliveryError> {
        let path = NodePath::native();
        let staging = text(&ctx.target.staging_path);
        for entry in entries {
            let relative = entry.path.replace('\\', path.sep());
            let target = path.resolve(&[&staging, &relative]);
            assert_inside_with(
                &path,
                &staging,
                &target,
                Some(&format!("Элемент архива \"{}\"", entry.path)),
            )?;
        }
        Ok(())
    }

    /// `checkExpectedSize(manifest, uncompressedSize)`: распакованный размер близок к ожидаемому
    /// (допуск — больше из 16 МиБ и 2 %).
    fn check_expected_size(manifest: &Value, uncompressed: f64) -> Result<(), DeliveryError> {
        let Some(expected_value) =
            jv::prop(manifest, "expectedInstalledSize").filter(|v| js::truthy(v))
        else {
            return Ok(());
        };
        let expected = jv::number(expected_value).unwrap_or(f64::NAN);
        let tolerance = (16.0 * 1024.0 * 1024.0_f64).max(js::math_round(expected * 0.02));
        if (uncompressed - expected).abs() > tolerance {
            let show = js::number::number_to_string;
            return Err(DeliveryError::new(
                Code::ArchiveLimitExceeded,
                format!(
                    "Распакованный размер {} расходится с манифестом {}",
                    show(uncompressed),
                    jv::template(Some(expected_value))
                ),
            )
            .with_details(json!({
                "expected": expected_value,
                "actual": js_number(uncompressed),
                "tolerance": js_number(tolerance),
            })));
        }
        Ok(())
    }

    /// `extract(ctx)`: распаковка в staging; прогресс — событиями `extracting`.
    async fn extract(&self, ctx: &Ctx) -> Result<(), DeliveryError> {
        let listeners = self.listeners.clone();
        let game_id = ctx.target.game_id.clone();
        let on_progress = Arc::new(move |fraction: f64| {
            let mut event = InstallEvent::new(&game_id, InstallState::Extracting);
            event.percent = Some(js::math_round(fraction * 100.0));
            emit_to(&listeners, &event);
        });
        let job = self.archive.extract(
            &ctx.archive_path,
            &ctx.target.staging_path,
            ExtractOptions {
                on_progress: Some(on_progress),
            },
        );
        self.set_extract(Some(job.cancel_handle()));
        let result = job.await;
        self.set_extract(None);
        result.map(|_| ())
    }

    /// `resolveContentRoot(ctx)`: корневая папка архива внутри staging (или сам staging).
    fn resolve_content_root(ctx: &Ctx, prefix: Option<&str>) -> Result<PathBuf, DeliveryError> {
        let Some(prefix) = prefix else {
            return Ok(ctx.target.staging_path.clone());
        };
        let staging = text(&ctx.target.staging_path);
        let candidate = assert_inside(
            &ctx.target.staging_path,
            &join(&[&staging, prefix]),
            "Корневая папка архива",
        )?;
        if !candidate.exists() {
            return Err(DeliveryError::new(
                Code::RequiredFileMissing,
                format!("После распаковки нет корневой папки \"{prefix}\""),
            ));
        }
        Ok(candidate)
    }

    /// `checkRequiredFiles(manifest, contentRoot)`.
    fn check_required_files(manifest: &Value, content_root: &Path) -> Result<(), DeliveryError> {
        let sep = NodePath::native().sep();
        let root = text(content_root);
        let required =
            strings(jv::prop(manifest, "filesManifest").and_then(|f| jv::prop(f, "requiredFiles")));
        let mut missing = Vec::new();
        for relative in required {
            let target = assert_inside(
                content_root,
                &join(&[&root, &relative.replace('/', sep)]),
                &format!("Обязательный файл \"{relative}\""),
            )?;
            if !target.exists() {
                missing.push(relative);
            }
        }
        if !missing.is_empty() {
            return Err(DeliveryError::new(
                Code::RequiredFileMissing,
                format!("После распаковки не хватает файлов: {}", missing.join(", ")),
            )
            .with_details(json!({ "missing": missing })));
        }
        Ok(())
    }

    // ------------------------------------------------------------ файлы игрока

    /// `preserveUserData(ctx, contentRoot)`: из старой установки в новую — `preservePaths`, данные
    /// игрока (`USER_DATA`) и cfg, которые игрок поменял. Как установщик лаунчера: не
    /// скопировался хоть один файл игрока — прерываемся до подмены папок, пока прежняя сборка цела
    /// (`preserveError`).
    fn preserve_user_data(&self, ctx: &Ctx, content_root: &Path) -> Result<(), DeliveryError> {
        if !ctx.target.install_path.exists() {
            return Ok(());
        }
        let preserve = strings(jv::prop(&ctx.manifest, "preservePaths"));
        self.preserve_declared_paths(ctx, content_root, &preserve)
            .and_then(|()| self.preserve_matched_files(ctx, content_root, &preserve))
            .map_err(preserve_error)
    }

    /// `userDataBytes(ctx)`: сколько места займут копии файлов игрока в новой сборке.
    pub fn user_data_bytes(&self, game_id: &str, install_path: &Path, manifest: &Value) -> u64 {
        if !install_path.exists() {
            return 0;
        }
        let preserve = strings(jv::prop(manifest, "preservePaths"));
        let sep = NodePath::native().sep();
        let install = text(install_path);
        let matched = Self::matched_user_files(game_id, install_path, &preserve);
        preserve
            .iter()
            .chain(&matched)
            .map(|rel| crate::game_removal::tree_size(&join(&[&install, &rel.replace('/', sep)])))
            .sum()
    }

    /// `preserveDeclaredPaths`: пути из `preservePaths` — `fs.cpSync` поверх новой сборки.
    fn preserve_declared_paths(
        &self,
        ctx: &Ctx,
        content_root: &Path,
        preserve: &[String],
    ) -> Result<(), Failure> {
        let path = NodePath::native();
        let (install, root) = (text(&ctx.target.install_path), text(content_root));
        for relative in preserve {
            let native = relative.replace('/', path.sep());
            let what = format!("Сохраняемый путь \"{relative}\"");
            let source =
                assert_inside(&ctx.target.install_path, &join(&[&install, &native]), &what)?;
            if !source.exists() {
                continue;
            }
            let target = assert_inside(content_root, &join(&[&root, &native]), &what)?;
            let parent = PathBuf::from(path.dirname(&text(&target)));
            node_fs::mkdir_recursive(&parent)?;
            cp_sync(&source, &target)?;
        }
        Ok(())
    }

    /// `previousFilesManifest(ctx)`: `files.json` версии, что стоит сейчас (по нему видно, какие
    /// cfg игрок поменял); версия неизвестна — `None`.
    fn previous_files_manifest(game_id: &str, install_path: &Path) -> Option<&'static Value> {
        let installed = read_json_safe(&install_path.join(INSTALLED_FILE));
        let version = jv::prop(&installed, "version").filter(|v| js::truthy(v))?;
        manifests::files_manifest_of(game_id, &js::to_string(version))
    }

    /// `preserveMatchedFiles`: данные игрока и изменённые cfg — в новую сборку.
    fn preserve_matched_files(
        &self,
        ctx: &Ctx,
        content_root: &Path,
        preserve: &[String],
    ) -> Result<(), Failure> {
        let install_path = &ctx.target.install_path;
        let files = Self::matched_user_files(&ctx.target.game_id, install_path, preserve);
        let path = NodePath::native();
        let (root, install) = (text(content_root), text(install_path));
        for rel in &files {
            let native = rel.replace('/', path.sep());
            let target = assert_inside(
                content_root,
                &join(&[&root, &native]),
                &format!("Пользовательский файл \"{rel}\""),
            )?;
            node_fs::mkdir_recursive(&PathBuf::from(path.dirname(&text(&target))))?;
            node_fs::copy_file(&join(&[&install, &native]), &target)?;
        }
        if !files.is_empty() {
            self.logger.info(&format!(
                "[game-delivery] {}: перенесено пользовательских файлов {}",
                ctx.target.game_id,
                files.len()
            ));
        }
        Ok(())
    }

    /// `matchedUserFiles(ctx, preserve)`: файлы игрока вне `preservePaths` — сейвы, демки,
    /// забеги, изменённые cfg. Эпизоды (`EPISODE_DIRS`) не обходятся: они переезжают rename.
    fn matched_user_files(game_id: &str, install_path: &Path, preserve: &[String]) -> Vec<String> {
        let user_data = Matcher::new(mutable_paths::USER_DATA);
        let previous = Self::previous_files_manifest(game_id, install_path);
        let declared = Matcher::new(preserve);
        let mut skip: Vec<String> = preserve.to_vec();
        skip.extend(
            mutable_paths::LIBRARY_INTERNALS
                .iter()
                .chain(mutable_paths::EPISODE_DIRS)
                .map(|s| (*s).to_owned()),
        );
        let skip_dir = Matcher::new(&skip);
        let matches = |rel: &str| {
            user_data.matches(rel)
                || user_configs::is_user_edited(install_path, rel, previous, None)
        };
        let mut files = Vec::new();

        // Обход в глубину в порядке `readdirSync`, как рекурсия `walk` в 0.4.7.
        fn walk(
            dir: &Path,
            relative: &str,
            visit: &mut dyn FnMut(&str, &js::fs::DirEntry) -> bool,
        ) {
            let Ok(entries) = js::fs::read_dir(dir) else {
                return;
            };
            for entry in entries {
                let rel = if relative.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{relative}/{}", entry.name)
                };
                if visit(&rel, &entry) {
                    walk(&dir.join(&entry.name), &rel, visit);
                }
            }
        }
        let mut visit = |rel: &str, entry: &js::fs::DirEntry| -> bool {
            if entry.file_type.is_symlink() {
                return false;
            }
            if entry.file_type.is_dir() {
                return !skip_dir.matches(rel);
            }
            if matches(rel) && !declared.matches(rel) {
                files.push(rel.to_owned());
            }
            false
        };
        walk(install_path, "", &mut visit);
        files
    }

    /// `swapIntoLibrary(ctx, contentRoot)`: старая папка игры — в `<staging>-previous`, новая — на
    /// её место; не вышло — старая возвращается (`INSTALL_FAILED`).
    pub fn swap_into_library(
        &self,
        target: &InstallTarget,
        content_root: &Path,
    ) -> Result<(), Failure> {
        node_fs::mkdir_recursive(&self.paths.games)?;
        let retired = join(&[
            &text(&self.paths.staging),
            &format!("{}-previous", basename(&text(&target.staging_path))),
        ]);
        let install_path = &target.install_path;
        let game_id = &target.game_id;
        let mut retired_used = false;
        if install_path.exists() {
            self.park_episodes(game_id, install_path)?;
            if let Err(error) = rename(install_path, &retired) {
                self.unpark_episodes(game_id, install_path);
                return Err(error.into());
            }
            retired_used = true;
        }
        if let Err(error) = rename(content_root, install_path) {
            if retired_used && !install_path.exists() {
                rename(&retired, install_path)?;
            }
            self.unpark_episodes(game_id, install_path);
            return Err(DeliveryError::new(
                Code::InstallFailed,
                format!("Не удалось перенести сборку: {}", error.message),
            )
            .into());
        }
        self.unpark_episodes(game_id, install_path);
        if retired_used {
            remove_within_zone(&self.paths.staging, &retired)?;
        }
        if target.staging_path.exists() {
            remove_within_zone(&self.paths.staging, &target.staging_path)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ восстановление

    /// `pendingFullUpdate(gameId)`: незавершённое обновление полной сборкой по журналу.
    pub fn pending_full_update(&self, game_id: &str) -> Option<Value> {
        let operation = self.read_operation(game_id)?;
        let field = |name: &str| jv::prop(&operation, name);
        if !jv::is_str(field("kind"), "full-update") {
            return None;
        }
        let mut out = Map::new();
        for key in ["gameId", "kind", "phase", "fromVersion", "toVersion"] {
            put(&mut out, key, field(key));
        }
        for key in ["stagingPath", "rollbackPath", "retiredPath", "archivePath"] {
            out.insert(key.to_owned(), or_null(field(key)));
        }
        put(&mut out, "startedAt", field("startedAt"));
        put(&mut out, "updatedAt", field("updatedAt"));
        Some(Value::Object(out))
    }

    /// `recoverFullUpdate(gameId)`: довести или откатить обновление, прерванное сбоем процесса.
    pub fn recover_full_update(&self, game_id: &str) -> Result<Recovery, Failure> {
        let Some(operation) = self.read_operation(game_id) else {
            return Ok(Recovery::default());
        };
        let field = |name: &str| jv::prop(&operation, name);
        if !jv::is_str(field("kind"), "full-update") {
            return Ok(Recovery::default());
        }
        let install_path = path_arg(field("installPath"))?;
        let installed_file = join(&[&install_path, INSTALLED_FILE]);
        let phase = field("phase");
        let phase_is = |name: &str| jv::is_str(phase, name);

        self.logger.warn(&format!(
            "[game-delivery] найдена незавершённая транзакция обновления {} -> {} на фазе {}, восстанавливаю",
            jv::template(field("fromVersion")),
            jv::template(field("toVersion")),
            jv::template(phase)
        ));

        if phase_is("commit-complete") || phase_is("installed-write") {
            let data = read_json_safe(&installed_file);
            if js::truthy(&data)
                && strict_eq(jv::prop(&data, "version"), field("toVersion"))
                && jv::is_str(jv::prop(&data, "state"), "ready")
            {
                self.cleanup_recovery_zones(&operation);
                self.clear_operation(game_id);
                self.logger.info(&format!(
                    "[game-delivery] версия {} уже установлена — доубираю за собой",
                    jv::template(field("toVersion"))
                ));
                return Ok(Recovery {
                    recovered: true,
                    direction: Some("forward"),
                    phase: phase.cloned(),
                    version: field("toVersion").cloned(),
                    ..Recovery::default()
                });
            }
        }

        let back = Recovery {
            recovered: true,
            direction: Some("back"),
            phase: phase.cloned(),
            restored_version: field("fromVersion").cloned(),
            ..Recovery::default()
        };
        if !UPDATE_SWAP_PHASES.iter().any(|swap| phase_is(swap)) {
            self.cleanup_recovery_zones(&operation);
            self.clear_operation(game_id);
            return Ok(back);
        }

        let restored = self.restore_retired(&operation, &install_path)?;
        self.cleanup_recovery_zones(&operation);
        self.clear_operation(game_id);
        if !restored {
            return Ok(Recovery {
                recovered: false,
                direction: Some("kept"),
                phase: phase.cloned(),
                needs_verify: true,
                ..Recovery::default()
            });
        }
        Ok(back)
    }

    /// `restoreRetired(operation)`: старая версия на месте — оставить; на месте новое дерево и есть
    /// копия — новое убрать, копию вернуть; копии нет — оставить как есть (`false`).
    fn restore_retired(&self, operation: &Value, install_path: &str) -> Result<bool, Failure> {
        let field = |name: &str| jv::prop(operation, name);
        let installed_file = join(&[install_path, INSTALLED_FILE]);
        let retired = field("retiredPath");
        let retired_exists = jv::truthy(retired) && exists_value(retired);
        let install = Path::new(install_path);

        if install.exists() {
            let state = read_json_state(&installed_file);
            let data = state.data();
            let old_in_place = js::truthy(&data)
                && strict_eq(jv::prop(&data, "version"), field("fromVersion"))
                && jv::is_str(jv::prop(&data, "state"), "ready");
            if old_in_place {
                return Ok(true);
            }
            if !retired_exists {
                let unreadable = matches!(state, JsonState::Invalid { .. });
                self.logger.error(&format!(
                    "[game-delivery] {}: на месте игры дерево, которое не признано версией {}{}, \
                     а копии в rollback нет — оставляю как есть. \
                     Состояние сборки нужно проверить через «Проверить файлы»",
                    jv::template(field("gameId")),
                    jv::template(field("fromVersion")),
                    if unreadable {
                        " (installed.json не читается)"
                    } else {
                        ""
                    }
                ));
                return Ok(false);
            }
            let staging = match field("stagingPath").filter(|s| js::truthy(s)) {
                Some(value) => path_arg(Some(value))?,
                None => format!(
                    "{}-recover-{}",
                    jv::template(field("gameId")),
                    js::base36(u128::try_from(self.now_ms()).unwrap_or(0))
                ),
            };
            self.discard_placed_tree(install, &staging)?;
        }

        if !install.exists()
            && retired_exists
            && let Some(retired) = retired.and_then(Value::as_str)
        {
            rename(Path::new(retired), install)?;
        }
        if !install.join(INSTALLED_FILE).exists() {
            self.logger.error(&format!(
                "[game-delivery] после отката {} нет installed.json — \
                 состояние сборки нужно проверить через «Проверить файлы»",
                jv::template(field("gameId"))
            ));
        }
        Ok(true)
    }

    /// `cleanupRecoveryZones(operation)`.
    fn cleanup_recovery_zones(&self, operation: &Value) {
        for (zone, key) in [
            (&self.paths.staging, "stagingPath"),
            (&self.paths.rollback, "rollbackPath"),
        ] {
            let target = jv::prop(operation, key);
            if !jv::truthy(target) || !exists_value(target) {
                continue;
            }
            let target = target.and_then(Value::as_str).unwrap_or_default();
            if let Err(error) = remove_within_zone(zone, Path::new(target)) {
                self.logger.warn(&format!(
                    "[game-delivery] не удалось убрать {target}: {error}"
                ));
            }
        }
    }

    /// `recordFailure(ctx, operation, error)`: `<id>.last-failure.json` и строка журнала.
    fn record_failure(&self, ctx: &Ctx, operation: &Map<String, Value>, error: &DeliveryError) {
        let game_id = &ctx.target.game_id;
        let log_path = join(&[
            &text(&self.paths.state),
            &format!("{game_id}.last-failure.json"),
        ]);
        let mut record = Map::new();
        record.insert("gameId".to_owned(), json!(game_id));
        put(&mut record, "phase", operation.get("phase"));
        record.insert("failedAt".to_owned(), json!(self.now_iso()));
        record.insert("error".to_owned(), error.to_public());
        record.insert(
            "stagingPath".to_owned(),
            json!(text(&ctx.target.staging_path)),
        );
        record.insert("archivePath".to_owned(), json!(text(&ctx.archive_path)));
        let written = node_fs::mkdir_recursive(&self.paths.state).and_then(|()| {
            node_fs::write_bytes(
                &log_path,
                stringify(&Value::Object(record), "  ").as_bytes(),
            )
        });
        if let Err(write_error) = written {
            self.logger.warn(&format!(
                "[game-delivery] не удалось записать диагностику: {}",
                write_error.message
            ));
        }
        self.logger.error(&format!(
            "[game-delivery] установка {game_id} прервана на фазе {}: {} {}",
            jv::template(operation.get("phase")),
            error.code,
            error.message
        ));
    }
}

fn emit_to(listeners: &Mutex<Vec<Listener>>, event: &InstallEvent) {
    let list: Vec<Listener> = listeners.lock().unwrap_or_else(|p| p.into_inner()).clone();
    for listener in list {
        listener(event);
    }
}

/// `crypto.randomBytes(4).toString("hex")`.
pub(crate) fn random_hex4() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).expect("системный генератор случайных чисел недоступен");
    file_ops::hex(&bytes)
}

/// `hashFile(ctx, filePath)`: sha256 блоками по 4 МиБ; просьба отменить — `CANCELLED`.
async fn hash_file(cancel: &AtomicBool, file: &Path) -> Result<String, DeliveryError> {
    let failed = |raw: RawError| hash_read_error(file, &raw, Platform::CURRENT);
    let mut handle = tokio::fs::File::open(file)
        .await
        .map_err(|e| failed(RawError::fs(&e, "open", file)))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; HASH_BLOCK];
    loop {
        let read = handle
            .read(&mut buffer)
            .await
            .map_err(|e| failed(fd_error(&e, "read")))?;
        if read == 0 {
            break;
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(DeliveryError::new(Code::Cancelled, "Установка отменена"));
        }
        hash.update(&buffer[..read]);
    }
    Ok(file_ops::hex(&hash.finalize()))
}

// ---------------------------------------------------------------- fs.cpSync Node 24

/// Ошибка `cpSync` из C++ Node (`ERR_FS_CP_*`): код и текст.
fn cp_error(code: &str, message: String) -> RawError {
    RawError {
        code: Some(code.to_owned()),
        syscall: Some("cp".to_owned()),
        message,
        ..RawError::default()
    }
}

/// `std::filesystem::weakly_canonical`: существующая часть пути — канонически, остаток — как есть.
fn weakly_canonical(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(&existing) {
            let mut out = canonical;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (
            existing.file_name().map(|n| n.to_owned()),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path, _dir: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn make_symlink(target: &Path, link: &Path, dir: bool) -> std::io::Result<()> {
    if dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

/// `fs.cpSync(src, dest, { recursive: true, force: true })` Node 24 для того, что сохраняет
/// установка: файл поверх файла, папка поверх папки (слиянием), ссылка — ссылкой на полный путь
/// цели. Файл на месте папки и наоборот — ошибки `ERR_FS_CP_*` с текстом Node.
fn cp_sync(src: &Path, dest: &Path) -> Result<(), RawError> {
    let src_meta = std::fs::symlink_metadata(src).map_err(|e| RawError::fs(&e, "lstat", src))?;
    let dest_meta = std::fs::symlink_metadata(dest).ok();
    let shown = |p: &Path| p.to_string_lossy().into_owned();
    if let Some(dest_meta) = &dest_meta {
        if src_meta.is_dir() && !dest_meta.is_dir() {
            return Err(cp_error(
                "ERR_FS_CP_DIR_TO_NON_DIR",
                format!(
                    "Cannot overwrite non-directory {} with directory {}",
                    shown(dest),
                    shown(src)
                ),
            ));
        }
        if !src_meta.is_dir() && dest_meta.is_dir() {
            return Err(cp_error(
                "ERR_FS_CP_NON_DIR_TO_DIR",
                format!(
                    "Cannot overwrite directory {} with non-directory {}",
                    shown(dest),
                    shown(src)
                ),
            ));
        }
    }
    if src_meta.is_dir() {
        return cp_dir(src, dest);
    }
    if src_meta.file_type().is_symlink() {
        // onLink: относительная цель — от папки ссылки.
        let target = std::fs::read_link(src).map_err(|e| RawError::fs(&e, "readlink", src))?;
        let path = NodePath::native();
        let resolved = if target.is_absolute() {
            target
        } else {
            PathBuf::from(path.resolve(&[&path.dirname(&shown(src)), &shown(&target)]))
        };
        if let Some(dest_meta) = dest_meta {
            if !dest_meta.file_type().is_symlink() {
                let error = std::io::Error::from(std::io::ErrorKind::AlreadyExists);
                return Err(RawError::fs_with_dest(
                    &error,
                    "symlink",
                    &resolved,
                    Some(dest),
                ));
            }
            std::fs::remove_file(dest).map_err(|e| RawError::fs(&e, "unlink", dest))?;
        }
        return make_symlink(&resolved, dest, src.is_dir())
            .map_err(|e| RawError::fs_with_dest(&e, "symlink", &resolved, Some(dest)));
    }
    if dest_meta.is_some() {
        std::fs::remove_file(dest).map_err(|e| RawError::fs(&e, "unlink", dest))?;
    }
    node_fs::copy_file(src, dest)
}

/// `cpSyncCopyDir` (C++ Node 24): папка создаётся, вложенное копируется поверх.
fn cp_dir(src: &Path, dest: &Path) -> Result<(), RawError> {
    let _ = std::fs::create_dir_all(dest);
    let entries = std::fs::read_dir(src).map_err(|e| RawError::fs(&e, "opendir", src))?;
    for entry in entries {
        let entry = entry.map_err(|e| RawError::fs(&e, "readdir", src))?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|e| RawError::fs(&e, "lstat", &from))?;
        if kind.is_symlink() {
            let target =
                std::fs::read_link(&from).map_err(|e| RawError::fs(&e, "readlink", &from))?;
            if let Ok(meta) = std::fs::symlink_metadata(&to)
                && !meta.is_dir()
            {
                std::fs::remove_file(&to).map_err(|e| RawError::fs(&e, "unlink", &to))?;
            }
            let absolute = weakly_canonical(&src.join(&target));
            make_symlink(&absolute, &to, from.is_dir())
                .map_err(|e| RawError::fs_with_dest(&e, "symlink", &absolute, Some(&to)))?;
        } else if kind.is_dir() {
            let _ = std::fs::create_dir(&to);
            cp_dir(&from, &to)?;
        } else if kind.is_file() {
            if std::fs::symlink_metadata(&to).is_ok_and(|meta| !meta.is_dir()) {
                std::fs::remove_file(&to).map_err(|e| RawError::fs(&e, "unlink", &to))?;
            }
            node_fs::copy_file(&from, &to)?;
        }
    }
    Ok(())
}
