//! Настройки игр, бинды и персонализация (`gameSettings`, `gameBinds`,
//! `inspectPlayerPersonalization`, `applyPersonalizationIfInstalled`, `pickPersonalizationAsset`,
//! `readPersonalizationPreviewAsset`, `personalizationCode`, `ghostingScreen` из `index.js`
//! 0.4.9). Бинды, скрипты и персонализация пишутся в файл игрока (см. `player_config`), каналы
//! импорта — в `api/player_config.rs`.

use super::{DeliveryApi, PLATFORM, error_code};
use crate::bind_presets;
use crate::custom_packs;
use crate::errors::Failure;
use crate::game_binds::{self, overrides_to_value};
use crate::game_binds_install::{self, ApplyOptions, PlayerBinds};
use crate::game_language;
use crate::game_settings_store::StoreError;
use crate::ghost_model;
use crate::ghosting_assets;
use crate::ghosting_settings;
use crate::goldsrc_packs;
use crate::js_value as jv;
use crate::launch_args;
use crate::manifests::{self, LaunchConfig};
use crate::personalization_code::{self, CodeError};
use crate::player_config;
use crate::ports::{FileFilter, OpenDialog};
use crate::source_screen;
use crate::vulkan_backend;
use hlsr_core::js;
use hlsr_core::json_text::{js_number, stringify};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::PathBuf;

const GHOSTING: &str = "ghosting-renewed";

/// `settings && settings[key]`: ложное `settings` — оно само; не объект или нет поля —
/// `undefined` (`None`).
fn and_prop(settings: &Value, key: &str) -> Option<Value> {
    if !js::truthy(settings) {
        return Some(settings.clone());
    }
    jv::prop(settings, key).cloned()
}

/// `Object.assign(target, {key: value})` с `undefined`: поле без значения выпадает (дальше его
/// читают как отсутствующее).
fn assign(target: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    match value {
        Some(value) => {
            target.insert(key.to_owned(), value);
        }
        None => {
            target.remove(key);
        }
    }
}

fn store_error_value(error: &StoreError) -> Option<Value> {
    match error {
        StoreError::Rejected(rejected) => Some(rejected.to_value()),
        StoreError::Io(_) => None,
    }
}

/// Сбой действия с настройками: текст для журнала и был ли это `CONFIG_CHANGED`.
struct Failed {
    message: String,
    changed: bool,
}

impl From<String> for Failed {
    fn from(message: String) -> Failed {
        Failed {
            message,
            changed: false,
        }
    }
}

impl From<player_config::Error> for Failed {
    fn from(error: player_config::Error) -> Failed {
        Failed {
            changed: error.is("CONFIG_CHANGED"),
            message: error.to_string(),
        }
    }
}

/// Что дало `applyPersonalizationIfInstalled`: оставленные правленые блоки и отпечаток файла
/// игрока (`None` — `undefined`: файл игрока не писался).
struct Applied {
    kept_edited: Vec<String>,
    config_sha256: Option<Option<String>>,
}

/// Ответ сохранения: `{status, settings, keptEdited[, configSha256]}`.
fn saved_answer(settings: Value, applied: Applied) -> Value {
    let mut out = Map::new();
    out.insert("status".into(), json!("ok"));
    out.insert("settings".into(), settings);
    out.insert("keptEdited".into(), json!(applied.kept_edited));
    if let Some(sha256) = applied.config_sha256 {
        out.insert("configSha256".into(), json!(sha256));
    }
    Value::Object(out)
}

