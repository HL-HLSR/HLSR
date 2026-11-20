//! Ремонт (`RepairService.js` 0.4.9): замена повреждённых и пропавших файлов эталонными — только
//! из набора, который уже скачан с пофайлового зеркала ремонта (`preparedRoot`). Полный архив не
//! используется никогда.
//!
//! Ход транзакции пишется в журнал `.hlsr/state/<gameId>.repair.json` (атомарно, `{version: 1, …}`):
//! `preflight` → `verify-source` (у эталона и цели путь внутри своих папок и после разворота
//! ссылок, файл эталона на месте, размер и SHA-256 как в манифесте) → `replace` (копия
//! заменяемого файла в rollback, запись в журнал, замена; новые файлы — в `created`) → `recheck`
//! (SHA-256 заменённых файлов). Данные игрока (`preservePaths`, изменяемые пути) не
//! ремонтируются: такой файл в запросе — `REPAIR_FAILED`. Исключение — стоковые cfg
//! (`stockConfigs` манифеста): они лежат в папках игрока, но чинятся по подписанному списку
//! версии.
//!
//! Сбой в том же процессе откатывается сразу (файлы из rollback, созданные — убрать, опустевшие
//! папки — убрать), неудачный откат — `ROLLBACK_FAILED`, журнал и rollback остаются. Сбой
//! процесса (точки [`FAULTS`]) разбирает [`RepairService::recover`] по журналу: возвращает файлы
//! на фазах `replace` и `recheck`, на прочих — только убирает за собой.
//!
//! Отмена. Сам ремонт не отменяется (отменяется загрузка с зеркала до него). Брошенное будущее
//! [`RepairService::repair`] ведёт себя как сбой процесса: сервис освобождается (`finally`), а
//! журнал и rollback остаются на диске. Бросить его можно только в точках ожидания — проверка
//! эталона (до первой замены, установка не тронута) и проверка после замены (файлы заменены,
//! журнал на фазе `recheck`); цикл замены ожиданий не содержит. Состояние в любой из них разбирает `recover` при
//! следующем запуске так же, как после сбоя процесса: на `recheck` заменённые файлы возвращаются к
//! повреждённым. Занятый сервис чужой вызов не пустит, пока брошенное будущее не сброшено.
//!
//! Отличия от 0.4.7: файлы запроса — строки (форма `{path}` в 0.4.7 никому не передаётся);
//! брошенная задача освобождает сервис; сервис занят уже при первой записи журнала (в 0.4.7 так
//! же: её ошибка не оставляет его занятым).

use crate::atomic::write_json_atomic;
use crate::errors::{Code, DeliveryError, Failure, RawError, to_delivery_error};
use crate::file_ops;
use crate::install_service::{
    FaultInjector, assert_inside, exists_value, join, path_arg, put, text,
};
use crate::js_value as jv;
use crate::mutable_paths::matches_mutable;
use crate::node_fs;
use crate::path_match::Matcher;
use crate::paths::is_game_id;
use crate::paths::{LibraryPaths, assert_real_inside, remove_within_zone};
use crate::ports::{Logger, TracingLogger};
use crate::timing::{Clock, iso_from_ms, system_clock};
use hlsr_core::js;
use hlsr_core::js::path::NodePath;
use hlsr_core::json_store::read_json_safe;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// `MAX_FILES_PER_PASS`: больше файлов за один ремонт не берётся — надёжнее переустановить.
pub const MAX_FILES_PER_PASS: usize = 2000;
/// `JOURNAL_VERSION`: версия журнала `<id>.repair.json`.
pub const JOURNAL_VERSION: u32 = 1;
/// `FAULTS`: точки отказа (для проверки восстановления).
pub const FAULTS: [&str; 2] = ["after-replace", "before-recheck"];
/// Фазы, сбой на которых требует отката файлов (`needsRollback`).
const ROLLBACK_PHASES: [&str; 2] = ["replace", "recheck"];

