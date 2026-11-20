//! Вкладки «Записи» и «Конфиги» (`runsLocation`, `listRuns`, `deleteRun`, `configGameId`,
//! `configInfo`, `readGameConfig`, `writeGameConfig`, `cfgFileCall` из `index.js` 0.4.9: до чтения
//! и записи готовится файл игрока, правки идут в него со сверкой отпечатка).

use super::player_config::PreparedPlayer;
use super::{DeliveryApi, PLATFORM, error_code};
use crate::bind_presets;
use crate::cfg_files::{self, CfgFilesError, WriteOptions};
use crate::errors::to_delivery_error;
use crate::game_config_install::{self, WriteConfigError};
use crate::js_value as jv;
use crate::manifests;
use crate::paths::assert_inside;
use crate::runs_scan::{self, DeleteResult, RunItem, ScanResult};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

/// `runsLocation(id)`: где лежат записи установленной игры.
struct RunsLocation {
    game_id: String,
    install_path: PathBuf,
    relative: String,
    dirs: Vec<String>,
    extension: Option<&'static str>,
}

fn item_value(item: &RunItem) -> Value {
    json!({"id": item.id, "name": item.name, "displayName": item.display_name, "size": item.size, "modifiedAt": item.modified_at})
}

/// Итог `runsScan.scan`/`scanMany` как объект 0.4.7.
fn scan_value(result: &ScanResult) -> Value {
    match result {
        ScanResult::Error { code } => json!({"status": "error", "code": code}),
        ScanResult::NoDirectory { path } => match path {
            Some(path) => {
                json!({"status": "no-directory", "path": path.to_string_lossy(), "items": []})
            }
            None => json!({"status": "no-directory", "items": []}),
        },
        ScanResult::Ok {
            path,
            items,
            truncated,
        } => json!({
            "status": "ok",
            "path": path.to_string_lossy(),
            "items": items.iter().map(item_value).collect::<Vec<_>>(),
            "truncated": truncated,
        }),
    }
}

impl DeliveryApi {
    fn runs_location(&self, id: &Value) -> Option<RunsLocation> {
        let game_id = self.resolve_game_id(id)?;
        let config = manifests::local_config_of(&game_id)?;
        let runs_path = config.runs_path.filter(|path| !path.is_empty())?;
        let install_path = self.installed_path_of(&game_id)?;
        // Демо HL1 с языком игры не английским ложатся в языковую папку мода.
        let dirs = if config.runs_language_dirs == Some(true) {
            runs_scan::run_dirs(&install_path, runs_path)
        } else {
            vec![runs_path.to_owned()]
        };
        Some(RunsLocation {
            game_id,
            install_path,
            relative: runs_path.to_owned(),
            dirs,
            extension: config.runs_extension,
        })
    }

    /// `game-runs-list`.
    pub fn game_runs_list(&self, id: &Value) -> Value {
        let Some(location) = self.runs_location(id) else {
            return json!({"status": "not-installed", "items": []});
        };
        scan_value(&runs_scan::scan_many(
            &location.install_path,
            &location.dirs,
            location.extension,
        ))
    }

    /// `game-runs-delete`.
    pub fn game_runs_delete(&self, id: &Value, name: &Value) -> Value {
        let Some(location) = self.runs_location(id) else {
            return error_code("NOT_INSTALLED");
        };
        if self.is_game_running(&location.game_id) {
            return error_code("GAME_RUNNING");
        }
        // Не строка — заведомо неверное имя: `deleteItem` ответит PATH_INVALID после проверки
        // каталога, как 0.4.7.
        let name = name.as_str().unwrap_or("/").to_owned();
        let relative = location
            .dirs
            .iter()
            .find(|dir| {
                runs_scan::resolve_item(&location.install_path, dir, &name, location.extension)
                    .is_some()
            })
            .cloned()
            .unwrap_or(location.relative.clone());
        match runs_scan::delete_item(&location.install_path, &relative, &name, location.extension) {
            DeleteResult::Ok { name } => json!({"status": "ok", "name": name}),
            DeleteResult::Error { code } => error_code(code),
        }
    }

    /// `game-runs-open-folder`: первый существующий каталог записей.
    pub async fn game_runs_open_folder(&self, id: &Value) -> Value {
        let Some(location) = self.runs_location(id) else {
            return error_code("NOT_INSTALLED");
        };
        let listed = location
            .dirs
            .iter()
            .map(|relative| runs_scan::scan(&location.install_path, relative, location.extension))
            .find(|result| {
                matches!(
                    result,
                    ScanResult::Ok { .. } | ScanResult::NoDirectory { .. }
                )
            });
        let target = match &listed {
            Some(ScanResult::Ok { path, .. }) => path.clone(),
            Some(ScanResult::NoDirectory { path: Some(_) }) => return error_code("NO_DIRECTORY"),
            _ => return error_code("PATH_INVALID"),
        };
        let failure = self.ports().shell.open_path(&target).await;
        if failure.is_empty() {
            json!({"status": "ok"})
        } else {
            error_code("OPEN_FAILED")
        }
    }

