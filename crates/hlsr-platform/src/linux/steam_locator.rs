//! Поиск Steam на Linux (`linux/SteamLocator.js`): `~/.local/share/Steam`, `~/.steam/root`,
//! `~/.steam/steam`, Flatpak (`~/.var/app/com.valvesoftware.Steam/data/Steam`); процесс Steam —
//! по `/proc/*/comm` среди процессов этого же пользователя; библиотеки и приложения — из
//! `libraryfolders.vdf` и `appmanifest_<id>.acf`.

use crate::steam_fs::{RealFs, SteamFs, read_to_string};
use crate::steam_locator::{
    AppInfo, Preflight, RunningSteam, SOURCE_SDK_APP_ID, SteamInstall, SteamLocating,
    failure_value, is_unsafe_installdir, steam_summary,
};
use crate::vdf::{self, VdfValue, get_insensitive, get_insensitive_in};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_delivery::errors::{Code, DeliveryError};
use hlsr_delivery::ports::Logger;
use serde_json::{Value, json};
use std::sync::Arc;

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct LocatorOptions {
    pub fs: Option<Arc<dyn SteamFs>>,
    /// `home`; по умолчанию — домашний каталог пользователя.
    pub home: Option<String>,
    pub logger: Option<Arc<dyn Logger>>,
    /// Владелец процессов, которые считаются своими (`process.getuid()`); по умолчанию —
    /// пользователь процесса. Не Linux — проверка владельца не делается.
    pub own_uid: Option<u32>,
}

