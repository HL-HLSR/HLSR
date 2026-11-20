//! Известные папки Windows, нужные установке: рабочий стол (`app.getPath("desktop")` в 0.4.7 —
//! `SHGetKnownFolderPath(FOLDERID_Desktop)`), меню «Пуск» пользователя, `%APPDATA%`,
//! `%LOCALAPPDATA%` и `%TEMP%`. Настоящие папки владельца в тестах не нужны: [`FakeKnownFolders`]
//! раскладывает их по временному каталогу.

use super::layout::{EnvLookup, win_path};
use std::io;
use std::path::{Path, PathBuf};

/// Папки пользователя. Ошибка — известная папка не определилась (в 0.4.7 `app.getPath` бросает).
pub trait KnownFolders: Send + Sync {
    /// Рабочий стол (может лежать в OneDrive).
    fn desktop(&self) -> io::Result<PathBuf>;
    /// `%APPDATA%\Microsoft\Windows\Start Menu\Programs`.
    fn start_menu_programs(&self) -> io::Result<PathBuf>;
    /// `%APPDATA%` (Roaming).
    fn appdata(&self) -> io::Result<PathBuf>;
    /// `%LOCALAPPDATA%`.
    fn local_appdata(&self) -> io::Result<PathBuf>;
    /// `%TEMP%` (`os.tmpdir()`).
    fn temp(&self) -> PathBuf;
}

/// `os.tmpdir()` Node на Windows: `TEMP`, затем `TMP`, затем `<SystemRoot|windir>\temp`; хвостовой
/// `\` отбрасывается, кроме корня диска (`C:\`). Нет ни одной переменной — `C:\Windows\Temp`.
pub fn node_tmpdir(env: EnvLookup<'_>) -> String {
    let get = |name: &str| env(name).filter(|value| !value.is_empty());
    let path = get("TEMP").or_else(|| get("TMP")).unwrap_or_else(|| {
        let root = get("SystemRoot")
            .or_else(|| get("windir"))
            .unwrap_or_else(|| "C:\\Windows".to_owned());
        format!("{root}\\temp")
    });
    if path.chars().count() > 1 && path.ends_with('\\') && !path.ends_with(":\\") {
        path[..path.len() - 1].to_owned()
    } else {
        path
    }
}

/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs` от каталога Roaming.
pub fn start_menu_of(appdata: &Path) -> PathBuf {
    let joined = win_path().join(&[
        &appdata.to_string_lossy(),
        "Microsoft",
        "Windows",
        "Start Menu",
        "Programs",
    ]);
    PathBuf::from(joined)
}

/// Папки внутри заданного каталога: `Desktop`, `AppData\Roaming`, `AppData\Local`, `Temp`.
/// Самих каталогов не создаёт — их создаёт тот, кто в них пишет.
#[derive(Debug, Clone)]
pub struct FakeKnownFolders {
    root: PathBuf,
}

impl FakeKnownFolders {
    pub fn under(root: &Path) -> FakeKnownFolders {
        FakeKnownFolders {
            root: root.to_owned(),
        }
    }
}

impl KnownFolders for FakeKnownFolders {
    fn desktop(&self) -> io::Result<PathBuf> {
        Ok(self.root.join("Desktop"))
    }

    fn start_menu_programs(&self) -> io::Result<PathBuf> {
        Ok(self
            .root
            .join("AppData")
            .join("Roaming")
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs"))
    }

    fn appdata(&self) -> io::Result<PathBuf> {
        Ok(self.root.join("AppData").join("Roaming"))
    }

    fn local_appdata(&self) -> io::Result<PathBuf> {
        Ok(self.root.join("AppData").join("Local"))
    }

    fn temp(&self) -> PathBuf {
        self.root.join("Temp")
    }
}

/// Папки ОС: рабочий стол — `SHGetKnownFolderPath`, остальное — переменные окружения, как в 0.4.7
/// (`env.APPDATA`, `env.LOCALAPPDATA`, `os.tmpdir()`); нет переменной — известная папка.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemKnownFolders;

#[cfg(windows)]
mod system {
    use super::{KnownFolders, SystemKnownFolders, node_tmpdir, start_menu_of};
    use crate::windows::layout;
    use ::windows::Win32::System::Com::CoTaskMemFree;
    use ::windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_RoamingAppData, KF_FLAG_DEFAULT,
        SHGetKnownFolderPath,
    };
    use ::windows::core::GUID;
    use std::io;
    use std::path::PathBuf;

    fn known_folder(id: &GUID) -> io::Result<PathBuf> {
        // SAFETY: `id` — константа FOLDERID; результат — строка, выделенная COM, освобождается ниже.
        let path = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }
            .map_err(|error| io::Error::other(error.to_string()))?;
        // SAFETY: SHGetKnownFolderPath вернула строку с нулём на конце.
        let text = String::from_utf16_lossy(unsafe { path.as_wide() });
        // SAFETY: память выделена CoTaskMemAlloc и освобождается один раз.
        unsafe { CoTaskMemFree(Some(path.0 as *const _)) };
        Ok(PathBuf::from(text))
    }

    impl KnownFolders for SystemKnownFolders {
        fn desktop(&self) -> io::Result<PathBuf> {
            known_folder(&FOLDERID_Desktop)
        }

        fn start_menu_programs(&self) -> io::Result<PathBuf> {
            Ok(start_menu_of(&self.appdata()?))
        }

        fn appdata(&self) -> io::Result<PathBuf> {
            match layout::process_env("APPDATA").filter(|value| !value.is_empty()) {
                Some(value) => Ok(PathBuf::from(value)),
                None => known_folder(&FOLDERID_RoamingAppData),
            }
        }

        fn local_appdata(&self) -> io::Result<PathBuf> {
            match layout::process_env("LOCALAPPDATA").filter(|value| !value.is_empty()) {
                Some(value) => Ok(PathBuf::from(value)),
                None => known_folder(&FOLDERID_LocalAppData),
            }
        }

        fn temp(&self) -> PathBuf {
            PathBuf::from(node_tmpdir(&layout::process_env))
        }
    }
}
