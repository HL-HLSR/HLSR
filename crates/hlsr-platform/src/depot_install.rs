//! `game-delivery/depot-install.js`: установка сборки из депо Steam. Депо скачиваются в
//! временную папку (`.hlsr/staging/<id>-depot-<время>`), сливаются в одну (`content`), проверяются
//! обязательные файлы, дописываются `autoexec.cfg`, `steam_appid.txt` и ключ реестра `DXLevel_V1`
//! (скрипт speedrunsetup), затем сборка заменяет прежнюю в `Games/<id>` с переносом файлов игрока
//! (сейвы, конфиги, демо). Итог — запись `installed.json`.

use crate::steam_depot::{
    DownloadOptions, DownloadProgress, Downloaded, ProgressFn, SteamDepotService,
};
use hlsr_core::js;
use hlsr_core::logger::{Logger, TracingLogger};
use hlsr_delivery::atomic::write_json_atomic;
use hlsr_delivery::errors::{Code, DeliveryError, Failure, RawError, preserve_error};
use hlsr_delivery::manifests;
use hlsr_delivery::mutable_paths::{LIBRARY_INTERNALS, USER_DATA};
use hlsr_delivery::path_match::Matcher;
use hlsr_delivery::paths::{LibraryPaths, assert_inside, game_install_path, remove_within_zone};
use hlsr_delivery::player_config;
use hlsr_delivery::ports::{DepotProgress, DepotProgressFn, SteamDepot};
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::timing::{Clock, iso_from_ms, system_clock};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const INSTALLED_FILE: &str = "installed.json";
pub const INSTALLED_SCHEMA_VERSION: u64 = 1;

/// Без этих файлов сборка не сборка.
pub const REQUIRED_FILES: [&str; 2] = ["hl2.exe", "hl2/gameinfo.txt"];

/// `DXLevel_V1`, который ставит speedrunsetup.
pub const SPEEDRUNSETUP_DXLEVEL: u32 = 0x5f;

/// Ключ реестра, в который пишется `DXLevel_V1`.
pub const DXLEVEL_KEY: &str = "HKCU\\Software\\Valve\\Source\\hl2\\Settings";

/// `setRegistryValue(keyPath, valueName, value)`: записать DWORD. Ошибка — текст для журнала.
pub trait RegistryWriter: Send + Sync {
    fn set_dword(&self, key_path: &str, value_name: &str, value: u32) -> Result<(), String>;
}

/// Скачивание депо сборки (`depotService.download`).
pub trait DepotDownloader: Send + Sync {
    fn download<'a>(
        &'a self,
        build_id: &'a str,
        target_dir: &'a str,
        library_root: &'a str,
        on_progress: Option<ProgressFn<'a>>,
    ) -> BoxFuture<'a, Result<Downloaded, Failure>>;
}

impl DepotDownloader for SteamDepotService {
    fn download<'a>(
        &'a self,
        build_id: &'a str,
        target_dir: &'a str,
        library_root: &'a str,
        on_progress: Option<ProgressFn<'a>>,
    ) -> BoxFuture<'a, Result<Downloaded, Failure>> {
        Box::pin(SteamDepotService::download(
            self,
            build_id,
            target_dir,
            DownloadOptions {
                library_root: Some(library_root),
                on_progress,
            },
        ))
    }
}

fn io_failure(error: &std::io::Error, syscall: &str, path: &Path) -> Failure {
    RawError::fs(error, syscall, path).into()
}

fn rename_failure(error: &std::io::Error, from: &Path, to: &Path) -> Failure {
    RawError::fs_with_dest(error, "rename", from, Some(to)).into()
}

