//! Сборки `.hlsrpack` (задача C, `GameDeliveryService.js` 0.4.9): изменённые файлы сборки при
//! экспорте (`PACK_MODIFIED`, [`GameDeliveryService::pack_modified_count`]) и оригиналы с зеркала
//! ремонта при импорте ([`GameDeliveryService::restore_pack_originals`]) вместо отказа
//! `PACK_CORRUPT` 0.4.7.

use super::{GameDeliveryService, PackCancel, lock, preserve_paths, truthy_text};
use crate::errors::{Code, DeliveryError, Failure, to_delivery_error};
use crate::file_ops::{copy_file_sync, resolve_inside};
use crate::game_pack;
use crate::install_service::IntegrityRequest;
use crate::integrity_service::IntegrityResult;
use crate::js_value as jv;
use crate::repair_mirror::{self, Plan, PlanOptions};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Итог [`GameDeliveryService::restore_pack_originals`].
#[derive(Debug, Clone)]
pub(super) struct RestoredOriginals {
    pub changed: usize,
    pub restored: usize,
    /// Скачанные оригиналы: подтверждены списком версии, как и прошедшие проверку файлы.
    pub restored_paths: Vec<String>,
    pub custom: usize,
    pub backup: usize,
    pub rejected: usize,
}

impl GameDeliveryService {
    /// `packModifiedCount(gameId, installed)`: сколько файлов сборки не совпало со списком версии.
    /// Это лишь предупреждение: без списка версии (нет сети) экспорт идёт как есть.
    pub(super) async fn pack_modified_count(
        &self,
        game_id: &str,
        installed: &Value,
    ) -> Result<usize, Failure> {
        let version = truthy_text(jv::prop(installed, "version"));
        let files_manifest = self
            .resolve_files_manifest(game_id, version.as_deref())
            .await
            .unwrap_or(Value::Null);
        let Some(listed) = jv::prop(&files_manifest, "files").and_then(Value::as_array) else {
            return Ok(0);
        };
        self.set_active(Some(game_id));
        self.set_operation(game_id, Some("export"));
        self.set_status(
            game_id,
            json!({"state": "checking-files", "percent": 0, "checked": 0,
                   "totalChecked": listed.len(), "error": null}),
        );
        lock(&self.state).pack_cancel = Some(PackCancel::Integrity);
        let install_path = PathBuf::from(jv::template(jv::prop(installed, "installPath")));
        let preserve = preserve_paths(&self.manifest_for(game_id)?.manifest);
        let check = self
            .integrity_service
            .check(IntegrityRequest {
                game_id,
                install_path: &install_path,
                files_manifest: &files_manifest,
                preserve_paths: &preserve,
                stock_configs: false,
            })
            .await;
        lock(&self.state).pack_cancel = None;
        Ok(check?.damaged)
    }

    /// `restorePackOriginals(gameId, version, stagingPath, filesManifest, check)`: файлы сборки в
    /// архиве не сошлись со списком версии — чаще всего кастомный HUD поверх файлов игры. Импорт
    /// не отменяется: изменённый контент откладывается, изменённые программы отбрасываются
    /// ([`game_pack::set_aside`]), а оригиналы всех — с зеркала ремонта, с проверкой размера и
    /// SHA-256. Нет сети или зеркала этой версии — `PACK_ORIGINALS_UNAVAILABLE`, установленная
    /// сборка не тронута (staging просто удаляется).
    pub(super) async fn restore_pack_originals(
        &self,
        game_id: &str,
        version: &str,
        staging_path: &Path,
        files_manifest: &Value,
        check: &IntegrityResult,
    ) -> Result<RestoredOriginals, Failure> {
        let by_path: HashMap<String, &Value> = jv::array(jv::prop(files_manifest, "files"))
            .into_iter()
            .flatten()
            .map(|file| (jv::template(jv::prop(file, "path")), file))
            .collect();
        let entries: Vec<Value> = Self::damaged_paths_of(check)
            .into_iter()
            .filter_map(|relative| by_path.get(relative).map(|entry| (*entry).clone()))
            .collect();
        let changed: Vec<String> = check
            .corrupted
            .iter()
            .map(|item| item.path.clone())
            .chain(check.size_mismatch.iter().map(|item| item.path.clone()))
            .collect();
        let version_value = json!(version);

        let fetched: Result<PathBuf, Failure> = async {
            let mirror = self.repair_mirror_of(game_id, Some(&version_value)).await?;
            let allowed = self.allowed_provider_types();
            let plan = repair_mirror::plan(
                Some(&mirror),
                &entries,
                &PlanOptions {
                    version: Some(version),
                    allowed_provider_types: &allowed,
                },
            );
            let (providers, total_bytes) = match plan {
                Plan::Usable {
                    providers,
                    total_bytes,
                    ..
                } => (providers, total_bytes),
                Plan::Unusable { reason, oversized } => {
                    return Err(self
                        .mirror_unavailable(
                            game_id,
                            Some(&version_value),
                            reason,
                            oversized.as_deref(),
                        )
                        .into());
                }
            };
            self.fetch_from_mirror(game_id, &mirror, &providers, total_bytes, &entries)
                .await
        }
        .await;
        let root = match fetched {
            Ok(root) => root,
            Err(error) => {
                let delivery = to_delivery_error(error, Some(Code::PackOriginalsUnavailable));
                if matches!(
                    delivery.code,
                    Code::Cancelled
                        | Code::NoSpace
                        | Code::PathNotWritable
                        | Code::AntivirusBlocked
                ) {
                    return Err(delivery.into());
                }
                return Err(DeliveryError::new(
                    Code::PackOriginalsUnavailable,
                    format!(
                        "В архиве изменено файлов сборки {}, оригиналы версии {version} не скачать: {}",
                        changed.len(),
                        delivery.message
                    ),
                )
                .with_details(json!({
                    "changed": changed.len(),
                    "cause": delivery.code.as_str(),
                    "samples": changed.iter().take(10).collect::<Vec<_>>(),
                }))
                .into());
            }
        };

        let result: Result<RestoredOriginals, Failure> = (|| {
            self.set_operation(game_id, Some("import"));
            let aside = game_pack::set_aside(staging_path, game_id, &changed)?;
            let mut restored_paths = Vec::with_capacity(entries.len());
            for entry in &entries {
                let relative = jv::template(jv::prop(entry, "path"));
                copy_file_sync(
                    &resolve_inside(&root, &relative, Some(&format!("Эталон \"{relative}\"")))?,
                    &resolve_inside(
                        staging_path,
                        &relative,
                        Some(&format!("Файл \"{relative}\"")),
                    )?,
                    Some(&relative),
                )?;
                restored_paths.push(relative);
            }
            self.logger.warn(&format!(
                "[game-delivery] {game_id}: в архиве изменены файлы сборки ({}): в custom {}, в копии {}, программ отброшено {}, оригиналов скачано {}",
                changed.iter().take(10).cloned().collect::<Vec<_>>().join(", "),
                aside.custom.len(),
                aside.backup.len(),
                aside.rejected.len(),
                entries.len()
            ));
            Ok(RestoredOriginals {
                changed: changed.len(),
                restored: entries.len(),
                restored_paths,
                custom: aside.custom.len(),
                backup: aside.backup.len(),
                rejected: aside.rejected.len(),
            })
        })();
        self.discard_mirror_source(&root);
        result
    }
}
