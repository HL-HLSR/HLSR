//! Каналы только для окна WinUI: то, что окно GPUI брало из крейтов напрямую.
//!
//! - `winui-bind-presets(gameId)` — каталог заготовок биндов игры (`bind_presets::catalog_for`):
//!   `{engine, runModes, categories: [{id, tone, title: {ru, en}}], presets: [{category, command,
//!   label: {ru, en}, note, scripted, practice}]}`; игры без каталога — `null`.

use hlsr_core::js::Platform;
use hlsr_delivery::bind_presets::{self, Loc};
use hlsr_delivery::providers::BoxFuture;
use hlsr_ui::services::{ChannelService, Reply};
use serde_json::{Value, json};

pub struct WinUiChannels;

fn loc(text: &Loc) -> Value {
    json!({"ru": text.ru, "en": text.en})
}

fn bind_presets(game_id: Option<&Value>) -> Value {
    let Some(game_id) = game_id.and_then(Value::as_str) else {
        return Value::Null;
    };
    let Some(catalog) = bind_presets::catalog_for(game_id, Platform::Win32) else {
        return Value::Null;
    };
    json!({
        "gameId": catalog.game_id,
        "engine": catalog.engine,
        "runModes": catalog.run_modes,
        "categories": catalog.categories.iter().map(|category| json!({
            "id": category.id,
            "tone": category.tone,
            "title": loc(&category.title),
        })).collect::<Vec<_>>(),
        "presets": catalog.presets.iter().filter(|preset| preset.listed).map(|preset| json!({
            "category": preset.category,
            "command": preset.command,
            "label": loc(&preset.label),
            "note": preset.note.as_ref().map(loc),
            "scripted": preset.scripted,
            "practice": preset.practice,
        })).collect::<Vec<_>>(),
    })
}

impl ChannelService for WinUiChannels {
    fn call<'a>(&'a self, channel: &'a str, args: &'a [Value]) -> BoxFuture<'a, Option<Reply>> {
        Box::pin(async move {
            match channel {
                "winui-bind-presets" => Some(Ok(bind_presets(args.first()))),
                _ => None,
            }
        })
    }
}
