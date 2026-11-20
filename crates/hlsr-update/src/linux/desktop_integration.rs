//! Интеграция с рабочим столом Linux (`linux/DesktopIntegration.js` 0.4.7): команда `~/.local/bin/hlsr`
//! (скрипт с маркером `# HLSR managed CLI launcher`, вызывает `start` установки), пункт меню
//! `hlsr.desktop` с `X-HLSR-Managed=true` и значок `hlsr-appimage.png`, обновление кешей меню
//! (`update-desktop-database`, `gtk-update-icon-cache`, `kbuildsycoca6/5`). Чужие файлы без маркера не
//! перезаписываются. Байты скрипта и `.desktop` — дословно 0.4.7 (карта §5.6).

use super::{EnvVars, SyncRunner, archive_install};
use crate::ports::Logger;
use crate::updater::processes::Spawn;
use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MANAGED_MARKER: &str = "X-HLSR-Managed=true";
pub const CLI_MARKER: &str = "# HLSR managed CLI launcher";
/// Имя пункта меню (и копии на рабочем столе).
pub const DESKTOP_FILE: &str = "hlsr.desktop";
/// Значок в теме hicolor.
pub const ICON_FILE: &str = "hlsr-appimage.png";
/// Сколько ждать утилиты обновления кешей.
pub const REFRESH_TIMEOUT: Duration = Duration::from_millis(10_000);
/// Переменные AppImage и Qt, которые не получает `kbuildsycoca` системы.
pub const KDE_DROPPED: [&str; 6] = [
    "APPIMAGE",
    "APPDIR",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "QT_PLUGIN_PATH",
    "QT_QPA_PLATFORM_PLUGIN_PATH",
];

fn posix() -> NodePath {
    NodePath::new(Platform::Linux, "/")
}

/// `cliScript(installRoot)`: скрипт команды `hlsr` (прямая ссылка на `start` сменила бы `$0`).
pub fn cli_script(install_root: &str) -> String {
    let posix = posix();
    let start = posix.join(&[&posix.resolve(&[install_root]), "start"]);
    let quoted = format!("'{}'", start.replace('\'', "'\\''"));
    format!("#!/bin/sh\n{CLI_MARKER}\nexec {quoted} \"$@\"\n")
}

/// `~/.local/bin/hlsr`.
pub fn cli_path(home: &Path) -> PathBuf {
    home.join(".local").join("bin").join("hlsr")
}

/// `managedCliPath(installRoot)`: команда наша — ссылка на `start` установки или ровно наш скрипт.
pub fn managed_cli_path(install_root: &Path, home: &Path) -> Option<PathBuf> {
    let file = cli_path(home);
    let stat = std::fs::symlink_metadata(&file).ok()?;
    if stat.file_type().is_symlink() {
        let target = std::fs::read_link(&file).ok()?;
        let resolved = super::resolve(&file.parent().unwrap_or(Path::new("/")).join(target));
        return (resolved == super::resolve(install_root).join("start")).then_some(file);
    }
    let expected = cli_script(&install_root.to_string_lossy());
    let ok = stat.is_file()
        && stat.len() == expected.len() as u64
        && std::fs::read(&file).ok()? == expected.as_bytes();
    ok.then_some(file)
}

/// `fs.accessSync(file, X_OK)`: файл есть и исполняемый (на Windows — просто есть, как у Node).
fn executable(file: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(file) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

/// `ensureManagedCli({installRoot, home})`: команда `hlsr` для установки с маркером и исполняемым
/// `start`. Чужой файл или ссылка не трогаются (`false`); старая ссылка на `start` заменяется скриптом.
pub fn ensure_managed_cli(install_root: &Path, home: &Path) -> Result<bool, String> {
    if !install_root.is_absolute() {
        return Ok(false);
    }
    let root = super::resolve(install_root);
    let marker = std::fs::read(root.join(archive_install::INSTALL_MARKER))
        .ok()
        .and_then(|bytes| hlsr_core::json_text::json_parse(&String::from_utf8_lossy(&bytes)).ok());
    if marker
        .as_ref()
        .and_then(|marker| marker.get("schema"))
        .and_then(serde_json::Value::as_f64)
        != Some(1.0)
        || !executable(&root.join("start"))
    {
        return Ok(false);
    }
    let file = cli_path(home);
    let previous = match std::fs::symlink_metadata(&file) {
        Ok(previous) => Some(previous),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(super::fs_error(&error, "lstat", &file)),
    };
    if let Some(previous) = previous {
        if managed_cli_path(&root, home).is_none() {
            return Ok(false);
        }
        if !previous.file_type().is_symlink() {
            super::chmod(&file, 0o755).map_err(|error| super::fs_error(&error, "chmod", &file))?;
            return Ok(true);
        }
        std::fs::remove_file(&file).map_err(|error| super::fs_error(&error, "unlink", &file))?;
    }
    if let Some(parent) = file.parent() {
        super::make_dirs(parent, 0o777)
            .map_err(|error| super::fs_error(&error, "mkdir", parent))?;
    }
    match super::write_file(
        &file,
        cli_script(&root.to_string_lossy()).as_bytes(),
        0o755,
        true,
    ) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(super::fs_error(&error, "open", &file)),
    }
}

