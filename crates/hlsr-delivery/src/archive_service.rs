//! 7-Zip (`ArchiveService.js`): листинг, проверка записей, полная и выборочная
//! распаковка, упаковка `.hlsrpack` в два прохода и zip демо, чтение одного файла. Бинарники
//! `7za` из `resources/7zip` (Windows — 7-Zip 26.05, как `seven-zip.js` 0.4.9; Linux — p7zip
//! из `7zip-bin`), те же аргументы и тексты ошибок.
//!
//! Распаковку целиком в 0.4.7 ведёт node-7z 3.0.0: разбор его вывода (куски, строки,
//! этапы шапки/тела/подвала, процент) повторён в [`ExtractProgressParser`] и сверен
//! с настоящим node-7z на разных разбиениях потока.
//!
//! Отступления от 0.4.7 (спецификация, решение 10: только там, где 0.4.7 зависает или
//! даёт случайный результат):
//! - у каждого запуска stdin пуст, а не открытый канал, в который никто не пишет: `extract`
//!   (без `-p`) на зашифрованном архиве в 0.4.7 ждёт пароль вечно; `list` и
//!   `readText` передают `-p` (пустой пароль) и не спрашивают его ни там, ни здесь;
//! - `kill_on_drop`: брошенная задача не оставляет 7za работать (в 0.4.7 node-7z запускает
//!   его `detached`);
//! - текст ошибки `extract`: node-7z берёт `stderr` первого куска вывода, а число и границы
//!   кусков зависят от тайминга канала (у `7za x` на отсутствующем архиве снято десять разных
//!   разбиений за 150 запусков); здесь — весь `stderr`, он начинается тем же текстом.

use crate::archive_entries::{
    InspectOptions, InspectReport, Listing, inspect_entries, parse_listing,
};
use crate::errors::{Code, DeliveryError, RawError, antivirus_error, is_antivirus_block};
use crate::game_binds::Encoding;
use crate::node_fs;
use crate::ports::Logger;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform, errno};
use hlsr_core::json_text::js_number;
use hlsr_core::paths::Resources;
use regex::Regex;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::watch;

/// Тайм-аут `7za l`: `LIST_TIMEOUT_MS`.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Сообщения о запуске 7za, коды выхода и обрезка вывода — общие для методов.
const NOT_STARTED: &str = "7-Zip не запустился";
const EXITED_WITH: &str = "7-Zip завершился с кодом";

// ---------------------------------------------------------------- отмена и задача

/// `cancel()` из `{ promise, cancel }` 0.4.7: останавливает 7za; задача отвечает `CANCELLED`.
#[derive(Clone)]
pub struct CancelHandle {
    flag: Arc<watch::Sender<bool>>,
}

impl Default for CancelHandle {
    fn default() -> CancelHandle {
        CancelHandle {
            flag: Arc::new(watch::channel(false).0),
        }
    }
}

impl CancelHandle {
    pub fn new() -> CancelHandle {
        CancelHandle::default()
    }

    pub fn cancel(&self) {
        self.flag.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.flag.borrow()
    }

    /// Готово, когда вызвали `cancel()` (уже вызванный — сразу).
    pub async fn cancelled(&self) {
        let mut receiver = self.flag.subscribe();
        let _ = receiver.wait_for(|cancelled| *cancelled).await;
    }
}

/// `{ promise, cancel }`: сама задача — будущее (`await`), отмена — [`Job::cancel_handle`].
/// Процесс 7za запускается при первом опросе; брошенная задача его убивает.
#[must_use = "задача запускает 7za при первом опросе (`.await`)"]
pub struct Job<T> {
    cancel: CancelHandle,
    future: Pin<Box<dyn Future<Output = Result<T, DeliveryError>> + Send>>,
}

impl<T> Job<T> {
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl<T> Future for Job<T> {
    type Output = Result<T, DeliveryError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().future.as_mut().poll(cx)
    }
}

// ---------------------------------------------------------------- параметры и итоги

/// Итог [`ArchiveService::extract`]: `{ destination }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Extracted {
    pub destination: PathBuf,
}

/// Итог [`ArchiveService::create`]: `{ archivePath }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Created {
    pub archive_path: PathBuf,
}

/// Итог [`ArchiveService::inspect`]: `{ listing, report }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    pub listing: Listing,
    pub report: InspectReport,
}

