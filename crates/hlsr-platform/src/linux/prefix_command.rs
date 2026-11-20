//! Команда в префиксе Proton (`linux/PrefixCommand.js`): ожидание с тайм-аутом, остановка группы
//! процессов (SIGTERM, через 2 с SIGKILL), вывод — последние 8 КБ без управляющих
//! последовательностей и секретов.

use crate::spawn::{Child, ChildExit};
use hlsr_core::redact::redact_text;
use regex::Regex;
use std::sync::LazyLock;
use std::time::Duration;

/// `OUTPUT_LIMIT`.
pub const OUTPUT_LIMIT: usize = 8 * 1024;
/// `DEFAULT_TIMEOUT_MS`.
pub const DEFAULT_TIMEOUT_MS: u64 = 5 * 60 * 1000;
/// `DEFAULT_KILL_GRACE_MS`.
pub const DEFAULT_KILL_GRACE_MS: u64 = 2000;

/// Ошибка модулей Proton: `Error` 0.4.7 с полями `code`, `exitCode`, `stdout`, `stderr`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProtonError {
    pub code: Option<String>,
    pub message: String,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

impl ProtonError {
    /// `new Error(message)`.
    pub fn new(message: impl Into<String>) -> ProtonError {
        ProtonError {
            message: message.into(),
            ..ProtonError::default()
        }
    }

    /// Ошибка с кодом (`error.code = …`).
    pub fn with_code(code: &str, message: impl Into<String>) -> ProtonError {
        ProtonError {
            code: Some(code.to_owned()),
            ..ProtonError::new(message)
        }
    }

    /// `AbortError` (`ABORT_ERR`).
    pub fn abort(message: impl Into<String>) -> ProtonError {
        ProtonError::with_code("ABORT_ERR", message)
    }

    pub fn is_abort(&self) -> bool {
        self.code.as_deref() == Some("ABORT_ERR")
    }
}

impl std::fmt::Display for ProtonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProtonError {}

impl From<hlsr_delivery::errors::RawError> for ProtonError {
    fn from(error: hlsr_delivery::errors::RawError) -> ProtonError {
        ProtonError {
            code: error.code,
            message: error.message,
            ..ProtonError::default()
        }
    }
}

// `\x1b\[[0-?]*[ -/]*[@-~]` и строка переменной окружения с секретом (`/gi`).
static ANSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("шаблон ANSI"));
static SECRET_ENV: LazyLock<Regex> = LazyLock::new(|| {
    // `\s` JS — один класс на весь проект (`hlsr_core::js::WHITESPACE_CLASS`).
    let ws = hlsr_core::js::WHITESPACE_CLASS;
    Regex::new(&format!(
        r"(?i-u:\b[A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL|API_KEY|COOKIE|AUTH)[A-Z0-9_]*)[{ws}]*=[{ws}]*[^\r\n]*"
    ))
    .expect("шаблон переменной с секретом")
});

/// Последние `units` единиц UTF-16 строки (`slice(-units)`).
fn tail_utf16(text: &str, units: usize) -> &str {
    let mut count = 0;
    for (index, c) in text.char_indices().rev() {
        count += c.len_utf16();
        if count > units {
            return &text[index + c.len_utf8()..];
        }
    }
    text
}

/// `cleanOutput(value)`: без ANSI, секретов окружения и того, что прячет `redactText`; последние
/// 8 КБ, без пробелов по краям.
pub fn clean_output(value: &str) -> String {
    let without_ansi = ANSI.replace_all(value, "");
    let without_env = SECRET_ENV.replace_all(&without_ansi, "<redacted environment>");
    let redacted = redact_text(&without_env);
    hlsr_core::js::trim(tail_utf16(&redacted, OUTPUT_LIMIT)).to_owned()
}

/// `terminate(child, signal)`: группа процессов, иначе сам процесс.
pub fn terminate(child: &Child, signal: &str) {
    if child.pid().is_some_and(|pid| pid > 0) && child.kill_group(signal) {
        return;
    }
    child.kill(signal);
}

/// Параметры `waitForPrefixCommand`.
#[derive(Debug, Clone, Default)]
pub struct WaitOptions {
    pub label: Option<String>,
    pub timeout_ms: Option<u64>,
    pub kill_grace_ms: Option<u64>,
}

/// Вывод команды: `{stdout, stderr}` после очистки.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

