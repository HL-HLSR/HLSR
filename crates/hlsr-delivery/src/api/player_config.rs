//! Единый файл игрока: каналы `player-config-*` 0.4.9 (`pickBindsConfig`, `importPlayerConfig`,
//! `undoPlayerConfigImport`, `playerConfigSetupState/Done`, `exportPlayerConfig`,
//! `playerConfigCheck`, `preparePlayerConfig`, `dropImportedOverrides` из `index.js`).

use super::{DeliveryApi, PLATFORM, error_code};
use crate::bind_presets;
use crate::cfg_import::{self, ImportFile, ImportFiles, Root};
use crate::default_configs;
use crate::game_binds;
use crate::game_binds_install::{
    self, BackupOptions, CheckFiles, ExportPlan, Flattened, ImportPlayer, UndoImport,
};
use crate::hl1_config_check::{self, Category};
use crate::js_value as jv;
use crate::manifests;
use crate::player_config::{self, Ctx, PrepareOptions};
use crate::ports::{FileFilter, OpenDialog, SaveDialog};
use hlsr_core::js;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Самый большой cfg, который читает выбор файла для импорта.
const MAX_PICKED_CFG_BYTES: u64 = 1024 * 1024;

/// Подготовленный файл игрока установленной игры (`preparePlayerConfig`).
pub(crate) struct PreparedPlayer {
    pub install_path: PathBuf,
    pub file: PathBuf,
}

/// `request.files` окна (`{ключ: {name, text, target}}`) для `flattenImport`; `text` уже
/// проверен.
fn import_files(files: &Map<String, Value>) -> ImportFiles {
    let text_of = |file: &Value, key: &str| {
        jv::prop(file, key)
            .filter(|value| js::truthy(value))
            .map(|value| jv::template(Some(value)))
    };
    files
        .iter()
        .map(|(key, file)| {
            let file = ImportFile {
                name: text_of(file, "name").unwrap_or_default(),
                text: jv::template(jv::prop(file, "text")),
                target: text_of(file, "target"),
            };
            (key.clone(), file)
        })
        .collect()
}