/// Слушатель событий `state` и `progress`.
pub type Listener = Arc<dyn Fn(&RepairEvent) + Send + Sync>;

// ---------------------------------------------------------------- события

/// Состояние в событии `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairState {
    Repairing,
    RepairComplete,
    Error,
}

impl RepairState {
    pub fn as_str(self) -> &'static str {
        match self {
            RepairState::Repairing => "repairing",
            RepairState::RepairComplete => "repair-complete",
            RepairState::Error => "error",
        }
    }
}

/// Событие сервиса: `state` или `progress`.
#[derive(Debug, Clone, PartialEq)]
pub enum RepairEvent {
    /// `{gameId, state, phase}`; `repair-complete` — `repaired`; `error` — `error` (`toPublic()`).
    State {
        game_id: String,
        state: RepairState,
        phase: Option<String>,
        total: Option<usize>,
        repaired: Option<usize>,
        error: Option<Value>,
    },
    /// `{gameId, checked, total, percent}` после каждого заменённого файла.
    Progress {
        game_id: String,
        checked: usize,
        total: usize,
        percent: f64,
    },
}

impl RepairEvent {
    fn state(game_id: &str, state: RepairState) -> RepairEvent {
        RepairEvent::State {
            game_id: game_id.to_owned(),
            state,
            phase: None,
            total: None,
            repaired: None,
            error: None,
        }
    }

    /// Имя события в `emit`: `state` или `progress`.
    pub fn name(&self) -> &'static str {
        match self {
            RepairEvent::State { .. } => "state",
            RepairEvent::Progress { .. } => "progress",
        }
    }

    /// Объект события, как его получает слушатель в 0.4.7.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        match self {
            RepairEvent::State {
                game_id,
                state,
                phase,
                total,
                repaired,
                error,
            } => {
                out.insert("gameId".to_owned(), json!(game_id));
                out.insert("state".to_owned(), json!(state.as_str()));
                if let Some(phase) = phase {
                    out.insert("phase".to_owned(), json!(phase));
                }
                if let Some(total) = total {
                    out.insert("total".to_owned(), json!(total));
                }
                if let Some(repaired) = repaired {
                    out.insert("repaired".to_owned(), json!(repaired));
                }
                if let Some(error) = error {
                    out.insert("error".to_owned(), error.clone());
                }
            }
            RepairEvent::Progress {
                game_id,
                checked,
                total,
                percent,
            } => {
                out.insert("gameId".to_owned(), json!(game_id));
                out.insert("checked".to_owned(), json!(checked));
                out.insert("total".to_owned(), json!(total));
                out.insert("percent".to_owned(), js_number(*percent));
            }
        }
        Value::Object(out)
    }
}

// ---------------------------------------------------------------- запрос, итоги, параметры

/// `repair(request)`.
#[derive(Debug, Clone, Default)]
pub struct RepairRequest {
    pub game_id: String,
    pub install_path: PathBuf,
    /// `files.json` установленной версии.
    pub files_manifest: Value,
    pub preserve_paths: Vec<String>,
    /// Пути файлов, которые заменить (повреждённые и пропавшие).
    pub damaged_files: Vec<String>,
    /// Каталог файлов эталона, уже скачанных с зеркала; нет — `REPAIR_SOURCE_MISSING`.
    pub prepared_root: Option<PathBuf>,
}

/// Итог ремонта.
#[derive(Debug, Clone, PartialEq)]
pub enum RepairReport {
    /// `{status: "nothing-to-repair", repaired: [], failed: []}`.
    NothingToRepair,
    /// `{status: "repaired", repaired, failed: []}`.
    Repaired { repaired: Vec<String> },
}

impl RepairReport {
    pub fn to_json(&self) -> Value {
        match self {
            RepairReport::NothingToRepair => {
                json!({"status": "nothing-to-repair", "repaired": [], "failed": []})
            }
            RepairReport::Repaired { repaired } => json!({
                "status": "repaired",
                "repaired": repaired,
                "failed": [],
            }),
        }
    }
}

