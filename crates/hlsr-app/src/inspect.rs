//! Живой инспектор (dev-hooks): канал для проверки окна на настоящем мониторе
//! (`scripts/gpui/live.py`). `HLSR_LIVE_PORT` — порт TCP на 127.0.0.1: одна строка-запрос JSON,
//! одна строка-ответ JSON, соединение закрывается. Запросы:
//! - `{"cmd": "dump"}` — [`hlsr_gpui::live::snapshot`] (кадр, экран, окна и панели, отмеченные
//!   элементы с рамками в физических px клиентской области), `pid`, `hwnd` и последние ошибки и
//!   предупреждения `main.log` (паника пишется туда же со стеком);
//! - `{"cmd": "key", "key": "a", "modifiers": ["control"]}` — нажатие через GPUI, как шаг `key`
//!   стенда: сочетания с модификаторами сообщениями WM_KEYDOWN не послать (GPUI читает
//!   модификаторы `GetKeyState`);
//! - `{"cmd": "frames"}` — начала последних кадров ([`hlsr_gpui::live::frame_times`]) и активно ли
//!   окно (неактивному GPUI по умолчанию даёт 30 кадров/с): замер частоты анимаций.

use gpui::App;
use hlsr_core::paths::Paths;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

/// Сколько ждать ответа потока окна.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(5);
/// Хвост журнала, где ищутся ошибки.
const LOG_TAIL: u64 = 512 * 1024;
/// Ошибок и предупреждений в ответе — не больше.
const LAST_ERRORS: usize = 20;
/// Длина одной записи журнала в ответе (паника — со стеком).
const ENTRY_CHARS: usize = 1200;

/// Поднять канал, если задан `HLSR_LIVE_PORT`.
pub fn start(paths: &Paths) {
    let Some(value) = crate::modes::dev_env("HLSR_LIVE_PORT") else {
        return;
    };
    let Ok(port) = value.parse::<u16>() else {
        tracing::warn!("[inspect] HLSR_LIVE_PORT={value} — не порт");
        return;
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!("[inspect] 127.0.0.1:{port} не открыт: {error}");
            return;
        }
    };
    tracing::info!("[inspect] инспектор на 127.0.0.1:{port}");
    let log = paths.logs_dir.join("main.log");
    let spawned = std::thread::Builder::new()
        .name("hlsr-inspect".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Err(error) = serve(stream, &log) {
                    tracing::warn!("[inspect] запрос не обслужен: {error}");
                }
            }
        });
    if let Err(error) = spawned {
        tracing::warn!("[inspect] поток не запущен: {error}");
    }
}

fn serve(stream: TcpStream, log: &Path) -> std::io::Result<()> {
    stream.set_read_timeout(Some(WINDOW_TIMEOUT))?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let reply = match serde_json::from_str::<Value>(line.trim()) {
        Ok(request) => answer(request, log),
        Err(error) => json!({"error": format!("запрос не JSON: {error}")}),
    };
    let mut stream = stream;
    writeln!(stream, "{reply}")
}

fn answer(request: Value, log: &Path) -> Value {
    let (tx, rx) = mpsc::channel();
    let dump = request["cmd"].as_str().unwrap_or("dump") == "dump";
    if !hlsr_gpui::bridge::invoke(move |cx| {
        let _ = tx.send(on_window(&request, cx));
    }) {
        return json!({"error": "цикла событий нет"});
    }
    let mut reply = rx
        .recv_timeout(WINDOW_TIMEOUT)
        .unwrap_or_else(|_| json!({"error": "окно не ответило за 5 с"}));
    reply["pid"] = json!(std::process::id());
    if dump {
        let (errors, warnings) = log_problems(log);
        reply["errors"] = json!(errors);
        reply["warnings"] = json!(warnings);
    }
    reply
}