/// Пользователь процесса (`process.getuid()`); вне Linux его нет.
pub fn current_uid() -> Option<u32> {
    #[cfg(unix)]
    {
        Some(rustix::process::getuid().as_raw())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// `SteamLocator` (Linux).
pub struct SteamLocator {
    fs: Arc<dyn SteamFs>,
    home: String,
    own_uid: Option<u32>,
    path: NodePath,
}

impl SteamLocator {
    pub fn new(options: LocatorOptions) -> SteamLocator {
        let home = options.home.unwrap_or_else(|| {
            std::env::home_dir()
                .map(|home| home.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        SteamLocator {
            fs: options.fs.unwrap_or_else(|| Arc::new(RealFs)),
            home,
            own_uid: options.own_uid.or_else(current_uid),
            path: NodePath::new(Platform::Linux, "/"),
        }
    }

    /// `roots()`: где ищется Steam, по порядку.
    pub fn roots(&self) -> Vec<String> {
        [
            ".local/share/Steam",
            ".steam/root",
            ".steam/steam",
            ".var/app/com.valvesoftware.Steam/data/Steam",
        ]
        .iter()
        .map(|rest| self.path.join(&[&self.home, rest]))
        .collect()
    }

    /// `locateSteam()`.
    pub fn locate_steam(&self) -> Result<SteamInstall, DeliveryError> {
        for candidate in self.roots() {
            let Ok(root) = self.fs.real_path(&candidate) else {
                continue;
            };
            let has_steamapps = self
                .fs
                .stat(&self.path.join(&[&root, "steamapps"]))
                .is_ok_and(|stat| stat.is_dir);
            if !has_steamapps {
                continue;
            }
            let steam_sh = self.path.join(&[&root, "steam.sh"]);
            let exe = ["/usr/bin/steam".to_owned(), steam_sh]
                .into_iter()
                .find(|file| self.fs.exists(file));
            let flatpak = candidate.contains(".var/app/com.valvesoftware.Steam");
            let (steam_exe, steam_args) = match exe {
                Some(exe) => (exe, Vec::new()),
                None if flatpak => (
                    "/usr/bin/flatpak".to_owned(),
                    vec!["run".to_owned(), "com.valvesoftware.Steam".to_owned()],
                ),
                None => ("steam".to_owned(), Vec::new()),
            };
            return Ok(SteamInstall {
                install_path: root,
                steam_exe,
                steam_args: Some(steam_args),
            });
        }
        Err(DeliveryError::new(
            Code::SteamNotInstalled,
            "Steam не найден в домашнем каталоге пользователя",
        ))
    }

    /// `assertSteamRunning()`: процесс `steam` или `steamwebhelper` этого пользователя.
    pub fn assert_steam_running(&self) -> Result<RunningSteam, DeliveryError> {
        let entries = self.fs.read_dir("/proc").unwrap_or_default();
        for name in entries {
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let dir = format!("/proc/{name}");
            let Ok(stat) = self.fs.stat(&dir) else {
                continue;
            };
            if let Some(own) = self.own_uid
                && stat.uid != Some(own)
            {
                continue;
            }
            let Ok(comm) = read_to_string(self.fs.as_ref(), &format!("{dir}/comm")) else {
                continue;
            };
            let comm = js::trim(&comm);
            if comm != "steam" && comm != "steamwebhelper" {
                continue;
            }
            return Ok(RunningSteam {
                pid: name.parse().unwrap_or(0),
                executable_path: None,
                started_at: None,
                source: Some("proc".to_owned()),
            });
        }
        Err(DeliveryError::new(
            Code::SteamNotRunning,
            "Steam не запущен",
        ))
    }

    /// `steamLibraries(steam)`: корень Steam и абсолютные папки из `libraryfolders.vdf`.
    pub fn steam_libraries(&self, steam: &SteamInstall) -> Vec<String> {
        let mut out = vec![steam.install_path.clone()];
        let file = self
            .path
            .join(&[&steam.install_path, "steamapps", "libraryfolders.vdf"]);
        let Ok(tree) = read_to_string(self.fs.as_ref(), &file)
            .map_err(|_| ())
            .and_then(|text| vdf::parse(&text).map_err(|_| ()))
        else {
            return out;
        };
        let Some(folders) = get_insensitive(&tree, "libraryfolders").and_then(VdfValue::as_object)
        else {
            return out;
        };
        for (key, entry) in folders.iter() {
            if key.is_empty() || !key.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let value = match entry {
                VdfValue::String(text) => Some(text.as_str()),
                VdfValue::Object(_) => {
                    get_insensitive_in(Some(entry), "path").and_then(VdfValue::as_str)
                }
            };
            if let Some(value) = value
                && self.path.is_absolute(value)
                && !out.iter().any(|existing| existing == value)
            {
                out.push(value.to_owned());
            }
        }
        out
    }

    /// `findApp(steam, appId)`: первое приложение с таким `appmanifest`, у которого всё
    /// в порядке; остальное пропускается.
    pub fn find_app(&self, steam: &SteamInstall, app_id: u32) -> Result<AppInfo, DeliveryError> {
        for library in self.steam_libraries(steam) {
            let manifest_path =
                self.path
                    .join(&[&library, "steamapps", &format!("appmanifest_{app_id}.acf")]);
            let Ok(tree) = read_to_string(self.fs.as_ref(), &manifest_path)
                .map_err(|_| ())
                .and_then(|text| vdf::parse(&text).map_err(|_| ()))
            else {
                continue;
            };
            let Some(state) = get_insensitive(&tree, "AppState") else {
                continue;
            };
            let declared = get_insensitive_in(Some(state), "appid")
                .map_or(f64::NAN, |value| js::to_number(&value.js_string()));
            if declared != f64::from(app_id) {
                continue;
            }
            let Some(installdir) = get_insensitive_in(Some(state), "installdir")
                .and_then(VdfValue::as_str)
                .filter(|dir| !is_unsafe_installdir(dir))
            else {
                continue;
            };
            let app_path = self
                .path
                .join(&[&library, "steamapps", "common", installdir]);
            if !self.fs.exists(&app_path) {
                continue;
            }
            return Ok(AppInfo {
                app_id,
                library,
                manifest_path,
                installdir: installdir.to_owned(),
                app_path,
                state_flags: None,
                name: None,
            });
        }
        Err(DeliveryError::new(
            Code::SourceSdkNotInstalled,
            format!("Приложение Steam {app_id} не установлено"),
        ))
    }

    /// `preflight(appId)`; `None` — Source SDK Base.
    pub fn preflight(&self, app_id: Option<u32>) -> Result<Preflight, DeliveryError> {
        let steam = self.locate_steam()?;
        let running = self.assert_steam_running()?;
        let app = self.find_app(&steam, app_id.unwrap_or(SOURCE_SDK_APP_ID))?;
        Ok(Preflight {
            steam,
            running,
            app,
        })
    }

    /// `describeAvailability(appId)`: `{ok: true, code: null, steam, app}` или
    /// `{ok: false, code, message}`.
    pub fn describe_availability(&self, app_id: Option<u32>) -> Value {
        match self.preflight(app_id) {
            Ok(result) => json!({
                "ok": true,
                "code": null,
                "steam": steam_summary(&result.steam, &result.running),
                "app": result.app.to_value(),
            }),
            Err(error) => failure_value(&error),
        }
    }
}

impl SteamLocating for SteamLocator {
    fn locate_steam(&self) -> Result<SteamInstall, DeliveryError> {
        SteamLocator::locate_steam(self)
    }

    /// `describeSteamOnly()`: `{ok: true, code: null, steam}` (поля `app` нет).
    fn describe_steam_only(&self) -> Value {
        let result = self
            .locate_steam()
            .and_then(|steam| self.assert_steam_running().map(|running| (steam, running)));
        match result {
            Ok((steam, running)) => json!({
                "ok": true,
                "code": null,
                "steam": steam_summary(&steam, &running),
            }),
            Err(error) => failure_value(&error),
        }
    }
}
