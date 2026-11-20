//! Удаление лаунчера на Linux (`uninstall/launcher-uninstall.js` 0.4.9): управляемый корень
//! (`~/.local/opt/hlsr` с маркером или осиротевший корень по умолчанию с `versions/`), план с размерами
//! для диалога, удаление команды `hlsr`, пункта меню, копий на рабочем столе, значка и корня. Игры — только
//! по явному согласию: наши подкаталоги библиотеки (Tools, Downloads, Backups, Demos — только в папке
//! `HLSR`), из `Games` — только папки с `installed.json` этой игры; чужое остаётся (`left`). Полное
//! удаление уносит и данные лаунчера (`~/.config/hlsr`, `~/.local/state/hlsr`).
//!
//! Отступление (решение 17 спецификации): копия `hlsr.desktop` ищется на рабочем столе из
//! `XDG_DESKTOP_DIR` (`LinuxDirs::desktop`), а не только в `~/Desktop`, — на системах с локализованным
//! рабочим столом 0.4.7 оставляла её.
//!
//! Отступления от 0.4.9 (ревью U-c): нечитаемый каталог не прерывает удаление (`readdirSync` бросал, окно
//! показывало ошибку, а лаунчер, ярлыки и данные оставались) — путь уходит в `failed`, остальное
//! удаляется; корень библиотеки-ссылку не обходим (план и так даёт ей 0 байт и не предлагает галочку);
//! каталог данных, который [`own_data_dir`] не отдаёт (ссылка на пути), не молчит, а уходит в `failed`.

use super::desktop_integration::{self, MANAGED_MARKER};
use super::library_cleanup::{self, Failure, Removal};
use super::{LinuxDirs, archive_install};
use crate::installer_window::UninstallPlan;
use std::path::{Path, PathBuf};

/// Подкаталоги библиотеки, которые создаёт HLSR (раскладка `game-delivery/paths.js`, Demos — архивы
/// демок); чужое в корне не трогаем.
pub const LIBRARY_KNOWN_SUBDIRS: [&str; 6] =
    ["Games", "Tools", "Downloads", "Backups", ".hlsr", "Demos"];

/// Отказ (`error.code`): `NOT_MANAGED` или `PATH_INVALID`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct UninstallError {
    pub code: &'static str,
    pub message: String,
}

fn fail(code: &'static str, message: impl Into<String>) -> UninstallError {
    UninstallError {
        code,
        message: message.into(),
    }
}

/// Корень ФС или домашний каталог.
fn refusal(target: &Path, home: &Path) -> bool {
    let full = super::resolve(target);
    full.parent().is_none() || full == super::resolve(home)
}

/// `assertManagedRoot(root)`: каталог (не ссылка, не корень, не дом) с маркером `{"schema":1}` и
/// каталогом `versions`, либо корень по умолчанию с уцелевшим `versions/`.
pub fn assert_managed_root(root: &Path, home: &Path) -> Result<PathBuf, UninstallError> {
    let full = super::resolve(root);
    let not_found = || fail("NOT_MANAGED", "Управляемая установка HLSR не найдена");
    let not_like = || fail("NOT_MANAGED", "Каталог не похож на установку HLSR");
    let stat = std::fs::symlink_metadata(&full).map_err(|_| not_found())?;
    if !stat.is_dir() || stat.file_type().is_symlink() || refusal(&full, home) {
        return Err(not_found());
    }
    let marker = std::fs::read(full.join(archive_install::INSTALL_MARKER))
        .ok()
        .and_then(|bytes| hlsr_core::json_text::json_parse(&String::from_utf8_lossy(&bytes)).ok());
    if marker
        .as_ref()
        .and_then(|marker| marker.get("schema"))
        .and_then(serde_json::Value::as_f64)
        == Some(1.0)
    {
        let versions = std::fs::symlink_metadata(full.join("versions")).map_err(|_| not_like())?;
        if !versions.is_dir() {
            return Err(not_like());
        }
        return Ok(full);
    }
    if full == super::resolve(&archive_install::default_root(home))
        && std::fs::symlink_metadata(full.join("versions")).is_ok_and(|versions| versions.is_dir())
    {
        return Ok(full);
    }
    Err(not_like())
}

/// `isManagedRoot(root)`.
pub fn is_managed_root(root: &Path, home: &Path) -> bool {
    assert_managed_root(root, home).is_ok()
}

/// `treeSize(target)`: сумма размеров файлов без перехода по ссылкам; ссылка — 0, нечитаемое — 0.
/// Её же берёт план удаления в Windows (`windows::uninstall::plan`).
pub fn tree_size(target: &Path) -> u64 {
    let Ok(stat) = std::fs::symlink_metadata(target) else {
        return 0;
    };
    if stat.file_type().is_symlink() {
        return 0;
    }
    if !stat.is_dir() {
        return stat.len();
    }
    hlsr_core::js::fs::read_dir_names(target)
        .map(|names| names.iter().map(|name| tree_size(&target.join(name))).sum())
        .unwrap_or(0)
}