/// `desktopExecPath(file)`: путь для `Exec=` в кавычках с экранированием; не абсолютный путь или
/// перевод строки и NUL — `None`.
pub fn desktop_exec_path(file: &str) -> Option<String> {
    if !file.starts_with('/') || file.contains(['\r', '\n', '\0']) {
        return None;
    }
    let mut escaped = String::with_capacity(file.len() + 2);
    for c in file.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '$' => escaped.push_str("\\$"),
            '`' => escaped.push_str("\\`"),
            '%' => escaped.push_str("%%"),
            other => escaped.push(other),
        }
    }
    Some(format!("\"{escaped}\""))
}

/// Текст `hlsr.desktop` для готового `Exec`.
pub fn desktop_entry(exec_path: &str) -> String {
    [
        "[Desktop Entry]",
        "Type=Application",
        "Name=HLSR",
        &format!("Exec={exec_path} --no-sandbox %U"),
        "Icon=hlsr-appimage",
        "StartupWMClass=hlsr",
        "Terminal=false",
        "Categories=Game;",
        MANAGED_MARKER,
        "",
    ]
    .join("\n")
}

/// `<dataHome>/applications/hlsr.desktop`.
pub fn desktop_path(data_home: &Path) -> PathBuf {
    data_home.join("applications").join(DESKTOP_FILE)
}

/// `<dataHome>/icons/hicolor/256x256/apps/hlsr-appimage.png`.
pub fn icon_path(data_home: &Path) -> PathBuf {
    data_home
        .join("icons")
        .join("hicolor")
        .join("256x256")
        .join("apps")
        .join(ICON_FILE)
}

fn command(file: &str, args: Vec<String>, env: EnvVars) -> Spawn {
    Spawn {
        file: PathBuf::from(file),
        args,
        cwd: None,
        env,
        hidden: true,
        detached: false,
    }
}

/// `refreshMenuCaches(dataHome)`: обновить кеши меню, чтобы пункт появился без перелогина. Ошибки и
/// отсутствие утилит не мешают.
pub fn refresh_menu_caches(data_home: &Path, runner: &dyn SyncRunner, env: &[(String, String)]) {
    let applications = data_home.join("applications");
    let hicolor = data_home.join("icons").join("hicolor");
    for (file, args) in [
        (
            "update-desktop-database",
            vec![applications.to_string_lossy().into_owned()],
        ),
        (
            "gtk-update-icon-cache",
            vec![
                "-f".to_owned(),
                "-t".to_owned(),
                hicolor.to_string_lossy().into_owned(),
            ],
        ),
    ] {
        let _ = runner.run(&command(file, args, env.to_vec()), REFRESH_TIMEOUT, None);
    }
}

/// `refreshKdeMenuCache(dataHome)`: `kbuildsycoca6`/`kbuildsycoca5 --noincremental` системной Plasma —
/// без библиотек и Qt-плагинов AppImage, с `XDG_DATA_HOME`.
pub fn refresh_kde_menu_cache(data_home: &Path, runner: &dyn SyncRunner, env: &[(String, String)]) {
    let mut kde: EnvVars = env
        .iter()
        .filter(|(name, _)| name != "XDG_DATA_HOME")
        .cloned()
        .collect();
    kde.push((
        "XDG_DATA_HOME".to_owned(),
        data_home.to_string_lossy().into_owned(),
    ));
    kde.retain(|(name, _)| !KDE_DROPPED.contains(&name.as_str()));
    for file in ["kbuildsycoca6", "kbuildsycoca5"] {
        let _ = runner.run(
            &command(file, vec!["--noincremental".to_owned()], kde.clone()),
            REFRESH_TIMEOUT,
            None,
        );
    }
}

