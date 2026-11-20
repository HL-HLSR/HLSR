//! Экспорт и импорт сборки: архив `.hlsrpack` (папка игры и настройки) и файл `.hlsrbuild`
//! (настройки и cfg игрока) — `exportBuild`, `exportSettings`, `importBuild`, `importPack`,
//! `confirmPackImport`, `applyPendingBuildImport` из `index.js` 0.4.7, `runPackExport` и
//! `confirmPackExport` из 0.4.9.

use super::{DeliveryApi, channels, error_code, lock};
use crate::build_transfer::{self, Build, TransferFailure};
use crate::errors::{Code, Failure, to_delivery_error};
use crate::game_delivery_service::PackExtra;
use crate::game_pack::{self, PackMeta};
use crate::js_value as jv;
use crate::manifests;
use crate::ports::{FileFilter, OpenDialog, SaveDialog};
use hlsr_core::js;
use hlsr_core::js::path::NodePath;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Самый большой файл `.hlsrbuild`, который читает импорт.
const MAX_BUILD_FILE_BYTES: u64 = 24 * 1024 * 1024;

/// `pendingPackExport`: экспорт с изменёнными файлами сборки ждёт подтверждения окна.
pub(super) struct PendingPackExport {
    token: String,
    game_id: String,
    file: PathBuf,
    extra: PackExtra,
}

/// Одноразовый токен подтверждения: 16 случайных байт в hex.
fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("системный генератор случайных чисел недоступен");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Код и текст для журнала: `error.code || error.message`.
fn transfer_code(failure: &TransferFailure) -> String {
    match failure {
        TransferFailure::Transfer(error) => error.code.as_str().to_owned(),
        TransferFailure::Raw(error) => error.code.clone().unwrap_or_else(|| error.message.clone()),
        TransferFailure::Js(message) => message.clone(),
    }
}

fn transfer_message(failure: &TransferFailure) -> String {
    match failure {
        TransferFailure::Transfer(error) => error.code.as_str().to_owned(),
        TransferFailure::Raw(error) => error.message.clone(),
        TransferFailure::Js(message) => message.clone(),
    }
}

impl DeliveryApi {
    fn stamp(&self) -> String {
        let time = UNIX_EPOCH + Duration::from_millis(self.now_ms().max(0) as u64);
        js::iso_string(time).chars().take(10).collect()
    }