impl DeliveryApi {
    pub(crate) fn player_ctx<'a>(
        &'a self,
        install: &'a Path,
        game_id: &'a str,
        game_directory: &'a str,
    ) -> Ctx<'a> {
        Ctx {
            search: self.cfg_search(),
            install,
            game_id,
            game_directory,
            platform: PLATFORM,
        }
    }

    /// Настройки игры из хранилища (`store.get(gameId)`); хранилища нет — `null`.
    fn player_settings(&self, game_id: &str) -> Value {
        self.settings_store()
            .map_or(Value::Null, |store| store.get(game_id).to_value())
    }

    /// `preparePlayerConfig(gameId)`: файл игрока готов к чтению и записи — переход с 0.4.8 или
    /// создание отсутствующего. Зовётся первым там, где читают или пишут конфиги.
    pub(crate) fn prepare_player_config(&self, game_id: &str) -> Option<PreparedPlayer> {
        let install_path = self.installed_path_of(game_id)?;
        let launch_config = manifests::launch_config_of(game_id)?;
        let settings = self.player_settings(game_id);
        let options = PrepareOptions {
            settings: Some(&settings),
            launcher_version: Some(&self.options.launcher_version),
            now: Some(self.now_ms()),
        };
        match default_configs::prepare_config(
            self.cfg_search(),
            game_id,
            &install_path,
            Some(launch_config),
            PLATFORM,
            &options,
        ) {
            Ok(Some(prepared)) => Some(PreparedPlayer {
                install_path,
                file: prepared.file,
            }),
            Ok(None) => None,
            Err(error) => {
                self.logger().warn(&format!(
                    "[player-config] {game_id}: файл игрока не подготовлен: {error}"
                ));
                None
            }
        }
    }

    /// `playerConfigBackupsFile()`: какую копию импорта записал сам лаунчер — в папке лаунчера,
    /// не игры.
    fn player_config_backups_file(&self) -> Option<PathBuf> {
        self.user_data_path()
            .map(|dir| dir.join("player-config-backups.json"))
    }

    fn backup_options<'a>(&self, backups_file: Option<&'a Path>) -> BackupOptions<'a> {
        BackupOptions {
            backups_file,
            now: Some(self.now_ms()),
        }
    }

    /// `player-config-setup-state`.
    pub fn player_config_setup_state(&self, id: &Value) -> Value {
        let game_id = self.resolve_game_id(id);
        let launch_config = game_id.as_deref().and_then(manifests::launch_config_of);
        let install_path = game_id.as_deref().and_then(|id| self.installed_path_of(id));
        let (Some(game_id), Some(config), Some(install_path)) =
            (game_id, launch_config, install_path)
        else {
            return json!({"needed": false});
        };
        if config.game_directory.is_empty() {
            return json!({"needed": false});
        }
        let ctx = self.player_ctx(&install_path, &game_id, config.game_directory);
        match player_config::setup_state(&ctx) {
            Ok(needed) => json!({"needed": needed}),
            Err(error) => {
                self.logger().warn(&format!(
                    "[player-config] {game_id}: состояние опроса не прочитано: {error}"
                ));
                json!({"needed": false})
            }
        }
    }

    /// `player-config-setup-done`.
    pub fn player_config_setup_done(&self, id: &Value, choice: &Value) -> Value {
        let game_id = self.resolve_game_id(id);
        let launch_config = game_id.as_deref().and_then(manifests::launch_config_of);
        let install_path = game_id.as_deref().and_then(|id| self.installed_path_of(id));
        let (Some(game_id), Some(config), Some(install_path)) =
            (game_id, launch_config, install_path)
        else {
            return error_code("NOT_INSTALLED");
        };
        if config.game_directory.is_empty() {
            return error_code("NOT_INSTALLED");
        }
        // Не строка — заведомо чужой ответ (`SETUP_CHOICES.includes` в 0.4.9 его не найдёт).
        let choice = match choice {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        let settings = self.player_settings(&game_id);
        let options = PrepareOptions {
            settings: Some(&settings),
            launcher_version: Some(&self.options.launcher_version),
            now: Some(self.now_ms()),
        };
        let ctx = self.player_ctx(&install_path, &game_id, config.game_directory);
        match player_config::setup_done(&ctx, &choice, &options) {
            Ok(()) => {
                self.logger().info(&format!(
                    "[player-config] {game_id}: ответ на опрос — {choice}"
                ));
                json!({"status": "ok"})
            }
            Err(error) => {
                self.logger().warn(&format!(
                    "[player-config] {game_id}: ответ на опрос не записан: {}",
                    error.code_or_message()
                ));
                error_code(if error.is("CONFIG_SETUP_CHOICE") {
                    "BAD_CHOICE"
                } else {
                    "PATH_NOT_WRITABLE"
                })
            }
        }
    }

    /// `player-config-import-pick`: выбор cfg для импорта (клавиатура и вкладка «Конфиги»).
    /// Файл только читается. `options.want` — файл цепочки exec (`{name, from}`): окно подписано его
    /// именем и открывается сразу на нём в папке предыдущего выбранного файла (`options.dir`).
    pub async fn player_config_import_pick(&self, id: &Value, options: &Value) -> Value {
        let Some(game_id) = self.resolve_game_id(id) else {
            return error_code("UNKNOWN_GAME");
        };
        let want = jv::prop(options, "want").filter(|want| {
            js::truthy(want) && jv::prop(want, "name").is_some_and(Value::is_string)
        });
        let path = hlsr_core::js::path::NodePath::native();
        let dir = jv::prop(options, "dir")
            .and_then(Value::as_str)
            .filter(|dir| path.is_absolute(dir));
        let installed = self.installed_path_of(&game_id);
        let base = dir
            .map(str::to_owned)
            .filter(|dir| !dir.is_empty())
            .or_else(|| installed.map(|path| path.to_string_lossy().into_owned()));
        let multiple = want.is_none() && jv::truthy(jv::prop(options, "multiple"));
        let title = want.map(|want| {
            let from = jv::prop(want, "from")
                .filter(|from| js::truthy(from))
                .map_or_else(String::new, |from| jv::template(Some(from)));
            let name = jv::prop(want, "name").and_then(Value::as_str).unwrap_or("");
            format!(
                "{} → exec {}",
                js::slice_utf16(&from, 120),
                js::slice_utf16(name, 120)
            )
        });
        let default_path = match (want, &base) {
            (Some(want), Some(base)) => {
                let name = jv::prop(want, "name").and_then(Value::as_str).unwrap_or("");
                Some(path.join(&[base, &path.basename(name)]))
            }
            _ => base.clone(),
        };
        let picked = self
            .ports()
            .dialogs
            .show_open_dialog(OpenDialog {
                title,
                default_path,
                properties: if multiple {
                    vec!["openFile", "multiSelections"]
                } else {
                    vec!["openFile"]
                },
                filters: vec![
                    FileFilter::new("Config (*.cfg)", &["cfg", "rc"]),
                    FileFilter::new("*", &["*"]),
                ],
            })
            .await;
        if picked.is_empty() {
            return json!({"status": "canceled"});
        }
        if picked.len() > cfg_import::MAX_FILES {
            return error_code("BINDS_FILE_TOO_LARGE");
        }
        let mut files = Vec::new();
        for file in &picked {
            let meta = match tokio::fs::metadata(file).await {
                Ok(meta) => meta,
                Err(error) => return self.binds_unreadable(&error, file),
            };
            if !meta.is_file() || meta.len() > MAX_PICKED_CFG_BYTES {
                return error_code("BINDS_FILE_TOO_LARGE");
            }
            let bytes = match tokio::fs::read(file).await {
                Ok(bytes) => bytes,
                Err(error) => return self.binds_unreadable(&error, file),
            };
            // Движки читают cfg побайтно: latin1 не теряет байты.
            let text = game_binds::cfg_text(&js::latin1_string(&bytes));
            let text_path = file.to_string_lossy();
            files.push(json!({
                "name": path.basename(&text_path),
                "dir": path.dirname(&text_path),
                "text": text,
            }));
        }
        let first = files.first().cloned().unwrap_or(Value::Null);
        let mut out = Map::new();
        out.insert("status".into(), json!("ok"));
        out.insert("files".into(), Value::Array(files));
        if let Value::Object(fields) = first {
            out.extend(fields);
        }
        Value::Object(out)
    }

    fn binds_unreadable(&self, error: &std::io::Error, file: &Path) -> Value {
        let raw = crate::errors::RawError::fs(error, "open", file);
        self.logger().warn(&format!(
            "[binds] импорт cfg не прочитан: {}",
            raw.code.unwrap_or(raw.message)
        ));
        error_code("BINDS_FILE_UNREADABLE")
    }

    /// `player-config-import-apply`: импорт готового конфига одним файлом — цепочка exec
    /// сливается в файл игрока, файлы, которые зовут alias и bind, ложатся в папку мода, прежнее
    /// уходит в `.hlsr/backup/<время>/`. `payload` — `{root, files, missing}` или текст. Отпечаток
    /// через IPC не передаётся (у обработчика 0.4.9 только `payload`).
    pub fn player_config_import_apply(&self, id: &Value, payload: Option<&Value>) -> Value {
        let game_id = self.resolve_game_id(id);
        let config = game_id.as_deref().and_then(manifests::launch_config_of);
        let engine = game_id.as_deref().and_then(bind_presets::engine_for);
        let (Some(game_id), Some(config), Some(engine)) = (game_id, config, engine) else {
            return error_code("UNKNOWN_GAME");
        };
        let request = match payload {
            Some(Value::String(text)) => json!({"root": {"name": "player.cfg", "text": text}}),
            Some(other) => other.clone(),
            None => Value::Null,
        };
        let files = jv::prop(&request, "files")
            .filter(|files| js::truthy(files) && files.is_object())
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let root = jv::prop(&request, "root").filter(|root| js::truthy(root));
        let missing = jv::prop(&request, "missing");
        let valid = js::truthy(&request)
            && root.is_some_and(|root| {
                jv::prop(root, "text").is_some_and(Value::is_string)
                    && jv::prop(root, "name").is_some_and(Value::is_string)
            })
            && files.len() <= cfg_import::MAX_FILES
            && files.values().all(|file| {
                js::truthy(file) && jv::prop(file, "text").is_some_and(Value::is_string)
            })
            && missing.is_none_or(Value::is_array);
        if !valid {
            return error_code("BINDS_FILE_UNREADABLE");
        }
        let Some(install_path) = self.installed_path_of(&game_id) else {
            return error_code("NOT_INSTALLED");
        };
        let missing: Vec<String> = missing
            .and_then(Value::as_array)
            .map(|items| items.iter().map(|item| jv::template(Some(item))).collect())
            .unwrap_or_default();
        let field = |key: &str| root.and_then(|root| jv::prop(root, key));
        let root = Root {
            name: jv::template(field("name")),
            text: jv::template(field("text")),
            virtual_root: field("virtual").is_some_and(js::truthy),
        };
        let flat = cfg_import::flatten_import(engine, &root, &import_files(&files), &missing);
        if flat.bytes > game_binds::MAX_IMPORT_BYTES {
            return error_code("BINDS_FILE_TOO_LARGE");
        }
        // Как game-settings-*: запись файла игрока и снятие переназначений не перемежаются с
        // сохранением настроек (в 0.4.9 их разводил единственный поток JS).
        let _guard = self.settings_guard();
        // Запущенная игра не мешает: при выходе она переписывает только config.cfg.
        self.prepare_player_config(&game_id);
        let backups_file = self.player_config_backups_file();
        let ctx = self.player_ctx(&install_path, &game_id, config.game_directory);
        let report = flat.report_value();
        let built = Flattened {
            text: flat.text,
            runtime_files: flat
                .runtime_files
                .into_iter()
                .map(|item| (item.file, item.text))
                .collect(),
        };
        let result = game_binds_install::import_player_config(
            &ctx,
            &built,
            None,
            &self.backup_options(backups_file.as_deref()),
        );
        match result {
            Ok(ImportPlayer::Changed) => error_code("CONFIG_CHANGED"),
            Ok(ImportPlayer::Ok {
                sha256,
                backup,
                files,
                ..
            }) => {
                self.drop_imported_overrides(&game_id);
                let names = if files.is_empty() {
                    "-".to_owned()
                } else {
                    files.join(", ")
                };
                self.logger().info(&format!(
                    "[player-config] {game_id}: конфиг импортирован, копия {backup}, файлы {names}"
                ));
                let mut out = Map::new();
                out.insert("status".into(), json!("ok"));
                out.insert("summary".into(), flat.summary.to_value());
                out.insert("backup".into(), json!(backup));
                out.insert("sha256".into(), json!(sha256));
                out.insert("report".into(), report);
                out.extend(self.game_binds_info(&game_id));
                Value::Object(out)
            }
            Ok(_) => error_code("BINDS_FILE_UNREADABLE"),
            Err(error) if error.is("CONFIG_CHANGED") => error_code("CONFIG_CHANGED"),
            Err(error) => {
                self.logger().warn(&format!(
                    "[player-config] импорт не записан: {}",
                    error.code_or_message()
                ));
                error_code("PATH_NOT_WRITABLE")
            }
        }
    }

    /// `dropImportedOverrides(gameId)`: импорт заменяет файл целиком, и главный теперь он — все
    /// переназначения и назначения скриптов игры снимаются из хранилища. «Вернуть прежний» их не
    /// возвращает: восстановленный файл уже содержит прежние бинды лаунчера. Зовущий держит
    /// `settings_guard`.
    fn drop_imported_overrides(&self, game_id: &str) {
        // Файл заменён целиком: «прежние бинды» правленых вручную блоков к нему уже не относятся.
        self.edited_previous_remove(game_id);
        let Some(store) = self.settings_store() else {
            return;
        };
        let saved = store.get(game_id).to_value();
        let has_binds = saved
            .get("binds")
            .and_then(Value::as_object)
            .is_some_and(|binds| !binds.is_empty());
        if !has_binds {
            return;
        }
        let mut next = saved.as_object().cloned().unwrap_or_default();
        next.insert("binds".into(), json!({}));
        if let Err(error) = store.save(game_id, &Value::Object(next)) {
            self.logger().warn(&format!(
                "[player-config] {game_id}: переназначения после импорта не сняты: {error}"
            ));
        }
    }

    /// `player-config-import-undo` («Вернуть прежний»): файл игрока и файлы скриптов из последней
    /// копии импорта, только своей (A-7).
    pub fn player_config_import_undo(&self, id: &Value) -> Value {
        let game_id = self.resolve_game_id(id);
        let config = game_id.as_deref().and_then(manifests::launch_config_of);
        let install_path = game_id.as_deref().and_then(|id| self.installed_path_of(id));
        let (Some(game_id), Some(config), Some(install_path)) = (game_id, config, install_path)
        else {
            return error_code("NOT_INSTALLED");
        };
        self.prepare_player_config(&game_id);
        let backups_file = self.player_config_backups_file();
        let ctx = self.player_ctx(&install_path, &game_id, config.game_directory);
        match game_binds_install::undo_import(&ctx, &self.backup_options(backups_file.as_deref())) {
            Ok(UndoImport::Nothing) => error_code("NO_BACKUP"),
            Ok(UndoImport::Ok { sha256, backup }) => {
                self.logger().info(&format!(
                    "[player-config] {game_id}: возвращено из {backup}"
                ));
                let mut out = Map::new();
                out.insert("status".into(), json!("ok"));
                out.insert("sha256".into(), json!(sha256));
                out.insert("backup".into(), json!(backup));
                out.extend(self.game_binds_info(&game_id));
                Value::Object(out)
            }
            Err(error) => {
                self.logger().warn(&format!(
                    "[player-config] возврат не удался: {}",
                    error.code_or_message()
                ));
                error_code("PATH_NOT_WRITABLE")
            }
        }
    }

    /// `player-config-check`: проверка конфига HL1 по правилам speedrun.com, только чтение.
    /// `category` — `scriptless` (по умолчанию) или `scripted`; другое — `BAD_REQUEST`.
    pub fn player_config_check(&self, id: &Value, category: Option<&Value>) -> Value {
        // `undefined` берёт значение по умолчанию; `null` и прочее — уже нет.
        let category = match category {
            None => "scriptless",
            Some(Value::String(text)) if matches!(text.as_str(), "scriptless" | "scripted") => {
                text.as_str()
            }
            Some(_) => return error_code("BAD_REQUEST"),
        };
        let game_id = self.resolve_game_id(id);
        let config = game_id.as_deref().and_then(manifests::launch_config_of);
        let (Some(game_id), Some(config)) = (game_id.filter(|id| id == "hl1-won"), config) else {
            return json!({"status": "unsupported"});
        };
        let Some(install_path) = self.installed_path_of(&game_id) else {
            return error_code("NOT_INSTALLED");
        };
        let ctx = self.player_ctx(&install_path, &game_id, config.game_directory);
        let failed = |message: String| {
            self.logger().warn(&format!(
                "[player-config] {game_id}: проверка по правилам не выполнена: {message}"
            ));
            error_code("CHECK_FAILED")
        };
        match game_binds_install::check_files(&ctx) {
            Ok(CheckFiles::Unsupported) => json!({"status": "unsupported"}),
            Ok(CheckFiles::Nothing) => json!({"status": "nothing"}),
            Ok(CheckFiles::Ok { root, texts, paths }) => {
                let result =
                    hl1_config_check::check_config(&texts, &root, Category::parse(category))
                        .to_value();
                let findings: Vec<Value> = result
                    .get("findings")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| {
                                let file = jv::template(item.get("file"));
                                let mut out = Map::new();
                                out.insert(
                                    "path".into(),
                                    paths.get(&file).map_or(Value::Null, |path| json!(path)),
                                );
                                if let Value::Object(fields) = item {
                                    out.extend(fields.clone());
                                }
                                Value::Object(out)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                json!({
                    "status": "ok",
                    "file": paths.get(&root),
                    "stats": result.get("stats").cloned().unwrap_or(Value::Null),
                    "findings": findings,
                })
            }
            Err(error) => failed(error.code_or_message()),
        }
    }

    /// `player-config-export`: файл игрока (с файлами скриптов, если он их зовёт) туда, куда
    /// укажет игрок.
    pub async fn player_config_export(&self, id: &Value) -> Value {
        let game_id = self.resolve_game_id(id);
        let config = game_id.as_deref().and_then(manifests::launch_config_of);
        let install_path = game_id.as_deref().and_then(|id| self.installed_path_of(id));
        let (Some(game_id), Some(config), Some(install_path)) = (game_id, config, install_path)
        else {
            return error_code("NOT_INSTALLED");
        };
        self.prepare_player_config(&game_id);
        let ctx = self.player_ctx(&install_path, &game_id, config.game_directory);
        let failed = |message: String| {
            self.logger()
                .warn(&format!("[player-config] экспорт не удался: {message}"));
            error_code("PATH_NOT_WRITABLE")
        };
        let plan = match game_binds_install::export_plan(&ctx) {
            Ok(plan) => plan,
            Err(error) => return failed(error.code_or_message()),
        };
        let written = match plan {
            ExportPlan::Unsupported | ExportPlan::Nothing => return error_code("NOT_INSTALLED"),
            ExportPlan::Single { file, name } => {
                let picked = self
                    .ports()
                    .dialogs
                    .show_save_dialog(SaveDialog {
                        default_path: Some(name),
                        filters: vec![FileFilter::new("Config (*.cfg)", &["cfg"])],
                    })
                    .await;
                let Some(target) = picked else {
                    return json!({"status": "canceled"});
                };
                game_binds_install::export_single(&file, &target).map(|()| (target, 1))
            }
            ExportPlan::Chain { file, chain } => {
                let picked = self
                    .ports()
                    .dialogs
                    .show_open_dialog(OpenDialog {
                        properties: vec!["openDirectory", "createDirectory"],
                        ..OpenDialog::default()
                    })
                    .await;
                let Some(chosen) = picked.into_iter().next() else {
                    return json!({"status": "canceled"});
                };
                game_binds_install::export_chain(&game_id, &file, &chain, &chosen, self.now_ms())
            }
        };
        match written {
            Ok((path, count)) => {
                self.logger().info(&format!(
                    "[player-config] {game_id}: экспорт в {}, файлов {count}",
                    path.to_string_lossy()
                ));
                json!({"status": "ok", "path": path.to_string_lossy()})
            }
            Err(failure) => failed(match failure {
                crate::errors::Failure::Raw(raw) => raw.code.unwrap_or(raw.message),
                crate::errors::Failure::Delivery(error) => error.code.as_str().to_owned(),
            }),
        }
    }
}
