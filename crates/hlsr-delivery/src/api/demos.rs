//! Демо игры (`demoContext`, `listGameDemos`, `packGameDemos` и `game-demos-archive-reveal` из
//! `index.js` 0.4.9): список, упаковка выбранных в zip и показ архива. Путь из окна в fs не
//! попадает.

use super::{DeliveryApi, error_code, lock};
use crate::demo_archive::{self, Demo, PackRequest};
use crate::manifests::{self, LocalConfig};
use chrono::{Local, TimeZone};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Событие окна: ход упаковки `{gameId, percent}`.
pub const DEMOS_PACK_PROGRESS: &str = "game-demos-pack-progress";

/// Игра и её папки с демо.
struct DemoContext {
    game_id: String,
    config: &'static LocalConfig,
    install_path: PathBuf,
}

/// Снимает `demoPackBusy` и при брошенной задаче.
struct PackSlot<'a>(&'a DeliveryApi);

impl Drop for PackSlot<'_> {
    fn drop(&mut self) {
        lock(&self.0.state).demo_pack_busy = false;
    }
}

impl DeliveryApi {
    /// `demoContext(id)`: `None` — игра не установлена или у неё нет демо.
    fn demo_context(&self, id: &Value) -> Option<DemoContext> {
        let game_id = self.resolve_game_id(id)?;
        let config = manifests::local_config_of(&game_id)?;
        let install_path = self.installed_path_of(&game_id)?;
        if demo_archive::demo_sources(config, &install_path)
            .0
            .is_empty()
        {
            return None;
        }
        Some(DemoContext {
            game_id,
            config,
            install_path,
        })
    }

    /// `game-demos-list`: `{status: "ok", demos}`.
    pub fn game_demos_list(&self, id: &Value) -> Value {
        let Some(context) = self.demo_context(id) else {
            return error_code("NOT_INSTALLED");
        };
        let demos: Vec<Value> = demo_archive::list_demos(context.config, &context.install_path)
            .iter()
            .map(Demo::to_value)
            .collect();
        json!({"status": "ok", "demos": demos})
    }

    /// `game-demos-pack`: zip из выбранных демо в `<библиотека>/Demos`, ход — событием
    /// [`DEMOS_PACK_PROGRESS`]. Одна упаковка за раз.
    pub async fn game_demos_pack(&self, id: &Value, requested: &Value) -> Value {
        let context = self.demo_context(id);
        let (Some(context), Some(service)) = (context, self.ensure_service()) else {
            return error_code("NOT_INSTALLED");
        };
        // Как в 0.4.9: занято — отказ до сканирования; слот держится и на время сканирования.
        {
            let mut state = lock(&self.state);
            if state.demo_pack_busy {
                return error_code("PACK_BUSY");
            }
            state.demo_pack_busy = true;
        }
        let _slot = PackSlot(self);
        let scanned = demo_archive::list_demos(context.config, &context.install_path);
        let items = match demo_archive::pick_items(requested, &scanned) {
            Ok(items) => items,
            Err(code) => return error_code(code),
        };
        let weak = self.weak.clone();
        let game_id = id.clone();
        let now = Local
            .timestamp_millis_opt(self.now_ms())
            .earliest()
            .map(|date| date.naive_local())
            .unwrap_or_default();
        let packed = demo_archive::pack_demos(PackRequest {
            install_path: &context.install_path,
            items: &items,
            archives_dir: &service.paths().root.join("Demos"),
            slug: if context.game_id == "ghosting-renewed" {
                "ghosting"
            } else {
                "hl1"
            },
            now,
            archive: service.archive_service(),
            on_progress: Some(Arc::new(move |percent| {
                if let Some(api) = weak.upgrade() {
                    api.emit_to(
                        DEMOS_PACK_PROGRESS,
                        json!({"gameId": game_id, "percent": percent}),
                    );
                }
            })),
        })
        .await;
        match packed {
            Ok(archive) => json!({"status": "ok", "archive": archive.to_value()}),
            Err(failure) => {
                self.logger().warn(&format!(
                    "[demos] {}: упаковка не удалась: {failure}",
                    context.game_id
                ));
                error_code(if demo_archive::is_demo_not_found(&failure) {
                    "DEMO_NOT_FOUND"
                } else {
                    "PACK_FAILED"
                })
            }
        }
    }

    /// `game-demos-archive-reveal`: только имя архива демо в `<библиотека>/Demos`.
    pub fn game_demos_archive_reveal(&self, name: &Value) -> Value {
        let target = self
            .service()
            .zip(
                name.as_str()
                    .filter(|name| demo_archive::is_archive_name(name)),
            )
            .map(|(service, name)| service.paths().root.join("Demos").join(name))
            .filter(|target| target.exists());
        let Some(target) = target else {
            return error_code("NOT_FOUND");
        };
        self.ports().shell.show_item_in_folder(&target);
        json!({"status": "ok"})
    }
}
