//! Патчи (`PatchService.js`): транзакция замены изменённых файлов из `payload/` архива патча и
//! удаления лишних.
//!
//! Ход транзакции пишется в журнал `.hlsr/state/<id>.patch.json` (атомарно, `{version: 1, …}`):
//! `preflight` (установлена версия, с которой патч, игра не запущена, хватает места) →
//! `verify-archive` (размер и sha256 архива) → `inspect` (каждый файл архива объявлен в манифесте
//! патча, каждый объявленный есть, всё — под `payload/`) → `extract` (в staging) →
//! `verify-payload` → `backup` (копии заменяемых и удаляемых файлов в rollback) → `replace` →
//! `delete` → `verify` → `commit-started` (копия `installed.json` в
//! `<rollback>/installed.json.previous`) → `installed-written` → `commit-complete`. Изменённые
//! игроком cfg патч не трогает (`user-configs.js`).
//!
//! Сбой в том же процессе откатывается сразу (файлы из rollback, добавленные — убрать, на фазах
//! `commit-*` — ещё и `installed.json` из копии); неудачный откат — `ROLLBACK_FAILED`, журнал
//! остаётся. Сбой процесса (точки [`FAULTS`]) разбирает [`PatchService::recover`]: до записи
//! `installed.json` — откат, после — уборка; прерванное между ними доводится, если файлы новой
//! версии на месте. Тексты журнала и ошибок — как в 0.4.7.
//!
//! Отличия от 0.4.7: путь архива — обязательный путь (в 0.4.7 его отсутствие давало `undefined` в
//! журнале и `SIZE_MISMATCH`); брошенная задача освобождает сервис, как `finally`.

use crate::archive_service::{ArchiveService, ExtractOptions};
use crate::atomic::write_json_atomic;
use crate::build_metadata::RELATIVE_PATH;
use crate::build_metadata_trust::{self, VerifyForInstall};
use crate::errors::{Code, DeliveryError, Failure, RawError, to_delivery_error};
use crate::file_ops;
use crate::files_manifest::hash_file_sync;
use crate::free_space::FreeSpaceCache;
use crate::install_service::{
    assert_inside, exists_value, join, or_null, path_arg, put, rename, strict_eq, text,
};
use crate::js_value as jv;
use crate::manifests;
use crate::node_fs;
use crate::patch_format::{self, PAYLOAD_ROOT, PatchValidateOptions};
use crate::paths::{LibraryPaths, game_install_path, remove_within_zone};
use crate::ports::{FreeSpace, Logger, SystemFreeSpace, TracingLogger};
use crate::timing::{Clock, iso_from_ms, system_clock};
use crate::user_configs;
use hlsr_core::js;
use hlsr_core::js::object::JsObject;
use hlsr_core::js::path::NodePath;
use hlsr_core::json_store::read_json_safe;
use hlsr_core::json_text::{js_key_order, js_number};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use crate::install_service::FaultInjector;

/// `JOURNAL_VERSION`: версия журнала `<id>.patch.json`.
pub const JOURNAL_VERSION: u32 = 1;
/// `INSTALLED_FILE`.
pub const INSTALLED_FILE: &str = "installed.json";
/// `INSTALLED_BACKUP`: копия `installed.json` в rollback патча.
pub const INSTALLED_BACKUP: &str = "installed.json.previous";
/// `SPACE_MARGIN`: запас места сверх двух размеров архива.
pub const SPACE_MARGIN: u64 = 256 * 1024 * 1024;
/// `COMMIT_PHASES`: фазы, после которых `recover` пробует довести патч, а не откатить.
pub const COMMIT_PHASES: [&str; 3] = ["commit-started", "installed-written", "commit-complete"];
/// `FAULTS`: точки отказа записи `installed.json` (для проверки восстановления).
pub const FAULTS: [&str; 3] = [
    "before-installed-write",
    "after-installed-write",
    "before-rollback-cleanup",
];
/// Фазы, сбой на которых требует отката файлов (`needsRollback`).
const ROLLBACK_PHASES: [&str; 3] = ["replace", "delete", "verify"];
/// `YIELD_EVERY`: проверка после патча уступает поток раз в столько файлов.
const YIELD_EVERY: usize = 16;

/// `filesManifestOf(gameId, version)`: `files.json` прежней версии (по нему видно, какие cfg игрок
/// поменял); `None` — неизвестен (в 0.4.7 ошибка этой функции — тоже `null`).
pub type FilesManifestOf = Arc<dyn Fn(&str, &str) -> Option<Value> + Send + Sync>;

/// Слушатель события `state`.
pub type Listener = Arc<dyn Fn(&PatchEvent) + Send + Sync>;

// ---------------------------------------------------------------- события

/// Состояние в событии `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchState {
    ApplyingUpdate,
    Ready,
    Error,
}

impl PatchState {
    pub fn as_str(self) -> &'static str {
        match self {
            PatchState::ApplyingUpdate => "applying-update",
            PatchState::Ready => "ready",
            PatchState::Error => "error",
        }
    }
}

/// Событие `state`: `{gameId, state, phase}` на каждой фазе, `{state, phase: "extract", percent}`
/// при распаковке (без `gameId`, как в 0.4.7), `{gameId, state: "ready", installed}` и
/// `{gameId, state: "error", error}`.
#[derive(Debug, Clone, PartialEq)]
pub struct PatchEvent {
    pub game_id: Option<String>,
    pub state: PatchState,
    pub phase: Option<String>,
    pub percent: Option<f64>,
    pub installed: Option<Value>,
    pub error: Option<Value>,
}

impl PatchEvent {
    fn new(game_id: Option<&str>, state: PatchState) -> PatchEvent {
        PatchEvent {
            game_id: game_id.map(str::to_owned),
            state,
            phase: None,
            percent: None,
            installed: None,
            error: None,
        }
    }

    /// Объект события, как его получает слушатель в 0.4.7.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        if let Some(game_id) = &self.game_id {
            out.insert("gameId".to_owned(), json!(game_id));
        }
        out.insert("state".to_owned(), json!(self.state.as_str()));
        if let Some(phase) = &self.phase {
            out.insert("phase".to_owned(), json!(phase));
        }
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

// ---------------------------------------------------------------- запрос, итоги, параметры

/// `apply(request)`.
#[derive(Debug, Clone)]
pub struct PatchRequest {
    /// Манифест игры целевой версии (`gameId`, описание метаданных сборки).
    pub game_manifest: Value,
    /// Манифест патча (`<id>-<from>-to-<to>.json`).
    pub patch_manifest: Value,
    /// Установленная версия: патч обязан быть рассчитан на неё (`None` — не сверяется).
    pub installed_version: Option<String>,
    pub archive_path: PathBuf,
}

/// Итог проверки файлов после патча (`verification`).
#[derive(Debug, Clone, PartialEq)]
pub struct Verification {
    /// `resultingFilesManifestSha256` манифеста патча.
    pub files_manifest_sha256: Option<Value>,
    /// `resultingTotalFiles`.
    pub total_files: Option<Value>,
    /// `resultingTotalSize`.
    pub total_size: Option<Value>,
    pub changed_files: usize,
    pub deleted_files: usize,
}