fn exists(target: &Path) -> bool {
    std::fs::symlink_metadata(target).is_ok()
}

fn has_marker(file: &Path) -> bool {
    std::fs::read(file).is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(MANAGED_MARKER))
}

/// `managedDesktopEntry()`: наш пункт меню.
pub fn managed_desktop_entry(dirs: &LinuxDirs) -> Option<PathBuf> {
    let file = desktop_integration::desktop_path(&dirs.data_home);
    has_marker(&file).then_some(file)
}

/// Наши копии `hlsr.desktop` на рабочем столе: `XDG_DESKTOP_DIR` и `~/Desktop` (решение 17).
pub fn managed_desktop_copies(dirs: &LinuxDirs) -> Vec<PathBuf> {
    let mut files = vec![dirs.desktop.join(desktop_integration::DESKTOP_FILE)];
    let legacy = dirs
        .home
        .join("Desktop")
        .join(desktop_integration::DESKTOP_FILE);
    if !files.contains(&legacy) {
        files.push(legacy);
    }
    files.into_iter().filter(|file| has_marker(file)).collect()
}

/// `planLauncherUninstall({installRoot, libraryRoot})`: размеры для диалога удаления.
pub fn plan_launcher_uninstall(
    install_root: &Path,
    library_root: &str,
    dirs: &LinuxDirs,
) -> Result<UninstallPlan, UninstallError> {
    let install_root = assert_managed_root(install_root, &dirs.home)?;
    let raw = hlsr_core::js::trim(library_root);
    if raw.is_empty() {
        return Err(fail("PATH_INVALID", "Путь библиотеки не задан"));
    }
    if !Path::new(raw).is_absolute() {
        return Err(fail(
            "PATH_INVALID",
            format!("Недопустимый путь библиотеки: {raw}"),
        ));
    }
    let library_root = super::resolve(Path::new(raw));
    if refusal(&library_root, &dirs.home) {
        return Err(fail(
            "PATH_INVALID",
            "Библиотекой не может быть корень диска или домашний каталог",
        ));
    }
    let library_exists = exists(&library_root);
    Ok(UninstallPlan {
        managed: true,
        install_bytes: tree_size(&install_root),
        install_root: install_root.to_string_lossy().into_owned(),
        library_root: Some(library_root.to_string_lossy().into_owned()),
        library_exists,
        library_bytes: if library_exists {
            tree_size(&library_root)
        } else {
            0
        },
        desktop_entry: managed_desktop_entry(dirs).map(|file| file.to_string_lossy().into_owned()),
        ..UninstallPlan::default()
    })
}

fn remove_if_managed(file: Option<&Path>, result: &mut Removal) -> bool {
    let Some(file) = file else {
        return false;
    };
    match std::fs::remove_file(file) {
        Ok(()) => {
            result.removed += 1;
            true
        }
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                result.failed.push(Failure {
                    path: file.to_string_lossy().into_owned(),
                    error: hlsr_core::js::errno::code(&error)
                        .map(|code| code.into_owned())
                        .unwrap_or_else(|| error.to_string()),
                });
            }
            false
        }
    }
}

/// `removeTreeNoFollow` без прерывания: отказ или нечитаемый каталог — запись в `failed`.
fn remove_tree(target: &Path, home: &Path, result: &mut Removal) {
    if let Err(error) = library_cleanup::remove_tree_no_follow(target, home, result) {
        result.failed.push(Failure {
            path: target.to_string_lossy().into_owned(),
            error,
        });
    }
}

fn game_id_of(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(dir.join("installed.json")).ok()?;
    let value = hlsr_core::json_text::json_parse(&String::from_utf8_lossy(&bytes)).ok()?;
    value.get("gameId")?.as_str().map(str::to_owned)
}

/// `removeLibrary(libraryRoot, result)`: только наши подкаталоги, из `Games` — только папки игр
/// (`installed.json` этой игры). Tools, Downloads, Backups, Demos — целиком, только если корень — папка
/// `HLSR` (`~/Games/HLSR`): во вписанной руками `~/Games` папки с такими именами могут быть чужими.
/// Чужое остаётся (`left`, пути от корня), корень — если опустел.
fn remove_library(library_root: &Path, home: &Path, result: &mut Removal, left: &mut Vec<String>) {
    // Имя — ровно «HLSR», как его даёт сам HLSR: установщик Linux маркер библиотеки не пишет.
    let own: &[&str] =
        if super::resolve(library_root).file_name() == Some(std::ffi::OsStr::new("HLSR")) {
            &LIBRARY_KNOWN_SUBDIRS
        } else {
            &["Games", ".hlsr"]
        };
    for name in own {
        let target = library_root.join(name);
        let Ok(stat) = std::fs::symlink_metadata(&target) else {
            continue;
        };
        if *name != "Games" || !stat.is_dir() || stat.file_type().is_symlink() {
            remove_tree(&target, home, result);
            continue;
        }
        let games = hlsr_core::js::fs::read_dir_names(&target).unwrap_or_else(|error| {
            result.failed.push(Failure {
                path: target.to_string_lossy().into_owned(),
                error: hlsr_core::js::errno::code(&error)
                    .map(|code| code.into_owned())
                    .unwrap_or_else(|| error.to_string()),
            });
            Vec::new()
        });
        for game in games {
            let dir = target.join(&game);
            if game_id_of(&dir).as_deref() == Some(game.as_str()) {
                remove_tree(&dir, home, result);
            } else {
                left.push(Path::new(name).join(&game).to_string_lossy().into_owned());
            }
        }
        // В Games осталось чужое — каталог остаётся.
        let _ = std::fs::remove_dir(&target);
    }
    // Пустые папки раскладки HLSR создаёт в любой библиотеке сам: их — убрать.
    for name in LIBRARY_KNOWN_SUBDIRS {
        let _ = std::fs::remove_dir(library_root.join(name));
    }
    if let Ok(names) = hlsr_core::js::fs::read_dir_names(library_root) {
        let empty = names.is_empty();
        left.extend(names.into_iter().filter(|name| name != "Games"));
        if empty {
            let _ = std::fs::remove_dir(library_root);
        }
    }
}

