//! Ремонт по файлам (задача B, `GameDeliveryService.js` 0.4.9). Файлы для ремонта берутся только
//! из пофайлового зеркала версии (`repairMirror` подписанного канала, раскладка `sha256-2`):
//! полный архив ради пары файлов не качается никогда. Проверка и ремонт включены по списку
//! (`manifests::is_file_repair_game`), не только в меню.
//!
//! Без подтверждения — что можно починить (`files: [{path, size, kind}]`, вид — `missing`,
//! `damaged`, `cfg-missing`, `cfg-changed`). С подтверждением — отмеченные игроком файлы
//! (`files`); без выбора — всё повреждённое, изменённые cfg не трогаются. Загрузку с зеркала
//! отменяет `cancel` (`repair_abort`): игра не тронута, статус — прежний (`files-valid` или
//! `files-damaged`). В итоге `remaining` — оставшиеся повреждёнными файлы сборки (стоковые cfg,
//! с которых сняли галочку, интерфейс показывает мягко по `integrity`).

use super::{
    GameDeliveryService, RepairCall, exists, join, lock, message_of, preserve_paths, strict_eq,
    text, truthy_text,
};
use crate::archive_service::CancelHandle;
use crate::errors::{Code, DeliveryError, Failure, to_delivery_error};
use crate::install_service::IntegrityRequest;
use crate::integrity_service::IntegrityResult;
use crate::js_value as jv;
use crate::manifest_service::ManifestService;
use crate::manifests;
use crate::paths::{assert_inside, remove_within_zone};
use crate::repair_mirror::{self, FetchOptions, Plan, PlanOptions};
use crate::repair_service::RepairRequest;
use crate::update_checker::CheckOptions;
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

impl GameDeliveryService {
    /// `assertFileRepairGame(gameId)`.
    pub(super) fn assert_file_repair_game(game_id: &str) -> Result<(), DeliveryError> {
        if manifests::is_file_repair_game(game_id) {
            return Ok(());
        }
        Err(DeliveryError::new(
            Code::RepairSourceMissing,
            format!("Проверка и восстановление по файлам для {game_id} пока недоступны"),
        )
        .with_details(json!({"gameId": game_id, "reason": "NOT_ENABLED"})))
    }

    /// `repair(gameId, options)`: замена повреждённых файлов; без подтверждения — что можно
    /// починить.
    pub async fn repair(&self, game_id: &str, options: RepairCall) -> Result<Value, Failure> {
        self.assert_not_depot_build(game_id, "восстановление файлов")?;
        Self::assert_file_repair_game(game_id)?;
        let _slot = self.reserve_slot(game_id, "repair")?;
        self.run_repair(game_id, options).await
    }