/// Параметры `inspect`: пределы (`maxEntries`, `maxUncompressedSize`; нет или 0 — без
/// предела) и единственная корневая папка (`rootPrefix`; пустая строка — не проверять).
#[derive(Debug, Clone, Default)]
pub struct InspectArgs {
    pub max_entries: Option<f64>,
    pub max_uncompressed_size: Option<f64>,
    pub root_prefix: Option<String>,
}

/// Доля готовности распаковки: `progress.percent / 100`.
pub type FractionFn = Arc<dyn Fn(f64) + Send + Sync>;
/// Целый процент упаковки (0–100), только при смене.
pub type PercentFn = Arc<dyn Fn(u32) + Send + Sync>;

#[derive(Clone, Default)]
pub struct ExtractOptions {
    pub on_progress: Option<FractionFn>,
}

/// Параметры `create`: `cwd` — папка, от которой отсчитаны пути; `files` — пути через `/`;
/// `head_files` ложатся первым блоком; `list_dir` — где держать список (по умолчанию
/// рядом с архивом); `threads` — потоки сжатия (1–4; `Number(threads) || 1`).
#[derive(Clone, Default)]
pub struct CreateOptions {
    pub cwd: Option<PathBuf>,
    pub files: Vec<String>,
    pub head_files: Vec<String>,
    pub list_dir: Option<PathBuf>,
    pub threads: Option<f64>,
    pub on_progress: Option<PercentFn>,
}

// ---------------------------------------------------------------- запуск процесса

struct Spawn<'a> {
    bin: &'a Path,
    args: Vec<OsString>,
    cwd: Option<&'a Path>,
    env: &'a [(OsString, OsString)],
}

enum Next {
    Continue,
    /// Остановить процесс (чтение продолжается до закрытия каналов).
    Kill,
}

struct Finished {
    /// Код выхода; `None` — процесс убит сигналом (в JS `null`).
    code: Option<i32>,
    /// Сработал `stop`: процесс убит по отмене или тайм-ауту.
    stopped: bool,
    /// Не удалось послать сигнал остановки.
    kill_error: Option<io::Error>,
}

