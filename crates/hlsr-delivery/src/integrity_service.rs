//! Проверка файлов (`IntegrityService.js`): файлы установки против `files.json` — размер и SHA-256.
//!
//! Отчёт: `valid`, `missing` (нет файла, или на его месте не файл), `corrupted` (размер тот же,
//! хеш другой), `sizeMismatch` (другой размер), `extra` (файл, которого нет в манифесте; имена
//! сравниваются без учёта регистра, ссылки не обходятся, `installed.json` в корне пропускается),
//! `skipped` (сохраняемые пути `preservePaths` и изменяемые данные игры — `mutable-paths`). Папки
//! `preservePaths` и изменяемые папки в поиск лишних файлов не заходят.
//!
//! cfg (0.4.9): пропавший — `configMissing` (восстанавливается как повреждение), изменённый —
//! `configChanged` (правка игрока, меняется только по выбору). Стоковые cfg из папок игрока
//! (`stockConfigs` файлового манифеста) сверяются только по запросу (`stock_configs`: «Проверить
//! файлы» и ремонт); их пропажа — `stockMissing`, в `damaged` она не входит: у установки из
//! базового архива их может не быть, игра от этого не «повреждена».
//!
//! Одна проверка за раз ([`IntegrityService::is_busy`]); её можно отменить
//! ([`IntegrityService::cancel`]): проверка остановится перед следующим файлом с `CANCELLED`.
//! Событие `progress`: `{checked: 0, total, phase: "size"}` в начале, затем `{checked, total,
//! percent, phase: "hash"}` каждые пять файлов и на последнем.
//!
//! Отмена будущего: брошенная проверка освобождает сервис, как `finally`; на диске она ничего не
//! меняет, так что откатывать нечего.
//!
//! Отличия от 0.4.7: `gameId` манифеста, если он не строка, берётся как строка (манифест уже
//! проверен схемой, там строка).

use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::file_ops;
use crate::install_service::{IntegrityCheck, IntegrityReport, IntegrityRequest};
use crate::js_value as jv;
use crate::mutable_paths::matches_mutable;
use crate::path_match::Matcher;
use crate::ports::{Logger, TracingLogger};
use crate::providers::BoxFuture;
use crate::user_configs;
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// `YIELD_EVERY`: проверка уступает поток раз в столько файлов (и каталогов при поиске лишних).
const YIELD_EVERY: usize = 25;

/// Слушатель события `progress`.
pub type Listener = Arc<dyn Fn(&IntegrityProgress) + Send + Sync>;

/// Событие `progress`.
#[derive(Debug, Clone, PartialEq)]
pub struct IntegrityProgress {
    pub game_id: Option<String>,
    pub checked: usize,
    pub total: usize,
    /// Нет у события начала проверки (`phase: "size"`).
    pub percent: Option<f64>,
    /// `size` или `hash`.
    pub phase: &'static str,
}

impl IntegrityProgress {
    /// Объект события, как его получает слушатель в 0.4.7.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        if let Some(game_id) = &self.game_id {
            out.insert("gameId".to_owned(), json!(game_id));
        }
        out.insert("checked".to_owned(), json!(self.checked));
        out.insert("total".to_owned(), json!(self.total));
        if let Some(percent) = self.percent {
            out.insert("percent".to_owned(), js_number(percent));
        }
        out.insert("phase".to_owned(), json!(self.phase));
        Value::Object(out)
    }
}

/// Файл с тем же размером, но другим хешем.
#[derive(Debug, Clone, PartialEq)]
pub struct Corrupted {
    pub path: String,
    /// `file.sha256` манифеста.
    pub expected: Option<Value>,
    pub actual: String,
}

/// Файл другого размера.
#[derive(Debug, Clone, PartialEq)]
pub struct SizeMismatch {
    pub path: String,
    /// `file.size` манифеста.
    pub expected: Option<Value>,
    pub actual: u64,
}

