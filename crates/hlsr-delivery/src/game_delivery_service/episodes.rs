//! Эпизоды Ghosting (задача E, `GameDeliveryService.js` 0.4.9): папки `episodic\` и `ep2\` внутри
//! установленной ghosting-renewed (`episodes.rs`, `episode_install.rs`). Операция идёт под её
//! gameId: прогресс, пауза, отмена и «занято» — как у установки игры.

use super::{GameDeliveryService, PackCancel, code_of, exists, join, lock, message_of, text};
use crate::archive_service::{ExtractOptions, FractionFn, InspectArgs};
use crate::episode_install::{self, GAME_ID};
use crate::episodes::{self, EPISODES};
use crate::errors::{Code, DeliveryError, Failure, to_delivery_error};
use crate::file_ops;
use crate::game_download_manager::{DownloadOutcome, DownloadRequest};
use crate::game_pack;
use crate::js_value as jv;
use crate::paths::{assert_inside, remove_within_zone};
use crate::providers::{self, ProviderOptions};
use crate::semver;
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Манифест эпизода в памяти — 10 минут, отказ сервера помнится минуту.
const EPISODE_MANIFEST_TTL_MS: f64 = 10.0 * 60.0 * 1000.0;
const EPISODE_MANIFEST_RETRY_MS: f64 = 60.0 * 1000.0;
/// Отказы, после которых архив эпизода доказанно плохой и удаляется.
const EPISODE_ARCHIVE_BAD: [&str; 5] = [
    "CHECKSUM_MISMATCH",
    "ARCHIVE_UNSAFE_ENTRY",
    "ARCHIVE_LIMIT_EXCEEDED",
    "EXTRACT_FAILED",
    "MANIFEST_INVALID",
];

fn install_path_of(installed: &Value) -> PathBuf {
    PathBuf::from(jv::template(installed.get("installPath")))
}

impl GameDeliveryService {
    /// `refresh`: у Ghosting — `episodes` в статусе (`episodesView`), загрузка эпизода не подменяет
    /// `totalBytes`, `episode` — только у идущей операции.
    pub(super) fn patch_episodes(
        &self,
        game_id: &str,
        installed: Option<&Value>,
        patch: &mut Map<String, Value>,
    ) {
        if game_id != GAME_ID {
            return;
        }
        patch.insert("episodes".to_owned(), self.episodes_view(installed));
        let state = lock(&self.state);
        // Идёт загрузка эпизода: её объём не подменять объёмом архива Ghosting.
        if state.operations.get(game_id).map(String::as_str) == Some("episode-install") {
            patch.shift_remove("totalBytes");
        }
        if state.active_game_id.as_deref() != Some(game_id) {
            patch.insert("episode".to_owned(), Value::Null);
        }
    }

    /// `cancel`: эпизод на паузе или его архив, оставленный отказом (Ghosting при этом в «ready»).
    /// `episode` из карточки эпизода — только его архив; без него — архивы всех. `true` — что-то
    /// убрано.
    pub(super) fn discard_episode_archives(
        &self,
        game_id: &str,
        episode: Option<&str>,
    ) -> Result<bool, Failure> {
        if game_id != GAME_ID {
            return Ok(false);
        }
        let episode = episode.filter(|id| episodes::is_episode(id));
        let removed: usize = EPISODES
            .iter()
            .map(|each| each.id)
            .filter(|id| episode.is_none_or(|only| only == *id))
            .map(|id| self.discard_stale_episode_archives(id, None))
            .sum();
        if removed == 0 {
            return Ok(false);
        }
        self.refresh(game_id)?;
        Ok(true)
    }

    /// `returnEpisodeConfigs(gameId, ids)`: `config.cfg` эпизодов (`ids`, по умолчанию все с
    /// отметкой) — в Ghosting, если она не запущена. Зовут на выходе эпизода (событие процесса
    /// `finished` с `episode`) и после сверки процессов. Не бросает.
    pub fn return_episode_configs(&self, game_id: &str, ids: Option<&[&str]>) {
        if game_id != GAME_ID || self.processes.is_running(game_id) {
            return;
        }
        match self.install_service.read_installed(game_id) {
            Ok(Some(installed)) => {
                episodes::return_pending(&install_path_of(&installed), &*self.logger, ids)
            }
            Ok(None) => {}
            Err(error) => self.logger.warn(&format!(
                "[game-delivery] config.cfg эпизодов не вернулся: {}",
                error.message
            )),
        }
    }