/// Запустить 7za и читать оба канала до закрытия, потом дождаться выхода — событие
/// `close` Node. `stop` готово → процесс убит (`child.kill()`), чтение идёт дальше.
async fn run_process<S, O, E>(
    spawn: Spawn<'_>,
    stop: S,
    mut on_stdout: O,
    mut on_stderr: E,
) -> io::Result<Finished>
where
    S: Future<Output = ()>,
    O: FnMut(&[u8]) -> Next,
    E: FnMut(&[u8]),
{
    let mut command = Command::new(spawn.bin);
    command
        .args(&spawn.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = spawn.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in spawn.env {
        command.env(key, value);
    }
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("stdout запрошен как канал");
    let mut stderr = child.stderr.take().expect("stderr запрошен как канал");
    let mut out_buffer = vec![0u8; 64 * 1024];
    let mut err_buffer = vec![0u8; 64 * 1024];
    let (mut out_open, mut err_open, mut stopped, mut killed) = (true, true, false, false);
    let mut kill_error = None;
    tokio::pin!(stop);

    while out_open || err_open {
        let mut kill = false;
        tokio::select! {
            read = stdout.read(&mut out_buffer), if out_open => match read {
                Ok(count) if count > 0 => {
                    if matches!(on_stdout(&out_buffer[..count]), Next::Kill) {
                        kill = true;
                    }
                }
                _ => out_open = false,
            },
            read = stderr.read(&mut err_buffer), if err_open => match read {
                Ok(count) if count > 0 => on_stderr(&err_buffer[..count]),
                _ => err_open = false,
            },
            () = &mut stop, if !stopped => {
                stopped = true;
                kill = true;
            }
        }
        if kill && !killed {
            killed = true;
            kill_error = child.start_kill().err();
        }
    }
    let status = child.wait().await?;
    Ok(Finished {
        code: status.code(),
        stopped,
        kill_error,
    })
}

fn nothing_to_stop() -> impl Future<Output = ()> {
    std::future::pending::<()>()
}

/// Сообщение Node об ошибке запуска: `spawn <файл> ENOENT`.
fn spawn_message(bin: &Path, error: &io::Error) -> String {
    match errno::code(error) {
        Some(code) => format!("spawn {} {code}", bin.display()),
        None => format!("spawn {} {error}", bin.display()),
    }
}

/// Код выхода в тексте шаблона: `null` у убитого сигналом.
fn code_text(code: Option<i32>) -> String {
    code.map_or_else(|| "null".to_owned(), |code| code.to_string())
}

/// `stderr.trim().slice(0, 400)`.
fn stderr_head(stderr: &str) -> String {
    js::slice_utf16(js::trim(stderr), 400)
}

fn lossy(bytes: &[u8]) -> String {
    js::utf8_string(bytes)
}

fn with_arg(prefix: &str, path: &Path) -> OsString {
    let mut arg = OsString::from(prefix);
    arg.push(path);
    arg
}

fn os(text: &str) -> OsString {
    OsString::from(text)
}

// ---------------------------------------------------------------- сервис

/// `ArchiveService`: обёртка над `7za`.
#[derive(Clone)]
pub struct ArchiveService {
    bin_path: PathBuf,
    logger: Arc<dyn Logger>,
    list_timeout: Duration,
    platform: Platform,
    env: Vec<(OsString, OsString)>,
}

impl ArchiveService {
    /// `new ArchiveService({ binPath, logger })`.
    pub fn new(bin_path: impl Into<PathBuf>, logger: Arc<dyn Logger>) -> ArchiveService {
        ArchiveService {
            bin_path: bin_path.into(),
            logger,
            list_timeout: LIST_TIMEOUT,
            platform: Platform::CURRENT,
            env: Vec::new(),
        }
    }

    /// Без `binPath`: 7za из каталога ресурсов (`defaultBinPath()`).
    pub fn with_default_bin(logger: Arc<dyn Logger>) -> ArchiveService {
        ArchiveService::new(ArchiveService::default_bin_path(), logger)
    }

    /// `defaultBinPath()`: `resources/7zip/<ОС>/x64/7za` рядом с лаунчером (в отладке —
    /// рабочего дерева; `HLSR_7ZA` — только с `dev-hooks`).
    pub fn default_bin_path() -> PathBuf {
        Resources::detect().sevenzip()
    }

    pub fn bin_path(&self) -> &Path {
        &self.bin_path
    }

    /// Свои переменные окружения 7za поверх унаследованных (в тестах: `LC_ALL`, `TZ`).
    pub fn with_env<K: Into<OsString>, V: Into<OsString>>(
        mut self,
        env: impl IntoIterator<Item = (K, V)>,
    ) -> ArchiveService {
        self.env.extend(
            env.into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        self
    }

    /// Тайм-аут `list` (в 0.4.7 — 5 минут).
    pub fn with_list_timeout(mut self, timeout: Duration) -> ArchiveService {
        self.list_timeout = timeout;
        self
    }

    /// `process.platform` для `isAntivirusBlock` (в тестах).
    pub fn with_platform(mut self, platform: Platform) -> ArchiveService {
        self.platform = platform;
        self
    }

    fn spawn<'a>(&'a self, args: Vec<OsString>, cwd: Option<&'a Path>) -> Spawn<'a> {
        Spawn {
            bin: &self.bin_path,
            args,
            cwd,
            env: &self.env,
        }
    }

    fn kill_warning(&self, finished: &Finished) {
        if let Some(error) = &finished.kill_error {
            self.logger.warn(&format!(
                "[game-delivery] не удалось остановить 7-Zip: {error}"
            ));
        }
    }

    /// `list(archivePath)`: `7za l -slt -sccUTF-8 -p <архив>`.
    pub async fn list(&self, archive_path: &Path) -> Result<Listing, DeliveryError> {
        let args = vec![
            os("l"),
            os("-slt"),
            os("-sccUTF-8"),
            os("-p"),
            archive_path.as_os_str().to_owned(),
        ];
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let finished = run_process(
            self.spawn(args, None),
            tokio::time::sleep(self.list_timeout),
            |chunk| {
                stdout.extend_from_slice(chunk);
                Next::Continue
            },
            |chunk| stderr.extend_from_slice(chunk),
        )
        .await
        .map_err(|error| {
            DeliveryError::new(
                Code::ArchiveListFailed,
                format!("{NOT_STARTED}: {}", spawn_message(&self.bin_path, &error)),
            )
        })?;
        if finished.stopped {
            return Err(DeliveryError::new(
                Code::ArchiveListFailed,
                "7-Zip не ответил за отведённое время",
            ));
        }
        if finished.code != Some(0) {
            return Err(DeliveryError::new(
                Code::ArchiveListFailed,
                format!(
                    "{EXITED_WITH} {}: {}",
                    code_text(finished.code),
                    stderr_head(&lossy(&stderr))
                ),
            ));
        }
        Ok(parse_listing(&lossy(&stdout)))
    }

    /// `inspect(archivePath, options)`: листинг и отказ на опасные записи, пределы, корень.
    pub async fn inspect(
        &self,
        archive_path: &Path,
        args: &InspectArgs,
    ) -> Result<Inspection, DeliveryError> {
        let listing = self.list(archive_path).await?;
        let report = inspect_entries(
            &listing.entries,
            &InspectOptions {
                max_entries: args.max_entries,
                max_uncompressed_size: args.max_uncompressed_size,
            },
        );
        let first_ten = |items: &[Value]| Value::Array(items.iter().take(10).cloned().collect());

        if let Some(first) = report.unsafe_entries.first() {
            let entries: Vec<Value> = report
                .unsafe_entries
                .iter()
                .map(|entry| json!({"path": entry.path, "problems": entry.problems}))
                .collect();
            return Err(DeliveryError::new(
                Code::ArchiveUnsafeEntry,
                format!("Архив содержит небезопасные пути: {}", first.path),
            )
            .with_details(json!({ "entries": first_ten(&entries) })));
        }
        if let Some(first) = report.symlinks.first() {
            let entries: Vec<Value> = report.symlinks.iter().map(|path| json!(path)).collect();
            return Err(DeliveryError::new(
                Code::ArchiveUnsafeEntry,
                format!("Архив содержит symlink/reparse point: {first}"),
            )
            .with_details(json!({ "entries": first_ten(&entries) })));
        }
        if let Some(first) = report.encrypted.first() {
            return Err(DeliveryError::new(
                Code::ArchiveUnsafeEntry,
                format!("Архив содержит зашифрованные элементы: {first}"),
            ));
        }
        let number = |value: Option<f64>| js::to_template_string(value.map(js_number).as_ref());
        if report.errors.contains(&"too-many-entries") {
            return Err(DeliveryError::new(
                Code::ArchiveLimitExceeded,
                format!(
                    "В архиве {} элементов при лимите {}",
                    report.total,
                    number(args.max_entries)
                ),
            )
            .with_details(json!({
                "limit": args.max_entries.map(js_number),
                "actual": report.total,
            })));
        }
        if report.errors.contains(&"uncompressed-size-limit") {
            return Err(DeliveryError::new(
                Code::ArchiveLimitExceeded,
                format!(
                    "Распакованный размер {} превышает лимит {}",
                    number(Some(report.uncompressed_size)),
                    number(args.max_uncompressed_size)
                ),
            )
            .with_details(json!({
                "limit": args.max_uncompressed_size.map(js_number),
                "actual": js_number(report.uncompressed_size),
            })));
        }
        if let Some(prefix) = args
            .root_prefix
            .as_deref()
            .filter(|prefix| !prefix.is_empty())
        {
            let roots = &report.roots;
            if roots.len() != 1 || roots[0] != prefix {
                return Err(DeliveryError::new(
                    Code::ArchiveUnsafeEntry,
                    format!(
                        "Ожидалась единственная корневая папка \"{prefix}\", найдено: {}",
                        roots.join(", ")
                    ),
                )
                .with_details(json!({ "roots": roots })));
            }
        }
        Ok(Inspection { listing, report })
    }

    /// `extract(archivePath, destination, options)`: `7za x <архив> -o<куда> -y -bb3 -bsp1 -snl-`
    /// (аргументы node-7z), прогресс — из его вывода. Ошибка — любой вывод в stderr
    /// (код выхода node-7z не смотрит) или сбой запуска.
    pub fn extract(
        &self,
        archive_path: &Path,
        destination: &Path,
        options: ExtractOptions,
    ) -> Job<Extracted> {
        self.extract_only(archive_path, destination, &[], options)
    }

    /// `extract(archivePath, destination, { files })` 0.4.9: только эти пути архива, плоско в
    /// `destination` (`7za e <архив> <пути…> -o<куда> …`, `$cherryPick` node-7z); пустой список —
    /// как [`ArchiveService::extract`].
    pub fn extract_only(
        &self,
        archive_path: &Path,
        destination: &Path,
        files: &[String],
        options: ExtractOptions,
    ) -> Job<Extracted> {
        let cancel = CancelHandle::new();
        let service = self.clone();
        let (archive, destination, files, handle) = (
            archive_path.to_owned(),
            destination.to_owned(),
            files.to_vec(),
            cancel.clone(),
        );
        Job {
            cancel,
            future: Box::pin(async move {
                service
                    .run_extract(&archive, &destination, &files, options, handle)
                    .await
            }),
        }
    }

    async fn run_extract(
        &self,
        archive_path: &Path,
        destination: &Path,
        files: &[String],
        options: ExtractOptions,
        cancel: CancelHandle,
    ) -> Result<Extracted, DeliveryError> {
        let cancelled_error = || DeliveryError::new(Code::Cancelled, "Распаковка отменена");
        if cancel.is_cancelled() {
            return Err(cancelled_error());
        }
        let mut args = vec![
            os(if files.is_empty() { "x" } else { "e" }),
            archive_path.as_os_str().to_owned(),
        ];
        args.extend(files.iter().map(|file| os(file)));
        args.extend([
            with_arg("-o", destination),
            os("-y"),
            os("-bb3"),
            os("-bsp1"),
            os("-snl-"),
        ]);
        let mut parser = ExtractProgressParser::new();
        let mut stderr = Vec::new();
        let finished = run_process(
            self.spawn(args, None),
            cancel.cancelled(),
            |chunk| {
                for percent in parser.feed(chunk) {
                    if let Some(on_progress) = &options.on_progress {
                        on_progress(percent / 100.0);
                    }
                }
                Next::Continue
            },
            |chunk| stderr.extend_from_slice(chunk),
        )
        .await;

        // Ошибка node-7z: сбой запуска или любой вывод в stderr; на «close» — 'error', потом 'end'.
        let error: Option<RawError> = match &finished {
            Err(error) => Some(RawError {
                code: errno::code(error).map(std::borrow::Cow::into_owned),
                syscall: Some(format!("spawn {}", self.bin_path.display())),
                path: None,
                message: spawn_message(&self.bin_path, error),
                stderr: None,
            }),
            Ok(finished) => {
                self.kill_warning(finished);
                (!stderr.is_empty()).then(|| node_7z_error(&lossy(&stderr)))
            }
        };
        if cancel.is_cancelled() {
            return Err(cancelled_error());
        }
        if let Some(error) = error {
            let details = stderr_head(error.stderr.as_deref().unwrap_or_default());
            if is_antivirus_block(&error, self.platform) {
                return Err(antivirus_error(
                    Some(&archive_path.to_string_lossy()),
                    &error.message,
                ));
            }
            let shown = if details.is_empty() {
                error.message
            } else {
                details
            };
            return Err(DeliveryError::new(
                Code::ExtractFailed,
                format!("Ошибка распаковки: {shown}"),
            ));
        }
        Ok(Extracted {
            destination: destination.to_owned(),
        })
    }

    /// `isAntivirusBlock({ stderr })` → `antivirusError(archivePath, { message: stderr.trim() })`.
    fn antivirus_or(
        &self,
        archive_path: &Path,
        stderr: &str,
        other: DeliveryError,
    ) -> DeliveryError {
        let error = RawError {
            stderr: Some(stderr.to_owned()),
            ..RawError::default()
        };
        if is_antivirus_block(&error, self.platform) {
            antivirus_error(Some(&archive_path.to_string_lossy()), js::trim(stderr))
        } else {
            other
        }
    }

    fn cleanup_list_file(&self, list_file: &Path) {
        match std::fs::remove_file(list_file) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => self.logger.warn(&format!(
                "[game-delivery] не удалось удалить список файлов: {}",
                RawError::fs(&error, "unlink", list_file).message
            )),
        }
    }

    /// `create(archivePath, options)`: `.hlsrpack` — 7z из файлов `cwd`; `head_files` — отдельным
    /// первым блоком без сжатия (читаются без распаковки остального), затем остальные
    /// с максимальным сжатием и не больше четырёх потоков: на `-mx=9` пара берёт около 0,7 ГБ.
    pub fn create(&self, archive_path: &Path, options: CreateOptions) -> Job<Created> {
        self.create_as(archive_path, options, false)
    }

    /// `create(archivePath, { type: "zip", … })` 0.4.9: zip (`-tzip`, `-mx=5`), без блока
    /// `head_files`.
    pub fn create_zip(&self, archive_path: &Path, options: CreateOptions) -> Job<Created> {
        self.create_as(archive_path, options, true)
    }

    fn create_as(&self, archive_path: &Path, options: CreateOptions, zip: bool) -> Job<Created> {
        let cancel = CancelHandle::new();
        let service = self.clone();
        let (archive, handle) = (archive_path.to_owned(), cancel.clone());
        Job {
            cancel,
            future: Box::pin(
                async move { service.run_create(&archive, options, zip, handle).await },
            ),
        }
    }

    async fn run_create(
        &self,
        archive_path: &Path,
        options: CreateOptions,
        zip: bool,
        cancel: CancelHandle,
    ) -> Result<Created, DeliveryError> {
        // `path.join(opts.listDir || path.dirname(archivePath), …)`.
        let node_path = NodePath::native();
        let list_dir = options
            .list_dir
            .as_ref()
            .filter(|dir| !dir.as_os_str().is_empty())
            .map_or_else(
                || node_path.dirname(&archive_path.to_string_lossy()),
                |dir| dir.to_string_lossy().into_owned(),
            );
        let list_file = PathBuf::from(node_path.join(&[
            &list_dir,
            &format!(
                ".hlsr-pack-{}-{}.txt",
                std::process::id(),
                js::base36(now_ms())
            ),
        ]));
        let threads = pack_threads(options.threads);
        let common = [
            if zip { "-tzip" } else { "-t7z" },
            "-snl-",
            "-scsUTF-8",
            "-sccUTF-8",
            "-y",
        ];

        let list_text = format!(
            "{}\r\n",
            options
                .files
                .iter()
                .map(|file| file.replace('/', std::path::MAIN_SEPARATOR_STR))
                .collect::<Vec<_>>()
                .join("\r\n")
        );
        node_fs::write_text(&list_file, &list_text, Encoding::Utf8).map_err(|error| {
            DeliveryError::new(
                Code::PackFailed,
                format!("Не удалось записать список файлов: {}", error.message),
            )
        })?;

        let result = async {
            if !zip && !options.head_files.is_empty() {
                let mut args = vec![os("a")];
                args.extend(common.iter().map(|arg| os(arg)));
                args.extend(["-mx=0", "-bso0", "-bsp0"].map(os));
                args.push(archive_path.as_os_str().to_owned());
                args.extend(options.head_files.iter().map(|file| os(file)));
                self.run_pack(archive_path, args, options.cwd.as_deref(), None, &cancel)
                    .await?;
            }
            let mut args = vec![os("a")];
            args.extend(common.iter().map(|arg| os(arg)));
            args.extend([
                os(if zip { "-mx=5" } else { "-mx=9" }),
                os(&format!("-mmt={threads}")),
            ]);
            args.extend(["-bso0", "-bsp1"].map(os));
            args.push(archive_path.as_os_str().to_owned());
            args.push(with_arg("@", &list_file));
            self.run_pack(
                archive_path,
                args,
                options.cwd.as_deref(),
                options.on_progress.as_ref(),
                &cancel,
            )
            .await
        }
        .await;
        self.cleanup_list_file(&list_file);
        result?;
        Ok(Created {
            archive_path: archive_path.to_owned(),
        })
    }

    /// Один проход `7za a` (`run` в `create`): прогресс — последнее `NN%` куска вывода.
    async fn run_pack(
        &self,
        archive_path: &Path,
        args: Vec<OsString>,
        cwd: Option<&Path>,
        on_progress: Option<&PercentFn>,
        cancel: &CancelHandle,
    ) -> Result<(), DeliveryError> {
        let cancelled_error = || DeliveryError::new(Code::Cancelled, "Экспорт отменён");
        if cancel.is_cancelled() {
            return Err(cancelled_error());
        }
        let mut stderr = String::new();
        let mut stderr_units = 0usize;
        let mut last: i64 = -1;
        let finished = run_process(
            self.spawn(args, cwd),
            cancel.cancelled(),
            |chunk| {
                if let Some(on_progress) = on_progress
                    && let Some(percent) = last_percent(&lossy(chunk))
                    && i64::from(percent) != last
                {
                    last = i64::from(percent);
                    on_progress(percent);
                }
                Next::Continue
            },
            |chunk| {
                // `if (stderr.length < 4000) stderr += chunk`.
                if stderr_units < 4000 {
                    let text = lossy(chunk);
                    stderr_units += text.encode_utf16().count();
                    stderr.push_str(&text);
                }
            },
        )
        .await
        .map_err(|error| {
            DeliveryError::new(
                Code::PackFailed,
                format!("{NOT_STARTED}: {}", spawn_message(&self.bin_path, &error)),
            )
        })?;
        self.kill_warning(&finished);
        if cancel.is_cancelled() {
            Err(cancelled_error())
        } else if finished.code == Some(0) {
            Ok(())
        } else {
            Err(self.antivirus_or(
                archive_path,
                &stderr,
                DeliveryError::new(
                    Code::PackFailed,
                    format!(
                        "{EXITED_WITH} {}: {}",
                        code_text(finished.code),
                        stderr_head(&stderr)
                    ),
                ),
            ))
        }
    }

    /// `readText(archivePath, archiveEntry, maxBytes)`: `7za e -so -sccUTF-8 -p <архив> <запись>`,
    /// вывод — строка UTF-8; больше `max_bytes` — `ARCHIVE_LIMIT_EXCEEDED` (процесс убит).
    pub async fn read_text(
        &self,
        archive_path: &Path,
        archive_entry: &str,
        max_bytes: Option<u64>,
    ) -> Result<String, DeliveryError> {
        let args = vec![
            os("e"),
            os("-so"),
            os("-sccUTF-8"),
            os("-p"),
            archive_path.as_os_str().to_owned(),
            os(archive_entry),
        ];
        let mut data: Vec<u8> = Vec::new();
        let mut size: u64 = 0;
        let mut too_large = false;
        let mut stderr = String::new();
        let mut stderr_units = 0usize;
        let finished = run_process(
            self.spawn(args, None),
            nothing_to_stop(),
            |chunk| {
                size += chunk.len() as u64;
                if max_bytes.is_some_and(|max| size > max) {
                    too_large = true;
                    return Next::Kill;
                }
                data.extend_from_slice(chunk);
                Next::Continue
            },
            |chunk| {
                if stderr_units < 4000 {
                    let text = lossy(chunk);
                    stderr_units += text.encode_utf16().count();
                    stderr.push_str(&text);
                }
            },
        )
        .await
        .map_err(|error| {
            DeliveryError::new(
                Code::ArchiveListFailed,
                format!("{NOT_STARTED}: {}", spawn_message(&self.bin_path, &error)),
            )
        })?;
        if too_large {
            return Err(DeliveryError::new(
                Code::ArchiveLimitExceeded,
                format!(
                    "{archive_entry} больше {} байт",
                    max_bytes.map_or_else(|| "undefined".to_owned(), |max| max.to_string())
                ),
            ));
        }
        if finished.code != Some(0) {
            return Err(self.antivirus_or(
                archive_path,
                &stderr,
                DeliveryError::new(
                    Code::ArchiveListFailed,
                    format!(
                        "{EXITED_WITH} {}: {}",
                        code_text(finished.code),
                        stderr_head(&stderr)
                    ),
                ),
            ));
        }
        Ok(lossy(&data))
    }

    /// `ArchiveService.walk(root, limit)`: файлы под `root` (пути через `/`), глубина вперёд,
    /// не больше `limit` (по умолчанию 500000).
    pub fn walk(root: &Path, limit: Option<usize>) -> Vec<String> {
        let max = limit.filter(|&limit| limit > 0).unwrap_or(500_000);
        let mut result = Vec::new();
        let mut stack = vec![root.to_owned()];
        while let Some(dir) = stack.pop() {
            let Ok(items) = js::fs::read_dir(&dir) else {
                continue;
            };
            for item in items {
                let full = dir.join(&item.name);
                if item.file_type.is_dir() {
                    stack.push(full);
                    continue;
                }
                let relative = full
                    .strip_prefix(root)
                    .unwrap_or(&full)
                    .to_string_lossy()
                    .replace('\\', "/");
                result.push(relative);
                if result.len() >= max {
                    return result;
                }
            }
        }
        result
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis())
}

/// `Math.max(1, Math.min(4, Number(threads) || 1))` как текст шаблона.
fn pack_threads(threads: Option<f64>) -> String {
    let requested = threads.filter(|t| !t.is_nan() && *t != 0.0).unwrap_or(1.0);
    let clamped = requested.clamp(1.0, 4.0);
    js::to_string(&js_number(clamped))
}

static PERCENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([0-9]{1,3})%").expect("шаблон процента"));

/// `chunk.match(/(\d{1,3})%/g)`: последнее совпадение, `Math.min(100, parseInt)`.
fn last_percent(chunk: &str) -> Option<u32> {
    let found = PERCENT.captures_iter(chunk).last()?;
    let number: u32 = found[1].parse().ok()?;
    Some(number.min(100))
}

static ERROR_LEVEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:WARNING|ERROR): ([^\n\r\u{2028}\u{2029}]*)").expect("шаблон ошибки")
});

