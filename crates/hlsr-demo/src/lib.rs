//! Инструмент «Демо» без интерфейса: модуль на модуль `src/main/demo-tool` 0.4.9
//! (`kebab-case.js` → `snake_case.rs`, те же имена функций).
//!
//! Здесь разбор (задача DM1): заголовки демок, плейлист Ghosting, кадры демок Source (главы,
//! финиш рана) и GoldSrc (карты, кнопки KeyHUD), список демок и приём файлов. Строже 0.4.9
//! (решение владельца 5): финиш Source читается окнами, а не файлом целиком.
//!
//! План и задания (задача DM2): настройки, битрейт и аргументы FFmpeg (`planner`), оценка
//! времени (`estimate`), поиск и запуск FFmpeg (`ffmpeg`), задание сырьё → MP4 (`jobs`). Строже
//! 0.4.9: `ffmpeg.exe` инструмента сверяется с sha256 манифеста перед запуском.
//!
//! Хост рендера и общее (задача DM3): помощник на Win32 вместо PowerShell — скрытый стол, задание
//! и звук (`render_host`); реестр игры, mute-флаг, предусловия, папки рендера и восстановление
//! после сбоя, в том числе 0.4.9 (`game_render`).
//!
//! Адаптер Ghosting (задача DM4): режим мода `-ghrender` — признак в `client.dll`, аргументы,
//! журнал `GHRENDER 2`, таймлайн (`ghrender`), кадры в FFmpeg и звук мода (`ghosting`). Строже
//! 0.4.9: старого пути `+startdemos` нет, нужен Ghosting 1.0.13 или новее.
//!
//! Адаптер HL1 (задача DM5): bxt-rs — `job.cfg`, разбор `qconsole.log`, вырезанное, метки и
//! KeyHUD в ASS (`goldsrc_render`), Injector в папке задания, журналы игрока (`goldsrc`). Строже
//! 0.4.9: видео сегмента не длиннее демки по заголовку.
//!
//! Фасад для окна (задача DM6): 30 каналов `demo-*` и 6 событий, Store `demo-tool.json`,
//! «Отрендеренные», адреса плеера, фоновый сброс mute (`api`). Строже 0.4.9: «Показать» и
//! удаление видео — только со своим `timeline.json`.

pub mod api;
pub mod errors;
pub mod estimate;
pub mod ffmpeg;
pub mod game_render;
pub mod ghosting;
pub mod ghrender;
pub mod goldsrc;
pub mod goldsrc_demo;
pub mod goldsrc_render;
pub mod headers;
pub mod jobs;
pub mod library;
pub mod planner;
pub mod playlist;
pub mod render_host;
pub mod source_demo;
pub mod source_render;
mod window;

use hlsr_core::js::math_round;
use hlsr_core::json_text::js_number;
use serde_json::{Value, json};

/// Метка таймлайна во времени демо: `{t, type, label}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub t: f64,
    /// `map`, `saveload`, `segment`, `timer`, `chapter`, `finish`.
    pub kind: &'static str,
    pub label: String,
}

impl Marker {
    pub fn to_value(&self) -> Value {
        json!({"t": js_number(self.t), "type": self.kind, "label": self.label})
    }
}

/// `Math.round(value * 1000) / 1000`.
pub(crate) fn round3(value: f64) -> f64 {
    math_round(value * 1000.0) / 1000.0
}

pub(crate) fn i32_at(bytes: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 байта"))
}

pub(crate) fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().expect("2 байта"))
}

/// `readFloatLE`: f32 как число JS.
pub(crate) fn f32_at(bytes: &[u8], at: usize) -> f64 {
    f64::from(f32::from_le_bytes(
        bytes[at..at + 4].try_into().expect("4 байта"),
    ))
}

pub(crate) fn markers_value(markers: &[Marker]) -> Value {
    Value::Array(markers.iter().map(Marker::to_value).collect())
}
