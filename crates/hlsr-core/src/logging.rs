//! Журнал `main.log` в формате electron-log 0.4.7: строки, ротация `log-policy.js`,
//! маскировка имени пользователя, перехват паник.

use crate::error::CoreError;
use crate::paths::Paths;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_log::NormalizeEvent;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// Категории `log-policy.js`: `target` события `tracing`, в строке — `[категория] `.
pub const CATEGORIES: [&str; 5] = [
    "launcher",
    "launcher-update",
    "game-delivery",
    "game-process",
    "steam-preflight",
];

/// Конец строки: electron-log пишет `os.EOL`.
pub const EOL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

const LOG_FILE: &str = "main.log";
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ARCHIVES: u32 = 3;

/// Строка журнала: `[YYYY-MM-DD HH:MM:SS.mmm] [уровень] [категория] сообщение`.
/// Уровень дополняется пробелами до ширины 5, как `{level}]`.padEnd(6) в electron-log.
pub fn format_line(
    at: chrono::DateTime<chrono::Local>,
    level: Level,
    category: Option<&str>,
    message: &str,
) -> String {
    let mut line = format!(
        "[{}] {:<7} ",
        at.format("%Y-%m-%d %H:%M:%S%.3f"),
        format!("[{}]", level_name(level))
    );
    if let Some(category) = category {
        let _ = write!(line, "[{category}] ");
    }
    line.push_str(message);
    line.push_str(EOL);
    line
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::ERROR => "error",
        Level::WARN => "warn",
        Level::INFO => "info",
        Level::DEBUG => "debug",
        Level::TRACE => "silly",
    }
}

/// Имя пользователя между разделителями пути → `***`, как хук журнала в `index.js` 0.4.7.
pub fn redact_user(text: &str, user: &str) -> String {
    if user.is_empty() {
        return text.to_owned();
    }
    text.replace(&format!("\\{user}\\"), "\\***\\")
        .replace(&format!("/{user}/"), "/***/")
}

/// Файл журнала с ротацией `log-policy.js`. Размер считается как в electron-log:
/// размер файла при первой проверке плюс записанные байты; больше 2 МБ — перед
/// записью архивы сдвигаются (`main.log` → `main.1.log` → … → `main.3.log`).
/// Файл открывается на каждую строку, как `appendFileSync`.
#[derive(Debug)]
pub struct LogFile {
    path: PathBuf,
    initial_size: Option<u64>,
    written: u64,
}