/// Ошибка node-7z из текста stderr (`Err.fromBuffer`): `stderr` — весь текст, `message` —
/// остаток первой строки «WARNING:/ERROR: …» или «unknown error». В 0.4.7 это по куску
/// вывода (первый кусок даёт `stderr`, последний — `message`); при одном куске — то же.
fn node_7z_error(stderr: &str) -> RawError {
    let message = ERROR_LEVEL
        .captures(stderr)
        .map_or_else(|| "unknown error".to_owned(), |found| found[1].to_owned());
    RawError {
        message,
        stderr: Some(stderr.to_owned()),
        ..RawError::default()
    }
}

// ---------------------------------------------------------------- вывод node-7z

const LINE_END: &str = r"[^\n\r\u{2028}\u{2029}]";

static LINE_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n|\r\n|\x08+|\r +\r").expect("LINE_SPLIT"));
static BODY_PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"^ *([0-9]+)% ?(?:[0-9]+)? ?{LINE_END}*$")).expect("BODY_PROGRESS")
});
static BODY_SYMBOL_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^[=TU+R.-] {LINE_END}+$")).expect("BODY_SYMBOL_FILE"));
static INFOS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^{LINE_END}+?(?: = |: +){LINE_END}+$")).expect("INFOS"));
static INFOS_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(", +# ").expect("INFOS_SPLIT"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Headers,
    Body,
    Footers,
}