fn on_window(request: &Value, cx: &mut App) -> Value {
    let Some(launcher) = crate::window::launcher(cx) else {
        return json!({"error": "окна лаунчера нет"});
    };
    // Окно без корня: `WindowHandle::update` занял бы `LauncherRoot`, и клавиша, которую корень
    // обрабатывает сам (Ctrl+A в поле), уронила бы лаунчер повторным `update`.
    gpui::AnyWindowHandle::from(launcher)
        .update(cx, |_, window, cx| {
            match request["cmd"].as_str().unwrap_or("dump") {
                "dump" => {
                    #[allow(unused_mut)]
                    let mut dump = hlsr_gpui::live::snapshot(window, cx);
                    #[cfg(windows)]
                    {
                        dump["hwnd"] = json!(hlsr_gpui::win32::hwnd(window));
                    }
                    dump
                }
                "frames" => {
                    let mut frames = hlsr_gpui::live::frame_times();
                    frames["active"] = json!(window.is_window_active());
                    frames
                }
                "key" => {
                    let modifiers: Vec<String> = request["modifiers"]
                        .as_array()
                        .map(|list| {
                            list.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default();
                    let key = request["key"].as_str().unwrap_or_default();
                    match hlsr_gpui::snap::keystroke(key, &modifiers) {
                        Ok(keystroke) => {
                            let handled = window.dispatch_keystroke(keystroke, cx);
                            json!({"ok": true, "handled": handled})
                        }
                        Err(error) => json!({"error": error}),
                    }
                }
                other => json!({"error": format!("неизвестная команда {other}")}),
            }
        })
        .unwrap_or_else(|error| json!({"error": format!("{error:#}")}))
}

/// Последние записи `[error]` и `[warn]` из хвоста журнала (запись — строка с отметкой времени и
/// строки после неё до следующей отметки: стек паники).
fn log_problems(log: &Path) -> (Vec<String>, Vec<String>) {
    let Ok(text) = tail(log, LOG_TAIL) else {
        return (Vec::new(), Vec::new());
    };
    problems(&text)
}

fn tail(path: &Path, bytes: u64) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(bytes)))?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

fn problems(text: &str) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    // (уровень, запись) текущей записи.
    let mut current: Option<(bool, String)> = None;
    let mut flush = |current: Option<(bool, String)>| {
        if let Some((error, entry)) = current {
            let entry: String = entry.trim_end().chars().take(ENTRY_CHARS).collect();
            if error {
                errors.push(entry);
            } else {
                warnings.push(entry);
            }
        }
    };
    for line in text.lines() {
        // `[YYYY-MM-DD HH:MM:SS.mmm] [уровень] …` (`hlsr_core::logging::format_line`).
        let stamped = line.starts_with('[') && line.get(24..26) == Some("] ");
        if stamped {
            flush(current.take());
            let level = line.get(26..).unwrap_or_default();
            if level.starts_with("[error]") {
                current = Some((true, line.to_owned()));
            } else if level.starts_with("[warn]") {
                current = Some((false, line.to_owned()));
            }
        } else if let Some((_, entry)) = current.as_mut() {
            entry.push('\n');
            entry.push_str(line);
        }
    }
    flush(current);
    let last = |list: Vec<String>| list[list.len().saturating_sub(LAST_ERRORS)..].to_vec();
    (last(errors), last(warnings))
}

#[cfg(test)]
mod tests {
    use super::problems;

    #[test]
    fn errors_and_warnings_come_with_their_continuation_lines() {
        let log = "garbage tail of a cut line\n\
            [2026-11-18 20:00:00.001] [info]  [startup] ok\n\
            [2026-11-18 20:00:00.002] [warn]  [image] slow\n\
            [2026-11-18 20:00:00.003] [error] [launcher] паника: boom в a.rs:1\n\
            stack line 1\n\
            stack line 2\n\
            [2026-11-18 20:00:00.004] [info]  after\n";
        let (errors, warnings) = problems(log);
        assert_eq!(warnings, ["[2026-11-18 20:00:00.002] [warn]  [image] slow"]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].ends_with("паника: boom в a.rs:1\nstack line 1\nstack line 2"));
    }
}