/// `mergeInto(source, target)`: перенести файлы `source` в `target` (ссылки пропускаются);
/// число перенесённых. Нечитаемая `source` — 0.
pub fn merge_into(source: &Path, target: &Path) -> Result<u64, Failure> {
    let Ok(entries) = std::fs::read_dir(source) else {
        return Ok(0);
    };
    let entries: Vec<_> = entries.flatten().collect();

    let mut moved = 0;
    std::fs::create_dir_all(target).map_err(|e| io_failure(&e, "mkdir", target))?;

    for entry in entries {
        let from = entry.path();
        let to = target.join(entry.file_name());
        let Ok(kind) = entry.file_type() else {
            continue;
        };

        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            moved += merge_into(&from, &to)?;
            continue;
        }

        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_failure(&e, "mkdir", parent))?;
        }
        std::fs::rename(&from, &to).map_err(|e| rename_failure(&e, &from, &to))?;
        moved += 1;
    }
    Ok(moved)
}

/// `existsInsensitive(root, relative)`: путь из частей через `/` есть без учёта регистра.
pub fn exists_insensitive(root: &Path, relative: &str) -> bool {
    let mut current = root.to_path_buf();
    for part in relative.split('/') {
        let Ok(entries) = std::fs::read_dir(&current) else {
            return false;
        };
        let wanted = part.to_lowercase();
        let found = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .find(|name| name.to_lowercase() == wanted);
        match found {
            Some(name) => current = current.join(name),
            None => return false,
        }
    }
    true
}

fn failure_message(failure: &Failure) -> String {
    match failure {
        Failure::Delivery(error) => error.message.clone(),
        Failure::Raw(error) => error.message.clone(),
    }
}

/// Параметры конструктора (`options`).
pub struct InstallerOptions {
    pub paths: LibraryPaths,
    pub depot_service: Arc<dyn DepotDownloader>,
    pub logger: Option<Arc<dyn Logger>>,
    pub set_registry_value: Option<Arc<dyn RegistryWriter>>,
    /// `now()`: время для `installedAt` (строка ISO).
    pub now: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    /// `Date.now()` для имени папки staging.
    pub clock: Option<Clock>,
}

/// `DepotInstaller`.
pub struct DepotInstaller {
    paths: LibraryPaths,
    depot_service: Arc<dyn DepotDownloader>,
    logger: Arc<dyn Logger>,
    set_registry_value: Option<Arc<dyn RegistryWriter>>,
    now: Arc<dyn Fn() -> String + Send + Sync>,
    clock: Clock,
}

impl DepotInstaller {
    pub fn new(options: InstallerOptions) -> DepotInstaller {
        DepotInstaller {
            paths: options.paths,
            depot_service: options.depot_service,
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            set_registry_value: options.set_registry_value,
            now: options
                .now
                .unwrap_or_else(|| Arc::new(|| iso_from_ms(system_clock()()))),
            clock: options.clock.unwrap_or_else(system_clock),
        }
    }

    /// `stagingFor(gameId)`.
    fn staging_for(&self, game_id: &str) -> Result<PathBuf, DeliveryError> {
        let name = format!(
            "{game_id}-depot-{}",
            js::base36((self.clock)().max(0) as u128)
        );
        assert_inside(
            &self.paths.staging,
            &self.paths.staging.join(name),
            Some("Папка staging"),
        )
    }

    /// `install(gameId, options)`: записанный `installed.json`.
    pub async fn install(
        &self,
        game_id: &str,
        version: Option<&str>,
        on_progress: Option<DepotProgressFn>,
    ) -> Result<Value, Failure> {
        let install_path = game_install_path(&self.paths, game_id)?;
        let staging = self.staging_for(game_id)?;

        std::fs::create_dir_all(&staging).map_err(|e| io_failure(&e, "mkdir", &staging))?;

        let result = self
            .install_from(game_id, version, on_progress, &install_path, &staging)
            .await;

        if staging.exists()
            && let Err(error) = remove_within_zone(&self.paths.staging, &staging)
        {
            self.logger.warn(&format!(
                "[depot-install] staging не убран: {}",
                failure_message(&error)
            ));
        }
        result
    }

