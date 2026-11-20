//! Инструменты (LiveSplit) и внешние ссылки: каналы `tool-*`, `open-discord`,
//! `open-external-link`, `hl1-guide-open-link`, `toolCard`, `bindLiveSplitOverlay` из
//! `index.js` 0.4.7.

use super::{DeliveryApi, channels, poll_now, with_status};
use crate::errors::{Code, Failure, to_delivery_error};
use crate::hl1_guide;
use crate::js_value as jv;
use crate::links;
use crate::livesplit_splits;
use crate::tool_service::LaunchOptions;
use crate::tools as tool_registry;
use hlsr_core::js;
use serde_json::{Value, json};
use std::path::PathBuf;

fn tool_error(failure: Failure, fallback: Code) -> Value {
    let error = to_delivery_error(failure, Some(fallback));
    json!({"status": "error", "error": {"code": error.code.as_str(), "message": error.message}})
}

/// `unknownTool(toolId)` 0.4.9: каналы `tool-*` — только инструменты панели; зависимости
/// (FFmpeg и bxt-rs «Демо») ставит и проверяет их владелец, запускать их нельзя.
fn unknown_tool(tool_id: &str) -> Option<Value> {
    tool_registry::manifest_of(tool_id)
        .is_none()
        .then(|| json!({"status": "error", "error": {"code": "TOOL_UNKNOWN"}}))
}

impl DeliveryApi {
    /// `toolCard(toolId)`: состояние инструмента и его оформление.
    pub(crate) fn tool_card(&self, tool_id: &str) -> Value {
        let presentation = tool_registry::presentation_of(tool_id);
        let manifest = tool_registry::manifest_of(tool_id);
        let mut card = match self.tools() {
            Some(tools) => tools.status(tool_id),
            None => json!({"toolId": tool_id, "state": "unknown", "version": null,
                           "updateAvailable": false}),
        };
        let text = |key: &str| {
            presentation
                .and_then(|presentation| presentation.get(key))
                .filter(|value| js::truthy(value))
                .cloned()
        };
        let fields = [
            (
                "shortName",
                text("shortName").unwrap_or_else(|| json!(tool_id)),
            ),
            ("title", text("title").unwrap_or_else(|| json!(tool_id))),
            ("subtitle", text("subtitle").unwrap_or(Value::Null)),
            ("description", text("description").unwrap_or(Value::Null)),
            (
                "icon",
                text("icon").unwrap_or_else(|| json!("fas fa-toolbox")),
            ),
            ("accent", text("accent").unwrap_or(Value::Null)),
            ("accentSoft", text("accentSoft").unwrap_or(Value::Null)),
            ("hasHelp", json!(text("help").is_some())),
            (
                "sizeBytes",
                manifest
                    .and_then(|manifest| manifest.get("archive"))
                    .filter(|archive| js::truthy(archive))
                    .map_or(Value::Null, |archive| {
                        archive.get("size").cloned().unwrap_or(Value::Null)
                    }),
            ),
        ];
        if let Value::Object(map) = &mut card {
            for (key, value) in fields {
                map.insert(key.to_owned(), value);
            }
        }
        card
    }

    /// Состояние инструмента сменилось (запуск, выход, остановка — подсистема запуска).
    pub fn tool_state_changed(&self, tool_id: &str) {
        self.emit_to(channels::TOOL_STATUS, self.tool_card(tool_id));
        if tool_id == tool_registry::LIVESPLIT {
            self.bind_live_split_overlay();
        }
    }

    /// `bindLiveSplitOverlay()`: оверлей — к игре, для которой открыт LiveSplit.
    pub fn bind_live_split_overlay(&self) {
        let (Some(overlay), Some(tools)) = (self.ports().overlay.clone(), self.tools()) else {
            return;
        };
        let running = tools
            .running_info(tool_registry::LIVESPLIT)
            .and_then(|running| running.game_id);
        let store = self.settings_store().cloned();
        let settings_of = |game_id: &str| store.as_ref().map(|store| store.get(game_id).to_value());
        if overlay.bind_to(running.as_deref(), &settings_of) {
            self.logger().info(&format!(
                "[livesplit-overlay] оверлей привязан к {}",
                running.as_deref().unwrap_or("любой игре")
            ));
        }
    }

    /// `tool-list`.
    pub fn tool_list(&self) -> Value {
        Value::Array(
            tool_registry::ids()
                .into_iter()
                .map(|tool_id| self.tool_card(tool_id))
                .collect(),
        )
    }

    /// `tool-status`.
    pub fn tool_status(&self, tool_id: &Value) -> Value {
        self.tool_card(&jv::template(Some(tool_id)))
    }

    /// `tool-cancel`.
    pub fn tool_cancel(&self, tool_id: &Value) -> Value {
        let tool_id = jv::template(Some(tool_id));
        json!(
            unknown_tool(&tool_id).is_none()
                && self.tools().is_some_and(|tools| tools.cancel(&tool_id))
        )
    }