impl Verification {
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        put(
            &mut out,
            "filesManifestSha256",
            self.files_manifest_sha256.as_ref(),
        );
        put(&mut out, "totalFiles", self.total_files.as_ref());
        put(&mut out, "totalSize", self.total_size.as_ref());
        out.insert("changedFiles".to_owned(), json!(self.changed_files));
        out.insert("deletedFiles".to_owned(), json!(self.deleted_files));
        Value::Object(out)
    }
}

/// Итог `apply`: `{status: "applied", installed, verification}`.
#[derive(Debug, Clone, PartialEq)]
pub struct PatchApplied {
    /// Записанный `installed.json`.
    pub installed: Value,
    pub verification: Verification,
}

impl PatchApplied {
    pub fn to_json(&self) -> Value {
        json!({
            "status": "applied",
            "installed": self.installed,
            "verification": self.verification.to_json(),
        })
    }
}

/// Итог `recover`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PatchRecovery {
    pub recovered: bool,
    /// `back` (возвращена прежняя версия) или `forward` (новая версия доведена).
    pub direction: Option<&'static str>,
    pub phase: Option<Value>,
    /// `version`: новая версия при `forward`.
    pub version: Option<Value>,
    /// `restoredVersion` при `back`.
    pub restored_version: Option<Value>,
    /// Журнал повреждён: убран в карантин.
    pub corrupt: bool,
    /// Куда убран повреждённый журнал (`None` — убрать не вышло).
    pub quarantined: Option<String>,
}

impl PatchRecovery {
    /// `{recovered}`, `{recovered, corrupt, quarantined, verifyRecommended}` или
    /// `{recovered, direction, phase, version | restoredVersion}`.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("recovered".to_owned(), json!(self.recovered));
        if self.corrupt {
            out.insert("corrupt".to_owned(), json!(true));
            out.insert("quarantined".to_owned(), json!(self.quarantined));
            out.insert("verifyRecommended".to_owned(), json!(true));
        }
        if let Some(direction) = self.direction {
            out.insert("direction".to_owned(), json!(direction));
        }
        put(&mut out, "phase", self.phase.as_ref());
        put(&mut out, "version", self.version.as_ref());
        put(&mut out, "restoredVersion", self.restored_version.as_ref());
        Value::Object(out)
    }
}

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct PatchOptions {
    pub archive_service: Option<ArchiveService>,
    pub logger: Option<Arc<dyn Logger>>,
    /// `spaceMargin`: `None` — [`SPACE_MARGIN`].
    pub space_margin: Option<u64>,
    /// `launcherVersion`: без неё совместимость метаданных сборки с лаунчером не проверяется.
    pub launcher_version: Option<String>,
    /// `filesManifestOf`: по умолчанию — вшитые `files.json` ([`manifests::files_manifest_of`]).
    pub files_manifest_of: Option<FilesManifestOf>,
    pub fault_injector: Option<FaultInjector>,
    /// Свободное место (`freeSpaceBytes`): по умолчанию — ОС с кешем на 5 с.
    pub free_space: Option<Arc<dyn FreeSpace>>,
    /// Часы (`Date.now()`); для тестов.
    pub clock: Option<Clock>,
}

// ---------------------------------------------------------------- сервис

/// Расхождение файла после патча: путь и причина.
#[derive(Debug, Clone, PartialEq)]
struct Problem {
    path: String,
    reason: String,
}

/// Журнал на диске (`readJournal`).
enum Journal {
    Valid(Value),
    Corrupt,
}

/// Идущий патч: при выходе из `apply` (и при брошенной задаче) сервис свободен, как `finally`.
struct Busy<'a>(&'a AtomicBool);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// То, что нужно транзакции, кроме журнала (`context`).
struct Context<'a> {
    game_id: &'a str,
    game_manifest: &'a Value,
    patch_manifest: &'a Value,
    install_path: &'a Path,
    staging_path: &'a Path,
    rollback_path: &'a Path,
    archive_path: &'a Path,
}

/// `PatchService`.
pub struct PatchService {
    paths: LibraryPaths,
    archive: ArchiveService,
    logger: Arc<dyn Logger>,
    space_margin: u64,
    launcher_version: Option<String>,
    files_manifest_of: FilesManifestOf,
    fault_injector: Option<FaultInjector>,
    free_space: Arc<dyn FreeSpace>,
    clock: Clock,
    busy: AtomicBool,
    listeners: Arc<Mutex<Vec<Listener>>>,
}

fn emit_to(listeners: &Mutex<Vec<Listener>>, event: &PatchEvent) {
    let list: Vec<Listener> = listeners.lock().unwrap_or_else(|p| p.into_inner()).clone();
    for listener in list {
        listener(event);
    }
}

/// Строка поля проверенного манифеста патча.
fn str_field<'a>(manifest: &'a Value, name: &str) -> &'a str {
    jv::prop(manifest, name)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Список поля манифеста патча или журнала; не массив — пусто.
fn list<'a>(value: &'a Value, name: &str) -> &'a [Value] {
    jv::array(jv::prop(value, name)).map_or(&[], Vec::as_slice)
}

/// `String(value)` пути из манифеста или журнала.
fn rel_of(value: &Value) -> String {
    jv::template(Some(value))
}

/// `path` записи `files` (у `null` — `TypeError`, как `file.path` в JS).
fn file_path(file: &Value) -> Result<String, RawError> {
    if file.is_null() {
        return Err(jv::read_error(Some(file), "path"));
    }
    Ok(jv::template(jv::prop(file, "path")))
}

/// `fileOps.resolveInside(root, relativePath, what)`.
fn resolve_inside(root: &str, relative: &str, what: &str) -> Result<PathBuf, DeliveryError> {
    file_ops::resolve_inside(Path::new(root), relative, Some(what))
}

/// `String(value || "").slice(0, 16) + "…"`.
fn short_hash(value: Option<&Value>) -> String {
    let text = if jv::truthy(value) {
        js::slice_utf16(&jv::template(value), 16)
    } else {
        String::new()
    };
    format!("{text}…")
}

/// `patchedFileProblem(file, stats, digest)`.
fn patched_file_problem(
    file: &Value,
    stats: Option<&std::fs::Metadata>,
    digest: Option<&str>,
) -> Option<String> {
    let Some(stats) = stats.filter(|meta| meta.is_file()) else {
        return Some("нет на диске".to_owned());
    };
    let size = jv::prop(file, "size");
    if !same_size(stats, size) {
        return Some(format!(
            "размер {}, ожидался {}",
            stats.len(),
            jv::template(size)
        ));
    }
    let sha256 = jv::prop(file, "sha256");
    if digest.is_none_or(|digest| !jv::is_str(sha256, digest)) {
        return Some(format!(
            "SHA-256 {}, ожидался {}",
            short_hash(digest.map(|d| json!(d)).as_ref()),
            short_hash(sha256)
        ));
    }
    None
}

/// `stats.size === file.size`.
fn same_size(stats: &std::fs::Metadata, size: Option<&Value>) -> bool {
    size.and_then(jv::number) == Some(stats.len() as f64)
}