    fn now_time(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.now_ms().max(0) as u64)
    }

    fn stored_settings(&self, game_id: &str) -> Option<Value> {
        self.settings_store()
            .map(|store| store.get(game_id).to_value())
    }

    /// `game-build-export`: архив `.hlsrpack`; сборка из Steam — только настройки и cfg.
    pub async fn game_build_export(&self, id: &Value) -> Value {
        let game_id = self.resolve_game_id(id);
        let Some(service) = self.ensure_service() else {
            return error_code("NOT_READY");
        };
        let installed = game_id.as_deref().and_then(|game_id| {
            service
                .install_service()
                .read_installed(game_id)
                .ok()
                .flatten()
        });
        let (Some(game_id), Some(installed)) = (game_id, installed) else {
            return error_code("NOT_INSTALLED");
        };
        if manifests::is_depot_build(&game_id) {
            return self.game_build_export_settings(&json!(game_id)).await;
        }
        if service.is_game_running(&game_id) {
            return error_code("GAME_RUNNING");
        }
        if service.is_busy() {
            return error_code("BUSY");
        }
        let launcher_id = manifests::launcher_id_of(&game_id)
            .unwrap_or(&game_id)
            .to_owned();
        let version = installed
            .get("version")
            .filter(|version| jv::truthy(Some(version)))
            .map(|version| format!("-{}", jv::template(Some(version))))
            .unwrap_or_default();
        let target = self
            .ports()
            .dialogs
            .show_save_dialog(SaveDialog {
                default_path: Some(format!(
                    "{game_id}{version}-{}.{}",
                    self.stamp(),
                    game_pack::EXTENSION
                )),
                filters: vec![FileFilter::new(
                    format!("HLSR (*.{})", game_pack::EXTENSION),
                    &[game_pack::EXTENSION],
                )],
            })
            .await;
        let Some(target) = target.filter(|target| !target.as_os_str().is_empty()) else {
            return json!({"status": "canceled"});
        };
        let path = NodePath::native();
        let text = target.to_string_lossy().into_owned();
        let file = if path.extname(&text).is_empty() {
            PathBuf::from(format!("{text}.{}", game_pack::EXTENSION))
        } else {
            target
        };
        let extra = PackExtra {
            launcher_id: Some(json!(launcher_id)),
            build: Some(json!(self.build_label(&launcher_id, &game_id))),
            launcher_version: Some(json!(self.options.launcher_version)),
            settings: Some(self.stored_settings(&game_id).unwrap_or(Value::Null)),
            allow_modified: false,
        };
        self.run_pack_export(&game_id, file, extra).await
    }

    /// `runPackExport(gameId, filePath, extra)`. Изменённые файлы сборки (`PACK_MODIFIED`): окно
    /// предупреждает, путь к архиву и сведения остаются здесь, окно возвращает одноразовый токен
    /// (`game-build-export-confirm`).
    async fn run_pack_export(&self, game_id: &str, file: PathBuf, extra: PackExtra) -> Value {
        let Some(service) = self.service() else {
            return error_code("NOT_READY");
        };
        match service.export_pack(game_id, &file, extra.clone()).await {
            Ok(exported) => json!({
                "status": "ok",
                "kind": "pack",
                "gameId": game_id,
                "size": exported.get("size"),
                "path": exported.get("path"),
            }),
            Err(Failure::Delivery(error)) if error.code == Code::PackModified => {
                let token = random_token();
                let details = error.details.unwrap_or(Value::Null);
                lock(&self.state).pending_pack_export = Some(PendingPackExport {
                    token: token.clone(),
                    game_id: game_id.to_owned(),
                    file,
                    extra,
                });
                json!({
                    "status": "confirmation-required",
                    "kind": "pack-modified",
                    "token": token,
                    "gameId": game_id,
                    "modified": details.get("modified"),
                    "custom": js::truthy(details.get("custom").unwrap_or(&Value::Null)),
                })
            }
            Err(failure) => self.pack_error(
                failure,
                Code::PackFailed,
                &format!("{game_id}: экспорт не удался"),
            ),
        }
    }

    /// `game-build-export-confirm`: окно вернуло одноразовый токен экспорта с изменёнными файлами.
    pub async fn game_build_export_confirm(&self, token: &Value) -> Value {
        let pending = lock(&self.state).pending_pack_export.take();
        let Some(pending) =
            pending.filter(|pending| token.as_str() == Some(pending.token.as_str()))
        else {
            return error_code("PACK_FAILED");
        };
        let Some(service) = self.ensure_service() else {
            return error_code("NOT_READY");
        };
        if service.is_game_running(&pending.game_id) {
            return error_code("GAME_RUNNING");
        }
        if service.is_busy() {
            return error_code("BUSY");
        }
        let extra = PackExtra {
            allow_modified: true,
            ..pending.extra
        };
        self.run_pack_export(&pending.game_id, pending.file, extra)
            .await
    }

    /// `game-build-export-settings`: файл `.hlsrbuild` с настройками и cfg игрока.
    pub async fn game_build_export_settings(&self, id: &Value) -> Value {
        let game_id = self.resolve_game_id(id);
        let installed = game_id
            .as_deref()
            .and_then(|game_id| self.read_installed(game_id));
        let install_path = installed
            .as_ref()
            .and_then(|installed| installed.get("installPath"))
            .filter(|path| jv::truthy(Some(path)))
            .map(|path| PathBuf::from(jv::template(Some(path))));
        let (Some(game_id), Some(installed), Some(install_path)) =
            (game_id, installed, install_path)
        else {
            return error_code("NOT_INSTALLED");
        };
        let version = installed
            .get("version")
            .filter(|version| jv::truthy(Some(version)))
            .map(|version| jv::template(Some(version)));
        let previous = version
            .as_deref()
            .and_then(|version| manifests::files_manifest_of(&game_id, version));
        let files = match build_transfer::collect(&install_path, previous) {
            Ok(files) => files,
            Err(failure) => return self.export_settings_failed(&game_id, failure),
        };
        let launcher_id = manifests::launcher_id_of(&game_id)
            .unwrap_or(&game_id)
            .to_owned();
        let build = self.build_label(&launcher_id, &game_id);
        let suffix = version
            .as_ref()
            .map(|version| format!("-{version}"))
            .unwrap_or_default();
        let target = self
            .ports()
            .dialogs
            .show_save_dialog(SaveDialog {
                default_path: Some(format!(
                    "{game_id}{suffix}-{}.{}",
                    self.stamp(),
                    build_transfer::EXTENSION
                )),
                filters: vec![FileFilter::new(
                    format!("HLSR (*.{})", build_transfer::EXTENSION),
                    &[build_transfer::EXTENSION],
                )],
            })
            .await;
        let Some(target) = target.filter(|target| !target.as_os_str().is_empty()) else {
            return json!({"status": "canceled"});
        };
        let settings = self.stored_settings(&game_id);
        let text = build_transfer::serialize(&Build {
            launcher_id: &launcher_id,
            game_id: &game_id,
            build,
            version: version.as_deref(),
            launcher_version: Some(&self.options.launcher_version),
            settings: settings.as_ref(),
            files: &files,
            exported_at: Some(self.now_time()),
        });
        if let Err(error) = tokio::fs::write(&target, text.as_bytes()).await {
            let raw = crate::errors::RawError::fs(&error, "open", &target);
            return self.export_settings_failed(&game_id, TransferFailure::Raw(raw));
        }
        self.logger().info(&format!(
            "[build-transfer] {game_id}: экспорт в {}, cfg: {}",
            target.display(),
            files.len()
        ));
        json!({"status": "ok", "gameId": game_id, "files": files.len(),
               "path": target.to_string_lossy()})
    }

    fn export_settings_failed(&self, game_id: &str, failure: TransferFailure) -> Value {
        if let TransferFailure::Transfer(error) = &failure {
            if error.file.is_some() {
                self.logger().warn(&format!(
                    "[build-transfer] {game_id}: экспорт не удался: {error}"
                ));
            }
            return json!({"status": "error", "code": error.code.as_str(), "file": error.file});
        }
        self.logger().warn(&format!(
            "[build-transfer] {game_id}: экспорт не удался: {}",
            transfer_message(&failure)
        ));
        error_code("IO_FAILED")
    }

    /// `game-build-import`: настройки — сразу, cfg — в установленную сборку или после её
    /// установки; архив `.hlsrpack` — после подтверждения.
    pub async fn game_build_import(&self) -> Value {
        let Some(service) = self.ensure_service() else {
            return error_code("NOT_READY");
        };
        let picked = self
            .ports()
            .dialogs
            .show_open_dialog(OpenDialog {
                properties: vec!["openFile"],
                filters: vec![FileFilter::new(
                    format!(
                        "HLSR (*.{}, *.{})",
                        game_pack::EXTENSION,
                        build_transfer::EXTENSION
                    ),
                    &[game_pack::EXTENSION, build_transfer::EXTENSION],
                )],
                ..OpenDialog::default()
            })
            .await;
        let Some(file) = picked.into_iter().next() else {
            return json!({"status": "canceled"});
        };
        let path = NodePath::native();
        if path.extname(&file.to_string_lossy()).to_lowercase()
            == format!(".{}", game_pack::EXTENSION)
        {
            return self.import_pack(&file).await;
        }
        let read = async {
            let meta = tokio::fs::metadata(&file)
                .await
                .map_err(|error| crate::errors::RawError::fs(&error, "stat", &file))?;
            if !meta.is_file() || meta.len() > MAX_BUILD_FILE_BYTES {
                return Ok(None);
            }
            let bytes = tokio::fs::read(&file)
                .await
                .map_err(|error| crate::errors::RawError::fs(&error, "open", &file))?;
            Ok::<_, crate::errors::RawError>(Some(js::utf8_string(&bytes)))
        };
        let text = match read.await {
            Ok(Some(text)) => text,
            Ok(None) => return error_code("TOO_LARGE"),
            Err(error) => {
                self.logger().warn(&format!(
                    "[build-transfer] файл сборки не прочитан: {}",
                    error.message
                ));
                return error_code("IO_FAILED");
            }
        };
        let parsed = match build_transfer::parse(&text, Some(&manifests::is_known_game)) {
            Ok(parsed) => parsed,
            Err(TransferFailure::Transfer(error)) => return error_code(error.code.as_str()),
            Err(failure) => {
                self.logger().warn(&format!(
                    "[build-transfer] файл сборки не прочитан: {}",
                    transfer_message(&failure)
                ));
                return error_code("IO_FAILED");
            }
        };
        let game_id = parsed.game_id.clone();
        if service.is_game_running(&game_id) {
            return error_code("GAME_RUNNING");
        }
        if service.is_busy() {
            return error_code("BUSY");
        }
        if let Some(settings) = parsed.settings.as_ref().filter(|s| js::truthy(s)) {
            self.save_imported_settings(&game_id, settings, "build-transfer", "файла");
        }
        let launcher_id = manifests::launcher_id_of(&game_id)
            .unwrap_or(&game_id)
            .to_owned();
        if launcher_id != game_id {
            self.select_build_of(&launcher_id, &game_id);
        }
        if let Some(install_path) = self.installed_path_of(&game_id) {
            return match build_transfer::apply_files(&install_path, &parsed.files) {
                Ok(written) => {
                    self.logger().info(&format!(
                        "[build-transfer] {game_id}: импортировано cfg: {}",
                        written.len()
                    ));
                    json!({"status": "ok", "gameId": game_id, "launcherId": launcher_id,
                           "installed": true, "files": written.len(),
                           "fromVersion": parsed.version})
                }
                Err(failure) => self.import_failed(&game_id, failure),
            };
        }
        let Some(pending) = self.pending_build_import_file(&game_id) else {
            return error_code("IO_FAILED");
        };
        let write = async {
            if let Some(dir) = pending.parent() {
                tokio::fs::create_dir_all(dir)
                    .await
                    .map_err(|error| crate::errors::RawError::fs(&error, "mkdir", dir))?;
            }
            tokio::fs::write(&pending, text.as_bytes())
                .await
                .map_err(|error| crate::errors::RawError::fs(&error, "open", &pending))
        };
        if let Err(error) = write.await {
            return self.import_failed(&game_id, TransferFailure::Raw(error));
        }
        self.logger().info(&format!(
            "[build-transfer] {game_id}: cfg ({}) лягут после установки",
            parsed.files.len()
        ));
        json!({"status": "ok", "gameId": game_id, "launcherId": launcher_id, "installed": false,
               "files": parsed.files.len(), "fromVersion": parsed.version})
    }

    fn import_failed(&self, game_id: &str, failure: TransferFailure) -> Value {
        if let TransferFailure::Transfer(error) = &failure {
            return error_code(error.code.as_str());
        }
        self.logger().warn(&format!(
            "[build-transfer] {game_id}: импорт не удался: {}",
            transfer_message(&failure)
        ));
        error_code("IO_FAILED")
    }

    /// Настройки из импорта: отказ проверки — в журнал.
    fn save_imported_settings(&self, game_id: &str, settings: &Value, area: &str, from: &str) {
        let Some(store) = self.settings_store() else {
            return;
        };
        let _guard = self.settings_guard();
        // S-1: из чужого файла — свои protonOptions, launchArgs только без команд и путей.
        if let Err(error) = store.save_imported(game_id, settings) {
            let code = match &error {
                crate::game_settings_store::StoreError::Rejected(rejected) => rejected.code,
                crate::game_settings_store::StoreError::Io(_) => "undefined",
            };
            self.logger().warn(&format!(
                "[{area}] {game_id}: настройки из {from} отклонены ({code})"
            ));
        }
    }

    /// `importPack(file)`: установленная сборка заменяется целиком — сперва подтверждение.
    async fn import_pack(&self, file: &Path) -> Value {
        let Some(service) = self.service() else {
            return error_code("NOT_READY");
        };
        let meta = match service.read_pack_meta(file).await {
            Ok(meta) => meta,
            Err(failure) => {
                return self.pack_error(
                    failure,
                    Code::PackBadFormat,
                    &format!("архив {} не прочитан", file.display()),
                );
            }
        };
        if service.is_game_running(&meta.game_id) {
            return error_code("GAME_RUNNING");
        }
        if service.is_busy() {
            return error_code("BUSY");
        }
        let installed_version = meta.installed_version.clone().flatten();
        let Some(installed_version) = installed_version.filter(|version| !version.is_empty())
        else {
            return self.run_pack_import(file, &meta).await;
        };
        let token = random_token();
        self.set_pending_pack(token.clone(), file.to_path_buf());
        json!({
            "status": "confirmation-required",
            "kind": "pack",
            "token": token,
            "gameId": meta.game_id,
            "version": meta.version,
            "installedVersion": installed_version,
        })
    }

    /// `game-build-import-confirm`: окно вернуло одноразовый токен.
    pub async fn game_build_import_confirm(&self, token: &Value) -> Value {
        let pending = self.take_pending_pack();
        let Some(file) = pending
            .filter(|(expected, _)| token.as_str() == Some(expected.as_str()))
            .map(|(_, file)| file)
        else {
            return error_code("PACK_FAILED");
        };
        let Some(service) = self.ensure_service() else {
            return error_code("NOT_READY");
        };
        match service.read_pack_meta(&file).await {
            Ok(meta) => self.run_pack_import(&file, &meta).await,
            Err(failure) => self.pack_error(
                failure,
                Code::PackBadFormat,
                &format!("архив {} не прочитан", file.display()),
            ),
        }
    }

    /// `runPackImport(file, meta)`.
    async fn run_pack_import(&self, file: &Path, meta: &PackMeta) -> Value {
        let Some(service) = self.service() else {
            return error_code("NOT_READY");
        };
        let game_id = meta.game_id.clone();
        let imported = match service.import_pack(file, meta).await {
            Ok(imported) => imported,
            Err(failure) => {
                return self.pack_error(
                    failure,
                    Code::PackFailed,
                    &format!("{game_id}: импорт не удался"),
                );
            }
        };
        if let Some(settings) = meta.settings.as_ref().filter(|s| js::truthy(s)) {
            self.save_imported_settings(&game_id, settings, "game-pack", "архива");
        }
        let launcher_id = manifests::launcher_id_of(&game_id)
            .unwrap_or(&game_id)
            .to_owned();
        if launcher_id != game_id {
            self.select_build_of(&launcher_id, &game_id);
        }
        json!({
            "status": "ok",
            "kind": "pack",
            "gameId": game_id,
            "launcherId": launcher_id,
            "installed": true,
            "version": imported.get("version"),
            "dropped": imported.get("dropped"),
            "modified": imported.get("modified").filter(|m| js::truthy(m)),
        })
    }

    /// `packError(error, fallback, context)`.
    fn pack_error(&self, failure: Failure, fallback: Code, context: &str) -> Value {
        let error = to_delivery_error(failure, Some(fallback));
        if error.code == Code::Cancelled {
            return json!({"status": "canceled"});
        }
        self.logger().warn(&format!(
            "[game-pack] {context}: {} {}",
            error.code, error.message
        ));
        json!({"status": "error", "code": error.code.as_str(), "error": error.to_public()})
    }

    /// `pendingBuildImportFile(gameId)`.
    fn pending_build_import_file(&self, game_id: &str) -> Option<PathBuf> {
        let dir = self.user_data_path()?;
        if !manifests::is_known_game(game_id) {
            return None;
        }
        Some(
            dir.join("build-imports")
                .join(format!("{game_id}.{}", build_transfer::EXTENSION)),
        )
    }

    /// `applyPendingBuildImport(gameId)`: cfg отложенного импорта — после установки.
    pub(crate) fn apply_pending_build_import(&self, game_id: &str) {
        let Some(file) = self.pending_build_import_file(game_id) else {
            return;
        };
        if !file.exists() {
            return;
        }
        let Some(install_path) = self.installed_path_of(game_id) else {
            return;
        };
        let applied = std::fs::read(&file)
            .map_err(|error| {
                TransferFailure::Raw(crate::errors::RawError::fs(&error, "open", &file))
            })
            .and_then(|bytes| {
                let known = |id: &str| id == game_id;
                build_transfer::parse(&js::utf8_string(&bytes), Some(&known))
            })
            .and_then(|parsed| build_transfer::apply_files(&install_path, &parsed.files));
        match applied {
            Ok(written) => {
                self.logger().info(&format!(
                    "[build-transfer] {game_id}: после установки импортировано cfg: {}",
                    written.len()
                ));
                self.emit_to(
                    channels::BUILD_IMPORT_APPLIED,
                    json!({"gameId": game_id, "files": written.len()}),
                );
            }
            Err(failure) => {
                let code = transfer_code(&failure);
                self.logger().warn(&format!(
                    "[build-transfer] {game_id}: отложенный импорт не удался: {code}"
                ));
                let error = match &failure {
                    TransferFailure::Transfer(error) => error.code.as_str().to_owned(),
                    TransferFailure::Raw(error) => {
                        error.code.clone().unwrap_or_else(|| "IO_FAILED".to_owned())
                    }
                    TransferFailure::Js(_) => "IO_FAILED".to_owned(),
                };
                self.emit_to(
                    channels::BUILD_IMPORT_APPLIED,
                    json!({"gameId": game_id, "error": error}),
                );
            }
        }
        // Не удалился — следующий успех попробует снова.
        let _ = std::fs::remove_file(&file);
    }
}