impl LogFile {
    pub fn open(dir: &Path, name: &str) -> io::Result<LogFile> {
        fs::create_dir_all(dir)?;
        Ok(LogFile {
            path: dir.join(name),
            initial_size: None,
            written: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        if self.size() > MAX_FILE_BYTES {
            // Как log-policy.js: неудача ротации не мешает записи.
            let _ = rotate(&self.path);
            self.initial_size = None;
            self.written = 0;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        self.written += line.len() as u64;
        Ok(())
    }

    fn size(&mut self) -> u64 {
        let path = &self.path;
        *self
            .initial_size
            .get_or_insert_with(|| fs::metadata(path).map(|meta| meta.len()).unwrap_or(0))
            + self.written
    }
}

fn archive_name(file: &Path, index: u32) -> PathBuf {
    let stem = file.file_stem().unwrap_or_default().to_string_lossy();
    let name = match file.extension() {
        Some(ext) => format!("{stem}.{index}.{}", ext.to_string_lossy()),
        None => format!("{stem}.{index}"),
    };
    file.with_file_name(name)
}

/// `rotate` из `log-policy.js`: первая ошибка прерывает сдвиг.
fn rotate(file: &Path) -> io::Result<()> {
    let oldest = archive_name(file, MAX_ARCHIVES);
    if oldest.exists() {
        fs::remove_file(&oldest)?;
    }
    for index in (1..MAX_ARCHIVES).rev() {
        let from = archive_name(file, index);
        if from.exists() {
            fs::rename(&from, archive_name(file, index + 1))?;
        }
    }
    if file.exists() {
        fs::rename(file, archive_name(file, 1))?;
    }
    Ok(())
}

/// Держать до выхода из процесса: журнал пишется, пока установлен подписчик.
#[derive(Debug)]
pub struct LoggingGuard {
    path: PathBuf,
}

impl LoggingGuard {
    /// Путь `main.log`.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Поставить глобальный подписчик, пишущий в `<журналы>/main.log`.
///
/// Куда идут события:
/// - `target` из [`CATEGORIES`] — info и выше, с `[категория] `;
/// - наши крейты (`hlsr*`) — info и выше, без категории: префикс вроде
///   `[ui-scale]` — часть сообщения, как в 0.4.7;
/// - прочие (Slint, winit, wgpu через `log`…) — warn и выше, без категории.
///
/// Папку журналов не удалось создать — лаунчер работает без журнала;
/// ошибка — только если подписчик уже установлен.
pub fn init(paths: &Paths) -> Result<LoggingGuard, CoreError> {
    let echo = io::stderr().is_terminal();
    let file = LogFile::open(&paths.logs_dir, LOG_FILE).unwrap_or_else(|error| {
        if echo {
            let _ = writeln!(
                io::stderr(),
                "журнал недоступен ({}): {error}",
                paths.logs_dir.display()
            );
        }
        LogFile {
            path: paths.logs_dir.join(LOG_FILE),
            initial_size: None,
            written: 0,
        }
    });
    let path = file.path().to_owned();
    let layer = FileLayer {
        file: Mutex::new(file),
        user: user_name(),
        echo,
    };
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer))
        .map_err(|error| CoreError::Logging(error.to_string()))?;
    tracing_log::LogTracer::init().map_err(|error| CoreError::Logging(error.to_string()))?;
    Ok(LoggingGuard { path })
}

/// Последний компонент домашнего каталога: `%USERPROFILE%` или `$HOME`.
fn user_name() -> String {
    let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(home)
        .and_then(|home| {
            Path::new(&home)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

/// Категория события или `None`, если событие не пишется.
fn route(target: &str, level: Level) -> Option<Option<&'static str>> {
    if let Some(category) = CATEGORIES.iter().find(|category| **category == target) {
        return (level <= Level::INFO).then_some(Some(*category));
    }
    let ours = target
        .split("::")
        .next()
        .is_some_and(|krate| krate.starts_with("hlsr"));
    let min = if ours { Level::INFO } else { Level::WARN };
    (level <= min).then_some(None)
}

struct FileLayer {
    file: Mutex<LogFile>,
    user: String,
    echo: bool,
}

impl<S: Subscriber> Layer<S> for FileLayer {
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        // События из `log` приходят с target "log"; настоящий target — в полях.
        if metadata.target() == "log" || route(metadata.target(), *metadata.level()).is_some() {
            Interest::always()
        } else {
            Interest::never()
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::INFO)
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let normalized = event.normalized_metadata();
        let metadata = normalized.as_ref().unwrap_or_else(|| event.metadata());
        let Some(category) = route(metadata.target(), *metadata.level()) else {
            return;
        };
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let message = redact_user(&visitor.finish(), &self.user);
        let line = format_line(chrono::Local::now(), *metadata.level(), category, &message);

        let mut file = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Записать не вышло — сообщить некуда; electron-log тоже продолжает.
        let _ = file.write_line(&line);
        drop(file);
        if self.echo {
            let _ = io::stderr().write_all(line.as_bytes());
        }
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl MessageVisitor {
    fn finish(mut self) -> String {
        self.message.push_str(&self.fields);
        self.message
    }
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message.push_str(value),
            name if name.starts_with("log.") => {}
            name => {
                let _ = write!(self.fields, " {name}={value}");
            }
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => {
                let _ = write!(self.message, "{value:?}");
            }
            name if name.starts_with("log.") => {}
            name => {
                let _ = write!(self.fields, " {name}={value:?}");
            }
        }
    }
}

/// Каждая паника пишется в журнал (`[launcher] паника: … в файл:строка` и стек);
/// затем срабатывает прежний обработчик.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info.payload_as_str().unwrap_or("(не строка)");
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "(место неизвестно)".to_owned());
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(target: "launcher", "паника: {message} в {location}\n{backtrace}");
        previous(info);
    }));
}