/// `Object.assign({}, value)`: свои перечислимые свойства — у объекта поля, у массива индексы, у
/// строки — символы по индексам (единицы UTF-16); у остальных — ничего.
fn assign_source(value: &Value) -> Vec<(String, Value)> {
    match value {
        Value::Object(map) => js_key_order(map)
            .into_iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value.clone()))
            .collect(),
        Value::String(text) => text
            .encode_utf16()
            .enumerate()
            .map(|(index, unit)| {
                let unit = char::decode_utf16([unit])
                    .next()
                    .and_then(Result::ok)
                    .unwrap_or(char::REPLACEMENT_CHARACTER);
                (index.to_string(), json!(unit.to_string()))
            })
            .collect(),
        _ => Vec::new(),
    }
}

impl PatchService {
    /// `new PatchService({ paths, … })`.
    pub fn new(paths: LibraryPaths, options: PatchOptions) -> PatchService {
        let logger = options
            .logger
            .unwrap_or_else(|| Arc::new(TracingLogger) as Arc<dyn Logger>);
        PatchService {
            archive: options
                .archive_service
                .unwrap_or_else(|| ArchiveService::with_default_bin(logger.clone())),
            logger,
            space_margin: options.space_margin.unwrap_or(SPACE_MARGIN),
            launcher_version: options.launcher_version.filter(|v| !v.is_empty()),
            files_manifest_of: options.files_manifest_of.unwrap_or_else(|| {
                Arc::new(|game_id: &str, version: &str| {
                    manifests::files_manifest_of(game_id, version).cloned()
                })
            }),
            fault_injector: options.fault_injector,
            free_space: options.free_space.unwrap_or_else(|| {
                Arc::new(FreeSpaceCache::new(Arc::new(SystemFreeSpace))) as Arc<dyn FreeSpace>
            }),
            clock: options.clock.unwrap_or_else(system_clock),
            busy: AtomicBool::new(false),
            listeners: Arc::new(Mutex::new(Vec::new())),
            paths,
        }
    }

    pub fn paths(&self) -> &LibraryPaths {
        &self.paths
    }