/// Итог `recover`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RepairRecovery {
    pub recovered: bool,
    /// Журнал был повреждён: убран, нужна проверка файлов.
    pub corrupt: bool,
    /// Фаза журнала.
    pub phase: Option<Value>,
    pub restored: usize,
    pub removed: usize,
}

impl RepairRecovery {
    /// `{recovered: false}`, `{recovered: false, corrupt: true, verifyRecommended: true}` или
    /// `{recovered: true, phase, restored, removed}`.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("recovered".to_owned(), json!(self.recovered));
        if self.corrupt {
            out.insert("corrupt".to_owned(), json!(true));
            out.insert("verifyRecommended".to_owned(), json!(true));
        }
        if self.recovered {
            put(&mut out, "phase", self.phase.as_ref());
            out.insert("restored".to_owned(), json!(self.restored));
            out.insert("removed".to_owned(), json!(self.removed));
        }
        Value::Object(out)
    }
}

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct RepairOptions {
    pub logger: Option<Arc<dyn Logger>>,
    pub fault_injector: Option<FaultInjector>,
    /// Часы (`Date.now()`); для тестов.
    pub clock: Option<Clock>,
}

// ---------------------------------------------------------------- сервис

/// Журнал на диске (`readJournal`).
enum Journal {
    Valid(Value),
    Corrupt,
}

/// Идущий ремонт: при выходе из `repair` (и при брошенном будущем) сервис свободен, как `finally`.
struct Busy<'a>(&'a AtomicBool);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// `RepairService`.
pub struct RepairService {
    paths: LibraryPaths,
    logger: Arc<dyn Logger>,
    fault_injector: Option<FaultInjector>,
    clock: Clock,
    busy: AtomicBool,
    listeners: Mutex<Vec<Listener>>,
}

/// `path` записи манифеста; у `null` — `TypeError`, как `file.path` в JS.
fn file_path(file: &Value) -> Result<String, RawError> {
    if file.is_null() {
        return Err(jv::read_error(Some(file), "path"));
    }
    Ok(jv::template(jv::prop(file, "path")))
}

/// `stats.size === file.size`.
fn same_size(len: u64, size: Option<&Value>) -> bool {
    size.and_then(jv::number) == Some(len as f64)
}

/// Список поля журнала; не массив — пусто.
fn list<'a>(value: &'a Value, name: &str) -> &'a [Value] {
    jv::array(jv::prop(value, name)).map_or(&[], Vec::as_slice)
}

/// `fileOps.resolveInside(root, relativePath, what)`.
fn resolve_inside(root: &str, relative: &str, what: &str) -> Result<PathBuf, DeliveryError> {
    file_ops::resolve_inside(Path::new(root), relative, Some(what))
}

impl RepairService {
    /// `new RepairService({ paths, logger, faultInjector })`.
    pub fn new(paths: LibraryPaths, options: RepairOptions) -> RepairService {
        RepairService {
            logger: options
                .logger
                .unwrap_or_else(|| Arc::new(TracingLogger) as Arc<dyn Logger>),
            fault_injector: options.fault_injector,
            clock: options.clock.unwrap_or_else(system_clock),
            busy: AtomicBool::new(false),
            listeners: Mutex::new(Vec::new()),
            paths,
        }
    }

    pub fn paths(&self) -> &LibraryPaths {
        &self.paths
    }