    async fn install_from(
        &self,
        game_id: &str,
        version: Option<&str>,
        on_progress: Option<DepotProgressFn>,
        install_path: &Path,
        staging: &Path,
    ) -> Result<Value, Failure> {
        let forward = on_progress.map(|callback| {
            move |progress: &DownloadProgress| {
                callback(&DepotProgress {
                    files: progress.files,
                    expected_files: progress.expected_files,
                    index: progress.index,
                    total: progress.total,
                });
            }
        });
        let forward_ref: Option<ProgressFn<'_>> =
            forward.as_ref().map(|callback| callback as ProgressFn<'_>);

        let downloaded = self
            .depot_service
            .download(
                game_id,
                &staging.to_string_lossy(),
                &self.paths.root.to_string_lossy(),
                forward_ref,
            )
            .await?;

        let content = staging.join("content");
        let mut files = 0;
        for (_, dir) in &downloaded.depots {
            files += merge_into(Path::new(dir), &content)?;
        }

        self.logger.info(&format!(
            "[depot-install] {game_id}: собрано {files} файлов из {} депотов",
            downloaded.depots.len()
        ));

        for relative in REQUIRED_FILES {
            if !exists_insensitive(&content, relative) {
                return Err(DeliveryError::new(
                    Code::RequiredFileMissing,
                    format!("Steam не отдал обязательный файл сборки: {relative}"),
                )
                .with_details(json!({"gameId": game_id, "file": relative}))
                .into());
            }
        }

        self.apply_post_install(&content, game_id);

        if let Some(parent) = install_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_failure(&e, "mkdir", parent))?;
        }

        let mut previous: Option<PathBuf> = None;
        if install_path.exists() {
            let mut name = install_path.as_os_str().to_owned();
            name.push(".hlsr-backup");
            let backup = PathBuf::from(name);
            if backup.exists() {
                remove_within_zone(&self.paths.games, &backup)?;
            }

            self.preserve_user_data(install_path, &content, game_id)?;

            std::fs::rename(install_path, &backup)
                .map_err(|e| rename_failure(&e, install_path, &backup))?;
            previous = Some(backup);
        }

        if let Err(error) = std::fs::rename(&content, install_path) {
            if let Some(previous) = &previous {
                std::fs::rename(previous, install_path)
                    .map_err(|e| rename_failure(&e, previous, install_path))?;
            }
            return Err(rename_failure(&error, &content, install_path));
        }

        let installed = json!({
            "schemaVersion": INSTALLED_SCHEMA_VERSION,
            "gameId": game_id,
            "version": version.filter(|v| !v.is_empty()).unwrap_or("1.0.0"),
            "installedAt": (self.now)(),
            "publishedAt": null,
            "manifestSha256": null,
            "installPath": install_path.to_string_lossy(),
            "archiveSha256": null,
            "source": "steam-depot",
            "state": "ready",
        });

        write_json_atomic(&install_path.join(INSTALLED_FILE), &installed)?;

        if let Some(previous) = &previous
            && let Err(error) = remove_within_zone(&self.paths.games, previous)
        {
            self.logger.warn(&format!(
                "[depot-install] прежняя версия осталась на диске: {}",
                failure_message(&error)
            ));
        }

        self.logger.info(&format!(
            "[depot-install] {game_id} установлена из депотов Steam"
        ));