    async fn run_repair(&self, game_id: &str, options: RepairCall) -> Result<Value, Failure> {
        self.assert_game_not_running(game_id)?;
        self.assert_steam_requirements(game_id).await?;

        let installed = self.installed_of(game_id)?;
        let manifest = self.manifest_for(game_id)?.manifest;
        let version = truthy_text(jv::prop(&installed, "version"));
        let files = self
            .resolve_files_manifest(game_id, version.as_deref())
            .await?;
        let mirror = self
            .repair_mirror_of(game_id, jv::prop(&installed, "version"))
            .await?;

        let last = lock(&self.state).last_integrity.get(game_id).cloned();
        let integrity = match last {
            Some(last) if options.skip_recheck => last,
            _ => self.run_verify_files(game_id).await?,
        };

        let offered: Vec<String> = Self::damaged_paths_of(&integrity)
            .into_iter()
            .chain(&integrity.config_changed)
            .cloned()
            .collect();
        if offered.is_empty() {
            self.set_status(
                game_id,
                json!({"state": "files-valid", "integrity": Self::summarize_integrity(&integrity)}),
            );
            return Ok(json!({"status": "no-damage", "repaired": []}));
        }

        // Стоковые cfg из папок игрока (files-manifest.js) — тоже из подписанного списка версии.
        let mut by_path: HashMap<String, &Value> = HashMap::new();
        for key in ["files", "stockConfigs"] {
            for file in jv::array(jv::prop(&files, key)).into_iter().flatten() {
                by_path.insert(jv::template(jv::prop(file, "path")), file);
            }
        }

        if !options.confirmed {
            return Ok(json!({
                "status": "confirmation-required",
                "damaged": integrity.damaged,
                "files": Self::repair_choices(&integrity, &by_path),
            }));
        }

        // Отмеченные игроком; без выбора — всё повреждённое, изменённые cfg не трогаем.
        let chosen: HashSet<&String> = match &options.files {
            Some(files) => files.iter().collect(),
            None => Self::damaged_paths_of(&integrity).into_iter().collect(),
        };
        let entries: Vec<Value> = offered
            .iter()
            .filter(|item| chosen.contains(item))
            .filter_map(|item| by_path.get(item).map(|entry| (*entry).clone()))
            .filter(js::truthy)
            .collect();
        if entries.is_empty() {
            return Ok(json!({"status": "nothing-selected", "repaired": []}));
        }

        let allowed = self.allowed_provider_types();
        let plan = repair_mirror::plan(
            Some(&mirror),
            &entries,
            &PlanOptions {
                version: jv::prop(&installed, "version").and_then(Value::as_str),
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
                let version = jv::prop(&installed, "version");
                return Err(self
                    .mirror_unavailable(game_id, version, reason, oversized.as_deref())
                    .into());
            }
        };

        self.set_active(Some(game_id));
        let mut root: Option<PathBuf> = None;
        let install_path = PathBuf::from(jv::template(jv::prop(&installed, "installPath")));
        let preserve = preserve_paths(&manifest);

        let result: Result<Value, Failure> = async {
            let fetched = self
                .fetch_from_mirror(game_id, &mirror, &providers, total_bytes, &entries)
                .await?;
            let root = root.insert(fetched);

            self.set_operation(game_id, Some("repair"));
            self.set_status(
                game_id,
                json!({"state": "repairing", "percent": 0, "error": null}),
            );

            let report = self
                .repair_service
                .repair(RepairRequest {
                    game_id: game_id.to_owned(),
                    install_path: install_path.clone(),
                    files_manifest: files.clone(),
                    preserve_paths: preserve.clone(),
                    damaged_files: entries
                        .iter()
                        .map(|entry| jv::template(jv::prop(entry, "path")))
                        .collect(),
                    prepared_root: Some(root.clone()),
                })
                .await?;

            let recheck = self
                .integrity_service
                .check(IntegrityRequest {
                    game_id,
                    install_path: &install_path,
                    files_manifest: &files,
                    preserve_paths: &preserve,
                    stock_configs: true,
                })
                .await?;

            lock(&self.state)
                .last_integrity
                .insert(game_id.to_owned(), recheck.clone());

            let summary = Self::summarize_integrity(&recheck);
            self.set_status(
                game_id,
                json!({
                    "state": if recheck.ok { "repair-complete" } else { "files-damaged" },
                    "percent": 100,
                    "integrity": summary,
                }),
            );

            // «Осталось» — только файлы сборки; стоковые cfg, с которых сняли галочку,
            // интерфейс покажет мягко по integrity (stockMissing, configChanged).
            let stock: HashSet<String> = jv::array(jv::prop(&files, "stockConfigs"))
                .into_iter()
                .flatten()
                .map(|file| jv::template(jv::prop(file, "path")))
                .collect();
            let remaining: Vec<&String> = Self::damaged_paths_of(&recheck)
                .into_iter()
                .filter(|path| !stock.contains(*path))
                .collect();

            let mut out = report.to_json().as_object().cloned().unwrap_or_default();
            out.insert("integrity".to_owned(), summary);
            out.insert("remaining".to_owned(), json!(remaining));
            Ok(Value::Object(out))
        }
        .await;

        let result = result.map_err(|error| {
            let delivery = to_delivery_error(error, Some(Code::RepairFailed));
            // Отмена загрузки ничего не меняла в игре: повреждения те же.
            if delivery.code == Code::Cancelled {
                self.set_status(
                    game_id,
                    json!({
                        "state": if integrity.ok { "files-valid" } else { "files-damaged" },
                        "percent": 100,
                        "integrity": Self::summarize_integrity(&integrity),
                        "error": null,
                    }),
                );
            } else {
                self.set_status(
                    game_id,
                    json!({"state": "error", "error": delivery.to_public()}),
                );
            }
            Failure::from(delivery)
        });

        if let Some(root) = &root {
            self.discard_mirror_source(root);
        }
        self.set_active(None);
        self.set_operation(game_id, None);
        result
    }