/// Отчёт проверки (`result`).
#[derive(Debug, Clone, PartialEq)]
pub struct IntegrityResult {
    pub game_id: Option<String>,
    /// `manifest.version`.
    pub version: Option<Value>,
    pub total: usize,
    pub checked: usize,
    pub valid: Vec<String>,
    pub missing: Vec<String>,
    pub corrupted: Vec<Corrupted>,
    pub size_mismatch: Vec<SizeMismatch>,
    /// Пропавшие cfg (из `files` и стоковые).
    pub config_missing: Vec<String>,
    /// Изменённые игроком cfg.
    pub config_changed: Vec<String>,
    pub extra: Vec<String>,
    pub skipped: Vec<String>,
    /// Пропавшие стоковые cfg (часть `config_missing`).
    pub stock_missing: usize,
    /// `missing + corrupted + sizeMismatch + configMissing - stockMissing`.
    pub damaged: usize,
    pub ok: bool,
}

impl IntegrityResult {
    /// Отчёт как объект JS: порядок ключей — как у 0.4.7.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        if let Some(game_id) = &self.game_id {
            out.insert("gameId".to_owned(), json!(game_id));
        }
        if let Some(version) = &self.version {
            out.insert("version".to_owned(), version.clone());
        }
        out.insert("total".to_owned(), json!(self.total));
        out.insert("checked".to_owned(), json!(self.checked));
        out.insert("valid".to_owned(), json!(self.valid));
        out.insert("missing".to_owned(), json!(self.missing));
        let corrupted: Vec<Value> = self
            .corrupted
            .iter()
            .map(|item| {
                let mut entry = Map::new();
                entry.insert("path".to_owned(), json!(item.path));
                if let Some(expected) = &item.expected {
                    entry.insert("expected".to_owned(), expected.clone());
                }
                entry.insert("actual".to_owned(), json!(item.actual));
                Value::Object(entry)
            })
            .collect();
        out.insert("corrupted".to_owned(), Value::Array(corrupted));
        let size_mismatch: Vec<Value> = self
            .size_mismatch
            .iter()
            .map(|item| {
                let mut entry = Map::new();
                entry.insert("path".to_owned(), json!(item.path));
                if let Some(expected) = &item.expected {
                    entry.insert("expected".to_owned(), expected.clone());
                }
                entry.insert("actual".to_owned(), json!(item.actual));
                Value::Object(entry)
            })
            .collect();
        out.insert("sizeMismatch".to_owned(), Value::Array(size_mismatch));
        out.insert("configMissing".to_owned(), json!(self.config_missing));
        out.insert("configChanged".to_owned(), json!(self.config_changed));
        out.insert("extra".to_owned(), json!(self.extra));
        out.insert("skipped".to_owned(), json!(self.skipped));
        out.insert("stockMissing".to_owned(), json!(self.stock_missing));
        out.insert("damaged".to_owned(), json!(self.damaged));
        out.insert("ok".to_owned(), json!(self.ok));
        Value::Object(out)
    }

    /// То, что читает установка: пути вместо записей о хеше и размере.
    pub fn report(&self) -> IntegrityReport {
        IntegrityReport {
            ok: self.ok,
            damaged: self.damaged,
            missing: self.missing.clone(),
            corrupted: self.corrupted.iter().map(|c| c.path.clone()).collect(),
            size_mismatch: self.size_mismatch.iter().map(|s| s.path.clone()).collect(),
            extra: self.extra.clone(),
        }
    }
}

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct IntegrityOptions {
    pub logger: Option<Arc<dyn Logger>>,
}

/// Идущая проверка: при выходе из `check` (и при брошенной задаче) сервис свободен, как `finally`.
struct Running<'a>(&'a IntegrityService);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::SeqCst);
        self.0.cancel_requested.store(false, Ordering::SeqCst);
    }
}

/// `IntegrityService`.
pub struct IntegrityService {
    logger: Arc<dyn Logger>,
    busy: AtomicBool,
    cancel_requested: AtomicBool,
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

impl IntegrityService {
    /// `new IntegrityService({ logger })`.
    pub fn new(options: IntegrityOptions) -> IntegrityService {
        IntegrityService {
            logger: options
                .logger
                .unwrap_or_else(|| Arc::new(TracingLogger) as Arc<dyn Logger>),
            busy: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            listeners: Mutex::new(Vec::new()),
        }
    }

    /// `on("progress", listener)`: слушатель зовётся сразу, в той же точке кода, где рождается событие.
    pub fn on_progress(&self, listener: impl Fn(&IntegrityProgress) + Send + Sync + 'static) {
        self.listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::new(listener));
    }

