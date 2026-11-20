//! Видео и звук плеера «Демо» (решение владельца 3): кадры и звук декодирует `ffmpeg.exe`
//! инструмента, звук играет cpal. Найти и сверить FFmpeg (`hlsr_demo::ffmpeg::{locate, verify}`)
//! — дело вызывающего, сюда приходят готовые пути.
//!
//! - [`video::probe`] — длина, размер, частота кадров, есть ли звук (`ffprobe`, звать в фоне);
//! - [`video::Video`] — плеер: кадры `rawvideo bgra` под размер рамки → `RenderImage`, звук
//!   [`audio::Audio`], перемотка перезапуском с `-ss`, скорость `setpts`/`atempo`, пауза,
//!   громкость, часы по звуку;
//! - процессы FFmpeg — только свои: гасятся по своему дескриптору при перемотке и закрытии.

pub mod audio;
pub mod video;

use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Ошибка видео (`@error` плеера 0.4.9): FFmpeg не запустился или файл не читается.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaError(pub String);

/// Команда FFmpeg без окна консоли.
fn command(exe: &Path) -> Command {
    let mut command = Command::new(exe);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// FFmpeg с выводом в трубу (`pipe:1`); stderr отброшен — в аргументах `-v error`.
fn spawn(exe: &Path, args: &[String]) -> Result<Child, MediaError> {
    command(exe)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| MediaError(format!("{}: {error}", exe.display())))
}

/// Свой процесс: погасить по его дескриптору (труба закрывается, поток чтения выходит сам);
/// выхода дожидается фоновый поток — окно не ждёт. Поток не создался — процесс погашен, но не
/// дождан.
fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = std::thread::Builder::new()
        .name("hlsr-reap".into())
        .spawn(move || child.wait());
}