enum Failure {
    Error(ProtonError),
    Timeout,
    Exit(Option<i32>, Option<String>),
}

/// `waitForPrefixCommand(child, options)`: вывод при коде 0; иначе ошибка `<label>: …` с кодом
/// `PROTON_PREFIX_COMMAND_FAILED` (или `…_TIMEOUT`, или кодом ошибки запуска), `exitCode`,
/// очищенными `stdout` и `stderr`.
pub async fn wait_for_prefix_command(
    child: &Child,
    options: &WaitOptions,
) -> Result<CommandOutput, ProtonError> {
    let label = options
        .label
        .clone()
        .unwrap_or_else(|| "umu-run".to_owned());
    let timeout_ms = options.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let kill_grace_ms = options.kill_grace_ms.unwrap_or(DEFAULT_KILL_GRACE_MS);
    let (mut stdout_rx, mut stderr_rx) = child.take_output();
    let mut stdout = String::new();
    let mut stderr = String::new();
    let append = |buffer: &mut String, chunk: &[u8]| {
        buffer.push_str(&String::from_utf8_lossy(chunk));
        let tail = tail_utf16(buffer, OUTPUT_LIMIT).to_owned();
        *buffer = tail;
    };
    let drain = |rx: &mut Option<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>,
                 buffer: &mut String| {
        if let Some(rx) = rx {
            while let Ok(chunk) = rx.try_recv() {
                append(buffer, &chunk);
            }
        }
    };

    // Сначала тайм-аут команды, после SIGTERM — пауза до SIGKILL.
    let timer = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(timer);
    let mut timed_out = false;
    let failure = loop {
        tokio::select! {
            biased;
            Some(chunk) = recv(&mut stdout_rx) => append(&mut stdout, &chunk),
            Some(chunk) = recv(&mut stderr_rx) => append(&mut stderr, &chunk),
            end = child.ended() => {
                drain(&mut stdout_rx, &mut stdout);
                drain(&mut stderr_rx, &mut stderr);
                break match end {
                    ChildExit::Error(error) => Some(Failure::Error(error.into())),
                    _ if timed_out => Some(Failure::Timeout),
                    ChildExit::Exit { code: Some(0), .. } => None,
                    ChildExit::Exit { code, signal } => Some(Failure::Exit(code, signal)),
                };
            }
            () = &mut timer => {
                if timed_out {
                    terminate(child, "SIGKILL");
                    drain(&mut stdout_rx, &mut stdout);
                    drain(&mut stderr_rx, &mut stderr);
                    break Some(Failure::Timeout);
                }
                timed_out = true;
                terminate(child, "SIGTERM");
                timer.as_mut().reset(tokio::time::Instant::now() + Duration::from_millis(kill_grace_ms));
            }
        }
    };
    let output = CommandOutput {
        stdout: clean_output(&stdout),
        stderr: clean_output(&stderr),
    };
    let Some(failure) = failure else {
        return Ok(output);
    };
    let (code, message, exit_code) = match failure {
        Failure::Error(error) => (error.code, error.message, None),
        Failure::Timeout => (
            Some("PROTON_PREFIX_COMMAND_TIMEOUT".to_owned()),
            format!(
                "превышено время ожидания ({} с)",
                hlsr_core::js::math_round(timeout_ms as f64 / 1000.0)
            ),
            None,
        ),
        Failure::Exit(code, signal) => (
            None,
            format!(
                "завершился с кодом {}",
                code.map_or_else(
                    || signal.unwrap_or_else(|| "null".to_owned()),
                    |c| c.to_string()
                )
            ),
            code,
        ),
    };
    let details: Vec<&str> = [output.stderr.as_str(), output.stdout.as_str()]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect();
    let details = details.join("\n");
    Err(ProtonError {
        code: Some(code.unwrap_or_else(|| "PROTON_PREFIX_COMMAND_FAILED".to_owned())),
        message: if details.is_empty() {
            format!("{label}: {message}")
        } else {
            format!("{label}: {message}\n{details}")
        },
        exit_code,
        stdout: Some(output.stdout),
        stderr: Some(output.stderr),
    })
}

/// Следующий кусок канала; канала нет или он закрыт — никогда.
async fn recv(rx: &mut Option<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>) -> Option<Vec<u8>> {
    match rx {
        Some(receiver) => match receiver.recv().await {
            Some(chunk) => Some(chunk),
            None => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}