    /// `game-runs-reveal`: показать запись в папке.
    pub fn game_runs_reveal(&self, id: &Value, name: &Value) -> Value {
        let Some(location) = self.runs_location(id) else {
            return error_code("NOT_INSTALLED");
        };
        let name_text = jv::template(Some(name));
        let target = location.dirs.iter().find_map(|relative| {
            name.as_str().and_then(|name| {
                runs_scan::resolve_item(&location.install_path, relative, name, location.extension)
            })
        });
        let Some(target) = target else {
            self.logger().warn(&format!(
                "[runs] отклонён показ файла вне каталога записей: {name_text}"
            ));
            return error_code("PATH_INVALID");
        };
        self.ports().shell.show_item_in_folder(&target);
        json!({"status": "ok"})
    }

    // ------------------------------------------------------------ конфиги

    /// `configGameId(id, build)`: сборка, чьи конфиги показывает вкладка «Конфиги».
    fn config_game_id(&self, id: &Value, build: &Value) -> Option<String> {
        let key = jv::template(Some(id));
        let launcher_id = self.config_launcher_id(id, &key);
        if jv::truthy(Some(build)) {
            return manifests::build_game_id_of(&launcher_id, &jv::template(Some(build)))
                .map(str::to_owned);
        }
        self.installed_build_or(&launcher_id, self.resolve_game_id(id))
    }

    /// `launcherIdOf(resolveGameId(key) || key) || key`.
    fn config_launcher_id(&self, id: &Value, key: &str) -> String {
        let resolved = self.resolve_game_id(id).unwrap_or_else(|| key.to_owned());
        manifests::launcher_id_of(&resolved).map_or_else(|| key.to_owned(), str::to_owned)
    }

    /// Не установленная сборка заменяется установленной сборкой той же игры.
    fn installed_build_or(&self, launcher_id: &str, game_id: Option<String>) -> Option<String> {
        let game_id = game_id?;
        if self.installed_path_of(&game_id).is_some() {
            return Some(game_id);
        }
        let installed = manifests::BUILDS_BY_LAUNCHER_ID
            .iter()
            .find(|(id, _)| *id == launcher_id)
            .and_then(|(_, builds)| {
                builds
                    .iter()
                    .find(|build| self.installed_path_of(build.game_id).is_some())
            });
        Some(installed.map_or(game_id, |build| build.game_id.to_owned()))
    }