    /// `on("state", listener)`: слушатель зовётся сразу, в той же точке кода, где рождается событие.
    pub fn on_event(&self, listener: impl Fn(&PatchEvent) + Send + Sync + 'static) {
        self.listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::new(listener));
    }

    fn emit(&self, event: &PatchEvent) {
        emit_to(&self.listeners, event);
    }

    fn now_ms(&self) -> i64 {
        (self.clock)()
    }

    fn now_iso(&self) -> String {
        iso_from_ms(self.now_ms())
    }

    /// `busy`: идёт патч.
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    /// `fault(name, journal)`.
    fn fault(&self, name: &str, journal: &Map<String, Value>) -> Result<(), Failure> {
        match &self.fault_injector {
            Some(injector) => injector(name, &Value::Object(journal.clone())),
            None => Ok(()),
        }
    }

    // ------------------------------------------------------------ журнал

    /// `journalPath(gameId)`: `.hlsr/state/<id>.patch.json`.
    pub fn journal_path(&self, game_id: &str) -> PathBuf {
        join(&[&text(&self.paths.state), &format!("{game_id}.patch.json")])
    }

    /// `writeJournal(journal)`: `updatedAt`, атомарная запись `{version: 1, …}`.
    fn write_journal(&self, journal: &mut Map<String, Value>) -> Result<(), RawError> {
        node_fs::mkdir_recursive(&self.paths.state)?;
        journal.insert("updatedAt".to_owned(), json!(self.now_iso()));
        let mut out = Map::new();
        out.insert("version".to_owned(), json!(JOURNAL_VERSION));
        for (key, value) in journal.iter() {
            out.insert(key.clone(), value.clone());
        }
        let game_id = jv::template(journal.get("gameId"));
        write_json_atomic(&self.journal_path(&game_id), &Value::Object(out))
    }

    /// `readJournal(gameId)`: нет файла — `None`; не JSON или не той версии — повреждён.
    fn read_journal(&self, game_id: &str) -> Option<Journal> {
        let file = self.journal_path(game_id);
        if !file.exists() {
            return None;
        }
        let data = read_json_safe(&file);
        let valid = js::truthy(&data)
            && matches!(jv::prop(&data, "version"), Some(Value::Number(n))
                if n.as_f64() == Some(f64::from(JOURNAL_VERSION)));
        Some(if valid {
            Journal::Valid(data)
        } else {
            Journal::Corrupt
        })
    }

    /// `quarantineJournal(gameId)`: повреждённый журнал — в `<журнал>.corrupt-<время36>`.
    fn quarantine_journal(&self, game_id: &str) -> Option<String> {
        let file = self.journal_path(game_id);
        let target = format!(
            "{}.corrupt-{}",
            text(&file),
            js::base36(u128::try_from(self.now_ms()).unwrap_or(0))
        );
        if let Err(error) = rename(&file, Path::new(&target)) {
            self.logger.warn(&format!(
                "[game-delivery] повреждённый журнал не убран: {}",
                error.message
            ));
            return None;
        }
        self.logger.error(&format!(
            "[game-delivery] журнал патча {game_id} повреждён и перемещён в {target}. \
             Состояние сборки нужно проверить через «Проверить файлы»."
        ));
        Some(target)
    }

    /// `clearJournal(gameId)`.
    fn clear_journal(&self, game_id: &str) {
        let file = self.journal_path(game_id);
        if file.exists()
            && let Err(error) = node_fs::unlink(&file)
        {
            self.logger.warn(&format!(
                "[game-delivery] журнал патча не удалён: {}",
                error.message
            ));
        }
    }

    /// `setPhase(journal, phase)`: фаза в журнал и событие `applying-update`.
    fn set_phase(&self, journal: &mut Map<String, Value>, phase: &str) -> Result<(), RawError> {
        journal.insert("phase".to_owned(), json!(phase));
        self.write_journal(journal)?;
        let game_id = jv::template(journal.get("gameId"));
        let mut event = PatchEvent::new(Some(&game_id), PatchState::ApplyingUpdate);
        event.phase = Some(phase.to_owned());
        self.emit(&event);
        Ok(())
    }

    /// `findPending(gameId)`: незавершённый патч по журналу.
    pub fn find_pending(&self, game_id: &str) -> Option<Value> {
        let journal = match self.read_journal(game_id)? {
            Journal::Corrupt => {
                return Some(json!({
                    "gameId": game_id,
                    "corrupt": true,
                    "phase": "unknown",
                    "needsRollback": false,
                }));
            }
            Journal::Valid(journal) => journal,
        };
        let field = |name: &str| jv::prop(&journal, name);
        let phase_in = |phases: &[&str]| phases.iter().any(|p| jv::is_str(field("phase"), p));
        let mut out = Map::new();
        for key in [
            "gameId",
            "fromVersion",
            "toVersion",
            "phase",
            "startedAt",
            "updatedAt",
        ] {
            put(&mut out, key, field(key));
        }
        out.insert(
            "needsRollback".to_owned(),
            json!(phase_in(&ROLLBACK_PHASES)),
        );
        out.insert("rollsForward".to_owned(), json!(phase_in(&COMMIT_PHASES)));
        out.insert("rollbackPath".to_owned(), or_null(field("rollbackPath")));
        Some(Value::Object(out))
    }

    // ------------------------------------------------------------ восстановление

    /// `recover(gameId)`: разобрать незавершённый патч по журналу — откатить или довести.
    pub fn recover(&self, game_id: &str) -> Result<PatchRecovery, Failure> {
        let journal = match self.read_journal(game_id) {
            None => return Ok(PatchRecovery::default()),
            Some(Journal::Corrupt) => {
                return Ok(PatchRecovery {
                    corrupt: true,
                    quarantined: self.quarantine_journal(game_id),
                    ..PatchRecovery::default()
                });
            }
            Some(Journal::Valid(journal)) => journal,
        };
        let field = |name: &str| jv::prop(&journal, name);
        if COMMIT_PHASES.iter().any(|p| jv::is_str(field("phase"), p)) {
            return self.recover_commit(&journal);
        }

        self.logger.warn(&format!(
            "[game-delivery] найдена незавершённая транзакция патча {} -> {} на фазе {}, выполняю откат",
            jv::template(field("fromVersion")),
            jv::template(field("toVersion")),
            jv::template(field("phase"))
        ));
        self.rollback(&journal)?;
        self.cleanup_transaction(&journal);
        self.clear_journal(game_id);
        Ok(PatchRecovery {
            recovered: true,
            direction: Some("back"),
            phase: field("phase").cloned(),
            restored_version: field("fromVersion").cloned(),
            ..PatchRecovery::default()
        })
    }

    /// `recoverCommit(journal)`: `installed.json` уже новой версии — уборка; файлы новой версии на
    /// месте — дописать `installed.json`; иначе — откат с прежним `installed.json`.
    fn recover_commit(&self, journal: &Value) -> Result<PatchRecovery, Failure> {
        let field = |name: &str| jv::prop(journal, name);
        let game_id = jv::template(field("gameId"));
        let install_path = path_arg(field("installPath"))?;
        let installed = read_json_safe(&join(&[&install_path, INSTALLED_FILE]));
        let forward = PatchRecovery {
            recovered: true,
            direction: Some("forward"),
            phase: field("phase").cloned(),
            version: field("toVersion").cloned(),
            ..PatchRecovery::default()
        };

        if js::truthy(&installed) && strict_eq(jv::prop(&installed, "version"), field("toVersion"))
        {
            self.logger.info(&format!(
                "[game-delivery] транзакция патча {game_id} была прервана после записи installed.json — \
                 версия {} уже установлена, доубираю за собой",
                jv::template(field("toVersion"))
            ));
            self.cleanup_transaction(journal);
            self.clear_journal(&game_id);
            return Ok(forward);
        }

        let mut problems = None;
        if let Some(expected) = jv::array(field("expectedFiles")) {
            let deleted = jv::array(field("expectedDeleted")).map_or(&[][..], Vec::as_slice);
            match self.verify_result_sync(&install_path, expected, deleted) {
                Ok(found) => problems = Some(found),
                Err(error) => self.logger.error(&format!(
                    "[game-delivery] проверка файлов патча не удалась: {error}"
                )),
            }
        } else {
            self.logger.warn(&format!(
                "[game-delivery] в журнале патча {game_id} нет списка файлов (журнал от прежней версии) — откатываюсь"
            ));
        }

        if problems.as_ref().is_some_and(Vec::is_empty) {
            self.logger.warn(&format!(
                "[game-delivery] транзакция патча {game_id} прервана перед записью installed.json, \
                 но файлы новой версии на месте — дописываю installed.json"
            ));
            self.write_installed_atomic(
                &install_path,
                &[
                    ("version", field("toVersion").cloned()),
                    ("migratedFrom", field("fromVersion").cloned()),
                    (
                        "filesManifestSha256",
                        Some(or_null(field("resultingFilesManifestSha256"))),
                    ),
                ],
            )?;
            self.cleanup_transaction(journal);
            self.clear_journal(&game_id);
            return Ok(forward);
        }

        let tail = match problems.as_ref().filter(|found| !found.is_empty()) {
            Some(found) => {
                let shown: Vec<&str> = found.iter().take(3).map(|p| p.path.as_str()).collect();
                format!(
                    "не совпало файлов: {} ({}) — откатываюсь к прежней версии",
                    found.len(),
                    shown.join(", ")
                )
            }
            None => "откатываюсь к прежней версии".to_owned(),
        };
        self.logger.error(&format!(
            "[game-delivery] транзакция патча {game_id} прервана на фазе {}, {tail}",
            jv::template(field("phase"))
        ));
        self.rollback(journal)?;
        self.restore_installed_backup(journal)?;
        self.cleanup_transaction(journal);
        self.clear_journal(&game_id);
        Ok(PatchRecovery {
            recovered: true,
            direction: Some("back"),
            phase: field("phase").cloned(),
            restored_version: field("fromVersion").cloned(),
            ..PatchRecovery::default()
        })
    }

    // ------------------------------------------------------------ применение

    /// `apply(request)`: применить патч. Ошибки транзакции — `DeliveryError` (чужие — с кодом
    /// `PATCH_FAILED`); ошибки до неё (манифест, путь игры, запись журнала) — как пришли. Журнал
    /// не записался — сервис свободен (в 0.4.7 оставался занятым до перезапуска).
    ///
    /// Не отменяется: вызывающие (`GameDeliveryService`, `DeliveryApi`) ждут будущее до конца и не
    /// гоняют его в `select!`/`timeout`. Брошенное будущее (выход из процесса) освобождает сервис
    /// без отката — журнал остаётся, его разберёт `recover`, как после сбоя процесса.
    pub async fn apply(&self, request: PatchRequest) -> Result<PatchApplied, Failure> {
        if self.is_busy() {
            return Err(
                DeliveryError::new(Code::AlreadyRunning, "Обновление уже выполняется").into(),
            );
        }
        let PatchRequest {
            game_manifest,
            patch_manifest,
            installed_version,
            archive_path,
        } = request;
        let game_id = match &game_manifest {
            Value::Null => return Err(jv::read_error(Some(&Value::Null), "gameId").into()),
            manifest => match jv::prop(manifest, "gameId") {
                Some(Value::String(id)) => id.clone(),
                other => {
                    return Err(DeliveryError::new(
                        Code::PathInvalid,
                        format!("Недопустимый gameId: {}", jv::template(other)),
                    )
                    .into());
                }
            },
        };
        let install_path = game_install_path(&self.paths, &game_id)?;

        patch_format::validate_patch_manifest(
            &patch_manifest,
            Some(&PatchValidateOptions {
                expected_from: installed_version,
            }),
        )?;
        if !jv::is_str(jv::prop(&patch_manifest, "gameId"), &game_id) {
            return Err(
                DeliveryError::new(Code::ManifestInvalid, "Патч собран для другой игры").into(),
            );
        }
        let (from, to) = (
            str_field(&patch_manifest, "fromVersion"),
            str_field(&patch_manifest, "toVersion"),
        );
        let id = format!(
            "{game_id}-{from}-to-{to}-{}",
            js::base36(u128::try_from(self.now_ms()).unwrap_or(0))
        );
        let staging_path = assert_inside(
            &self.paths.staging,
            &join(&[&text(&self.paths.staging), &id]),
            "staging патча",
        )?;
        let rollback_path = assert_inside(
            &self.paths.rollback,
            &join(&[&text(&self.paths.rollback), &id]),
            "rollback патча",
        )?;

        let mut journal = Map::new();
        journal.insert("gameId".to_owned(), json!(game_id));
        journal.insert("fromVersion".to_owned(), json!(from));
        journal.insert("toVersion".to_owned(), json!(to));
        journal.insert("phase".to_owned(), json!("preflight"));
        journal.insert("installPath".to_owned(), json!(text(&install_path)));
        journal.insert("stagingPath".to_owned(), json!(text(&staging_path)));
        journal.insert("rollbackPath".to_owned(), json!(text(&rollback_path)));
        journal.insert("archivePath".to_owned(), json!(text(&archive_path)));
        journal.insert("replaced".to_owned(), json!([]));
        journal.insert("added".to_owned(), json!([]));
        journal.insert("deleted".to_owned(), json!([]));
        journal.insert("startedAt".to_owned(), json!(self.now_iso()));

        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(
                DeliveryError::new(Code::AlreadyRunning, "Обновление уже выполняется").into(),
            );
        }
        // В 0.4.7 запись журнала — до `try`: её ошибка оставляла сервис занятым до перезапуска
        // (все операции и запуск игр — ALREADY_RUNNING). Здесь сервис освобождается и в этом случае.
        let busy = Busy(&self.busy);
        self.write_journal(&mut journal)?;

        let context = Context {
            game_id: &game_id,
            game_manifest: &game_manifest,
            patch_manifest: &patch_manifest,
            install_path: &install_path,
            staging_path: &staging_path,
            rollback_path: &rollback_path,
            archive_path: &archive_path,
        };
        let outcome = match self.run_transaction(&mut journal, &context).await {
            Ok(applied) => {
                let journal = Value::Object(journal);
                self.cleanup_transaction(&journal);
                self.clear_journal(&game_id);
                Ok(applied)
            }
            Err(error) => Err(self.fail(&journal, error, &game_id)),
        };
        drop(busy);
        outcome
    }

    /// `catch` патча: откат, на фазах `commit-*` — и `installed.json`; неудачный откат —
    /// `ROLLBACK_FAILED` (журнал и rollback остаются).
    fn fail(&self, journal: &Map<String, Value>, error: Failure, game_id: &str) -> Failure {
        let delivery = to_delivery_error(error, Some(Code::PatchFailed));
        let field = |name: &str| journal.get(name);
        self.logger.error(&format!(
            "[game-delivery] патч {} -> {} прерван на фазе {}: {} {}",
            jv::template(field("fromVersion")),
            jv::template(field("toVersion")),
            jv::template(field("phase")),
            delivery.code,
            delivery.message
        ));
        let journal_value = Value::Object(journal.clone());
        let committing = COMMIT_PHASES.iter().any(|p| jv::is_str(field("phase"), p));
        let rolled_back = self.rollback(&journal_value).and_then(|_| {
            if committing {
                self.restore_installed_backup(&journal_value)?;
            }
            Ok(())
        });
        let mut event = PatchEvent::new(Some(game_id), PatchState::Error);
        event.error = Some(delivery.to_public());
        if let Err(rollback_error) = rolled_back {
            self.logger.error(&format!(
                "[game-delivery] откат не удался: {rollback_error}"
            ));
            self.emit(&event);
            let rollback_path = jv::template(field("rollbackPath"));
            return DeliveryError::new(
                Code::RollbackFailed,
                format!(
                    "Патч не применён и откат не удался: {rollback_error}. \
                     Файлы прежней версии сохранены в {rollback_path}"
                ),
            )
            .with_details(json!({
                "rollbackPath": field("rollbackPath"),
                "original": delivery.to_public(),
            }))
            .into();
        }
        self.cleanup_transaction(&journal_value);
        self.clear_journal(game_id);
        self.emit(&event);
        delivery.into()
    }

    /// `runTransaction(context)`.
    async fn run_transaction(
        &self,
        journal: &mut Map<String, Value>,
        context: &Context<'_>,
    ) -> Result<PatchApplied, Failure> {
        let patch_manifest = context.patch_manifest;

        self.set_phase(journal, "preflight")?;
        Self::assert_installed(
            context.install_path,
            str_field(patch_manifest, "fromVersion"),
        )?;
        Self::assert_game_not_running(context.install_path)?;
        self.assert_space(patch_manifest)?;

        self.set_phase(journal, "verify-archive")?;
        Self::verify_archive_file(context.archive_path, patch_manifest)?;

        self.set_phase(journal, "inspect")?;
        let listing = self.archive.list(context.archive_path).await?;
        patch_format::validate_payload_entries(patch_manifest, &listing.entries)?;

        self.set_phase(journal, "extract")?;
        node_fs::mkdir_recursive(context.staging_path)?;
        self.extract(context.archive_path, context.staging_path)
            .await?;

        self.set_phase(journal, "verify-payload")?;
        let staging = text(context.staging_path);
        let payload_root = assert_inside(
            context.staging_path,
            &join(&[&staging, PAYLOAD_ROOT]),
            "payload патча",
        )?;
        Self::verify_payload(&payload_root, patch_manifest).await?;

        let applied =
            self.without_user_configs(context.game_id, context.install_path, patch_manifest);

        self.set_phase(journal, "backup")?;
        node_fs::mkdir_recursive(context.rollback_path)?;
        self.backup_affected(journal, context, &applied)?;

        self.set_phase(journal, "replace")?;
        Self::replace_files(&payload_root, context.install_path, &applied)?;

        self.set_phase(journal, "delete")?;
        Self::delete_files(context.install_path, &applied)?;

        self.set_phase(journal, "verify")?;
        let verification = Self::verify_result(context.install_path, &applied).await?;

        let installed = self.commit(journal, context, &applied, &verification)?;

        let mut event = PatchEvent::new(Some(context.game_id), PatchState::Ready);
        event.installed = Some(installed.clone());
        self.emit(&event);
        Ok(PatchApplied {
            installed,
            verification,
        })
    }

    /// `withoutUserConfigs(gameId, installPath, patchManifest)`: cfg, изменённые игроком, патч не
    /// заменяет и не удаляет (`user-configs.js`). Нечего оставлять — манифест как есть.
    pub fn without_user_configs(
        &self,
        game_id: &str,
        install_path: &Path,
        patch_manifest: &Value,
    ) -> Value {
        let from = jv::template(jv::prop(patch_manifest, "fromVersion"));
        let previous = (self.files_manifest_of)(game_id, &from);
        let kept = user_configs::kept_by_patch(install_path, patch_manifest, previous.as_ref());
        if kept.is_empty() {
            return patch_manifest.clone();
        }
        self.logger.info(&format!(
            "[game-delivery] {game_id}: cfg игрока оставлены как есть: {}",
            kept.join(", ")
        ));
        let is_kept = |relative: Option<&Value>| {
            relative
                .and_then(Value::as_str)
                .is_some_and(|relative| kept.iter().any(|k| k == relative))
        };
        let mut applied = patch_manifest.clone();
        let files: Vec<Value> = list(patch_manifest, "files")
            .iter()
            .filter(|file| !is_kept(jv::prop(file, "path")))
            .cloned()
            .collect();
        let deleted: Vec<Value> = list(patch_manifest, "deletedFiles")
            .iter()
            .filter(|relative| !is_kept(Some(relative)))
            .cloned()
            .collect();
        applied["files"] = Value::Array(files);
        applied["deletedFiles"] = Value::Array(deleted);
        applied
    }

    /// `commit(journal, installPath, patchManifest, verification, context)`: ожидаемые файлы — в
    /// журнал, копия `installed.json`, новый `installed.json`.
    fn commit(
        &self,
        journal: &mut Map<String, Value>,
        context: &Context<'_>,
        applied: &Value,
        verification: &Verification,
    ) -> Result<Value, Failure> {
        let install_path = context.install_path;
        let installed_file = join(&[&text(install_path), INSTALLED_FILE]);
        let backup = join(&[&text(context.rollback_path), INSTALLED_BACKUP]);

        put(
            journal,
            "resultingFilesManifestSha256",
            jv::prop(applied, "resultingFilesManifestSha256"),
        );
        journal.insert("installedBackup".to_owned(), json!(text(&backup)));
        let expected: Vec<Value> = list(applied, "files")
            .iter()
            .map(|file| {
                let mut entry = Map::new();
                for key in ["path", "size", "sha256"] {
                    put(&mut entry, key, jv::prop(file, key));
                }
                Value::Object(entry)
            })
            .collect();
        journal.insert("expectedFiles".to_owned(), Value::Array(expected));
        journal.insert(
            "expectedDeleted".to_owned(),
            Value::Array(list(applied, "deletedFiles").to_vec()),
        );

        self.set_phase(journal, "commit-started")?;
        if installed_file.exists() {
            file_ops::copy_file_sync(&installed_file, &backup, Some(INSTALLED_FILE))?;
        }
        self.fault("before-installed-write", journal)?;

        let mut changes = vec![
            ("version", jv::prop(applied, "toVersion").cloned()),
            ("migratedFrom", jv::prop(applied, "fromVersion").cloned()),
            (
                "filesManifestSha256",
                verification.files_manifest_sha256.clone(),
            ),
            ("totalFiles", verification.total_files.clone()),
            ("totalSize", verification.total_size.clone()),
        ];
        if let Some(snapshot) = self.build_metadata_snapshot(install_path, applied, context) {
            for key in [
                "buildMetadata",
                "buildMetadataSha256",
                "buildMetadataVerifiedAt",
            ] {
                changes.push((key, snapshot.get(key).cloned()));
            }
        }
        let installed = self.write_installed_atomic(&text(install_path), &changes)?;

        self.set_phase(journal, "installed-written")?;
        self.fault("after-installed-write", journal)?;

        self.set_phase(journal, "commit-complete")?;
        self.fault("before-rollback-cleanup", journal)?;
        Ok(installed)
    }

    /// `assertInstalled(installPath, expectedVersion)`: `installed.json` в состоянии `ready` той
    /// версии, на которую рассчитан патч. Разбор — `JSON.parse` без среза BOM.
    fn assert_installed(install_path: &Path, expected: &str) -> Result<Value, Failure> {
        let file = join(&[&text(install_path), INSTALLED_FILE]);
        if !file.exists() {
            return Err(DeliveryError::new(
                Code::NotInstalled,
                "Игра не установлена — патч применять не к чему",
            )
            .into());
        }
        let bytes = node_fs::read_bytes(&file)?;
        let installed = hlsr_core::json_text::json_parse(&js::utf8_string(&bytes))
            .map_err(|error| RawError::new(error.to_string()))?;
        if installed.is_null() {
            return Err(jv::read_error(Some(&installed), "state").into());
        }
        let state = jv::prop(&installed, "state");
        if !jv::is_str(state, "ready") {
            return Err(DeliveryError::new(
                Code::NotInstalled,
                format!("Установка не завершена (state: {})", jv::template(state)),
            )
            .into());
        }
        let version = jv::prop(&installed, "version");
        if !jv::is_str(version, expected) {
            let mut details = Map::new();
            details.insert("expected".to_owned(), json!(expected));
            put(&mut details, "installed", version);
            return Err(DeliveryError::new(
                Code::VersionMismatch,
                format!(
                    "Патч рассчитан на версию {expected}, установлена {}",
                    jv::template(version)
                ),
            )
            .with_details(Value::Object(details))
            .into());
        }
        Ok(installed)
    }

    /// `assertGameNotRunning(installPath)`.
    fn assert_game_not_running(install_path: &Path) -> Result<(), DeliveryError> {
        match file_ops::is_game_running(install_path, &["hl2.exe"]) {
            Some(path) => Err(DeliveryError::new(
                Code::GameRunning,
                format!("Игра запущена ({path}). Закройте её и повторите обновление."),
            )
            .with_details(json!({ "path": path }))),
            None => Ok(()),
        }
    }

    /// `assertSpace(patchManifest)`: места хватает на два размера архива и запас; узнать место
    /// не вышло — не проверяется.
    fn assert_space(&self, patch_manifest: &Value) -> Result<(), DeliveryError> {
        let Some(free) = self.free_space.free_bytes(&self.paths.staging) else {
            return Ok(());
        };
        let size =
            jv::as_number(jv::prop(patch_manifest, "archive").and_then(|a| jv::prop(a, "size")));
        let required = size * 2.0 + self.space_margin as f64;
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

    /// `verifyArchiveFile(archivePath, patchManifest)`: размер и sha256 архива патча.
    fn verify_archive_file(archive_path: &Path, patch_manifest: &Value) -> Result<(), Failure> {
        let archive = jv::prop(patch_manifest, "archive");
        let field = |name: &str| archive.and_then(|a| jv::prop(a, name));
        let Ok(stats) = std::fs::metadata(archive_path) else {
            return Err(DeliveryError::new(
                Code::SizeMismatch,
                format!("Архив патча не найден: {}", text(archive_path)),
            )
            .into());
        };
        if field("size").and_then(jv::number) != Some(stats.len() as f64) {
            return Err(DeliveryError::new(
                Code::SizeMismatch,
                format!(
                    "Размер патча {} не совпадает с манифестом {}",
                    stats.len(),
                    jv::template(field("size"))
                ),
            )
            .into());
        }
        let digest = hash_file_sync(archive_path)?;
        if !jv::is_str(field("sha256"), &digest) {
            let mut details = Map::new();
            put(&mut details, "expected", field("sha256"));
            details.insert("actual".to_owned(), json!(digest));
            return Err(DeliveryError::new(
                Code::ChecksumMismatch,
                "SHA-256 архива патча не совпадает",
            )
            .with_details(Value::Object(details))
            .into());
        }
        Ok(())
    }

    /// `extract(archivePath, destination)`: прогресс — события `{state, phase: "extract", percent}`.
    async fn extract(&self, archive_path: &Path, destination: &Path) -> Result<(), DeliveryError> {
        let listeners = self.listeners.clone();
        let on_progress = Arc::new(move |fraction: f64| {
            let mut event = PatchEvent::new(None, PatchState::ApplyingUpdate);
            event.phase = Some("extract".to_owned());
            event.percent = Some(js::math_round(fraction * 100.0));
            emit_to(&listeners, &event);
        });
        self.archive
            .extract(
                archive_path,
                destination,
                ExtractOptions {
                    on_progress: Some(on_progress),
                },
            )
            .await
            .map(|_| ())
    }

    /// `verifyPayload(payloadRoot, patchManifest)`: каждый объявленный файл лежит в `payload/` с
    /// тем же размером и sha256.
    async fn verify_payload(payload_root: &Path, patch_manifest: &Value) -> Result<(), Failure> {
        let root = text(payload_root);
        for file in list(patch_manifest, "files") {
            let relative = file_path(file)?;
            let source = resolve_inside(&root, &relative, &format!("Файл патча \"{relative}\""))?;
            if !source.exists() {
                return Err(DeliveryError::new(
                    Code::RequiredFileMissing,
                    format!("В payload нет файла {relative}"),
                )
                .into());
            }
            let stats =
                std::fs::metadata(&source).map_err(|e| RawError::fs(&e, "stat", &source))?;
            if !same_size(&stats, jv::prop(file, "size")) {
                return Err(DeliveryError::new(
                    Code::SizeMismatch,
                    format!(
                        "Размер {relative} в патче {}, ожидался {}",
                        stats.len(),
                        jv::template(jv::prop(file, "size"))
                    ),
                )
                .into());
            }
            let digest = file_ops::hash_file(&source).await?;
            if !jv::is_str(jv::prop(file, "sha256"), &digest) {
                return Err(DeliveryError::new(
                    Code::ChecksumMismatch,
                    format!("SHA-256 не совпал для {relative}"),
                )
                .with_details(json!({ "path": relative }))
                .into());
            }
        }
        Ok(())
    }

    /// `backupAffected(journal, installPath, patchManifest)`: копии заменяемых и удаляемых файлов
    /// в rollback; что заменено, добавлено и удалено — в журнал.
    fn backup_affected(
        &self,
        journal: &mut Map<String, Value>,
        context: &Context<'_>,
        applied: &Value,
    ) -> Result<(), Failure> {
        let install = text(context.install_path);
        let rollback = text(context.rollback_path);
        let record = |journal: &mut Map<String, Value>, key: &str, relative: &str| {
            if let Some(Value::Array(items)) = journal.get_mut(key) {
                items.push(json!(relative));
            }
        };
        for file in list(applied, "files") {
            let relative = file_path(file)?;
            let target = resolve_inside(&install, &relative, &format!("Файл \"{relative}\""))?;
            if target.exists() {
                let backup = resolve_inside(&rollback, &relative, "Резервная копия")?;
                file_ops::copy_file_sync(&target, &backup, Some(&relative))?;
                record(journal, "replaced", &relative);
            } else {
                record(journal, "added", &relative);
            }
        }
        for item in list(applied, "deletedFiles") {
            let relative = rel_of(item);
            let target = resolve_inside(&install, &relative, &format!("Файл \"{relative}\""))?;
            if !target.exists() {
                continue;
            }
            let backup = resolve_inside(&rollback, &relative, "Резервная копия")?;
            file_ops::copy_file_sync(&target, &backup, Some(&relative))?;
            record(journal, "deleted", &relative);
        }
        self.write_journal(journal)?;
        Ok(())
    }

    /// `replaceFiles(journal, payloadRoot, installPath, patchManifest)`.
    fn replace_files(
        payload_root: &Path,
        install_path: &Path,
        applied: &Value,
    ) -> Result<(), Failure> {
        let (root, install) = (text(payload_root), text(install_path));
        for file in list(applied, "files") {
            let relative = file_path(file)?;
            let source = resolve_inside(&root, &relative, &format!("Файл патча \"{relative}\""))?;
            let target = resolve_inside(&install, &relative, &format!("Файл \"{relative}\""))?;
            file_ops::copy_file_sync(&source, &target, Some(&relative))?;
        }
        Ok(())
    }

    /// `deleteFiles(journal, installPath, patchManifest)`: удалить и убрать опустевшие папки.
    fn delete_files(install_path: &Path, applied: &Value) -> Result<(), Failure> {
        let install = text(install_path);
        for item in list(applied, "deletedFiles") {
            let relative = rel_of(item);
            let target = resolve_inside(&install, &relative, &format!("Файл \"{relative}\""))?;
            if !target.exists() {
                continue;
            }
            file_ops::remove_file_sync(&target, Some(&relative))?;
            let parent = NodePath::native().dirname(&text(&target));
            file_ops::prune_empty_dirs(install_path, Path::new(&parent));
        }
        Ok(())
    }

    /// `verifyResult(context, installPath, patchManifest)`: после замены файлы — как в манифесте
    /// патча, удалённых нет; иначе `VERIFY_FAILED` (первые 10 расхождений).
    async fn verify_result(install_path: &Path, applied: &Value) -> Result<Verification, Failure> {
        let install = text(install_path);
        let files = list(applied, "files");
        let deleted = list(applied, "deletedFiles");
        let mut problems = Vec::new();
        for (index, file) in files.iter().enumerate() {
            let relative = file_path(file)?;
            let full = resolve_inside(&install, &relative, &format!("Файл \"{relative}\""))?;
            let stats = tokio::fs::metadata(&full).await.ok();
            let digest = match &stats {
                Some(meta) if meta.is_file() && same_size(meta, jv::prop(file, "size")) => {
                    Some(file_ops::hash_file(&full).await?)
                }
                _ => None,
            };
            if let Some(reason) = patched_file_problem(file, stats.as_ref(), digest.as_deref()) {
                problems.push(Problem {
                    path: relative,
                    reason,
                });
            }
            if index % YIELD_EVERY == 0 {
                tokio::task::yield_now().await;
            }
        }
        for item in deleted {
            let relative = rel_of(item);
            let full = resolve_inside(&install, &relative, &format!("Файл \"{relative}\""))?;
            if full.exists() {
                problems.push(Problem {
                    path: relative,
                    reason: "должен был исчезнуть, но остался".to_owned(),
                });
            }
        }
        if !problems.is_empty() {
            let shown: Vec<&Problem> = problems.iter().take(10).collect();
            let listed: Vec<String> = shown
                .iter()
                .map(|p| format!("{} — {}", p.path, p.reason))
                .collect();
            let more = if problems.len() > shown.len() {
                format!(" (и ещё {})", problems.len() - shown.len())
            } else {
                String::new()
            };
            let details: Vec<Value> = shown
                .iter()
                .map(|p| json!({"path": p.path, "reason": p.reason}))
                .collect();
            return Err(DeliveryError::new(
                Code::VerifyFailed,
                format!(
                    "Патч лёг не полностью, не совпало файлов: {}. {}{more}",
                    problems.len(),
                    listed.join("; ")
                ),
            )
            .with_details(json!({ "problems": details, "total": problems.len() }))
            .into());
        }
        Ok(Verification {
            files_manifest_sha256: jv::prop(applied, "resultingFilesManifestSha256").cloned(),
            total_files: jv::prop(applied, "resultingTotalFiles").cloned(),
            total_size: jv::prop(applied, "resultingTotalSize").cloned(),
            changed_files: files.len(),
            deleted_files: deleted.len(),
        })
    }

    /// `buildMetadataSnapshot(installPath, patchManifest, context)`: патч заменил метаданные
    /// сборки — их снимок для `installed.json` (сверенный с манифестом игры); не сошлось — `None`
    /// и строка журнала.
    fn build_metadata_snapshot(
        &self,
        install_path: &Path,
        applied: &Value,
        context: &Context<'_>,
    ) -> Option<Map<String, Value>> {
        let manifest = context.game_manifest;
        let descriptor = jv::prop(manifest, "buildMetadata").filter(|d| js::truthy(d))?;
        let touched = list(applied, "files")
            .iter()
            .any(|file| jv::is_str(jv::prop(file, "path"), RELATIVE_PATH));
        if !touched {
            return None;
        }
        let options = VerifyForInstall {
            install_root: install_path,
            descriptor,
            manifest: Some(manifest),
            files_manifest: None,
            launcher_version: self.launcher_version.as_deref(),
            expected: None,
            now: Some(self.now_ms() as f64),
        };
        match build_metadata_trust::verify_for_install(&options) {
            Ok(verified) => build_metadata_trust::snapshot_from_verified(&verified)
                .to_value()
                .as_object()
                .cloned(),
            Err(error) => {
                self.logger.warn(&format!(
                    "[game-delivery] сведения о сборке после патча не обновлены ({}): {}",
                    error.code.as_str(),
                    error.message
                ));
                None
            }
        }
    }

    /// `verifyResultSync(installPath, { files, deletedFiles })`: расхождения файлов с журналом.
    fn verify_result_sync(
        &self,
        install_path: &str,
        files: &[Value],
        deleted: &[Value],
    ) -> Result<Vec<Problem>, Failure> {
        let mut problems = Vec::new();
        for file in files {
            let relative = file_path(file)?;
            let full = resolve_inside(install_path, &relative, &format!("Файл \"{relative}\""))?;
            let stats = std::fs::metadata(&full).ok();
            let digest = match &stats {
                Some(meta) if meta.is_file() && same_size(meta, jv::prop(file, "size")) => {
                    Some(hash_file_sync(&full)?)
                }
                _ => None,
            };
            if let Some(reason) = patched_file_problem(file, stats.as_ref(), digest.as_deref()) {
                problems.push(Problem {
                    path: relative,
                    reason,
                });
            }
        }
        for item in deleted {
            let relative = rel_of(item);
            let full = resolve_inside(install_path, &relative, &format!("Файл \"{relative}\""))?;
            if full.exists() {
                problems.push(Problem {
                    path: relative,
                    reason: "должен был исчезнуть, но остался".to_owned(),
                });
            }
        }
        Ok(problems)
    }

    /// `writeInstalledAtomic(installPath, changes)`: `Object.assign({}, installed, changes,
    /// {updatedAt, state: "ready"})`; `None` в изменениях — `undefined` (поле пропадает из файла).
    fn write_installed_atomic(
        &self,
        install_path: &str,
        changes: &[(&str, Option<Value>)],
    ) -> Result<Value, RawError> {
        let file = join(&[install_path, INSTALLED_FILE]);
        let installed = read_json_safe(&file);
        let mut updated: JsObject<Option<Value>> = JsObject::new();
        if js::truthy(&installed) {
            for (key, value) in assign_source(&installed) {
                updated.insert(key, Some(value));
            }
        }
        for (key, value) in changes {
            updated.insert(*key, value.clone());
        }
        updated.insert("updatedAt", Some(json!(self.now_iso())));
        updated.insert("state", Some(json!("ready")));
        let mut out = Map::new();
        for (key, value) in updated.iter() {
            if let Some(value) = value {
                out.insert(key.to_owned(), value.clone());
            }
        }
        let out = Value::Object(out);
        write_json_atomic(&file, &out)?;
        Ok(out)
    }

    /// `restoreInstalledBackup(journal)`: `installed.json` из копии в rollback.
    fn restore_installed_backup(&self, journal: &Value) -> Result<bool, Failure> {
        let backup = jv::prop(journal, "installedBackup");
        if !jv::truthy(backup) || !exists_value(backup) {
            return Ok(false);
        }
        let backup = backup.and_then(Value::as_str).unwrap_or_default();
        let install = path_arg(jv::prop(journal, "installPath"))?;
        file_ops::copy_file_sync(
            Path::new(backup),
            &join(&[&install, INSTALLED_FILE]),
            Some(INSTALLED_FILE),
        )?;
        Ok(true)
    }

    /// `rollback(journal)`: заменённые и удалённые файлы — из rollback, добавленные — убрать.
    fn rollback(&self, journal: &Value) -> Result<usize, Failure> {
        let rollback = jv::prop(journal, "rollbackPath");
        if !jv::truthy(rollback) || !exists_value(rollback) {
            return Ok(0);
        }
        let rollback = rollback.and_then(Value::as_str).unwrap_or_default();
        let install = || path_arg(jv::prop(journal, "installPath"));
        let mut restored = 0;
        let touched = list(journal, "replaced")
            .iter()
            .chain(list(journal, "deleted"));
        for item in touched {
            let relative = rel_of(item);
            let backup = resolve_inside(rollback, &relative, "Резервная копия")?;
            if !backup.exists() {
                continue;
            }
            let target = resolve_inside(&install()?, &relative, &format!("Файл \"{relative}\""))?;
            file_ops::copy_file_sync(&backup, &target, Some(&relative))?;
            restored += 1;
        }
        for item in list(journal, "added") {
            let relative = rel_of(item);
            let install = install()?;
            let target = resolve_inside(&install, &relative, &format!("Файл \"{relative}\""))?;
            if !target.exists() {
                continue;
            }
            file_ops::remove_file_sync(&target, Some(&relative))?;
            let parent = NodePath::native().dirname(&text(&target));
            file_ops::prune_empty_dirs(Path::new(&install), Path::new(&parent));
        }
        self.logger.info(&format!(
            "[game-delivery] откат патча: восстановлено файлов — {restored}"
        ));
        Ok(restored)
    }

    /// `cleanupTransaction(journal)`: убрать staging и rollback патча (только внутри своих зон).
    fn cleanup_transaction(&self, journal: &Value) {
        for (zone, key) in [
            (&self.paths.staging, "stagingPath"),
            (&self.paths.rollback, "rollbackPath"),
        ] {
            let target = jv::prop(journal, key);
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
}