    /// `fetchEpisodeManifest(id, {fresh})`: манифест и его адрес из памяти (10 минут) или с
    /// сервера; отказ помнится минуту.
    async fn fetch_episode_manifest(
        &self,
        id: &str,
        fresh: bool,
    ) -> Result<(Value, String), DeliveryError> {
        let now = self.now_ms() as f64;
        if !fresh {
            let state = lock(&self.state);
            if let Some(cached) = state.episode_manifests.get(id)
                && now - jv::number(&cached["fetchedAt"]).unwrap_or(f64::NAN)
                    < EPISODE_MANIFEST_TTL_MS
            {
                return Ok((cached["manifest"].clone(), jv::template(cached.get("url"))));
            }
            if let Some((error, at)) = state.episode_manifest_failures.get(id)
                && now - at < EPISODE_MANIFEST_RETRY_MS
            {
                return Err(error.clone());
            }
        }
        match episode_install::fetch_manifest(id, &self.env, &self.http).await {
            Ok((manifest, url)) => {
                let mut state = lock(&self.state);
                state.episode_manifests.insert(
                    id.to_owned(),
                    json!({ "manifest": manifest, "url": url, "fetchedAt": js_number(now) }),
                );
                state.episode_manifest_failures.remove(id);
                Ok((manifest, url))
            }
            Err(failure) => {
                let code = code_of(&failure).unwrap_or_default();
                let error = to_delivery_error(failure, Some(Code::InstallFailed));
                self.logger.warn(&format!(
                    "[game-delivery] манифест эпизода {id} недоступен: {code} {}",
                    error.message
                ));
                lock(&self.state)
                    .episode_manifest_failures
                    .insert(id.to_owned(), (error.clone(), now));
                Err(error)
            }
        }
    }

    fn episode_downloads(&self) -> PathBuf {
        join(&[&text(&self.paths.downloads), GAME_ID])
    }

