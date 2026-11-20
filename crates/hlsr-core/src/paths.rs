//! Папка данных и папка журналов — те же, что у 0.4.7.

use crate::error::CoreError;
use std::path::{Path, PathBuf};

/// Имя боевого HLSR — `app.getName()` 0.4.7: `name` упакованного `package.json` (`productName` в
/// нём нет). QA-сборка Windows упакована под своим именем (`hlsr-qa`,
/// `hlsr_update::windows::layout::QA_NAME`).
pub const APP_NAME: &str = "hlsr";

/// Где лаунчер хранит данные (`settings.json`, `library.json`, …) и журналы.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
    /// Имя приложения (`app.getName()`), от которого построены папки Windows: [`APP_NAME`] или
    /// имя QA-сборки. Его же берут очистка после удаления и отчёт диагностики.
    pub app_name: &'static str,
}

impl Paths {
    /// Пути текущего пользователя для приложения `app_name`. Windows: `%APPDATA%\<имя>`,
    /// `%LOCALAPPDATA%\<имя>\logs` ([`Paths::windows_from`]). Linux: `$XDG_CONFIG_HOME/hlsr` или
    /// `~/.config/hlsr`, журналы `~/.local/state/hlsr/logs`; QA-сборки Linux в 0.4.7 нет
    /// (`qa-launcher-build.js` собирает только Windows), имя там всегда [`APP_NAME`].
    pub fn detect(app_name: &'static str) -> Result<Paths, CoreError> {
        let paths = Self::detect_platform(app_name)?;
        #[cfg(any(debug_assertions, feature = "dev-hooks"))]
        let paths = Paths {
            data_dir: non_empty_env("HLSR_DATA_DIR").unwrap_or(paths.data_dir),
            logs_dir: non_empty_env("HLSR_LOGS_DIR").unwrap_or(paths.logs_dir),
            ..paths
        };
        Ok(paths)
    }

    #[cfg(not(windows))]
    fn detect_platform(_app_name: &'static str) -> Result<Paths, CoreError> {
        let home = non_empty_env("HOME").ok_or(CoreError::NoDataDir("HOME"))?;
        Self::detect_from(non_empty_env("XDG_CONFIG_HOME"), home)
    }

    #[cfg(windows)]
    fn detect_platform(app_name: &'static str) -> Result<Paths, CoreError> {
        let app_data = non_empty_env("APPDATA").ok_or(CoreError::NoDataDir("APPDATA"))?;
        let local_app_data =
            non_empty_env("LOCALAPPDATA").ok_or(CoreError::NoDataDir("LOCALAPPDATA"))?;
        Ok(Self::windows_from(&app_data, &local_app_data, app_name))
    }

    /// Пути Windows по `%APPDATA%` и `%LOCALAPPDATA%`: данные — `userData` Electron
    /// (`<appData>\<app.getName()>`), журналы — `logsDir()` из `index.js` 0.4.7
    /// (`%LOCALAPPDATA%\<app.getName()>\logs`).
    pub fn windows_from(app_data: &Path, local_app_data: &Path, app_name: &'static str) -> Paths {
        Paths {
            data_dir: app_data.join(app_name),
            logs_dir: local_app_data.join(app_name).join("logs"),
            app_name,
        }
    }

    /// Пути Linux по значениям окружения. Пустой `XDG_CONFIG_HOME` — то же, что не заданный.
    /// Журналы — именно от `home`, без XDG, как в 0.4.7.
    #[cfg(not(windows))]
    pub fn detect_from(
        xdg_config_home: Option<PathBuf>,
        home: PathBuf,
    ) -> Result<Paths, CoreError> {
        if home.as_os_str().is_empty() {
            return Err(CoreError::NoDataDir("HOME"));
        }
        let config = match xdg_config_home {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => home.join(".config"),
        };
        Ok(Paths {
            data_dir: config.join("hlsr"),
            logs_dir: home.join(".local").join("state").join("hlsr").join("logs"),
            app_name: APP_NAME,
        })
    }

    /// Пути внутри одной папки: `root/data` и `root/logs` (тесты и отладка).
    pub fn under(root: &Path) -> Paths {
        Paths {
            data_dir: root.join("data"),
            logs_dir: root.join("logs"),
            app_name: APP_NAME,
        }
    }

    /// Файл в папке данных.
    pub fn file(&self, name: &str) -> PathBuf {
        self.data_dir.join(name)
    }
}