    /// `on("state" | "progress", listener)`: слушатель зовётся сразу, в той же точке кода, где
    /// рождается событие; вид события — [`RepairEvent::name`].
    pub fn on_event(&self, listener: impl Fn(&RepairEvent) + Send + Sync + 'static) {
        self.listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::new(listener));
    }

    fn emit(&self, event: &RepairEvent) {
        let list: Vec<Listener> = self
            .listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        for listener in list {
            listener(event);
        }
    }

    /// `busy`: идёт ремонт.
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    fn now_ms(&self) -> i64 {
        (self.clock)()
    }

    fn now_iso(&self) -> String {
        iso_from_ms(self.now_ms())
    }

    /// `fault(name, context)`.
    fn fault(&self, name: &str, context: &Value) -> Result<(), Failure> {
        match &self.fault_injector {
            Some(injector) => injector(name, context),
            None => Ok(()),
        }
    }

    // ------------------------------------------------------------ журнал

    /// `journalPath(gameId)`: `.hlsr/state/<id>.repair.json`; id — строчные слова через дефис.
    pub fn journal_path(&self, game_id: &str) -> Result<PathBuf, DeliveryError> {
        if !is_game_id(game_id) {
            return Err(DeliveryError::new(
                Code::PathInvalid,
                format!("Недопустимый gameId: {game_id}"),
            ));
        }
        Ok(join(&[
            &text(&self.paths.state),
            &format!("{game_id}.repair.json"),
        ]))
    }

    /// `writeJournal(journal)`: `updatedAt`, атомарная запись `{version: 1, …}`.
    fn write_journal(&self, journal: &mut Map<String, Value>) -> Result<(), Failure> {
        node_fs::mkdir_recursive(&self.paths.state)?;
        journal.insert("updatedAt".to_owned(), json!(self.now_iso()));
        let mut out = Map::new();
        out.insert("version".to_owned(), json!(JOURNAL_VERSION));
        for (key, value) in journal.iter() {
            out.insert(key.clone(), value.clone());
        }
        let game_id = jv::template(journal.get("gameId"));
        write_json_atomic(&self.journal_path(&game_id)?, &Value::Object(out))?;
        Ok(())
    }

    /// `readJournal(gameId)`: нет файла — `None`; не JSON или не той версии — повреждён.
    fn read_journal(&self, game_id: &str) -> Result<Option<Journal>, DeliveryError> {
        let file = self.journal_path(game_id)?;
        if !file.exists() {
            return Ok(None);
        }
        let data = read_json_safe(&file);
        let valid = js::truthy(&data)
            && matches!(jv::prop(&data, "version"), Some(Value::Number(n))
                if n.as_f64() == Some(f64::from(JOURNAL_VERSION)));
        Ok(Some(if valid {
            Journal::Valid(data)
        } else {
            Journal::Corrupt
        }))
    }

    /// `clearJournal(gameId)`: ошибка — строка журнала, не исключение.
    fn clear_journal(&self, game_id: &str) {
        let removed = self
            .journal_path(game_id)
            .map_err(Failure::from)
            .and_then(|file| {
                if file.exists() {
                    node_fs::unlink(&file)?;
                }
                Ok(())
            });
        if let Err(error) = removed {
            self.logger.warn(&format!(
                "[game-delivery] журнал ремонта не удалён: {error}"
            ));
        }
    }

    /// `setPhase(journal, phase)`: фаза в журнал и событие `repairing`.
    fn set_phase(&self, journal: &mut Map<String, Value>, phase: &str) -> Result<(), Failure> {
        journal.insert("phase".to_owned(), json!(phase));
        self.write_journal(journal)?;
        let game_id = jv::template(journal.get("gameId"));
        let mut event = RepairEvent::state(&game_id, RepairState::Repairing);
        if let RepairEvent::State { phase: slot, .. } = &mut event {
            *slot = Some(phase.to_owned());
        }
        self.emit(&event);
        Ok(())
    }

    /// `findPending(gameId)`: незавершённый ремонт по журналу.
    pub fn find_pending(&self, game_id: &str) -> Result<Option<Value>, DeliveryError> {
        let journal = match self.read_journal(game_id)? {
            None => return Ok(None),
            Some(Journal::Corrupt) => {
                return Ok(Some(
                    json!({"gameId": game_id, "corrupt": true, "phase": "unknown"}),
                ));
            }
            Some(Journal::Valid(journal)) => journal,
        };
        let field = |name: &str| jv::prop(&journal, name);
        let mut out = Map::new();
        for key in ["gameId", "phase", "startedAt", "updatedAt"] {
            put(&mut out, key, field(key));
        }
        out.insert(
            "replaced".to_owned(),
            json!(list(&journal, "replaced").len()),
        );
        out.insert("created".to_owned(), json!(list(&journal, "created").len()));
        out.insert(
            "needsRollback".to_owned(),
            json!(
                ROLLBACK_PHASES
                    .iter()
                    .any(|p| jv::is_str(field("phase"), p))
            ),
        );
        Ok(Some(Value::Object(out)))
    }

    /// `recover(gameId)`: разобрать незавершённый ремонт по журналу — вернуть файлы в исходное
    /// состояние и убрать за собой. Повреждённый журнал убирается: нужна проверка файлов.
    pub fn recover(&self, game_id: &str) -> Result<RepairRecovery, Failure> {
        let journal = match self.read_journal(game_id)? {
            None => return Ok(RepairRecovery::default()),
            Some(Journal::Corrupt) => {
                self.clear_journal(game_id);
                return Ok(RepairRecovery {
                    corrupt: true,
                    ..RepairRecovery::default()
                });
            }
            Some(Journal::Valid(journal)) => journal,
        };
        self.logger.warn(&format!(
            "[game-delivery] найден незавершённый ремонт {game_id} на фазе {}, \
             возвращаю файлы в исходное состояние",
            jv::template(jv::prop(&journal, "phase"))
        ));
        let (restored, removed) = self.rollback(&journal)?;
        self.cleanup_transaction(&journal);
        self.clear_journal(game_id);
        Ok(RepairRecovery {
            recovered: true,
            corrupt: false,
            phase: jv::prop(&journal, "phase").cloned(),
            restored,
            removed,
        })
    }

    // ------------------------------------------------------------ источник

    /// `preparedRoot(request)`: эталонные файлы приходят только из пофайлового зеркала
    /// (`repair-mirror.js`); каталог должен существовать.
    fn prepared_root(request: &RepairRequest) -> Result<String, DeliveryError> {
        match &request.prepared_root {
            Some(root) if root.exists() => Ok(text(root)),
            root => Err(DeliveryError::new(
                Code::RepairSourceMissing,
                format!(
                    "Каталог подготовленных эталонных файлов не существует: {}",
                    root.as_ref()
                        .map_or_else(|| "undefined".to_owned(), |root| text(root))
                ),
            )),
        }
    }

    // ------------------------------------------------------------ ремонт

    /// `repair(request)`: заменить повреждённые файлы эталонными. Ошибки до транзакции (запрос,
    /// источник, игра запущена) — как пришли и сервис не занимают; ошибки транзакции —
    /// `DeliveryError` (чужие — с кодом `REPAIR_FAILED`) после отката.
    pub async fn repair(&self, request: RepairRequest) -> Result<RepairReport, Failure> {
        if self.is_busy() {
            return Err(
                DeliveryError::new(Code::AlreadyRunning, "Восстановление уже выполняется").into(),
            );
        }

        let manifest = &request.files_manifest;
        let preserved = Matcher::new(&request.preserve_paths);
        let Some(files) = jv::array(jv::prop(manifest, "files")) else {
            return Err(jv::read_error(jv::prop(manifest, "files"), "concat").into());
        };
        // Стоковые cfg из папок игрока чинятся только по подписанному списку версии.
        let stock_list =
            jv::array(jv::prop(manifest, "stockConfigs")).map_or(&[][..], Vec::as_slice);
        let mut stock: HashSet<String> = HashSet::new();
        for file in stock_list {
            stock.insert(file_path(file)?);
        }
        let mut by_path: HashMap<String, &Value> = HashMap::new();
        for file in files.iter().chain(stock_list) {
            let path = file_path(file)?;
            by_path.insert(path, file);
        }

        let mut wanted: Vec<&Value> = Vec::new();
        for relative in &request.damaged_files {
            if !stock.contains(relative)
                && (preserved.matches(relative) || matches_mutable(relative))
            {
                return Err(DeliveryError::new(
                    Code::RepairFailed,
                    format!(
                        "Файл {relative} относится к пользовательским данным и не восстанавливается"
                    ),
                )
                .into());
            }
            let Some(&entry) = by_path.get(relative.as_str()) else {
                return Err(DeliveryError::new(
                    Code::RepairFailed,
                    format!("Файла {relative} нет в файловом манифесте"),
                )
                .into());
            };
            wanted.push(entry);
        }

        if wanted.is_empty() {
            return Ok(RepairReport::NothingToRepair);
        }
        if wanted.len() > MAX_FILES_PER_PASS {
            return Err(DeliveryError::new(
                Code::RepairFailed,
                format!(
                    "Повреждено слишком много файлов ({}) — надёжнее переустановить сборку",
                    wanted.len()
                ),
            )
            .with_details(json!({ "damaged": wanted.len() }))
            .into());
        }

        let prepared = Self::prepared_root(&request)?;

        if let Some(path) = file_ops::is_game_running(&request.install_path, &["hl2.exe"]) {
            return Err(DeliveryError::new(
                Code::GameRunning,
                format!("Игра запущена ({path}). Закройте её и повторите восстановление."),
            )
            .into());
        }

        let id = format!(
            "repair-{}-{}",
            request.game_id,
            js::base36(u128::try_from(self.now_ms()).unwrap_or(0))
        );
        let rollback_path = assert_inside(
            &self.paths.rollback,
            &join(&[&text(&self.paths.rollback), &id]),
            "rollback ремонта",
        )?;

        let mut journal = Map::new();
        journal.insert("gameId".to_owned(), json!(request.game_id));
        journal.insert("phase".to_owned(), json!("preflight"));
        journal.insert("installPath".to_owned(), json!(text(&request.install_path)));
        journal.insert("rollbackPath".to_owned(), json!(text(&rollback_path)));
        journal.insert("preparedRoot".to_owned(), json!(prepared));
        journal.insert("replaced".to_owned(), json!([]));
        journal.insert("created".to_owned(), json!([]));
        journal.insert("startedAt".to_owned(), json!(self.now_iso()));

        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(
                DeliveryError::new(Code::AlreadyRunning, "Восстановление уже выполняется").into(),
            );
        }
        // Сервис занят уже на первой записи журнала: её ошибка (как любая дальше) не оставит
        // его занятым.
        let busy = Busy(&self.busy);

        let outcome = match self.transaction(&mut journal, &request, &wanted).await {
            Ok(report) => {
                let journal = Value::Object(journal);
                self.cleanup_transaction(&journal);
                self.clear_journal(&request.game_id);
                Ok(report)
            }
            Err(error) => Err(self.fail(&journal, error, &request.game_id)),
        };
        drop(busy);
        outcome
    }

    /// Тело `try` ремонта: каталог отката, первая запись журнала, `runTransaction`.
    async fn transaction(
        &self,
        journal: &mut Map<String, Value>,
        request: &RepairRequest,
        wanted: &[&Value],
    ) -> Result<RepairReport, Failure> {
        node_fs::mkdir_recursive(Path::new(&jv::template(journal.get("rollbackPath"))))?;
        self.write_journal(journal)?;
        self.run_transaction(journal, request, wanted).await
    }

    /// `catch` ремонта: откат; неудачный откат — `ROLLBACK_FAILED` (журнал и rollback остаются).
    fn fail(&self, journal: &Map<String, Value>, error: Failure, game_id: &str) -> Failure {
        let delivery = to_delivery_error(error, Some(Code::RepairFailed));
        let journal_value = Value::Object(journal.clone());
        let mut event = RepairEvent::state(game_id, RepairState::Error);
        if let RepairEvent::State { error, .. } = &mut event {
            *error = Some(delivery.to_public());
        }
        if let Err(rollback_error) = self.rollback(&journal_value) {
            self.logger.error(&format!(
                "[game-delivery] откат ремонта не удался: {rollback_error}"
            ));
            self.emit(&event);
            let rollback_path = jv::template(journal.get("rollbackPath"));
            return DeliveryError::new(
                Code::RollbackFailed,
                format!(
                    "Ремонт не выполнен и откат не удался: {rollback_error}. \
                     Исходные файлы сохранены в {rollback_path}"
                ),
            )
            .with_details(json!({
                "rollbackPath": journal.get("rollbackPath"),
                "original": delivery.to_public(),
            }))
            .into();
        }
        self.cleanup_transaction(&journal_value);
        self.clear_journal(game_id);
        self.emit(&event);
        delivery.into()
    }

    /// `runTransaction(journal, request, wanted)`.
    async fn run_transaction(
        &self,
        journal: &mut Map<String, Value>,
        request: &RepairRequest,
        wanted: &[&Value],
    ) -> Result<RepairReport, Failure> {
        let game_id = request.game_id.as_str();
        let install_path = jv::template(journal.get("installPath"));
        let extracted_root = jv::template(journal.get("preparedRoot"));
        let rollback_path = jv::template(journal.get("rollbackPath"));

        self.set_phase(journal, "verify-source")?;

        for file in wanted {
            let path = file_path(file)?;
            let source = resolve_inside(&extracted_root, &path, &format!("Эталон \"{path}\""))?;
            let target = resolve_inside(&install_path, &path, &format!("Файл \"{path}\""))?;

            // Ни одной записи, пока у всех целей путь не проверен и после разворота ссылок:
            // junction вместо папки игры увёл бы файл за её пределы.
            assert_real_inside(
                Path::new(&extracted_root),
                &source,
                Some(&format!("Эталон \"{path}\"")),
            )?;
            assert_real_inside(
                Path::new(&install_path),
                &target,
                Some(&format!("Файл \"{path}\"")),
            )?;

            if !source.exists() {
                return Err(DeliveryError::new(
                    Code::RepairFailed,
                    format!("В наборе эталонных файлов нет {path}"),
                )
                .into());
            }
            let size = std::fs::metadata(&source)
                .map_err(|e| RawError::fs(&e, "stat", &source))?
                .len();
            if !same_size(size, jv::prop(file, "size")) {
                let mut details = Map::new();
                details.insert("path".to_owned(), json!(path));
                put(&mut details, "expected", jv::prop(file, "size"));
                details.insert("actual".to_owned(), json!(size));
                return Err(DeliveryError::new(
                    Code::RepairFailed,
                    format!("Размер эталонного файла {path} не совпал с манифестом"),
                )
                .with_details(Value::Object(details))
                .into());
            }
            let digest = file_ops::hash_file(&source).await?;
            if !jv::is_str(jv::prop(file, "sha256"), &digest) {
                let mut details = Map::new();
                details.insert("path".to_owned(), json!(path));
                put(&mut details, "expected", jv::prop(file, "sha256"));
                details.insert("actual".to_owned(), json!(digest));
                return Err(DeliveryError::new(
                    Code::RepairFailed,
                    format!("Эталонный файл {path} не совпал с манифестом"),
                )
                .with_details(Value::Object(details))
                .into());
            }
        }

        self.set_phase(journal, "replace")?;
        let mut restored: Vec<String> = Vec::new();
        for (index, file) in wanted.iter().enumerate() {
            let path = file_path(file)?;
            let source = resolve_inside(&extracted_root, &path, &format!("Эталон \"{path}\""))?;
            let target = resolve_inside(&install_path, &path, &format!("Файл \"{path}\""))?;

            if target.exists() {
                let backup = resolve_inside(&rollback_path, &path, "Резервная копия")?;
                file_ops::copy_file_sync(&target, &backup, Some(&path))?;
                push_to(journal, "replaced", &path);
            } else {
                push_to(journal, "created", &path);
            }
            self.write_journal(journal)?;

            file_ops::copy_file_sync(&source, &target, Some(&path))?;
            restored.push(path.clone());

            self.fault(
                "after-replace",
                &json!({
                    "index": index,
                    "path": path,
                    "journal": Value::Object(journal.clone()),
                }),
            )?;

            self.emit(&RepairEvent::Progress {
                game_id: game_id.to_owned(),
                checked: index + 1,
                total: wanted.len(),
                percent: ((index + 1) as f64 / wanted.len() as f64) * 100.0,
            });
        }

        self.set_phase(journal, "recheck")?;
        self.fault(
            "before-recheck",
            &json!({ "journal": Value::Object(journal.clone()) }),
        )?;

        let mut failed: Vec<String> = Vec::new();
        for file in wanted {
            let path = file_path(file)?;
            let target = resolve_inside(&install_path, &path, &format!("Файл \"{path}\""))?;
            let digest = file_ops::hash_file(&target).await?;
            if !jv::is_str(jv::prop(file, "sha256"), &digest) {
                failed.push(path);
            }
        }
        if !failed.is_empty() {
            return Err(DeliveryError::new(
                Code::RepairFailed,
                format!(
                    "После замены не совпали файлы: {}",
                    failed
                        .iter()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with_details(json!({ "failed": failed }))
            .into());
        }

        self.logger.info(&format!(
            "[game-delivery] восстановлено файлов: {} ({game_id}), источник — зеркало",
            restored.len()
        ));
        let mut event = RepairEvent::state(game_id, RepairState::RepairComplete);
        if let RepairEvent::State { repaired, .. } = &mut event {
            *repaired = Some(restored.len());
        }
        self.emit(&event);

        Ok(RepairReport::Repaired { repaired: restored })
    }

    /// `rollback(journal)`: заменённые файлы — из rollback, созданные — убрать (и опустевшие
    /// папки). Возвращает (возвращено, убрано).
    fn rollback(&self, journal: &Value) -> Result<(usize, usize), Failure> {
        let rollback = || path_arg(jv::prop(journal, "rollbackPath"));
        let install = || path_arg(jv::prop(journal, "installPath"));
        let mut restored = 0;
        let mut removed = 0;

        for item in list(journal, "replaced") {
            let relative = jv::template(Some(item));
            let backup = resolve_inside(&rollback()?, &relative, "Резервная копия")?;
            if !backup.exists() {
                continue;
            }
            let target = resolve_inside(&install()?, &relative, &format!("Файл \"{relative}\""))?;
            file_ops::copy_file_sync(&backup, &target, Some(&relative))?;
            restored += 1;
        }

        for item in list(journal, "created") {
            let relative = jv::template(Some(item));
            let install = install()?;
            let target = resolve_inside(&install, &relative, &format!("Файл \"{relative}\""))?;
            if !target.exists() {
                continue;
            }
            file_ops::remove_file_sync(&target, Some(&relative))?;
            let parent = NodePath::native().dirname(&text(&target));
            file_ops::prune_empty_dirs(Path::new(&install), Path::new(&parent));
            removed += 1;
        }

        self.logger.info(&format!(
            "[game-delivery] откат ремонта: возвращено {restored}, удалено созданных {removed}"
        ));
        Ok((restored, removed))
    }

    /// `cleanupTransaction(journal)`: убрать staging и rollback ремонта (только внутри своих зон).
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

/// `journal[key].push(path)`.
fn push_to(journal: &mut Map<String, Value>, key: &str, path: &str) {
    if let Some(Value::Array(items)) = journal.get_mut(key) {
        items.push(json!(path));
    }
}