    /// `pausedEpisodes()`: загрузки эпизодов на паузе — по файлам состояния `.part.json` в
    /// Downloads (кэш манифестов после перезапуска лаунчера пуст): id → скачано байт.
    fn paused_episodes(&self) -> HashMap<&'static str, u64> {
        let mut paused = HashMap::new();
        let Ok(names) = js::fs::read_dir_names(&self.episode_downloads()) else {
            return paused;
        };
        for name in names {
            let Some(file_name) = name.strip_suffix(".part.json") else {
                continue;
            };
            let Some(id) = episode_install::episode_of_archive(file_name) else {
                continue;
            };
            if let Ok(Some(resumable)) = self.download_manager.find_resumable(GAME_ID, file_name) {
                paused.insert(id, resumable.bytes_downloaded);
            }
        }
        paused
    }

    /// `discardStaleEpisodeArchives(id, keepFileName)`: архивы эпизода в Downloads (`.part` с
    /// состоянием или целый, оставленный отказом), кроме `keep`: прежняя версия, отмена, удаление
    /// эпизода. Сколько файлов убрано.
    fn discard_stale_episode_archives(&self, id: &str, keep: Option<&str>) -> usize {
        let dir = self.episode_downloads();
        let Ok(names) = js::fs::read_dir_names(&dir) else {
            return 0;
        };
        let mut removed = 0;
        for name in names {
            let base = name
                .strip_suffix(".part.json")
                .or_else(|| name.strip_suffix(".part"))
                .unwrap_or(&name);
            if Some(base) == keep || episode_install::episode_of_archive(base) != Some(id) {
                continue;
            }
            match remove_within_zone(&self.paths.downloads, &dir.join(&name)) {
                Ok(()) => removed += 1,
                Err(error) => self.logger.warn(&format!(
                    "[game-delivery] не удалось удалить архив эпизода {name}: {}",
                    message_of(&error)
                )),
            }
        }
        removed
    }

    /// `keptEpisodeArchive(archive)`: архив, оставленный прошлой попыткой (игра была запущена, не
    /// хватило места), — без второй загрузки.
    async fn kept_episode_archive(
        &self,
        file_name: &str,
        size: u64,
        sha256: &str,
    ) -> Result<Option<PathBuf>, Failure> {
        let file = self.episode_downloads().join(file_name);
        if !std::fs::metadata(&file).is_ok_and(|meta| meta.len() == size) {
            return Ok(None);
        }
        self.set_status(GAME_ID, json!({"state": "verifying", "percent": 0}));
        Ok((file_ops::hash_file(&file).await? == sha256).then_some(file))
    }

    /// `episodesView(installed)`: `{ep1: {installed, version, size, paused}, …}`.
    fn episodes_view(&self, installed: Option<&Value>) -> Value {
        let install_path = installed.map(install_path_of);
        let state = episodes::status(install_path.as_deref());
        let paused = self.paused_episodes();
        let manifests = lock(&self.state).episode_manifests.clone();
        let view: Map<String, Value> = state
            .into_iter()
            .map(|(id, mut entry)| {
                let size = manifests.get(&id).map_or(Value::Null, |cached| {
                    cached["manifest"]["archive"]["size"].clone()
                });
                entry["size"] = size;
                entry["paused"] = json!(paused.get(id.as_str()).is_some_and(|bytes| *bytes > 0));
                (id, entry)
            })
            .collect();
        Value::Object(view)
    }

    /// `episodesStatus()` для «Версии сборки»: не бросает, недоступный манифест — `size: null`,
    /// `available: false`.
    pub async fn episodes_status(&self) -> Value {
        let (first, second) = tokio::join!(
            self.fetch_episode_manifest(EPISODES[0].id, false),
            self.fetch_episode_manifest(EPISODES[1].id, false)
        );
        let available = [first.is_ok(), second.is_ok()];
        let installed = self.read_installed(GAME_ID).ok().flatten();
        let mut view = self.episodes_view(installed.as_ref());
        for (episode, available) in EPISODES.iter().zip(available) {
            view[episode.id]["available"] = json!(available);
        }
        let _ = self.refresh(GAME_ID);
        view
    }

    /// `installEpisode(id)`: Episode Two без Episode One сначала ставит его, в том же слоте.
    pub async fn install_episode(&self, id: &str) -> Result<Value, Failure> {
        let id = episodes::episode_of(id)?.id;
        let slot = self.reserve_slot(GAME_ID, "episode-install")?;

        let result: Result<Value, Failure> = async {
            self.assert_game_not_running(GAME_ID)?;
            let installed = self.installed_of(GAME_ID)?;
            let install_path = install_path_of(&installed);
            self.ensure_layout()?;
            self.set_active(Some(GAME_ID));
            self.set_operation(GAME_ID, Some("episode-install"));

            let mut result = Value::Null;
            for each in episodes::episode_of(id)?
                .requires
                .iter()
                .copied()
                .chain([id])
            {
                if each != id
                    && jv::truthy(episodes::status(Some(&install_path))[each].get("installed"))
                {
                    continue;
                }
                result = self.run_episode_install(each, &install_path).await?;
                if result.get("status").and_then(Value::as_str) != Some("installed") {
                    return Ok(result);
                }
            }
            Ok(result)
        }
        .await;

        let result = result.map_err(|failure| {
            let delivery = to_delivery_error(failure, Some(Code::InstallFailed));
            if self.active_game_id().as_deref() == Some(GAME_ID) {
                let state = if delivery.code == Code::Cancelled {
                    "cancelled"
                } else {
                    "error"
                };
                self.set_status(
                    GAME_ID,
                    json!({"state": state, "error": delivery.to_public()}),
                );
            }
            Failure::from(delivery)
        });
        {
            let mut state = lock(&self.state);
            state.pack_cancel = None;
            if state.active_game_id.as_deref() == Some(GAME_ID) {
                state.active_game_id = None;
            }
        }
        self.set_operation(GAME_ID, None);
        drop(slot);
        // Ошибка эпизода уходит в ответ IPC; в статусе Ghosting она читалась бы как беда самой игры.
        if let Some(status) = lock(&self.state).statuses.get_mut(GAME_ID) {
            status.insert("error".to_owned(), Value::Null);
        }
        self.refresh(GAME_ID)?;
        result
    }

    /// `runEpisodeInstall(id, installPath)`: манифест, загрузка (или оставленный архив), проверка
    /// записей с `rootPrefix`, распаковка в staging, подмена папки. Архив удаляется только после
    /// установки или когда он доказанно плохой.
    async fn run_episode_install(&self, id: &str, install_path: &Path) -> Result<Value, Failure> {
        let dir = episodes::episode_of(id)?.dir;
        self.set_status(
            GAME_ID,
            json!({"state": "preparing", "episode": id, "percent": 0, "bytesDownloaded": 0, "error": null}),
        );

        let (manifest, url) = self.fetch_episode_manifest(id, true).await?;
        let offered = jv::template(manifest.get("version"));
        // Повтор старого подписанного манифеста не должен откатить эпизод; та же версия — переустановка.
        if let Some(installed) = episodes::read_marker(install_path, id)
            .get("version")
            .and_then(Value::as_str)
            && semver::lt(&offered, installed)
        {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                format!("Откат версии эпизода {id}: {installed} -> {offered}"),
            )
            .with_details(json!({"id": id, "installed": installed, "offered": offered}))
            .into());
        }

        let archive = &manifest["archive"];
        let file_name = jv::template(archive.get("fileName"));
        let size = archive["size"].as_f64().unwrap_or(0.0) as u64;
        let sha256 = jv::template(archive.get("sha256"));
        self.discard_stale_episode_archives(id, Some(&file_name));
        self.set_status(GAME_ID, json!({"totalBytes": size}));
        let archive_path = match self.kept_episode_archive(&file_name, size, &sha256).await? {
            Some(path) => path,
            None => {
                let options = ProviderOptions::with_allowed_provider_types(
                    self.http.clone(),
                    self.allowed_provider_types(),
                );
                let descriptor = episode_install::provider_descriptor(&manifest, &url)?;
                let provider = providers::create(&descriptor, &options)?;
                let download = self
                    .download_manager
                    .start(DownloadRequest {
                        game_id: GAME_ID.to_owned(),
                        provider: Arc::from(provider),
                        filename: file_name.clone(),
                        expected_size: Some(size),
                        expected_sha256: Some(sha256.clone()),
                    })
                    .await?;
                match download {
                    DownloadOutcome::Completed { file_path, .. } => file_path,
                    other => return Ok(other.to_json()),
                }
            }
        };

        let staging = assert_inside(
            &self.paths.staging,
            &join(&[
                &text(&self.paths.staging),
                &format!(
                    "{GAME_ID}-episode-{id}-{}",
                    crate::install_service::random_hex4()
                ),
            ]),
            Some("Папка эпизода"),
        )?;
        let mut locked = false;
        let result: Result<(), Failure> = async {
            self.install_service.check_writable()?;
            // Каждая запись — под <dir>/: ни .., ни абсолютных путей, ни второго корня — до распаковки.
            let inspection = self
                .archive_service
                .inspect(
                    &archive_path,
                    &InspectArgs {
                        max_entries: Some(game_pack::MAX_ENTRIES as f64),
                        max_uncompressed_size: Some(game_pack::MAX_UNCOMPRESSED_BYTES as f64),
                        root_prefix: Some(dir.to_owned()),
                    },
                )
                .await?;
            let uncompressed = inspection.report.uncompressed_size;
            self.install_service
                .check_space(if uncompressed != 0.0 && !uncompressed.is_nan() {
                    uncompressed
                } else {
                    1.0
                })?;
            self.install_service.acquire_lock(GAME_ID)?;
            locked = true;

            crate::node_fs::mkdir_recursive(&staging)?;
            self.set_status(GAME_ID, json!({"state": "extracting", "percent": 0}));
            let me = self.me.clone();
            let on_progress: FractionFn = Arc::new(move |fraction: f64| {
                if let Some(service) = me.upgrade() {
                    service.set_status(
                        GAME_ID,
                        json!({"percent": js_number(js::math_round(fraction * 100.0))}),
                    );
                }
            });
            let job = self.archive_service.extract(
                &archive_path,
                &staging,
                ExtractOptions {
                    on_progress: Some(on_progress),
                },
            );
            lock(&self.state).pack_cancel = Some(PackCancel::Archive(job.cancel_handle()));
            job.await?;
            lock(&self.state).pack_cancel = None;

            let fresh = staging.join(dir);
            if !episodes::has_gameinfo(&fresh) {
                return Err(DeliveryError::new(
                    Code::ExtractFailed,
                    format!("В архиве эпизода нет {dir}/gameinfo.txt"),
                )
                .with_details(json!({"id": id}))
                .into());
            }

            self.set_status(GAME_ID, json!({"state": "finalizing", "percent": 100}));
            self.assert_game_not_running(GAME_ID)?;
            // config.cfg прошлого запуска эпизода, не вернувшийся из-за краха, — в Ghosting до подмены папки.
            episodes::sync_config_back(install_path, id)?;
            // Хвост прошлой подмены, прерванной крахом: данные игрока из park — в эпизод, а не в .stale.
            self.install_service.unpark_episodes(GAME_ID, install_path);
            let stock_cfg = episode_install::stock_cfg_of(&fresh)?;
            let marker = json!({
                "version": offered,
                "sha256": sha256,
                "installedAt": self.now_iso(),
                "stockCfg": stock_cfg,
            });
            episode_install::swap_in(&episode_install::Swap {
                install_path,
                id,
                fresh: &fresh,
                park: &self.install_service.episodes_park_path(GAME_ID),
                marker: &marker,
                now_ms: self.now_ms(),
                logger: &*self.logger,
            })
        }
        .await;
        // Архив эпизода — гигабайты: после установки или когда он доказанно плохой.
        let drop_archive = match &result {
            Ok(()) => true,
            Err(error) => {
                code_of(error).is_some_and(|code| EPISODE_ARCHIVE_BAD.contains(&code.as_str()))
            }
        };

        lock(&self.state).pack_cancel = None;
        if locked {
            self.install_service.release_lock();
        }
        if exists(&staging)
            && let Err(error) = remove_within_zone(&self.paths.staging, &staging)
        {
            self.logger.warn(&format!(
                "[game-delivery] не удалось убрать {}: {}",
                text(&staging),
                message_of(&error)
            ));
        }
        if drop_archive {
            if let Err(error) = self.download_manager.discard(GAME_ID, &file_name) {
                self.logger.warn(&format!(
                    "[game-delivery] не удалось удалить архив эпизода {file_name}: {}",
                    message_of(&error)
                ));
            }
            self.remove_archive(GAME_ID, &file_name);
        }
        result?;

        self.logger.info(&format!(
            "[game-delivery] {GAME_ID}: эпизод {id} {offered} установлен в {dir}"
        ));
        Ok(json!({"status": "installed", "episode": id, "version": offered}))
    }

    /// `removeEpisode(id)`: удаляет эпизод и зависимые (ep1 → ep2); их бинды, не вернувшиеся из-за
    /// краха, — сначала в Ghosting.
    pub fn remove_episode(&self, id: &str) -> Result<Value, Failure> {
        let id = episodes::episode_of(id)?.id;
        let slot = self.reserve_slot(GAME_ID, "episode-remove")?;
        let result = (|| -> Result<Value, Failure> {
            self.assert_game_not_running(GAME_ID)?;
            let install_path = install_path_of(&self.installed_of(GAME_ID)?);
            for each in episodes::dependents_of(id).into_iter().chain([id]) {
                if let Err(error) = episodes::sync_config_back(&install_path, each) {
                    self.logger.warn(&format!(
                        "[game-delivery] config.cfg эпизода {each} не вернулся: {}",
                        message_of(&error)
                    ));
                }
                self.discard_stale_episode_archives(each, None);
            }
            let removed = episodes::remove(&install_path, id)?;
            let shown = if removed.is_empty() {
                "—".to_owned()
            } else {
                removed.join(", ")
            };
            self.logger.info(&format!(
                "[game-delivery] {GAME_ID}: удалены эпизоды {shown}"
            ));
            Ok(json!({"status": "removed", "removed": removed}))
        })();
        drop(slot);
        self.refresh(GAME_ID)?;
        result
    }
}
