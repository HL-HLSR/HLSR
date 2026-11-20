//! Перенаправления для сквозных прогонов установщика и удаления без настоящего реестра, ярлыков
//! и запуска процессов (только `debug_assertions` или `feature = "dev-hooks"`). Приложение читает их
//! при сборке портов режимов `Setup`, `Uninstall`, `UninstallCleanup` и `Updater` (`hlsr-app`,
//! задача 9; задано не всё — режим не запускается), сквозные проверки —
//! `scripts/slint/check-setup.ps1` и приёмка 0.4.7 (`accept-windows.ps1 -DevHooks`):
//!
//! - `HLSR_FAKE_REGISTRY=<json>` — реестр в файле ([`JsonFileRegistry`]: вид
//!   `FakeRegistry::to_json`, сохраняется после каждой записи);
//! - `HLSR_KNOWN_FOLDERS=<dir>` — рабочий стол, `AppData\Roaming`, `AppData\Local` и `Temp` внутри
//!   каталога ([`FakeKnownFolders`]); `APPDATA`, `LOCALAPPDATA`, `USERPROFILE` в окружении —
//!   туда же;
//! - `HLSR_TEMP_DIR=<dir>` — `%TEMP%` (`TEMP`, `TMP`, рабочий каталог отложенного удаления);
//! - `HLSR_LAUNCH_DRY_RUN=<file>` — вместо запуска `HLSR.exe`, PowerShell и `icacls` дописать
//!   команду, рабочий каталог и окружение строкой JSON ([`DryRunProcesses`]).

use super::known_folders::{FakeKnownFolders, KnownFolders};
use super::processes::{DryRunProcesses, Env};
use super::registry::{FakeRegistry, RegValue, Registry, RegistryError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const FAKE_REGISTRY: &str = "HLSR_FAKE_REGISTRY";
pub const KNOWN_FOLDERS: &str = "HLSR_KNOWN_FOLDERS";
pub const TEMP_DIR: &str = "HLSR_TEMP_DIR";
pub const LAUNCH_DRY_RUN: &str = "HLSR_LAUNCH_DRY_RUN";

/// Реестр в файле JSON: читается при открытии (нет файла — пустой), пишется после каждого изменения.
pub struct JsonFileRegistry {
    file: PathBuf,
    inner: Mutex<FakeRegistry>,
}

impl JsonFileRegistry {
    pub fn open(file: &Path) -> Result<JsonFileRegistry, RegistryError> {
        let inner = match std::fs::read_to_string(file) {
            Ok(text) => {
                let model = serde_json::from_str(&text)
                    .map_err(|error| RegistryError(format!("{}: {error}", file.display())))?;
                FakeRegistry::from_json(&model)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => FakeRegistry::new(),
            Err(error) => return Err(RegistryError(format!("{}: {error}", file.display()))),
        };
        Ok(JsonFileRegistry {
            file: file.to_owned(),
            inner: Mutex::new(inner),
        })
    }

    fn with<T>(
        &self,
        change: impl FnOnce(&FakeRegistry) -> Result<T, RegistryError>,
    ) -> Result<T, RegistryError> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let value = change(&inner)?;
        let text = hlsr_core::json_text::stringify(&inner.to_json(), "  ") + "\n";
        std::fs::write(&self.file, text)
            .map_err(|error| RegistryError(format!("{}: {error}", self.file.display())))?;
        Ok(value)
    }
}

impl Registry for JsonFileRegistry {
    fn read(&self, key: &str) -> Option<Vec<(String, RegValue)>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .read(key)
    }

    fn write(&self, key: &str, name: &str, value: &RegValue) -> Result<(), RegistryError> {
        self.with(|inner| inner.write(key, name, value))
    }

    fn delete_value(&self, key: &str, name: &str) -> Result<(), RegistryError> {
        self.with(|inner| inner.delete_value(key, name))
    }

    fn delete_tree(&self, key: &str) -> Result<(), RegistryError> {
        self.with(|inner| inner.delete_tree(key))
    }

    fn subkeys(&self, key: &str) -> Option<Vec<String>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .subkeys(key)
    }
}

/// Заданные перенаправления.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redirects {
    pub registry: Option<PathBuf>,
    pub known_folders: Option<PathBuf>,
    pub temp: Option<PathBuf>,
    pub dry_run: Option<PathBuf>,
}

impl Redirects {
    /// Перенаправления из окружения (пустые значения не считаются).
    pub fn from_env(env: &Env) -> Redirects {
        let path = |name: &str| {
            env.get(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Redirects {
            registry: path(FAKE_REGISTRY),
            known_folders: path(KNOWN_FOLDERS),
            temp: path(TEMP_DIR),
            dry_run: path(LAUNCH_DRY_RUN),
        }
    }

    /// Всё перенаправлено: реестр, папки, `%TEMP%` и запуск. Только так сквозной прогон не тронет
    /// настоящие реестр, ярлыки, `%APPDATA%` и процессы.
    pub fn complete(&self) -> bool {
        self.registry.is_some()
            && self.known_folders.is_some()
            && self.temp.is_some()
            && self.dry_run.is_some()
    }

    /// Известные папки внутри `HLSR_KNOWN_FOLDERS` (`Temp` — `HLSR_TEMP_DIR`, если задан).
    pub fn folders(&self) -> Option<RedirectedFolders> {
        let root = self.known_folders.as_ref()?;
        Some(RedirectedFolders {
            inner: FakeKnownFolders::under(root),
            temp: self.temp.clone(),
        })
    }

    pub fn registry(&self) -> Option<Result<JsonFileRegistry, RegistryError>> {
        self.registry.as_deref().map(JsonFileRegistry::open)
    }

    pub fn processes(&self) -> Option<DryRunProcesses> {
        self.dry_run.as_deref().map(DryRunProcesses::new)
    }

    /// Окружение с перенаправленными папками пользователя: `APPDATA`, `LOCALAPPDATA`,
    /// `USERPROFILE`, `TEMP`, `TMP`.
    pub fn apply_env(&self, env: &mut Env) {
        if let Some(folders) = self.folders() {
            let text =
                |path: io::Result<PathBuf>| path.ok().map(|p| p.to_string_lossy().into_owned());
            if let Some(appdata) = text(folders.appdata()) {
                env.set("APPDATA", &appdata);
            }
            if let Some(local) = text(folders.local_appdata()) {
                env.set("LOCALAPPDATA", &local);
            }
            if let Some(root) = &self.known_folders {
                env.set("USERPROFILE", &root.to_string_lossy());
            }
        }
        if let Some(temp) = &self.temp {
            env.set("TEMP", &temp.to_string_lossy());
            env.set("TMP", &temp.to_string_lossy());
        }
    }
}

/// Известные папки внутри каталога перенаправления.
#[derive(Debug, Clone)]
pub struct RedirectedFolders {
    inner: FakeKnownFolders,
    temp: Option<PathBuf>,
}

impl KnownFolders for RedirectedFolders {
    fn desktop(&self) -> io::Result<PathBuf> {
        self.inner.desktop()
    }

    fn start_menu_programs(&self) -> io::Result<PathBuf> {
        self.inner.start_menu_programs()
    }

    fn appdata(&self) -> io::Result<PathBuf> {
        self.inner.appdata()
    }

    fn local_appdata(&self) -> io::Result<PathBuf> {
        self.inner.local_appdata()
    }

    fn temp(&self) -> PathBuf {
        self.temp.clone().unwrap_or_else(|| self.inner.temp())
    }
}