    /// `repairChoices(integrity, byPath)`: что можно починить, с размером и видом.
    fn repair_choices(
        integrity: &IntegrityResult,
        by_path: &HashMap<String, &Value>,
    ) -> Vec<Value> {
        let corrupted = integrity.corrupted.iter().map(|item| &item.path);
        let size_mismatch = integrity.size_mismatch.iter().map(|item| &item.path);
        let kinds: [(Vec<&String>, &str); 5] = [
            (integrity.missing.iter().collect(), "missing"),
            (corrupted.collect(), "damaged"),
            (size_mismatch.collect(), "damaged"),
            (integrity.config_missing.iter().collect(), "cfg-missing"),
            (integrity.config_changed.iter().collect(), "cfg-changed"),
        ];
        let mut choices = Vec::new();
        for (list, kind) in kinds {
            for item in list {
                if let Some(entry) = by_path.get(item) {
                    choices.push(json!({
                        "path": item,
                        "size": jv::prop(entry, "size"),
                        "kind": kind,
                    }));
                }
            }
        }
        choices
    }

    /// `repairMirrorFor(gameId, version)`: зеркало ремонта цели последней проверки для этой
    /// версии.
    fn repair_mirror_for(&self, game_id: &str, version: Option<&Value>) -> Option<Value> {
        let last = self.update_checker.last_result(game_id)?;
        let mirror = last
            .target()
            .and_then(|target| jv::prop(target, "repairMirror"))
            .filter(|mirror| js::truthy(mirror))?;
        strict_eq(jv::prop(mirror, "version"), version).then(|| mirror.clone())
    }

    /// `repairMirrorOf(gameId, version)`: зеркало объявляет подписанный канал версии; без него
    /// ремонт не начинается.
    pub(super) async fn repair_mirror_of(
        &self,
        game_id: &str,
        version: Option<&Value>,
    ) -> Result<Value, Failure> {
        if self.update_checker.last_result(game_id).is_none() {
            self.update_checker
                .check(game_id, &CheckOptions::default())
                .await;
        }
        let mirror = self.repair_mirror_for(game_id, version);
        let allowed = self.allowed_provider_types();
        let options = PlanOptions {
            version: version.and_then(Value::as_str),
            allowed_provider_types: &allowed,
        };
        match repair_mirror::unavailable_reason(mirror.as_ref(), &options) {
            Some(reason) => Err(self
                .mirror_unavailable(game_id, version, reason, None)
                .into()),
            None => Ok(mirror.unwrap_or(Value::Null)),
        }
    }

