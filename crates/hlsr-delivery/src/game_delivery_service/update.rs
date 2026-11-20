//! Обновления и доверие (задача G): свежесть канала, полный архив только с закреплённым SHA-256,
//! удаления патча только по файловым манифестам, хук рендера демо.

use super::{GameDeliveryService, lock, patch_manifest_path, strict_eq};
use crate::errors::{Code, DeliveryError, Failure};
use crate::file_ops::rm_sync;
use crate::files_manifest;
use crate::js_value as jv;
use crate::patch_format::{self, PatchValidateOptions};
use crate::path_match::Matcher;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Пути файлов из файлового манифеста (`files[].path`).
fn paths_of(manifest: &Value) -> HashSet<String> {
    jv::prop(manifest, "files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|file| jv::template(jv::prop(file, "path")))
        .collect()
}

impl GameDeliveryService {
    /// `assertNotRendering(gameId)`: инструмент «Демо» рендерит игру в фоне — её файлы заняты, а
    /// реестр и папка мода вернутся только в конце рендера.
    pub(super) fn assert_not_rendering(&self, game_id: &str) -> Result<(), DeliveryError> {
        let hook = lock(&self.render_active).clone();
        if hook.is_some_and(|active| active(game_id)) {
            return Err(
                DeliveryError::new(Code::RenderActive, "Идёт рендер демо этой игры")
                    .with_details(json!({"gameId": game_id})),
            );
        }
        Ok(())
    }

    /// `channelFreshnessFile` (фасад: `userData/game-channel-freshness.json`): где канал игр помнит
    /// `publishedAt` принятых манифестов.
    pub fn set_channel_freshness_file(&self, file: PathBuf) {
        if let Some(channel) = &self.channel_service {
            channel.set_freshness_file(file);
        }
    }

    /// Полный архив без закреплённого SHA-256 (канал или встроенный манифест) не ставится.
    pub(super) fn assert_pinned_archive(
        game_id: &str,
        descriptor: &Value,
        version: &Value,
    ) -> Result<(), DeliveryError> {
        if jv::truthy(jv::prop(descriptor, "sha256")) {
            return Ok(());
        }
        Err(DeliveryError::new(
            Code::ChecksumMismatch,
            "У полного архива нет закреплённого SHA-256 — такая сборка не ставится",
        )
        .with_details(json!({"gameId": game_id, "version": version})))
    }

    /// `dropPatchManifest(archivePath)`: отвергнутый манифест патча убирается из Downloads (повтор
    /// скачает его заново). Ошибка удаления не подменяет настоящую ошибку патча.
    pub(super) fn drop_patch_manifest(&self, archive_path: &str) {
        let candidate = patch_manifest_path(archive_path);
        // Имя архива без `.7z`: «манифест» — сам скачанный архив, его не удалять.
        if candidate == archive_path {
            return;
        }
        if let Err(error) = rm_sync(Path::new(&candidate), false, 0, Duration::ZERO) {
            self.logger.warn(&format!(
                "[game-delivery] отвергнутый манифест патча не удалён: {}",
                error.message
            ));
        }
    }

    /// `checkPatchManifest(gameId, step, data)`: манифест патча приходит с зеркала без подписи.
    /// `files[]` привязаны к закреплённому архиву, остальное сверяется с подписанным шагом.
    pub(super) fn check_patch_manifest(
        &self,
        game_id: &str,
        step: &Value,
        data: &Value,
    ) -> Result<Value, Failure> {
        let patch_manifest = patch_format::validate_patch_manifest(
            data,
            Some(&PatchValidateOptions {
                expected_from: jv::prop(step, "fromVersion")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }),
        )?
        .clone();

        if !strict_eq(
            jv::prop(&patch_manifest, "toVersion"),
            jv::prop(step, "toVersion"),
        ) {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                "Версия патча не совпадает с манифестом игры",
            )
            .into());
        }

        let archive_sha = |value: &Value| {
            jv::prop(value, "archive")
                .and_then(|archive| jv::prop(archive, "sha256"))
                .cloned()
        };
        if !strict_eq(
            archive_sha(&patch_manifest).as_ref(),
            archive_sha(step).as_ref(),
        ) {
            return Err(DeliveryError::new(
                Code::ChecksumMismatch,
                "SHA-256 патча не совпадает с манифестом игры",
            )
            .into());
        }

        if !strict_eq(
            jv::prop(&patch_manifest, "resultingFilesManifestSha256"),
            jv::prop(step, "resultingFilesManifestSha256"),
        ) {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                "Ожидаемый файловый манифест патча не совпадает с манифестом игры",
            )
            .into());
        }

        // deletedFiles ничем не подписаны. Удалять можно только то, что прежняя версия поставляла,
        // а итоговая (её список закреплён подписанным resultingFilesManifestSha256) уже не
        // содержит: сейвы и демки ни в один файловый манифест не входят.
        let version_of = |key: &str| jv::prop(step, key).and_then(Value::as_str);
        let files_of = |version| self.files_manifest_for(game_id, version).ok();
        let previous = files_of(version_of("fromVersion"));
        let mut resulting = files_of(version_of("toVersion"));
        if let Some(manifest) = &resulting
            && !jv::is_str(
                jv::prop(step, "resultingFilesManifestSha256"),
                &files_manifest::hash_manifest(manifest)?,
            )
        {
            resulting = None;
        }

        let listed: Vec<String> = jv::prop(&patch_manifest, "deletedFiles")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|relative| jv::template(Some(relative)))
            .collect();
        let mut deleted = listed.clone();

        match (&previous, &resulting) {
            (Some(previous), Some(resulting)) => {
                let (shipped, kept) = (paths_of(previous), paths_of(resulting));
                let foreign: Vec<&String> = deleted
                    .iter()
                    .filter(|relative| !shipped.contains(*relative) || kept.contains(*relative))
                    .collect();
                if !foreign.is_empty() {
                    let shown: Vec<&str> = foreign.iter().take(5).map(|s| s.as_str()).collect();
                    return Err(DeliveryError::new(
                        Code::ManifestInvalid,
                        format!(
                            "Манифест патча удаляет файлы не из прежней сборки: {}",
                            shown.join(", ")
                        ),
                    )
                    .with_details(json!({"files": foreign.iter().take(20).collect::<Vec<_>>()}))
                    .into());
                }
            }
            _ => {
                if !deleted.is_empty() {
                    self.logger.warn(&format!(
                        "[game-delivery] {game_id} {} -> {}: нет подписанных файловых манифестов обеих версий — \
                         удаления из манифеста патча ({}) пропущены",
                        jv::template(jv::prop(step, "fromVersion")),
                        jv::template(jv::prop(step, "toVersion")),
                        deleted.len()
                    ));
                }
                deleted.clear();
            }
        }

        // Как build-patch (patch-format.diff): пути игрока из preservePaths патч не удаляет.
        let preserve: Vec<String> =
            jv::prop(&self.manifest_for(game_id)?.manifest, "preservePaths")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|pattern| jv::template(Some(pattern)))
                .collect();
        let preserved = Matcher::new(&preserve);
        deleted.retain(|relative| !preserved.matches(relative));

        if deleted.len() == listed.len() {
            return Ok(patch_manifest);
        }

        // Самохеш покрывает deletedFiles: без пересчёта PatchService.apply отвергнет манифест. Сам
        // хеш ничего не закрепляет, файлы патча проверяются по архиву и своим SHA-256.
        let mut trimmed = patch_manifest;
        trimmed["deletedFiles"] = json!(deleted);
        if jv::truthy(jv::prop(&trimmed, "patchManifestSha256")) {
            trimmed["patchManifestSha256"] = json!(patch_format::hash_patch_manifest(&trimmed)?);
        }
        Ok(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::GameDeliveryService;
    use crate::errors::Code;
    use serde_json::json;

    /// `hl1-selftest.js` 0.4.9: цель без хеша, как-то дошедшая до установки, не ставится.
    #[test]
    fn a_full_archive_without_a_pinned_sha256_is_not_installed() {
        let version = json!("1.20260920.509");
        for descriptor in [json!({}), json!({"sha256": null}), json!({"sha256": ""})] {
            let error =
                GameDeliveryService::assert_pinned_archive("hl1-won", &descriptor, &version)
                    .unwrap_err();
            assert_eq!(error.code, Code::ChecksumMismatch);
        }
        let pinned = json!({"sha256": "a".repeat(64)});
        assert!(GameDeliveryService::assert_pinned_archive("hl1-won", &pinned, &version).is_ok());
    }
}