    /// `tool-install`: установка с событиями прогресса и состояния.
    pub async fn tool_install(&self, tool_id: &Value) -> Value {
        let tool_id = jv::template(Some(tool_id));
        if let Some(unknown) = unknown_tool(&tool_id) {
            return unknown;
        }
        let Some(tools) = self.tools() else {
            return tool_error(
                crate::errors::RawError::new("Cannot read properties of null").into(),
                Code::ToolInstallFailed,
            );
        };
        let progress = |percent: f64, _bytes: u64| {
            self.emit_to(
                channels::TOOL_PROGRESS,
                json!({"toolId": tool_id, "percent": percent}),
            );
        };
        let mut pending = std::pin::pin!(tools.install(&tool_id, Some(&progress)));
        // Синхронная часть установки (состояние «устанавливается») — до первого события.
        let first = poll_now(pending.as_mut());
        self.emit_to(channels::TOOL_STATUS, self.tool_card(&tool_id));
        let result = match first {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => pending.await,
        };
        match result {
            Ok(result) => {
                self.emit_to(channels::TOOL_STATUS, self.tool_card(&tool_id));
                with_status("ok", result)
            }
            Err(failure) => {
                let error = to_delivery_error(failure, Some(Code::ToolInstallFailed));
                if error.code == Code::Cancelled {
                    self.emit_to(channels::TOOL_STATUS, self.tool_card(&tool_id));
                    return json!({"status": "cancelled"});
                }
                self.logger().error(&format!(
                    "[tools] установка {tool_id} не удалась: {} {}",
                    error.code, error.message
                ));
                self.emit_to(channels::TOOL_STATUS, self.tool_card(&tool_id));
                json!({"status": "error",
                       "error": {"code": error.code.as_str(), "message": error.message}})
            }
        }
    }

    /// `toolGameId(gameChoice)`: игра из окна запуска, запущенная игра или последняя выбранная.
    fn tool_game_id(&self, game_choice: &Value) -> String {
        let chosen = js::truthy(game_choice)
            .then(|| self.resolve_game_id(game_choice))
            .flatten();
        let running = || {
            self.service()
                .and_then(|service| self.running_game(&service))
        };
        chosen
            .or_else(running)
            .unwrap_or_else(|| self.last_selected_game_id())
    }

    /// `tool-launch`: `options.restart` — LiveSplit уже открыт для другой игры, и игрок согласился
    /// его перезапустить.
    pub async fn tool_launch(
        &self,
        tool_id: &Value,
        splits: &Value,
        game_choice: &Value,
        options: &Value,
    ) -> Value {
        let tool_id = jv::template(Some(tool_id));
        if let Some(unknown) = unknown_tool(&tool_id) {
            return unknown;
        }
        let Some(tools) = self.tools() else {
            return tool_error(
                crate::errors::RawError::new("Cannot read properties of null").into(),
                Code::ToolLaunchFailed,
            );
        };
        let game_id = self.tool_game_id(game_choice);
        let preset = splits
            .as_str()
            .filter(|splits| !splits.is_empty())
            .map(str::to_owned)
            .or_else(|| self.live_split_choice(&game_id));
        let launch = LaunchOptions {
            splits: preset,
            server_port: self.live_split_server_port(&game_id),
            game_id: Some(game_id),
            restart: jv::prop(options, "restart") == Some(&json!(true)),
        };
        match tools.launch(&tool_id, &launch).await {
            Ok(launched) => {
                if tool_id == tool_registry::LIVESPLIT {
                    self.achieve("stopwatch");
                }
                with_status("ok", launched)
            }
            Err(failure) => tool_error(failure, Code::ToolLaunchFailed),
        }
    }

    /// `tool-stop`.
    pub async fn tool_stop(&self, tool_id: &Value, options: &Value) -> Value {
        let tool_id = jv::template(Some(tool_id));
        if let Some(unknown) = unknown_tool(&tool_id) {
            return unknown;
        }
        let Some(tools) = self.tools() else {
            return tool_error(
                crate::errors::RawError::new("Cannot read properties of null").into(),
                Code::StopFailed,
            );
        };
        let force = jv::truthy(jv::prop(options, "force"));
        match tools.stop(&tool_id, force).await {
            Ok(status) => json!({"status": status.as_str()}),
            Err(failure) => tool_error(failure, Code::StopFailed),
        }
    }

    /// `tool-remove`.
    pub fn tool_remove(&self, tool_id: &Value) -> Value {
        let tool_id = jv::template(Some(tool_id));
        if let Some(unknown) = unknown_tool(&tool_id) {
            return unknown;
        }
        let Some(tools) = self.tools() else {
            return tool_error(
                crate::errors::RawError::new("Cannot read properties of null").into(),
                Code::ToolRemoveFailed,
            );
        };
        match tools.remove(&tool_id) {
            Ok(result) => {
                self.emit_to(channels::TOOL_STATUS, self.tool_card(&tool_id));
                with_status("ok", result)
            }
            Err(failure) => tool_error(failure, Code::ToolRemoveFailed),
        }
    }