/// Каталог ресурсов — `process.resourcesPath` 0.4.7: рядом с лаунчером лежат 7za, мост
/// и оверлей LiveSplit, шрифты, файлы для Linux (`resources/7zip`, `resources/linux`…).
/// Выпуск: `<папка исполняемого файла>/resources`. Отладка и `dev-hooks`: `resources`
/// рабочего дерева (там же, где git хранит эти файлы), а если его нет (сборка с
/// `dev-hooks` у того, у кого нет исходников) — как в выпуске.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resources {
    dir: PathBuf,
}

impl Resources {
    /// Каталог ресурсов этого запуска.
    pub fn detect() -> Resources {
        let exe = std::env::current_exe().ok();
        Resources::locate(exe.as_deref(), dev_resources_dir())
    }

    /// Каталог по исполняемому файлу и каталогу рабочего дерева (если он есть).
    /// Без исполняемого файла (ОС не сказала путь) — `resources` от текущей папки.
    pub fn locate(exe: Option<&Path>, workspace: Option<PathBuf>) -> Resources {
        let dir = workspace.unwrap_or_else(|| {
            exe.and_then(Path::parent)
                .unwrap_or_else(|| Path::new(""))
                .join("resources")
        });
        Resources { dir }
    }

    /// Заданный каталог (тесты и упаковщик).
    pub fn at(dir: impl Into<PathBuf>) -> Resources {
        Resources { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Путь внутри каталога: компоненты через `/`, как в `resources/7zip/linux/x64`.
    pub fn path(&self, relative: &str) -> PathBuf {
        relative
            .split('/')
            .fold(self.dir.clone(), |path, part| path.join(part))
    }

    /// `7za` этой ОС. Отладка и `dev-hooks`: переменная `HLSR_7ZA` задаёт путь явно.
    pub fn sevenzip(&self) -> PathBuf {
        #[cfg(any(debug_assertions, feature = "dev-hooks"))]
        let explicit = non_empty_env("HLSR_7ZA");
        #[cfg(not(any(debug_assertions, feature = "dev-hooks")))]
        let explicit = None;
        self.sevenzip_or(explicit)
    }

    fn sevenzip_or(&self, explicit: Option<PathBuf>) -> PathBuf {
        explicit.unwrap_or_else(|| self.sevenzip_for(crate::js::Platform::CURRENT))
    }

    /// `7za` для ОС `platform` без переопределений: `7zip/linux/x64/7za` (из `7zip-bin` 5.2.0),
    /// `7zip/win/x64/7za.exe` (7-Zip 26.05, как `resources/7zip` у 0.4.9).
    pub fn sevenzip_for(&self, platform: crate::js::Platform) -> PathBuf {
        match platform {
            crate::js::Platform::Linux => self.path("7zip/linux/x64/7za"),
            crate::js::Platform::Win32 => self.path("7zip/win/x64/7za.exe"),
        }
    }
}

/// `resources` рабочего дерева; в выпуске без `dev-hooks` его нет.
fn dev_resources_dir() -> Option<PathBuf> {
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    {
        // crates/hlsr-core → корень рабочего дерева.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
        let dir = root.join("resources");
        dir.is_dir().then_some(dir)
    }
    #[cfg(not(any(debug_assertions, feature = "dev-hooks")))]
    {
        None
    }
}

fn non_empty_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod resources_tests {
    use super::*;
    use crate::js::Platform;

    #[test]
    fn release_resources_lie_next_to_the_executable() {
        let resources = Resources::locate(Some(&Path::new("opt").join("hlsr").join("hlsr")), None);
        assert_eq!(
            resources.dir(),
            Path::new("opt").join("hlsr").join("resources")
        );
        let resources = Resources::locate(None, None);
        assert_eq!(resources.dir(), Path::new("resources"));
    }

    #[test]
    fn workspace_resources_win_over_the_executable_folder() {
        let workspace = Path::new("ws").join("resources");
        let resources = Resources::locate(
            Some(&Path::new("target").join("debug").join("hlsr")),
            Some(workspace.clone()),
        );
        assert_eq!(resources.dir(), workspace);
    }

    #[test]
    fn sevenzip_binaries_lie_where_7zip_bin_keeps_them() {
        let resources = Resources::at("res");
        assert_eq!(
            resources.sevenzip_for(Platform::Linux),
            Path::new("res")
                .join("7zip")
                .join("linux")
                .join("x64")
                .join("7za")
        );
        assert_eq!(
            resources.sevenzip_for(Platform::Win32),
            Path::new("res")
                .join("7zip")
                .join("win")
                .join("x64")
                .join("7za.exe")
        );
        assert_eq!(
            resources.path("livesplit-fonts/OFL.txt"),
            Path::new("res").join("livesplit-fonts").join("OFL.txt")
        );
    }

    /// Тесты и отладка находят `resources/7zip` рабочего дерева; файл настоящий
    /// (sha256 закреплён: Linux — `7zip-bin` 5.2.0, Windows — 7-Zip 26.05) и на Linux исполняемый.
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    #[test]
    fn debug_build_finds_the_repository_7za() {
        use sha2::{Digest, Sha256};
        let resources = Resources::detect();
        let platform = Platform::CURRENT;
        let path = resources.sevenzip_for(platform);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let sha256: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let expected = match platform {
            Platform::Linux => "afc9448bd0cc2eeda131cce313ef4994f9656417e0a15c8465fcda9ca859b280",
            // 7-Zip 26.05 из release-0.4.9:resources/7zip/win-x64 (S-3).
            Platform::Win32 => "7ddf0a3bc451e166679296be630d6f0573eb386159c04184761c43020c484291",
        };
        assert_eq!(sha256, expected);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_ne!(mode & 0o111, 0, "у 7za нет бита исполнения: {mode:o}");
        }
    }

    #[test]
    fn explicit_7za_replaces_the_bundled_one() {
        let resources = Resources::at("res");
        let bundled = resources.sevenzip_for(Platform::CURRENT);
        assert_eq!(resources.sevenzip_or(None), bundled);
        assert_eq!(
            resources.sevenzip_or(Some(PathBuf::from("my-7za"))),
            PathBuf::from("my-7za")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_places_data_and_logs_in_root() {
        let p = Paths::under(Path::new("/tmp/x"));
        assert_eq!(p.data_dir, PathBuf::from("/tmp/x/data"));
        assert_eq!(p.logs_dir, PathBuf::from("/tmp/x/logs"));
        assert_eq!(
            p.file("settings.json"),
            PathBuf::from("/tmp/x/data/settings.json")
        );
        assert_eq!(p.app_name, APP_NAME);
    }

    /// Боевой HLSR и QA-сборка (`app.getName()` 0.4.7 — `hlsr` и `hlsr-qa`): у каждой свои папки
    /// данных и журналов, профиль боевой QA-сборка не делит.
    #[test]
    fn windows_paths_follow_the_app_name() {
        let roaming = Path::new("Roaming");
        let local = Path::new("Local");
        let production = Paths::windows_from(roaming, local, APP_NAME);
        assert_eq!(production.data_dir, roaming.join("hlsr"));
        assert_eq!(production.logs_dir, local.join("hlsr").join("logs"));
        assert_eq!(production.app_name, "hlsr");
        let qa = Paths::windows_from(roaming, local, "hlsr-qa");
        assert_eq!(qa.data_dir, roaming.join("hlsr-qa"));
        assert_eq!(qa.logs_dir, local.join("hlsr-qa").join("logs"));
        assert_eq!(qa.app_name, "hlsr-qa");
        assert_ne!(qa.data_dir, production.data_dir);
        assert_ne!(qa.logs_dir, production.logs_dir);
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_are_the_0_4_7_folders() {
        let roaming = Path::new(r"C:\Users\u\AppData\Roaming");
        let local = Path::new(r"C:\Users\u\AppData\Local");
        let production = Paths::windows_from(roaming, local, APP_NAME);
        assert_eq!(
            production.data_dir,
            Path::new(r"C:\Users\u\AppData\Roaming\hlsr")
        );
        assert_eq!(
            production.logs_dir,
            Path::new(r"C:\Users\u\AppData\Local\hlsr\logs")
        );
        let qa = Paths::windows_from(roaming, local, "hlsr-qa");
        assert_eq!(
            qa.data_dir,
            Path::new(r"C:\Users\u\AppData\Roaming\hlsr-qa")
        );
        assert_eq!(
            qa.logs_dir,
            Path::new(r"C:\Users\u\AppData\Local\hlsr-qa\logs")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_paths_follow_0_4_7() {
        let p = Paths::detect_from(Some("/home/u/.cfg".into()), "/home/u".into()).unwrap();
        assert_eq!(p.data_dir, PathBuf::from("/home/u/.cfg/hlsr"));
        assert_eq!(p.logs_dir, PathBuf::from("/home/u/.local/state/hlsr/logs"));
        assert_eq!(p.app_name, APP_NAME);
        let p = Paths::detect_from(None, "/home/u".into()).unwrap();
        assert_eq!(p.data_dir, PathBuf::from("/home/u/.config/hlsr"));
    }
}