impl DeliveryApi {
    fn launch_config(&self, game_id: &str) -> Option<&'static LaunchConfig> {
        manifests::launch_config_of(game_id)
    }

    /// `gamePacks(gameId)`: каталог паков HL1.
    fn game_packs(&self, game_id: &str) -> Option<Value> {
        self.launch_config(game_id)
            .filter(|config| config.packs.is_some())
            .map(|_| goldsrc_packs::catalog())
    }

    fn installed_install_path(&self, game_id: &str) -> Result<Option<PathBuf>, String> {
        let Some(service) = self.service() else {
            return Ok(None);
        };
        let installed = service
            .install_service()
            .read_installed(game_id)
            .map_err(|error| error.message)?;
        Ok(installed
            .as_ref()
            .and_then(|installed| installed.get("installPath"))
            .filter(|path| jv::truthy(Some(path)))
            .map(|path| PathBuf::from(jv::template(Some(path)))))
    }

    /// `gameBinds(gameId)`: бинды из cfg игры для вкладки «Клавиатура».
    pub(crate) fn game_binds_info(&self, game_id: &str) -> Map<String, Value> {
        let mut info = Map::new();
        info.insert("bindsGameId".into(), json!(game_id));
        let (Some(config), Some(engine), Some(_)) = (
            self.launch_config(game_id),
            bind_presets::engine_for(game_id),
            self.service(),
        ) else {
            return info;
        };
        let install_path = match self.installed_install_path(game_id) {
            Ok(Some(path)) => path,
            Ok(None) => return info,
            Err(message) => {
                self.logger()
                    .warn(&format!("[game-settings] бинды не прочитаны: {message}"));
                return info;
            }
        };
        self.prepare_player_config(game_id);
        let mut key_binds = game_binds_install::read_install_binds(
            self.cfg_search(),
            &install_path,
            config.game_directory,
            engine,
        );
        let record = (engine == "goldsrc")
            .then(|| game_binds_install::read_import_record(&install_path))
            .flatten();
        let has_record = record.is_some();
        if let Some(record) = record {
            key_binds.imported = record
                .files
                .iter()
                .map(|item| item.path.to_lowercase())
                .collect();
        }
        let mut value = Map::new();
        value.insert(
            "binds".into(),
            Value::Object(
                key_binds
                    .binds
                    .iter()
                    .map(|(key, bind)| {
                        (
                            key.to_owned(),
                            json!({"command": bind.command, "source": bind.source}),
                        )
                    })
                    .collect(),
            ),
        );
        for (name, map) in [("aliases", &key_binds.aliases), ("cvars", &key_binds.cvars)] {
            value.insert(
                name.into(),
                Value::Object(
                    map.iter()
                        .map(|(key, text)| (key.to_owned(), json!(text)))
                        .collect(),
                ),
            );
        }
        value.insert("files".into(), json!(key_binds.files));
        if has_record {
            value.insert("imported".into(), json!(key_binds.imported));
        }
        info.insert("keyBinds".into(), Value::Object(value));
        info
    }

    /// `gameLanguages(gameId)`: языки установленной сборки.
    fn game_languages(&self, game_id: &str) -> Vec<String> {
        let Some(install_path) = self.installed_path_of(game_id) else {
            return Vec::new();
        };
        let Some(config) = self
            .launch_config(game_id)
            .filter(|config| !config.game_directory.is_empty())
        else {
            return Vec::new();
        };
        game_language::available_languages(&install_path, config.game_directory)
    }

    /// `gameVulkan(gameId)`: переключатель Vulkan — `None`, если у игры его нет, иначе есть ли DXVK
    /// в установленной сборке.
    fn game_vulkan(&self, game_id: &str) -> Option<bool> {
        let config = self.launch_config(game_id);
        if !vulkan_backend::supported(config, PLATFORM) {
            return None;
        }
        vulkan_backend::availability(self.installed_path_of(game_id).as_deref(), config, PLATFORM)
    }

    /// `liveSplitServerPort(gameId)`: порт LiveSplit Server, если в Ghosting включено «Время из
    /// LiveSplit» (`gh_timer_mirror`); иначе `None`.
    pub fn live_split_server_port(&self, game_id: &str) -> Option<i64> {
        let store = self.settings_store()?;
        if game_id.is_empty() {
            return None;
        }
        let settings = store.get(game_id).to_value();
        let extra = settings.get("personalization")?.get("extra")?;
        if !js::truthy(extra)
            || extra.get("gh_timer_mirror") != Some(&json!(true))
            || extra.get("gh_timer") == Some(&json!(false))
        {
            return None;
        }
        let port = extra.get("gh_timer_mirror_port").map_or(0.0, jv::to_number);
        Some(if port.fract() == 0.0 && port > 0.0 && port <= 65535.0 {
            port as i64
        } else {
            16834
        })
    }

    /// `inspectPlayerPersonalization(gameId, stored)`: настройки внешнего вида с поправкой на то,
    /// что игрок написал в своём файле, и отпечаток этого файла (окно вернёт его при сохранении).
    /// Нет игры на диске или файл не читается — настройки как есть.
    pub(crate) fn inspect_player_personalization(
        &self,
        game_id: &str,
        stored: &Value,
    ) -> player_config::Inspected {
        let as_is = || player_config::Inspected {
            settings: stored.clone(),
            sha256: None,
        };
        let result = (|| -> Result<player_config::Inspected, String> {
            let (Some(install_path), Some(config)) = (
                self.installed_install_path(game_id)?,
                self.launch_config(game_id),
            ) else {
                return Ok(as_is());
            };
            let ctx = self.player_ctx(&install_path, game_id, config.game_directory);
            player_config::inspect_personalization(&ctx, stored).map_err(|error| error.to_string())
        })();
        result.unwrap_or_else(|message| {
            self.logger().warn(&format!(
                "[personalization] файл игрока не прочитан для {game_id}: {message}"
            ));
            as_is()
        })
    }

    /// `this.editedPrevious`: прежние бинды по играм, пока в файле остаются правленые блоки.
    fn with_edited_previous<T>(&self, f: impl FnOnce(&mut HashMap<String, Value>) -> T) -> T {
        f(&mut super::lock(&self.state).edited_previous)
    }

    pub(crate) fn edited_previous_remove(&self, game_id: &str) {
        self.with_edited_previous(|map| {
            map.remove(game_id);
        });
    }

    // ------------------------------------------------------------ настройки игры

    /// `game-settings-get`.
    pub fn game_settings_get(&self, id: &Value) -> Value {
        self.game_settings(Action::Get, id, &Value::Null, None, &Value::Null)
    }

    /// `game-settings-save` (`options`: `{removeEdited, configSha256, reset}`).
    pub fn game_settings_save(
        &self,
        id: &Value,
        settings: &Value,
        scope: Option<&Value>,
        options: &Value,
    ) -> Value {
        self.game_settings(Action::Save, id, settings, scope, options)
    }

    /// `game-settings-reset` (`options`: `{configSha256}`).
    pub fn game_settings_reset(&self, id: &Value, options: &Value) -> Value {
        self.game_settings(Action::Reset, id, &Value::Null, None, options)
    }

    fn game_settings(
        &self,
        action: Action,
        id: &Value,
        settings: &Value,
        scope: Option<&Value>,
        options: &Value,
    ) -> Value {
        let Some(store) = self.settings_store().cloned() else {
            return json!({"status": "error", "error": {"code": "SETTINGS_UNAVAILABLE"}});
        };
        let Some(game_id) = self.resolve_game_id(id) else {
            return json!({"status": "error", "error": {"code": "UNKNOWN_GAME"}});
        };
        let _guard = self.settings_guard();
        let expected = expected_of(options, "configSha256");
        let result: Result<Value, Failed> = (|| {
            self.prepare_player_config(&game_id);
            match action {
                Action::Get => Ok(self.settings_get(&store, &game_id)),
                Action::Save => {
                    if scope.is_some_and(|scope| scope.as_str() != Some("personalization")) {
                        return Ok(
                            json!({"status": "error", "error": {"code": "SETTINGS_SCOPE_INVALID"}}),
                        );
                    }
                    let previous = store.get(&game_id).to_value();
                    let next = if scope.is_some() {
                        // Панель могла загрузиться до смены биндов или параметров запуска:
                        // внешний вид — поверх актуальных настроек.
                        let mut next = previous.as_object().cloned().unwrap_or_default();
                        assign(
                            &mut next,
                            "personalization",
                            and_prop(settings, "personalization"),
                        );
                        assign(
                            &mut next,
                            "liveSplitOverlay",
                            and_prop(settings, "liveSplitOverlay"),
                        );
                        if game_id == "hl1-won" {
                            assign(&mut next, "packs", and_prop(settings, "packs"));
                            assign(&mut next, "bxt", and_prop(settings, "bxt"));
                        }
                        Value::Object(next)
                    } else {
                        // Оверлей LiveSplit меняет только панель персонализации.
                        let mut next = previous.as_object().cloned().unwrap_or_default();
                        if let Value::Object(fields) = settings {
                            for (key, value) in fields {
                                next.insert(key.clone(), value.clone());
                            }
                        }
                        assign(
                            &mut next,
                            "liveSplitOverlay",
                            previous.get("liveSplitOverlay").cloned(),
                        );
                        Value::Object(next)
                    };
                    let normalized = match launch_args::normalize_settings(&next) {
                        Ok(normalized) => normalized.to_value(),
                        Err(error) => {
                            return Ok(json!({"status": "error", "error": error.to_value()}));
                        }
                    };
                    // Файл игрока пишется до хранилища: не записался (CONFIG_CHANGED, права) —
                    // бинды в хранилище не попадают, и следующее сохранение возьмёт прежние.
                    let remove_edited = jv::truthy(jv::prop(options, "removeEdited"));
                    let remembered = self.with_edited_previous(|map| map.get(&game_id).cloned());
                    let previous_binds = remembered
                        .filter(|_| remove_edited)
                        .filter(js::truthy)
                        .unwrap_or_else(|| previous.get("binds").cloned().unwrap_or(Value::Null));
                    let applied = self.apply_personalization_if_installed(
                        &game_id,
                        &normalized,
                        &PersonalizationApply {
                            previous_binds: &previous_binds,
                            remove_edited,
                            previous_settings: &previous,
                            expected,
                            explicit: jv::truthy(jv::prop(options, "reset")),
                        },
                    )?;
                    let saved = match store.save(&game_id, &next) {
                        Ok(saved) => saved.to_value(),
                        Err(error) => {
                            return match store_error_value(&error) {
                                Some(error) => Ok(json!({"status": "error", "error": error})),
                                None => Err(error.to_string().into()),
                            };
                        }
                    };
                    self.remember_edited(&game_id, &applied, previous_binds);
                    let binds =
                        |value: &Value| stringify(value.get("binds").unwrap_or(&Value::Null), "");
                    if binds(&previous) != binds(&saved) {
                        self.achieve("own-binds");
                    }
                    let look = |value: &Value| {
                        stringify(
                            &json!([
                                value.get("personalization"),
                                value.get("packs"),
                                value.get("bxt")
                            ]),
                            "",
                        )
                    };
                    if look(&previous) != look(&saved) {
                        self.achieve("own-style");
                    }
                    Ok(saved_answer(saved, applied))
                }
                Action::Reset => {
                    // Как в save: файл игрока пишется до хранилища, ошибка записи видна в окне.
                    let previous = store.get(&game_id).to_value();
                    let mut defaults = launch_args::default_settings().to_value();
                    if game_id == GHOSTING {
                        let extra = previous
                            .get("personalization")
                            .and_then(|look| look.get("extra"))
                            .unwrap_or(&Value::Null);
                        defaults["personalization"]["extra"] =
                            ghosting_settings::reset_extras(extra).to_value();
                    }
                    let normalized = match launch_args::normalize_settings(&defaults) {
                        Ok(normalized) => normalized.to_value(),
                        Err(error) => {
                            return Ok(json!({"status": "error", "error": error.to_value()}));
                        }
                    };
                    let previous_binds = previous.get("binds").cloned().unwrap_or(Value::Null);
                    let applied = self.apply_personalization_if_installed(
                        &game_id,
                        &normalized,
                        &PersonalizationApply {
                            previous_binds: &previous_binds,
                            remove_edited: false,
                            previous_settings: &previous,
                            expected,
                            explicit: false,
                        },
                    )?;
                    let result = if game_id == GHOSTING {
                        store.save(&game_id, &defaults)
                    } else {
                        store.reset(&game_id)
                    };
                    let saved = match result {
                        Ok(saved) => saved.to_value(),
                        Err(error) => {
                            return match store_error_value(&error) {
                                Some(error) => Ok(json!({"status": "error", "error": error})),
                                None => Err(error.to_string().into()),
                            };
                        }
                    };
                    self.remember_edited(&game_id, &applied, previous_binds);
                    Ok(saved_answer(saved, applied))
                }
            }
        })();
        result.unwrap_or_else(|failed| {
            self.logger().warn(&format!(
                "[game-settings] действие {} не выполнено: {}",
                action.name(),
                failed.message
            ));
            let code = if failed.changed {
                "CONFIG_CHANGED"
            } else {
                "SETTINGS_IO_ERROR"
            };
            json!({"status": "error", "error": {"code": code}})
        })
    }

    /// Блоки, оставленные из-за правок: ответ «Удалить» придёт уже после записи в хранилище.
    fn remember_edited(&self, game_id: &str, applied: &Applied, previous_binds: Value) {
        self.with_edited_previous(|map| {
            if applied.kept_edited.is_empty() {
                map.remove(game_id);
            } else {
                map.insert(game_id.to_owned(), previous_binds);
            }
        });
    }

    fn settings_get(
        &self,
        store: &crate::game_settings_store::GameSettingsStore,
        game_id: &str,
    ) -> Value {
        let binds_info = self.game_binds_info(game_id);
        let from_store = store.get(game_id);
        let mut stored = from_store.to_value();
        if let Some(key_binds) = binds_info.get("keyBinds") {
            let file_binds = file_binds_of(key_binds);
            stored["binds"] =
                overrides_to_value(&game_binds::prune_overrides(&from_store.binds, &file_binds));
        }
        let inspected = self.inspect_player_personalization(game_id, &stored);
        let mut out = Map::new();
        out.insert("status".into(), json!("ok"));
        out.insert("settings".into(), inspected.settings);
        out.insert("languages".into(), json!(self.game_languages(game_id)));
        out.insert("vulkan".into(), json!(self.game_vulkan(game_id)));
        if let Some(sha256) = inspected.sha256 {
            out.insert("configSha256".into(), json!(sha256));
        }
        if let Some(packs) = self.game_packs(game_id) {
            out.insert("packs".into(), packs);
        }
        out.extend(binds_info);
        Value::Object(out)
    }

    /// `applyPersonalizationIfInstalled(gameId, settings, options)`: оверлей LiveSplit, затем
    /// бинды, скрипты и персонализация — в файл игрока сразу, одной записью, в том числе при
    /// запущенной игре. Ошибка записи файла игрока — ошибка вызывающему, прочее — в журнал.
    fn apply_personalization_if_installed(
        &self,
        game_id: &str,
        settings: &Value,
        options: &PersonalizationApply,
    ) -> Result<Applied, Failed> {
        // Оверлей LiveSplit в запущенной игре перечитывает overlay.ini сам.
        if js::truthy(settings)
            && let Some(overlay) = &self.ports().overlay
            && let Err(message) = overlay.write_config(game_id, settings)
        {
            self.logger().warn(&format!(
                "[livesplit-overlay] настройки не записаны для {game_id}: {message}"
            ));
        }
        let mut applied = Applied {
            kept_edited: Vec::new(),
            config_sha256: None,
        };
        if !js::truthy(settings) {
            return Ok(applied);
        }
        let install_path = match self.installed_install_path(game_id) {
            Ok(Some(path)) => path,
            Ok(None) => return Ok(applied),
            Err(message) => {
                self.logger().warn(&format!(
                    "[personalization] не применена для {game_id}: {message}"
                ));
                return Ok(applied);
            }
        };
        let Some(config) = self
            .launch_config(game_id)
            .filter(|_| bind_presets::engine_for(game_id).is_some())
        else {
            return Ok(applied);
        };
        let ctx = self.player_ctx(&install_path, game_id, config.game_directory);
        let result = game_binds_install::apply_player_binds(
            &ctx,
            settings.get("binds").unwrap_or(&Value::Null),
            &ApplyOptions {
                previous: Some(options.previous_binds),
                remove_edited: options.remove_edited,
                settings: Some(settings),
                previous_settings: Some(options.previous_settings),
                expected: options.expected,
                explicit_personalization: options.explicit,
            },
        )?;
        if let PlayerBinds::Applied {
            kept_edited,
            sha256,
            ..
        } = result
        {
            applied.kept_edited = kept_edited;
            applied.config_sha256 = Some(sha256);
        }
        Ok(applied)
    }

    // ------------------------------------------------------------ персонализация Ghosting

    /// `game-personalization-pick-asset`: шрифт или спрей из файла игрока — в папку мода; модель
    /// призрака — значение `gh_model` файла внутри мода.
    pub async fn game_personalization_pick_asset(&self, id: &Value, kind: &Value) -> Value {
        let game_id = self.resolve_game_id(id);
        let kind = kind
            .as_str()
            .filter(|kind| matches!(*kind, "font" | "spray" | "model"));
        let (Some(GHOSTING), Some(kind)) = (game_id.as_deref(), kind) else {
            return json!({"status": "error", "code": "INVALID_ASSET"});
        };
        let Some(install_path) = self.installed_path_of(GHOSTING) else {
            return json!({"status": "error", "code": "NOT_INSTALLED"});
        };
        if kind == "model" {
            return self.pick_ghost_model(&install_path).await;
        }
        let filters = if kind == "font" {
            vec![FileFilter::new("TrueType / OpenType", &["ttf", "otf"])]
        } else {
            vec![FileFilter::new("PNG / BMP / TGA", &["png", "bmp", "tga"])]
        };
        let picked = self
            .ports()
            .dialogs
            .show_open_dialog(OpenDialog {
                properties: vec!["openFile"],
                filters,
                ..OpenDialog::default()
            })
            .await;
        let Some(file) = picked.into_iter().next() else {
            return json!({"status": "canceled"});
        };
        if self.installed_path_of(GHOSTING).as_ref() != Some(&install_path) {
            return json!({"status": "error", "code": "NOT_INSTALLED"});
        }
        let install = install_path.to_string_lossy().into_owned();
        let source = file.to_string_lossy().into_owned();
        let kind = kind.to_owned();
        let imported = tokio::task::spawn_blocking(move || {
            ghosting_assets::import_asset(&install, &source, &kind)
        })
        .await;
        match imported {
            Ok(Ok(asset)) => json!({
                "status": "ok",
                "value": asset.value,
                "name": asset.name,
                "bytes": asset.bytes,
                "sha256": asset.sha256,
            }),
            Ok(Err(error)) => {
                self.logger().warn(&format!(
                    "[personalization] asset import failed: {}",
                    error.code
                ));
                error_code(error.code)
            }
            Err(_) => {
                self.logger()
                    .warn("[personalization] asset import failed: ASSET_IMPORT_FAILED");
                error_code("ASSET_IMPORT_FAILED")
            }
        }
    }

    /// Модель призрака не копируется: игра видит только модели внутри мода, поэтому выбирается
    /// файл там (`ghostingmod/models`, `ghostingmod/custom/<пак>/models`).
    async fn pick_ghost_model(&self, install_path: &std::path::Path) -> Value {
        // Своих моделей в ghostingmod/models обычно нет (папки нет) — тогда окно открывается в custom.
        let r#mod = install_path.join("ghostingmod");
        let default_path = [r#mod.join("models"), r#mod.join("custom")]
            .into_iter()
            .find(|dir| dir.exists())
            .unwrap_or(r#mod);
        let picked = self
            .ports()
            .dialogs
            .show_open_dialog(OpenDialog {
                default_path: Some(default_path.to_string_lossy().into_owned()),
                properties: vec!["openFile"],
                filters: vec![FileFilter::new("Source model (*.mdl)", &["mdl"])],
                ..OpenDialog::default()
            })
            .await;
        let Some(file) = picked.into_iter().next() else {
            return json!({"status": "canceled"});
        };
        match ghost_model::value_of_picked(install_path, &file) {
            Ok(value) => json!({"status": "ok", "value": value}),
            Err(error) => {
                self.logger().warn(&format!(
                    "[personalization] asset import failed: {}",
                    error.code
                ));
                error_code(&error.code)
            }
        }
    }

    /// `game-personalization-preview-asset`: файл шрифта или спрея для предпросмотра; у модели
    /// призрака — геометрия `{mesh: {triangles, positions}}`.
    pub async fn game_personalization_preview_asset(&self, id: &Value, options: &Value) -> Value {
        let game_id = self.resolve_game_id(id);
        let kind = jv::prop(options, "kind")
            .and_then(Value::as_str)
            .filter(|kind| matches!(*kind, "font" | "spray" | "model"));
        let (Some(GHOSTING), Some(kind)) =
            (game_id.as_deref(), kind.filter(|_| js::truthy(options)))
        else {
            return json!({"status": "error", "code": "INVALID_ASSET"});
        };
        let Some(install_path) = self.installed_path_of(GHOSTING) else {
            return json!({"status": "error", "code": "NOT_INSTALLED"});
        };
        if kind == "model" {
            // Ищется там же, где игра (ghost_model).
            let value = jv::prop(options, "value")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let read = tokio::task::spawn_blocking(move || {
                ghost_model::read_ghost_mesh(&install_path, value.as_deref())
            })
            .await;
            return match read {
                Ok(Ok(mesh)) => json!({"status": "ok", "mesh": {
                    "triangles": mesh.triangles,
                    "positions": mesh.positions.into_iter().map(js_number).collect::<Vec<_>>(),
                }}),
                Ok(Err(error)) => error_code(&error.code),
                Err(_) => error_code("ASSET_READ_FAILED"),
            };
        }
        let Some(value) = jv::prop(options, "value").and_then(Value::as_str) else {
            return error_code("ASSET_INVALID_PATH");
        };
        let prefix = self
            .ports()
            .proton_prefix
            .as_ref()
            .and_then(|prefix_for| prefix_for(GHOSTING));
        let install = install_path.to_string_lossy().into_owned();
        let (kind, value) = (kind.to_owned(), value.to_owned());
        let read = tokio::task::spawn_blocking(move || {
            ghosting_assets::read_preview_asset(&install, &kind, &value, prefix.as_deref())
        })
        .await;
        match read {
            Ok(Ok(asset)) => json!({"status": "ok", "dataUrl": asset.data_url}),
            Ok(Err(error)) => error_code(error.code),
            Err(_) => error_code("ASSET_READ_FAILED"),
        }
    }

    /// `game-personalization-code-export`: код внешнего вида (ничего не записывает). Окно
    /// присылает черновик; пока он не отличается от сохранённого, код берёт то, что сейчас в
    /// файле игрока (его могли поправить руками после открытия окна). Моды — с диска.
    pub fn game_personalization_code_export(&self, id: &Value, look: &Value) -> Value {
        let code = self.personalization_game(id).and_then(|game_id| {
            let mut look = look.clone();
            if let Some(store) = self.settings_store() {
                let saved = store.get(&game_id).to_value();
                let draft_of = |key: &str| {
                    look.as_object()
                        .and_then(|draft| draft.get(key))
                        .or_else(|| saved.get(key))
                        .cloned()
                        .unwrap_or(Value::Null)
                };
                let look_of = |of: &dyn Fn(&str) -> Value| {
                    stringify(&json!([of("personalization"), of("packs"), of("bxt")]), "")
                };
                let saved_of = |key: &str| saved.get(key).cloned().unwrap_or(Value::Null);
                if look_of(&draft_of) == look_of(&saved_of) {
                    look = self
                        .inspect_player_personalization(&game_id, &saved)
                        .settings;
                }
            }
            // Моды с вкладки модов включаются сразу, без черновика: их состояние — то, что на диске.
            let install = custom_packs::has_packs(&game_id)
                .then(|| self.installed_path_of(&game_id))
                .flatten();
            if let Some(install) = install {
                match custom_packs::list(Some(&install), &game_id, false, self.logger()) {
                    Ok(packs) => {
                        let mods: Map<String, Value> = packs
                            .iter()
                            .map(|pack| (jv::template(pack.get("id")), pack["enabled"].clone()))
                            .collect();
                        let mut with_mods = look.as_object().cloned().unwrap_or_default();
                        with_mods.insert("mods".into(), Value::Object(mods));
                        look = Value::Object(with_mods);
                    }
                    Err(error) => self.logger().warn(&format!(
                        "[personalization-code] моды не вошли в код: {} {error}",
                        match &error {
                            Failure::Delivery(error) => error.code.as_str().to_owned(),
                            Failure::Raw(error) => error.code.clone().unwrap_or_default(),
                        }
                    )),
                }
            }
            personalization_code::encode(&game_id, &look)
        });
        match code {
            Ok(code) => json!({"status": "ok", "code": code}),
            Err(error) => json!({"status": "error", "error": {"code": error.code()}}),
        }
    }

    /// `game-personalization-code-import`: снимок внешнего вида из кода (ничего не записывает).
    /// Моды отдельно от вида: вид идёт в черновик до «Сохранить», моды окно переключает сразу.
    pub fn game_personalization_code_import(&self, id: &Value, code: &Value) -> Value {
        let look = self
            .personalization_game(id)
            .and_then(|game_id| personalization_code::decode(&game_id, code));
        match look {
            Ok(mut look) => {
                let mods = look.mods.take().map_or(Value::Null, Value::Object);
                json!({"status": "ok", "look": look.to_value(), "mods": mods})
            }
            Err(error) => json!({"status": "error", "error": {"code": error.code()}}),
        }
    }

    fn personalization_game(&self, id: &Value) -> Result<String, CodeError> {
        self.resolve_game_id(id).ok_or(CodeError::GameUnsupported)
    }

    /// `ghosting-screen-get`: разрешение Ghosting для предпросмотра HUD.
    pub fn ghosting_screen_get(&self, id: &Value) -> Value {
        if self.resolve_game_id(id).as_deref() != Some(GHOSTING) {
            return Value::Null;
        }
        let settings = self
            .settings_store()
            .map(|store| store.get(GHOSTING).to_value());
        let prefix = self
            .ports()
            .proton_prefix
            .as_ref()
            .and_then(|prefix_for| prefix_for(GHOSTING));
        source_screen::screen_of(
            settings.as_ref(),
            "ghostingmod",
            PLATFORM,
            prefix.as_deref(),
            self.ports().registry.as_ref(),
        )
        .map_or(Value::Null, |screen| {
            json!({"width": screen.width, "height": screen.height, "source": screen.source.as_str()})
        })
    }
}

/// Бинды файлов из `keyBinds` (для `pruneOverrides`).
fn file_binds_of(key_binds: &Value) -> hlsr_core::js::object::JsObject<game_binds::Bind> {
    let mut binds = hlsr_core::js::object::JsObject::new();
    if let Some(Value::Object(map)) = key_binds.get("binds") {
        for (key, bind) in map {
            binds.insert(
                key.clone(),
                game_binds::Bind {
                    command: jv::template(bind.get("command")),
                    source: jv::template(bind.get("source")),
                },
            );
        }
    }
    binds
}

/// Отпечаток из `options` окна: `None` — `undefined`, `Some(None)` — `null`. Не строка —
/// заведомо чужой отпечаток (в 0.4.9 такое сравнение тоже не совпадёт).
fn expected_of<'a>(options: &'a Value, key: &str) -> Option<Option<&'a str>> {
    if !js::truthy(options) {
        return None;
    }
    match jv::prop(options, key)? {
        Value::Null => Some(None),
        Value::String(text) => Some(Some(text)),
        _ => Some(Some("-")),
    }
}

/// Что передаёт сохранение в `applyPersonalizationIfInstalled`.
struct PersonalizationApply<'a> {
    previous_binds: &'a Value,
    remove_edited: bool,
    previous_settings: &'a Value,
    expected: Option<Option<&'a str>>,
    /// «Сбросить» в «Персонализации»: умолчания явными строками.
    explicit: bool,
}

#[derive(Debug, Clone, Copy)]
enum Action {
    Get,
    Save,
    Reset,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Action::Get => "get",
            Action::Save => "save",
            Action::Reset => "reset",
        }
    }
}