    pub fn live_split_choice(&self, game_id: &str) -> Option<String> {
        self.live_split_choices()
            .into_iter()
            .find(|(id, _)| id == game_id)
            .map(|(_, preset)| preset)
    }

    /// `tool-splits-set`: выбранные сплиты по играм.
    pub fn tool_splits_set(&self, tool_id: &Value, choices: &Value) -> Value {
        if tool_id.as_str() != Some(tool_registry::LIVESPLIT) || !js::truthy(choices) {
            return json!({"status": "unknown-splits"});
        }
        let entries: Vec<(String, &Value)> = match choices {
            Value::Object(map) => map
                .iter()
                .map(|(key, value)| (key.clone(), value))
                .collect(),
            Value::Array(list) => list
                .iter()
                .enumerate()
                .map(|(index, value)| (index.to_string(), value))
                .collect(),
            _ => return json!({"status": "unknown-splits"}),
        };
        let next: Vec<(String, String)> = entries
            .into_iter()
            .filter_map(|(game_id, choice)| {
                let choice = choice.as_str()?;
                livesplit_splits::presets_of(&game_id)
                    .iter()
                    .any(|preset| preset.id == choice)
                    .then(|| (game_id, choice.to_owned()))
            })
            .collect();
        self.set_live_split_choices(next);
        json!({"status": "ok"})
    }

    /// `tool-splits-presets`: наборы сплитов для игр окна запуска LiveSplit.
    pub fn tool_splits_presets(&self, ids: &Value) -> Value {
        let Some(ids) = ids.as_array() else {
            return json!([]);
        };
        Value::Array(
            ids.iter()
                .take(32)
                .map(|id| {
                    let text = jv::template(Some(id));
                    let game_id = self.resolve_game_id(&json!(text));
                    let presets = game_id
                        .as_deref()
                        .map(livesplit_splits::describe)
                        .unwrap_or_default();
                    json!({"id": text, "gameId": game_id, "presets": presets})
                })
                .collect(),
        )
    }

    /// `tool-open-folder`.
    pub async fn tool_open_folder(&self, tool_id: &Value) -> Value {
        let tool_id = jv::template(Some(tool_id));
        if let Some(unknown) = unknown_tool(&tool_id) {
            return unknown;
        }
        let Some(tools) = self.tools() else {
            return json!({"status": "unavailable"});
        };
        let state = tools.status(&tool_id);
        let install_path = state
            .get("installPath")
            .filter(|path| js::truthy(path))
            .map(|path| jv::template(Some(path)));
        let (Some("ready"), Some(install_path)) =
            (state.get("state").and_then(Value::as_str), install_path)
        else {
            return json!({"status": "not-installed"});
        };
        let failure = self
            .ports()
            .shell
            .open_path(&PathBuf::from(install_path))
            .await;
        if failure.is_empty() {
            json!({"status": "ok"})
        } else {
            json!({"status": "error", "error": {"code": "PATH_INVALID", "message": failure}})
        }
    }

    /// `tool-open-help`.
    pub fn tool_open_help(&self, tool_id: &Value) -> Value {
        let help = tool_registry::presentation_of(&jv::template(Some(tool_id)))
            .and_then(|presentation| presentation.get("help"))
            .and_then(Value::as_str)
            .filter(|help| !help.is_empty());
        let Some(help) = help else {
            return json!({"status": "unknown-tool"});
        };
        self.open_link(help);
        json!({"status": "ok"})
    }

    // ------------------------------------------------------------ ссылки

    /// `open-discord`.
    pub fn open_discord(&self) -> Value {
        if let Some(url) = links::url_for(&json!("discord")) {
            self.open_link(url);
        }
        json!({"status": "ok"})
    }

    /// `open-external-link`: только ссылки лаунчера по ключу.
    pub fn open_external_link(&self, key: &Value) -> Value {
        let Some(url) = links::url_for(key) else {
            self.logger().warn(&format!(
                "[links] отклонён неизвестный ключ ссылки: {}",
                jv::template(Some(key))
            ));
            return json!({"status": "error", "code": "UNKNOWN_LINK"});
        };
        self.open_link(url);
        json!({"status": "ok"})
    }

    /// `hl1-guide-open-link`: только ссылки из данных гайда HL1.
    pub fn hl1_guide_open_link(&self, url: &Value) -> Value {
        let Some(url) = url.as_str().filter(|url| hl1_guide::is_guide_link(url)) else {
            self.logger()
                .warn("[links] отклонена ссылка не из гайда HL1");
            return json!({"status": "error", "code": "UNKNOWN_LINK"});
        };
        self.open_link(url);
        json!({"status": "ok"})
    }

    /// `shell.openExternal(url)` без ожидания, как в 0.4.7: ответ окну не зависит от браузера;
    /// отказ ОС остаётся в журнале.
    fn open_link(&self, url: &str) {
        if let Err(error) = self.ports().shell.open_external(url) {
            self.logger()
                .warn(&format!("[links] ссылку открыть не удалось: {error}"));
        }
    }
}