    /// `mirrorUnavailable(gameId, version, plan)`: понятная ошибка — файла нет в зеркале, сначала
    /// обновить игру или переустановить.
    pub(super) fn mirror_unavailable(
        &self,
        game_id: &str,
        version: Option<&Value>,
        reason: &str,
        oversized: Option<&[String]>,
    ) -> DeliveryError {
        let file = oversized.and_then(<[String]>::first);
        // Канал описывает новую версию: обновление заменит и повреждённые файлы.
        let update = file.is_none()
            && self
                .update_checker
                .last_result(game_id)
                .is_some_and(|last| last.flag("updateAvailable"));
        let shown = jv::template(version);

        self.logger.warn(&format!(
            "[game-delivery] ремонт {game_id} {shown} по файлам недоступен: {reason}"
        ));

        let message = match file {
            Some(file) => format!("Файл {file} недоступен в зеркале ремонта"),
            None if update => format!(
                "Восстановление по файлам для версии {shown} недоступно — сначала обнови игру"
            ),
            None => format!(
                "Восстановление по файлам для версии {shown} пока недоступно — переустанови игру"
            ),
        };
        DeliveryError::new(Code::RepairSourceMissing, message).with_details(json!({
            "gameId": game_id,
            "version": version,
            "reason": if update { "UPDATE_AVAILABLE" } else { reason },
            "path": file,
        }))
    }

    /// `fetchFromMirror(gameId, mirror, plan, entries)`: файлы с зеркала во временный каталог;
    /// отмена — `CANCELLED` (`cancel` → `repair_abort`). Каталог — вызывающему.
    pub(super) async fn fetch_from_mirror(
        &self,
        game_id: &str,
        mirror: &Value,
        providers: &[Value],
        total_bytes: f64,
        entries: &[Value],
    ) -> Result<PathBuf, Failure> {
        let stamp = js::base36(self.now_ms().max(0) as u128);
        let root = assert_inside(
            &self.paths.staging,
            &join(&[
                &text(&self.paths.staging),
                &format!("repair-mirror-{game_id}-{stamp}"),
            ]),
            Some("staging зеркала ремонта"),
        )?;
        let signal = CancelHandle::new();

        lock(&self.state).repair_abort = Some(signal.clone());
        self.set_operation(game_id, Some("repair-download"));
        self.set_status(
            game_id,
            json!({
                "state": "preparing",
                "percent": 0,
                "bytesDownloaded": 0,
                "totalBytes": js_number(total_bytes),
                "error": null,
            }),
        );

        let me = self.me.clone();
        let id = game_id.to_owned();
        let on_progress: repair_mirror::ProgressFn = Arc::new(move |progress| {
            if let Some(service) = me.upgrade() {
                service.set_status(
                    &id,
                    json!({
                        "state": "preparing",
                        "percent": js_number(progress.percent),
                        "bytesDownloaded": progress.bytes,
                        "totalBytes": js_number(progress.total_bytes),
                    }),
                );
            }
        });

        let fetched = self
            .repair_mirror
            .fetch_into(
                &root,
                entries,
                &FetchOptions {
                    providers: ManifestService::order_providers(providers),
                    layout: jv::template(jv::prop(mirror, "layout")),
                    signal: Some(signal.clone()),
                    on_progress: Some(on_progress),
                },
            )
            .await
            // Крестик нажат, пока докачивался последний файл: в игру ничего не пишем.
            .and_then(|result| match signal.is_cancelled() {
                true => Err(DeliveryError::new(Code::Cancelled, "Восстановление отменено").into()),
                false => Ok(result),
            });
        lock(&self.state).repair_abort = None;

        match fetched {
            Ok(result) => {
                self.logger.info(&format!(
                    "[game-delivery] с зеркала ремонта получено файлов: {}, {} B ({game_id})",
                    result.files.len(),
                    result.bytes
                ));
                Ok(root)
            }
            Err(error) => {
                self.discard_mirror_source(&root);
                Err(error)
            }
        }
    }

    /// `discardMirrorSource(root)`.
    pub(super) fn discard_mirror_source(&self, root: &Path) {
        if exists(root)
            && let Err(error) = remove_within_zone(&self.paths.staging, root)
        {
            self.logger.warn(&format!(
                "[game-delivery] не удалось убрать {}: {}",
                text(root),
                message_of(&error)
            ));
        }
    }
}