/// `ownDataDir(dir)`: каталог данных HLSR (`~/.config/hlsr`, `~/.local/state/hlsr`) — своё имя, не корень
/// и не домашний каталог, без ссылок на пути.
pub fn own_data_dir(dir: &Path, home: &Path) -> bool {
    let full = super::resolve(dir);
    full.file_name() == Some(std::ffi::OsStr::new("hlsr"))
        && !refusal(&full, home)
        && library_cleanup::find_reparse_point(&full).is_none()
}

/// Итог удаления.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallOutcome {
    /// `ok` или `partial`.
    pub status: &'static str,
    pub failed: Vec<Failure>,
    pub games_removed: bool,
    pub desktop_removed: bool,
    pub icon_removed: bool,
    pub cli_removed: bool,
    pub library_root: String,
    /// Чужое в библиотеке, что удаление не тронуло (пути от корня библиотеки).
    pub left: Vec<String>,
}

/// `uninstallLauncher({installRoot, libraryRoot, removeGames, dataDirs})`: полное удаление
/// (`remove_games`) уносит и `data_dirs`, прошедшие [`own_data_dir`].
pub fn uninstall_launcher(
    install_root: &Path,
    library_root: &str,
    remove_games: bool,
    data_dirs: &[PathBuf],
    dirs: &LinuxDirs,
) -> Result<UninstallOutcome, UninstallError> {
    let plan = plan_launcher_uninstall(install_root, library_root, dirs)?;
    let install_root = PathBuf::from(&plan.install_root);
    let library_root = PathBuf::from(plan.library_root.clone().unwrap_or_default());
    let mut result = Removal::default();

    let mut left = Vec::new();
    let mut games_removed = false;
    // Корень-ссылку не обходим: удаление шло бы в каталог по её цели.
    let library_link =
        std::fs::symlink_metadata(&library_root).is_ok_and(|stat| stat.file_type().is_symlink());
    if remove_games && plan.library_exists && !library_link {
        remove_library(&library_root, &dirs.home, &mut result, &mut left);
        games_removed = true;
    }
    if remove_games {
        for dir in data_dirs {
            if own_data_dir(dir, &dirs.home) {
                remove_tree(dir, &dirs.home, &mut result);
            } else if exists(dir) {
                // Данные остались — окно скажет «Часть файлов осталась», а не промолчит.
                result.failed.push(Failure {
                    path: dir.to_string_lossy().into_owned(),
                    error: "ссылка на пути".to_owned(),
                });
            }
        }
    }

    let desktop = managed_desktop_entry(dirs);
    let copies = managed_desktop_copies(dirs);
    let cli = desktop_integration::managed_cli_path(&install_root, &dirs.home);
    let cli_removed = remove_if_managed(cli.as_deref(), &mut result);
    let mut desktop_removed = remove_if_managed(desktop.as_deref(), &mut result);
    for copy in &copies {
        if remove_if_managed(Some(copy), &mut result) {
            desktop_removed = true;
        }
    }
    let mut icon_removed = false;
    if desktop_removed {
        let icon = desktop_integration::icon_path(&dirs.data_home);
        match std::fs::remove_file(&icon) {
            Ok(()) => {
                result.removed += 1;
                icon_removed = true;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => result.failed.push(Failure {
                path: icon.to_string_lossy().into_owned(),
                error: hlsr_core::js::errno::code(&error)
                    .map(|code| code.into_owned())
                    .unwrap_or_else(|| error.to_string()),
            }),
        }
    }

    remove_tree(&install_root, &dirs.home, &mut result);

    Ok(UninstallOutcome {
        status: if result.failed.is_empty() {
            "ok"
        } else {
            "partial"
        },
        failed: result.failed,
        games_removed,
        desktop_removed,
        icon_removed,
        cli_removed,
        library_root: library_root.to_string_lossy().into_owned(),
        left,
    })
}