/// Разбор стандартного вывода `7za x … -bb3 -bsp1`, как его читает node-7z 3.0.0
/// (`lines.js`, `events.js`, `maybe.js`, `parser.js` для `extractFull`): куски режутся на
/// строки по `\n|\r\n|\x08+|\r +\r`, хвост после последнего `\n` откладывается до
/// следующего куска (и его целые строки читаются ещё раз — повторы возможны), прогресс
/// читается только в теле, то есть после первой строки «символ имя» и до двух пустых
/// строк подряд. Возвращает проценты событий `progress` в порядке появления.
#[derive(Debug)]
pub struct ExtractProgressParser {
    partial: Vec<u8>,
    stage: Stage,
    last_line_empty: bool,
}

impl Default for ExtractProgressParser {
    fn default() -> ExtractProgressParser {
        ExtractProgressParser {
            partial: Vec::new(),
            stage: Stage::Headers,
            last_line_empty: false,
        }
    }
}

impl ExtractProgressParser {
    pub fn new() -> ExtractProgressParser {
        ExtractProgressParser::default()
    }

    /// Следующий кусок вывода → проценты новых событий `progress`.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<f64> {
        let mut buffer = std::mem::take(&mut self.partial);
        buffer.extend_from_slice(chunk);
        let text = lossy(&buffer);
        let mut lines: Vec<&str> = LINE_SPLIT.split(&text).collect();
        // Хвост после последнего `\n`: пустой — строки целые, иначе откладывается.
        let tail_from = buffer
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |at| at + 1);
        if tail_from < buffer.len() {
            self.partial = buffer[tail_from..].to_vec();
            lines.pop();
        }
        let mut events = Vec::new();
        for line in lines {
            self.line(line, &mut events);
        }
        events
    }

    fn line(&mut self, line: &str, events: &mut Vec<f64>) {
        // Сведения шапки и подвала (`Path = …`, `Extracting archive: …`).
        if self.stage != Stage::Body && is_info(line) {
            return;
        }
        if self.stage == Stage::Headers && BODY_SYMBOL_FILE.is_match(line) {
            self.stage = Stage::Body;
        }
        if self.stage != Stage::Body {
            return;
        }
        if self.end_of_body(line) {
            self.stage = Stage::Footers;
            return;
        }
        if is_blank(line) {
            return;
        }
        if let Some(found) = BODY_PROGRESS.captures(line) {
            // `Number.parseInt` цифр.
            events.push(found[1].parse::<f64>().unwrap_or(f64::NAN));
        }
    }

    /// Две пустые строки подряд (`matchEndOfBodySymbol` при `-bsp1`).
    fn end_of_body(&mut self, line: &str) -> bool {
        let last_was_empty = self.last_line_empty;
        if !is_blank(line) {
            self.last_line_empty = false;
            false
        } else if last_was_empty {
            true
        } else {
            self.last_line_empty = true;
            false
        }
    }
}

fn is_blank(line: &str) -> bool {
    js::trim(line).is_empty()
}

fn is_info(line: &str) -> bool {
    INFOS_SPLIT.split(line).any(|part| INFOS.is_match(part))
}