    /// `buildLabelOf(launcherId, gameId)`.
    fn build_label_of(&self, launcher_id: &str, game_id: &str) -> Option<&'static str> {
        manifests::BUILDS_BY_LAUNCHER_ID
            .iter()
            .find(|(id, _)| *id == launcher_id)
            .and_then(|(_, builds)| builds.iter().find(|build| build.game_id == game_id))
            .map(|build| build.build)
    }

    pub(crate) fn build_label(&self, launcher_id: &str, game_id: &str) -> Option<&'static str> {
        self.build_label_of(launcher_id, game_id)
    }

    /// `configBuilds(launcherId)`.
    fn config_builds(&self, launcher_id: &str) -> Value {
        let builds: Vec<Value> = manifests::BUILDS_BY_LAUNCHER_ID
            .iter()
            .find(|(id, _)| *id == launcher_id)
            .map(|(_, builds)| {
                builds
                    .iter()
                    .map(|build| {
                        json!({"build": build.build,
                               "installed": self.installed_path_of(build.game_id).is_some()})
                    })
                    .collect()
            })
            .unwrap_or_default();
        Value::Array(builds)
    }

    /// `game-config-info`: папка конфигов сборки.
    pub fn game_config_info(&self, id: &Value, build: &Value) -> Value {
        let key = jv::template(Some(id));
        let launcher_id = self.config_launcher_id(id, &key);
        let builds = self.config_builds(&launcher_id);
        let game_id = if jv::truthy(Some(build)) {
            let build = jv::template(Some(build));
            match manifests::build_game_id_of(&launcher_id, &build) {
                Some(game_id) => Some(game_id.to_owned()),
                None => {
                    return json!({"status": "unknown-build", "exists": false, "build": build,
                                  "builds": builds});
                }
            }
        } else {
            self.installed_build_or(&launcher_id, self.resolve_game_id(id))
        };
        let config = game_id
            .as_deref()
            .and_then(manifests::local_config_of)
            .filter(|config| !config.config_path.is_empty());
        let (Some(game_id), Some(config)) = (game_id, config) else {
            return json!({"status": "unsupported", "exists": false, "build": null,
                          "builds": builds});
        };
        let build_label = self.build_label_of(&launcher_id, &game_id);
        let Some(install_path) = self.installed_path_of(&game_id) else {
            return json!({"status": "not-installed", "exists": false, "build": build_label,
                          "builds": builds});
        };
        let native = if PLATFORM == hlsr_core::js::Platform::Win32 {
            config.config_path.replace('/', "\\")
        } else {
            config.config_path.to_owned()
        };
        match assert_inside(
            &install_path,
            &install_path.join(native),
            Some("Каталог конфигов"),
        ) {
            Ok(directory) => json!({
                "status": "ok",
                "path": directory.to_string_lossy(),
                "exists": directory.exists(),
                "build": build_label,
                "builds": builds,
            }),
            Err(_) => json!({"status": "error", "code": "PATH_INVALID", "exists": false,
                             "build": build_label, "builds": builds}),
        }
    }

    /// `game-config-open-folder`.
    pub async fn game_config_open_folder(&self, id: &Value, build: &Value) -> Value {
        let info = self.game_config_info(id, build);
        if info.get("status").and_then(Value::as_str) != Some("ok") {
            return error_code("NOT_INSTALLED");
        }
        if info.get("exists") != Some(&json!(true)) {
            return error_code("NO_DIRECTORY");
        }
        let path = PathBuf::from(jv::template(info.get("path")));
        let failure = self.ports().shell.open_path(&path).await;
        if failure.is_empty() {
            json!({"status": "ok"})
        } else {
            error_code("OPEN_FAILED")
        }
    }

    /// `game-config-read`: значения настроек сборки из её cfg.
    pub fn game_config_read(&self, id: &Value, build: &Value) -> Value {
        let game_id = self.config_game_id(id, build);
        let launch_config = game_id.as_deref().and_then(manifests::launch_config_of);
        let engine = game_id.as_deref().and_then(bind_presets::engine_for);
        let (Some(game_id), Some(launch_config), Some(engine)) = (game_id, launch_config, engine)
        else {
            return json!({"status": "unsupported"});
        };
        let Some(install_path) = self.installed_path_of(&game_id) else {
            return json!({"status": "not-installed", "gameId": game_id, "engine": engine});
        };
        self.prepare_player_config(&game_id);
        let read = game_config_install::read_config(
            self.cfg_search(),
            &install_path,
            launch_config.game_directory,
            &game_id,
            engine,
        );
        match read {
            Ok(None) => json!({"status": "unsupported"}),
            Ok(Some(result)) => json!({
                "status": "ok",
                "gameId": game_id,
                "engine": engine,
                "running": self.is_game_running(&game_id),
                "values": object_of(result.values.iter()),
                "sources": object_of(result.sources.iter()),
                "sha256": result.sha256,
                "playerFile": result.player_file,
                "shadowed": result.shadowed,
            }),
            Err(error) => {
                self.logger()
                    .warn(&format!("[game-config] конфиг не прочитан: {error}"));
                error_code("READ_FAILED")
            }
        }
    }

    /// `game-config-write`: правки пишутся в файл игрока, а его запущенная игра не переписывает
    /// (при выходе она пишет только config.cfg), поэтому запись идёт и при запущенной игре.
    /// `options.expectedSha256` — отпечаток файла игрока из `game-config-read`.
    pub fn game_config_write(
        &self,
        id: &Value,
        build: &Value,
        changes: &Value,
        options: &Value,
    ) -> Value {
        let game_id = self.config_game_id(id, build);
        let launch_config = game_id.as_deref().and_then(manifests::launch_config_of);
        let engine = game_id.as_deref().and_then(bind_presets::engine_for);
        let (Some(game_id), Some(launch_config), Some(engine)) = (game_id, launch_config, engine)
        else {
            return error_code("UNSUPPORTED");
        };
        let Some(install_path) = self.installed_path_of(&game_id) else {
            return error_code("NOT_INSTALLED");
        };
        self.prepare_player_config(&game_id);
        let expected = jv::prop(options, "expectedSha256")
            .filter(|_| jv::truthy(Some(options)))
            .and_then(Value::as_str);
        match game_config_install::write_config(
            self.cfg_search(),
            &install_path,
            launch_config.game_directory,
            &game_id,
            engine,
            changes,
            expected.map(Some),
        ) {
            Ok(result) => {
                if !result.written.is_empty() {
                    self.logger().info(&format!(
                        "[game-config] {game_id}: записаны {}",
                        result.written.join(", ")
                    ));
                }
                json!({"status": "ok", "sha256": result.sha256})
            }
            Err(WriteConfigError::Player(error)) if error.is("CONFIG_CHANGED") => {
                self.logger().warn(&format!(
                    "[game-config] {game_id}: файл игрока изменён вне лаунчера, правка отклонена"
                ));
                error_code("CONFIG_CHANGED")
            }
            Err(WriteConfigError::Config(error)) => {
                self.logger().warn(&format!(
                    "[game-config] {game_id}: правка отклонена ({error})"
                ));
                json!({"status": "error", "code": error.code.as_str(), "cvar": error.cvar})
            }
            Err(WriteConfigError::Player(error)) => {
                self.logger().error(&format!(
                    "[game-config] {game_id}: запись не удалась: {error}"
                ));
                error_code("WRITE_FAILED")
            }
            Err(WriteConfigError::Other(failure)) => {
                self.logger().error(&format!(
                    "[game-config] {game_id}: запись не удалась: {}",
                    to_delivery_error(failure, None).message
                ));
                error_code("WRITE_FAILED")
            }
        }
    }
    // ------------------------------------------------------------ редактор cfg

    /// `cfgFileCall(id, build, action)`: действие над папкой установленной сборки.
    fn cfg_file_call(
        &self,
        id: &Value,
        build: &Value,
        action: impl FnOnce(&PathBuf, &str, Option<&PreparedPlayer>) -> Result<Value, CfgFilesError>,
    ) -> Value {
        let game_id = self
            .config_game_id(id, build)
            .filter(|game_id| manifests::launch_config_of(game_id).is_some());
        let Some(game_id) = game_id else {
            return error_code("UNSUPPORTED");
        };
        let Some(install_path) = self.installed_path_of(&game_id) else {
            return error_code("NOT_INSTALLED");
        };
        let prepared = self.prepare_player_config(&game_id);
        match action(&install_path, &game_id, prepared.as_ref()) {
            Ok(result) => {
                let mut out = Map::new();
                out.insert("status".into(), json!("ok"));
                out.insert("gameId".into(), json!(game_id));
                out.insert("running".into(), json!(self.is_game_running(&game_id)));
                if let Value::Object(fields) = result {
                    out.extend(fields);
                }
                Value::Object(out)
            }
            Err(CfgFilesError::Cfg(error)) => error_code(error.code.as_str()),
            Err(CfgFilesError::Raw(error)) => {
                self.logger()
                    .warn(&format!("[cfg-files] {game_id}: {}", error.message));
                error_code("IO_FAILED")
            }
        }
    }

    /// `game-cfg-files-list`.
    pub fn game_cfg_files_list(&self, id: &Value, build: &Value) -> Value {
        // player — основной файл игрока (HL1 userconfig.cfg, Ghosting и HL2 autoexec.cfg), список
        // помечает его меткой.
        self.cfg_file_call(id, build, |install_path, _, prepared| {
            let listing = cfg_files::list(install_path);
            let player = prepared.map(|prepared| {
                crate::config_migration::relative_to(&prepared.install_path, &prepared.file)
                    .replace('\\', "/")
            });
            Ok(json!({
                "files": listing.files.iter()
                    .map(|file| json!({"path": file.path, "size": file.size, "mtime": file.mtime}))
                    .collect::<Vec<_>>(),
                "truncated": listing.truncated,
                "player": player,
            }))
        })
    }

    /// `game-cfg-file-read`.
    pub fn game_cfg_file_read(&self, id: &Value, build: &Value, relative: &Value) -> Value {
        let relative = relative_text(relative);
        self.cfg_file_call(id, build, |install_path, _, _| {
            let file = cfg_files::read(install_path, &relative)?;
            Ok(json!({
                "path": file.path,
                "content": file.content,
                "encoding": file.encoding.as_str(),
                "eol": file.eol.as_str(),
                "mtime": file.mtime,
            }))
        })
    }

    /// `game-cfg-file-write`: запись не ждёт закрытия игры.
    pub fn game_cfg_file_write(
        &self,
        id: &Value,
        build: &Value,
        relative: &Value,
        content: &Value,
        meta: &Value,
    ) -> Value {
        let relative = relative_text(relative);
        let content = cfg_files::content_text(content);
        let options = WriteOptions::from_meta(meta);
        self.cfg_file_call(id, build, |install_path, game_id, _| {
            let written = cfg_files::write(install_path, &relative, &content, &options)?;
            self.logger()
                .info(&format!("[cfg-files] {game_id}: сохранён {}", written.path));
            Ok(json!({"path": written.path, "mtime": written.mtime}))
        })
    }
}

fn object_of<'a>(entries: impl Iterator<Item = (&'a str, &'a String)>) -> Value {
    Value::Object(
        entries
            .map(|(key, value)| (key.to_owned(), json!(value)))
            .collect(),
    )
}

/// `String(relative || "")`.
fn relative_text(relative: &Value) -> String {
    if jv::truthy(Some(relative)) {
        jv::template(Some(relative))
    } else {
        String::new()
    }
}