        Ok(installed)
    }

    /// `preserveUserData(installPath, content, gameId)`: файлы игрока старой установки — в новую.
    /// Как `InstallService`: не скопировался хоть один — прерываемся до подмены папок, пока
    /// прежняя сборка цела (`preserveError`).
    fn preserve_user_data(
        &self,
        install_path: &Path,
        content: &Path,
        game_id: &str,
    ) -> Result<(), DeliveryError> {
        let matches = Matcher::new(USER_DATA);
        let skip_dir = Matcher::new(LIBRARY_INTERNALS);
        let mut copied = 0u64;

        Self::walk_user_data(install_path, "", content, &matches, &skip_dir, &mut copied)
            .map_err(preserve_error)?;

        if copied > 0 {
            self.logger.info(&format!(
                "[depot-install] {game_id}: перенесено пользовательских файлов {copied}"
            ));
        }
        Ok(())
    }

    fn walk_user_data(
        dir: &Path,
        relative: &str,
        content: &Path,
        matches: &Matcher,
        skip_dir: &Matcher,
        copied: &mut u64,
    ) -> Result<(), Failure> {
        // Строже 0.4.9 (там папка без доступа пропускалась): её файлы игрока пропали бы вместе
        // с прежней сборкой — прерываемся до подмены папок.
        let entries = std::fs::read_dir(dir).map_err(|e| RawError::fs(&e, "scandir", dir))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };

            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !skip_dir.matches(&rel) {
                    Self::walk_user_data(
                        &dir.join(&name),
                        &rel,
                        content,
                        matches,
                        skip_dir,
                        copied,
                    )?;
                }
                continue;
            }
            if !matches.matches(&rel) {
                continue;
            }

            let mut destination = content.to_path_buf();
            for part in rel.split('/') {
                destination.push(part);
            }
            let target = assert_inside(
                content,
                &destination,
                Some(&format!("Пользовательский файл \"{rel}\"")),
            )?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| RawError::fs(&e, "mkdir", parent))?;
            }
            let source = dir.join(&name);
            std::fs::copy(&source, &target)
                .map_err(|e| RawError::fs_with_dest(&e, "copyfile", &source, Some(&target)))?;
            *copied += 1;
        }
        Ok(())
    }

    /// `applyPostInstall(content, gameId)`.
    fn apply_post_install(&self, content: &Path, game_id: &str) {
        let cfg_dir = content.join("hl2").join("cfg");

        let autoexec = std::fs::create_dir_all(&cfg_dir)
            .map_err(|e| RawError::fs(&e, "mkdir", &cfg_dir).message)
            .and_then(|()| {
                let file = cfg_dir.join("autoexec.cfg");
                std::fs::write(
                    &file,
                    player_config::template(game_id, js::Platform::CURRENT),
                )
                .map_err(|e| RawError::fs(&e, "open", &file).message)
            });
        if let Err(message) = autoexec {
            self.logger.warn(&format!(
                "[depot-install] autoexec.cfg не записан: {message}"
            ));
        }

        if let Some(launch) = manifests::launch_config_of(game_id)
            && launch.steam_app_id != 0
        {
            let file = content.join("steam_appid.txt");
            if let Err(error) = std::fs::write(&file, format!("{}\n", launch.steam_app_id)) {
                self.logger.warn(&format!(
                    "[depot-install] steam_appid.txt не записан: {}",
                    RawError::fs(&error, "open", &file).message
                ));
            }
        }

        let Some(registry) = &self.set_registry_value else {
            return;
        };
        if let Err(message) = registry.set_dword(DXLEVEL_KEY, "DXLevel_V1", SPEEDRUNSETUP_DXLEVEL) {
            self.logger.warn(&format!(
                "[depot-install] ключ DXLevel не установлен: {message}"
            ));
        }
    }
}

// ------------------------------------------------------------------------ реестр Windows

/// Запись DWORD в реестр: Windows — `RegCreateKeyExW` + `RegSetValueExW` (то же, что
/// `reg.exe add … /f /v … /t REG_DWORD /d …` 0.4.7, без запуска программы); на других ОС реестра
/// нет, и запись ничего не делает (0.4.7 возвращал `false`, и установка этого не замечала).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRegistryWriter;

impl RegistryWriter for SystemRegistryWriter {
    #[cfg(windows)]
    fn set_dword(&self, key_path: &str, value_name: &str, value: u32) -> Result<(), String> {
        windows_registry::set_dword_at_path(key_path, value_name, value)
    }

    #[cfg(not(windows))]
    fn set_dword(&self, _key_path: &str, _value_name: &str, _value: u32) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(windows)]