/// Параметры [`ensure_app_image_desktop`].
#[derive(Debug, Clone)]
pub struct AppImageDesktop {
    /// Что запускать (`<корень>/start`).
    pub executable: String,
    /// Исходный значок (`resources/icons/hlsr.png`).
    pub icon_source: PathBuf,
    pub data_home: PathBuf,
    /// Обновить кеши меню после записи (по умолчанию да).
    pub refresh: bool,
}

/// `ensureAppImageDesktop(options)`: пункт меню и значок. `false` — исполняемого файла или значка нет,
/// путь не годится для `Exec` или пункт меню чужой (без маркера).
pub fn ensure_app_image_desktop(
    options: &AppImageDesktop,
    runner: &dyn SyncRunner,
    env: &[(String, String)],
) -> Result<bool, String> {
    let Some(exec_path) = desktop_exec_path(&options.executable) else {
        return Ok(false);
    };
    if !Path::new(&options.executable).exists() || !options.icon_source.exists() {
        return Ok(false);
    }
    let desktop_file = desktop_path(&options.data_home);
    let icon_file = icon_path(&options.data_home);
    // Пустой файл для 0.4.7 — как его отсутствие (`previous && …`): пункт записывается заново.
    let previous = std::fs::read(&desktop_file)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .filter(|text| !text.is_empty());
    if previous
        .as_ref()
        .is_some_and(|text| !text.contains(MANAGED_MARKER))
    {
        return Ok(false);
    }
    let desktop = desktop_entry(&exec_path);
    for dir in [desktop_file.parent(), icon_file.parent()]
        .into_iter()
        .flatten()
    {
        super::make_dirs(dir, 0o777).map_err(|error| super::fs_error(&error, "mkdir", dir))?;
    }
    let pid = std::process::id();
    let temp = |file: &Path| PathBuf::from(format!("{}.{pid}.tmp", file.display()));
    let (desktop_temp, icon_temp) = (temp(&desktop_file), temp(&icon_file));
    let written = (|| -> Result<(), String> {
        std::fs::copy(&options.icon_source, &icon_temp).map_err(|error| {
            super::fs_error2(&error, "copyfile", &options.icon_source, &icon_temp)
        })?;
        std::fs::rename(&icon_temp, &icon_file)
            .map_err(|error| super::fs_error2(&error, "rename", &icon_temp, &icon_file))?;
        if previous.as_deref() != Some(desktop.as_str()) {
            super::write_file(&desktop_temp, desktop.as_bytes(), 0o644, false)
                .map_err(|error| super::fs_error(&error, "open", &desktop_temp))?;
            std::fs::rename(&desktop_temp, &desktop_file).map_err(|error| {
                super::fs_error2(&error, "rename", &desktop_temp, &desktop_file)
            })?;
        }
        Ok(())
    })();
    for file in [&desktop_temp, &icon_temp] {
        let _ = std::fs::remove_file(file);
    }
    written?;
    if options.refresh {
        refresh_menu_caches(&options.data_home, runner, env);
    }
    Ok(true)
}

/// `ensureLinuxIntegration()` (`index.js:150-165`): у управляемой установки — команда `hlsr` и пункт
/// меню на `start`; ошибки — только в журнал (`[cli]`, `[desktop]`). `icon_source` — значок ресурсов
/// запущенной версии, `data_home` и `home` — каталоги пользователя.
pub fn ensure_linux_integration(
    executable: &Path,
    icon_source: &Path,
    dirs: &super::LinuxDirs,
    runner: &dyn SyncRunner,
    env: &[(String, String)],
    log: &dyn Logger,
) {
    let Some(root) = archive_install::managed_root(executable) else {
        return;
    };
    if let Err(error) = ensure_managed_cli(&root, &dirs.home) {
        log.warn(&format!("[cli] {error}"));
    }
    let options = AppImageDesktop {
        executable: root.join("start").to_string_lossy().into_owned(),
        icon_source: icon_source.to_owned(),
        data_home: dirs.data_home.clone(),
        refresh: true,
    };
    if let Err(error) = ensure_app_image_desktop(&options, runner, env) {
        log.warn(&format!("[desktop] {error}"));
    }
}