    fn emit(&self, event: &IntegrityProgress) {
        let list: Vec<Listener> = self
            .listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        for listener in list {
            listener(event);
        }
    }

    /// `busy`: идёт проверка.
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    /// `cancel()`: попросить остановить идущую проверку; `false` — проверки нет.
    pub fn cancel(&self) -> bool {
        if !self.is_busy() {
            return false;
        }
        self.cancel_requested.store(true, Ordering::SeqCst);
        true
    }

    /// `check(request)`: проверить установку. Ошибки до начала (`ALREADY_RUNNING`,
    /// `MANIFEST_INVALID`, `NOT_INSTALLED`) не занимают сервис.
    pub async fn check(&self, request: IntegrityRequest<'_>) -> Result<IntegrityResult, Failure> {
        if self.is_busy() {
            return Err(DeliveryError::new(
                Code::AlreadyRunning,
                "Проверка файлов уже выполняется",
            )
            .into());
        }
        let manifest = request.files_manifest;
        let Some(files) = jv::array(jv::prop(manifest, "files")) else {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                "Нет файлового манифеста для проверки",
            )
            .into());
        };
        let install_path = request.install_path;
        if !install_path.exists() {
            return Err(DeliveryError::new(
                Code::NotInstalled,
                format!("Папка игры не найдена: {}", install_path.display()),
            )
            .into());
        }

        let preserved = Matcher::new(request.preserve_paths);
        let game_id = if request.game_id.is_empty() {
            jv::prop(manifest, "gameId")
                .filter(|v| js::truthy(v))
                .map(|v| jv::template(Some(v)))
        } else {
            Some(request.game_id.to_owned())
        };

        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(DeliveryError::new(
                Code::AlreadyRunning,
                "Проверка файлов уже выполняется",
            )
            .into());
        }
        self.cancel_requested.store(false, Ordering::SeqCst);
        let _running = Running(self);

        let mut result = IntegrityResult {
            game_id: game_id.clone(),
            version: jv::prop(manifest, "version").cloned(),
            total: 0,
            checked: 0,
            valid: Vec::new(),
            missing: Vec::new(),
            corrupted: Vec::new(),
            size_mismatch: Vec::new(),
            config_missing: Vec::new(),
            config_changed: Vec::new(),
            extra: Vec::new(),
            skipped: Vec::new(),
            stock_missing: 0,
            damaged: 0,
            ok: false,
        };

        // Стоковые cfg из папок игрока сверяются только по запросу «Проверить файлы» и ремонта:
        // установка и обновление по ним не судят, там такие cfg остаются у игрока как есть.
        let stock = match request.stock_configs {
            true => jv::array(jv::prop(manifest, "stockConfigs")).map_or(&[][..], Vec::as_slice),
            false => &[],
        };
        let mut targets: Vec<(&Value, String)> = Vec::new();
        for file in files {
            let path = file_path(file)?;
            if preserved.matches(&path) || matches_mutable(&path) {
                result.skipped.push(path);
            } else {
                targets.push((file, path));
            }
        }
        for file in stock {
            targets.push((file, file_path(file)?));
        }

        result.total = targets.len();
        self.emit(&IntegrityProgress {
            game_id: game_id.clone(),
            checked: 0,
            total: result.total,
            percent: None,
            phase: "size",
        });

        for (index, (file, path)) in targets.iter().enumerate() {
            if self.cancel_requested.load(Ordering::SeqCst) {
                return Err(DeliveryError::new(Code::Cancelled, "Проверка отменена").into());
            }

            let full =
                file_ops::resolve_inside(install_path, path, Some(&format!("Файл \"{path}\"")))?;
            let stats = tokio::fs::metadata(&full).await.ok();

            let config = user_configs::is_config_path(path);
            let size = jv::prop(file, "size");
            match stats.filter(|meta| meta.is_file()) {
                None if config => result.config_missing.push(path.clone()),
                None => result.missing.push(path.clone()),
                Some(meta) if !same_size(meta.len(), size) => {
                    if config {
                        result.config_changed.push(path.clone());
                    } else {
                        result.size_mismatch.push(SizeMismatch {
                            path: path.clone(),
                            expected: size.cloned(),
                            actual: meta.len(),
                        });
                    }
                }
                Some(_) => {
                    let digest = file_ops::hash_file(&full).await?;
                    let sha256 = jv::prop(file, "sha256");
                    if jv::is_str(sha256, &digest) {
                        result.valid.push(path.clone());
                    } else if config {
                        result.config_changed.push(path.clone());
                    } else {
                        result.corrupted.push(Corrupted {
                            path: path.clone(),
                            expected: sha256.cloned(),
                            actual: digest,
                        });
                    }
                }
            }

            result.checked = index + 1;

            if index % 5 == 0 || index == targets.len() - 1 {
                self.emit(&IntegrityProgress {
                    game_id: game_id.clone(),
                    checked: result.checked,
                    total: result.total,
                    percent: Some(if result.total == 0 {
                        100.0
                    } else {
                        (result.checked as f64 / result.total as f64) * 100.0
                    }),
                    phase: "hash",
                });
            }

            if index.is_multiple_of(YIELD_EVERY) {
                tokio::task::yield_now().await;
            }
        }

        result.extra = self.find_extra_files(install_path, files, &preserved).await;
        let stock_paths: HashSet<String> = stock
            .iter()
            .map(|file| file_path(file).unwrap_or_default())
            .collect();
        result.stock_missing = result
            .config_missing
            .iter()
            .filter(|path| stock_paths.contains(*path))
            .count();
        result.damaged = result.missing.len()
            + result.corrupted.len()
            + result.size_mismatch.len()
            + result.config_missing.len()
            - result.stock_missing;
        result.ok = result.damaged == 0;

        self.logger.info(&format!(
            "[game-delivery] проверка файлов {}: проверено {}, повреждено {}, стоковых cfg нет {}, \
             изменённых cfg {}, лишних {}",
            game_id.as_deref().unwrap_or("undefined"),
            result.checked,
            result.damaged,
            result.stock_missing,
            result.config_changed.len(),
            result.extra.len()
        ));
        Ok(result)
    }

    /// `findExtraFiles(installPath, manifest, preserved)`: файлы, которых нет в манифесте, по
    /// возрастанию (порядок кодовых единиц UTF-16). Каталог, который не прочитать, пропускается.
    async fn find_extra_files(
        &self,
        install_path: &std::path::Path,
        files: &[Value],
        preserved: &Matcher,
    ) -> Vec<String> {
        let known: HashSet<String> = files
            .iter()
            .map(|file| file_path(file).unwrap_or_default().to_lowercase())
            .collect();
        let mut extra = Vec::new();
        // Каталог и его путь от установки (через «/»).
        let mut stack: Vec<(PathBuf, String)> = vec![(install_path.to_path_buf(), String::new())];
        let mut steps = 0usize;

        while let Some((dir, prefix)) = stack.pop() {
            let Ok(mut items) = tokio::fs::read_dir(&dir).await else {
                continue;
            };
            // Ошибка посреди чтения каталога обрывает его, как исключение `readdir`.
            let mut names = Vec::new();
            while let Ok(Some(item)) = items.next_entry().await {
                let Ok(kind) = item.file_type().await else {
                    continue;
                };
                names.push((item.file_name(), item.path(), kind));
            }

            for (name, full, kind) in names {
                // `path.relative(...).replace(/\\/g, "/")`.
                let name = name.to_string_lossy().replace('\\', "/");
                let relative = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };

                if kind.is_symlink() {
                    continue;
                }
                if preserved.matches(&relative) || matches_mutable(&relative) {
                    continue;
                }
                if kind.is_dir() {
                    stack.push((full, relative));
                    continue;
                }
                if relative == "installed.json" {
                    continue;
                }
                if !known.contains(&relative.to_lowercase()) {
                    extra.push(relative);
                }
            }

            steps += 1;
            if steps.is_multiple_of(YIELD_EVERY) {
                tokio::task::yield_now().await;
            }
        }

        extra.sort_by(|a, b| js::cmp_utf16(a, b));
        extra
    }
}

impl IntegrityCheck for IntegrityService {
    fn check<'a>(
        &'a self,
        request: IntegrityRequest<'a>,
    ) -> BoxFuture<'a, Result<IntegrityReport, Failure>> {
        Box::pin(async move {
            IntegrityService::check(self, request)
                .await
                .map(|result| result.report())
        })
    }
}