pub mod windows_registry {
    use hlsr_core::win32::wide;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_DWORD,
        REG_OPTION_NON_VOLATILE, RegCloseKey, RegCreateKeyExW, RegSetValueExW,
    };

    struct Handle(HKEY);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: дескриптор получен от RegCreateKeyExW и закрывается один раз.
            unsafe { RegCloseKey(self.0) };
        }
    }

    /// `HKCU\Software\…` / `HKLM\…` / полные имена кустов → (куст, подключ).
    pub fn split_hive(key_path: &str) -> Option<(HKEY, &str)> {
        let (hive, rest) = key_path.split_once('\\')?;
        let root = match hive.to_ascii_uppercase().as_str() {
            "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
            "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
            _ => return None,
        };
        Some((root, rest))
    }

    /// Записать DWORD `name` в ключ `subkey` под `root` (ключ создаётся, если его нет).
    ///
    /// # Safety
    /// `root` — открытый ключ реестра (куст или ключ с правом создавать подключи).
    pub unsafe fn set_dword_at(
        root: HKEY,
        subkey: &str,
        name: &str,
        value: u32,
    ) -> Result<(), String> {
        let wide_key = wide(subkey);
        let wide_name = wide(name);
        let mut raw: HKEY = std::ptr::null_mut();
        // SAFETY: строки завершены нулём, `raw` — локальная переменная для дескриптора.
        let status = unsafe {
            RegCreateKeyExW(
                root,
                wide_key.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut raw,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("RegCreateKeyExW: код {status}"));
        }
        let handle = Handle(raw);
        let bytes = value.to_le_bytes();
        // SAFETY: данные — четыре байта DWORD, длина передана.
        let status = unsafe {
            RegSetValueExW(
                handle.0,
                wide_name.as_ptr(),
                0,
                REG_DWORD,
                bytes.as_ptr(),
                bytes.len() as u32,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("RegSetValueExW: код {status}"));
        }
        Ok(())
    }

    pub fn set_dword_at_path(key_path: &str, name: &str, value: u32) -> Result<(), String> {
        let (root, subkey) =
            split_hive(key_path).ok_or_else(|| format!("неизвестный куст реестра: {key_path}"))?;
        // SAFETY: `root` — предопределённый куст из `split_hive`.
        unsafe { set_dword_at(root, subkey, name, value) }
    }
}

// --------------------------------------------------------------------- порт SteamDepot

/// `SteamDepot` для `GameDeliveryService`: лицензии и установка сборки из депо (то, что
/// `depotService` и `DepotInstaller` дают службе доставки 0.4.7).
pub struct SteamDepotBridge {
    service: Arc<SteamDepotService>,
    logger: Arc<dyn Logger>,
    registry: Option<Arc<dyn RegistryWriter>>,
}

impl SteamDepotBridge {
    pub fn new(
        service: Arc<SteamDepotService>,
        logger: Option<Arc<dyn Logger>>,
        registry: Option<Arc<dyn RegistryWriter>>,
    ) -> SteamDepotBridge {
        SteamDepotBridge {
            service,
            logger: logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            registry,
        }
    }

    pub fn service(&self) -> &Arc<SteamDepotService> {
        &self.service
    }
}

impl SteamDepot for SteamDepotBridge {
    fn assert_owns<'a>(&'a self, app_ids: &'a [u32]) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move { Ok(self.service.assert_owns(app_ids).await?) })
    }

    fn install<'a>(
        &'a self,
        paths: &'a LibraryPaths,
        game_id: &'a str,
        on_progress: DepotProgressFn,
    ) -> BoxFuture<'a, Result<Value, Failure>> {
        Box::pin(async move {
            let installer = DepotInstaller::new(InstallerOptions {
                paths: paths.clone(),
                depot_service: self.service.clone(),
                logger: Some(self.logger.clone()),
                set_registry_value: self.registry.clone(),
                now: None,
                clock: None,
            });
            installer.install(game_id, None, Some(on_progress)).await
        })
    }
}
