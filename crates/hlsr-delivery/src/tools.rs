//! Реестр инструментов и оформление их карточек (`tools.js`).
//!
//! Манифест LiveSplit — вшитый `config/tools/livesplit.manifest.json`. Поиск по идентификатору
//! видит только свои ключи (`own` в 0.4.7).
//!
//! 0.4.9: FFmpeg и bxt-rs — зависимости «Демо» (`DEPENDENCIES`). Их ставит тот же `ToolService`,
//! но на панели инструментов карточек нет и каналы `tool-*` их не знают (`manifest_of` — только
//! `REGISTRY`).

use crate::manifests::livesplit_tool_manifest;
use hlsr_core::json_text::parse_js_json;
use serde_json::{Map, Value, json};
use std::sync::LazyLock;

/// `LIVESPLIT`.
pub const LIVESPLIT: &str = "livesplit";
/// `FFMPEG`.
pub const FFMPEG: &str = "ffmpeg";
/// `BXTRS`.
pub const BXTRS: &str = "bxt-rs";

static FFMPEG_MANIFEST: LazyLock<Value> = LazyLock::new(|| {
    parse_js_json(include_str!("../../../config/tools/ffmpeg.manifest.json"))
        .expect("вшитый манифест FFmpeg — JSON")
});
static BXTRS_MANIFEST: LazyLock<Value> = LazyLock::new(|| {
    parse_js_json(include_str!("../../../config/tools/bxt-rs.manifest.json"))
        .expect("вшитый манифест bxt-rs — JSON")
});

/// `PRESENTATION`: оформление карточки инструмента — `{shortName, title, subtitle, description,
/// icon, accent, accentSoft, help}`.
static PRESENTATION: LazyLock<Value> = LazyLock::new(|| {
    json!({
        (LIVESPLIT): {
            "shortName": "LiveSplit",
            "title": "LiveSplit",
            "subtitle": "#L_TOOL_LIVESPLIT_SUB",
            "description": "#L_TOOL_LIVESPLIT_DESC",
            "icon": "fas fa-stopwatch",
            "accent": "#3fa7ff",
            "accentSoft": "rgba(63, 167, 255, 0.16)",
            "help": "https://youtu.be/l-lZ1yr_Mlc?t=316",
        }
    })
});

/// Реестр `ToolService`: `Object.assign({}, REGISTRY, DEPENDENCIES)` из `index.js` 0.4.9
/// (копия, чтобы сервис мог её менять).
pub fn registry() -> Map<String, Value> {
    let mut registry = Map::new();
    registry.insert(
        LIVESPLIT.to_owned(),
        livesplit_tool_manifest().value().clone(),
    );
    registry.insert(FFMPEG.to_owned(), FFMPEG_MANIFEST.clone());
    registry.insert(BXTRS.to_owned(), BXTRS_MANIFEST.clone());
    registry
}

/// `manifestOf(toolId)`: только инструменты панели (`REGISTRY`).
pub fn manifest_of(tool_id: &str) -> Option<&'static Value> {
    (tool_id == LIVESPLIT).then(|| livesplit_tool_manifest().value())
}

/// `DEPENDENCIES[toolId]`: манифест FFmpeg или bxt-rs.
pub fn dependency_of(tool_id: &str) -> Option<&'static Value> {
    match tool_id {
        FFMPEG => Some(&FFMPEG_MANIFEST),
        BXTRS => Some(&BXTRS_MANIFEST),
        _ => None,
    }
}

/// `presentationOf(toolId)`.
pub fn presentation_of(tool_id: &str) -> Option<&'static Value> {
    PRESENTATION.get(tool_id)
}

/// `ids()`.
pub fn ids() -> Vec<&'static str> {
    vec![LIVESPLIT]
}
