//! Удаление HLSR в Windows (`windows-install/uninstall.js` 0.4.9). Запускается самим `HLSR.exe` с
//! `--hlsr-uninstall` (так записано в «Программах и компонентах») и открывает то же окно, что и
//! установщик. С галочкой «Также удалить игры» удаление полное: игры, папка HLSR и данные в
//! `%APPDATA%` и `%LOCALAPPDATA%` (настройки, вход, журналы). Без неё данные остаются, и
//! переустановка сохранит вход.
//!
//! Каталог программы и данные удаляются уже после выхода: работающий `HLSR.exe` держит файлы. Перед
//! этим проверяется, что каталог наш, — так же, как это делал прежний деинсталлятор: путь не
//! системный, на пути нет junction и symlink, рядом лежит `HLSR.exe`, реестр указывает на этот
//! каталог, а маркер владения байт в байт совпадает с `installId` из реестра. Чужие установки HLSR
//! (другой маркер владения) рядом и в библиотеке не трогаются.

use super::file_list;
use super::install::{remove_all, sz_value};
use super::layout::{self, EnvLookup, Identity, win_path};
use super::processes::{Command, Env, Processes};
use super::registry::{self, Change, Registry, ValueChange};
use super::shortcuts::Shortcuts;
use crate::errors::cleanup_exit;
use crate::installer_window::UninstallPlan;
use crate::linux::launcher_uninstall::tree_size;
use crate::uninstall::library_cleanup::{self, CleanupLibraryResult, UNREADABLE_MARKER};
use crate::updater::launch::{UPDATER_EXE, is_stage_name};
use base64::prelude::{BASE64_STANDARD, Engine as _};
use hlsr_core::json_text::parse_js_json;
use hlsr_core::logger::Logger;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Протокол Discord Rich Presence (`discord-rpc/constants.js` `APPLICATION_ID`): ключ
/// `HKCU\Software\Classes\discord-<id>`.
pub const DISCORD_PROTOCOL: &str = "discord-1531057982518071296";
/// Профиль Chromium апдейтера 0.4.x (`updater-mode.js` `APP_NAME`) в `%APPDATA%`.
pub const UPDATER_APP_NAME: &str = "HLSR Updater";
/// Сколько ждать короткий PowerShell, который запускает скрипт удаления.
pub const SCHEDULE_TIMEOUT: Duration = Duration::from_secs(30);

fn exists(target: &str) -> bool {
    std::fs::symlink_metadata(target).is_ok()
}

/// `realDir(dir)`: каталог в полной форме (регистр и короткие имена 8.3 раскрыты); нет пути или он не
/// читается — как есть. Только для сравнения: ссылки на пути проверяются по исходному пути.
fn real_dir(dir: &str) -> String {
    let resolved = win_path().resolve(&[dir]);
    library_cleanup::real_path(&resolved).unwrap_or(resolved)
}

fn same_dir(a: &str, b: &str) -> bool {
    layout::is_same_dir(&real_dir(a), &real_dir(b))
}

/// Свой маркер владения или `None`, если его нет или он не читается.
fn own_marker_of(install_dir: &str) -> Option<String> {
    library_cleanup::owner_marker_of(install_dir).filter(|marker| marker != UNREADABLE_MARKER)
}

/// `otherInstall(root, installDir)`: другая установка HLSR (маркер владения не этой установки) на
/// глубине до трёх уровней, без перехода по ссылкам; свой каталог программы пропускается.
fn other_install(root: &str, install_dir: &str) -> Option<String> {
    fn walk(dir: &str, level: u32, install_dir: &str, own: Option<&str>) -> Option<String> {
        if layout::is_same_dir(dir, install_dir) {
            return None;
        }
        let marker = library_cleanup::owner_marker_of(dir);
        if marker.is_some() && marker.as_deref() != own {
            return Some(dir.to_owned());
        }
        if level >= 3 {
            return None;
        }
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() || kind.is_symlink() {
                continue;
            }
            let child = win_path().join(&[dir, &entry.file_name().to_string_lossy()]);
            if let Some(found) = walk(&child, level + 1, install_dir, own) {
                return Some(found);
            }
        }
        None
    }
    let own = own_marker_of(install_dir);
    walk(&win_path().resolve(&[root]), 0, install_dir, own.as_deref())
}

/// Папка, которую полное удаление не трогает, и почему: `unverified` (установку не удалось
/// проверить), `test` (не боевая сборка), `other` (в папке другая установка HLSR).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    pub root: String,
    pub why: &'static str,
    pub reason: String,
}

/// `libraryRoots(installDir, libraryRoot, identity, problem)`: что очищает полное удаление —
/// библиотеку и папку вокруг каталога программы с маркером библиотеки; остальное — в `blocked`.
pub fn library_roots(
    install_dir: &str,
    library_root: Option<&str>,
    id: &Identity,
    problem: Option<&str>,
) -> (Vec<String>, Vec<Blocked>) {
    let path = win_path();
    let parent = path.dirname(&path.resolve(&[install_dir]));
    let parent_root = library_cleanup::read_marker(&parent)
        .is_some()
        .then_some(parent);
    let mut roots: Vec<String> = Vec::new();
    let mut blocked: Vec<Blocked> = Vec::new();
    for root in [library_root.map(str::to_owned), parent_root]
        .into_iter()
        .flatten()
    {
        let seen = roots
            .iter()
            .chain(blocked.iter().map(|item| &item.root))
            .any(|other| layout::is_same_dir(other, &root));
        if root.is_empty() || seen {
            continue;
        }
        if let Some(problem) = problem {
            blocked.push(Blocked {
                reason: format!("Игры не удалены: установку не удалось проверить ({problem})."),
                root,
                why: "unverified",
            });
        } else if !is_production(id, problem) {
            blocked.push(Blocked {
                reason: format!(
                    "Игры не удалены: тестовая сборка {} не удаляет библиотеку {root}, ею может пользоваться основной HLSR.",
                    id.product_name
                ),
                root,
                why: "test",
            });
        } else if other_install(&root, install_dir).is_some() {
            blocked.push(Blocked {
                reason: format!("Игры не удалены: в папке {root} есть другая установка HLSR."),
                root,
                why: "other",
            });
        } else {
            roots.push(root);
        }
    }
    (roots, blocked)
}

/// GUID сборки из маркера владения каталога программы (строчными) или `None`.
fn marker_guid(install_dir: &str) -> Option<String> {
    let marker = library_cleanup::owner_marker_of(install_dir)?;
    let value = parse_js_json(&marker).ok()?;
    let guid = value.get("appId")?.as_str()?;
    layout::is_guid(guid, false).then(|| guid.to_owned())
}

/// `identityOf(installDir)`: сборка каталога программы — по списку файлов, если его GUID совпал с
/// маркером владения. Иначе — по GUID маркера, и тогда она не боевая (ключи реестра другой сборки
/// так не подставятся). Ни списка, ни маркера — боевая по ключам, но без маркера
/// [`ownership_problem`] не даст ни полного удаления, ни чужих ключей.
pub fn identity_of(install_dir: &str) -> Identity {
    let guid = marker_guid(install_dir);
    let listed = file_list::read(Path::new(install_dir))
        .ok()
        .and_then(|list| layout::identity(Some(&list.app)).ok());
    if let Some(id) = listed
        && guid.as_ref().is_none_or(|guid| *guid == id.guid)
    {
        return id;
    }
    match guid {
        Some(guid) => layout::identity_of_guid(&guid).expect("GUID маркера проверен"),
        None => layout::identity(None).expect("боевое имя продукта допустимо"),
    }
}

/// Боевая сборка подтверждена: `appId` боевой и маркер владения совпал с реестром.
fn is_production(id: &Identity, problem: Option<&str>) -> bool {
    id.app_id == layout::PRODUCTION_APP_ID && problem.is_none()
}

/// `plan({ installDir, libraryRoot, identity, registry })`: размеры для окна подтверждения удаления —
/// свои папки библиотеки (без каталога программы и чужих файлов; вложенная библиотека уходит с
/// программой целиком) и почему переключателя игр нет (`libraryBlocked`). Без `registry` совпадение
/// маркера с реестром не проверяется.
pub fn plan(
    install_dir: &str,
    library_root: Option<&str>,
    identity: &Identity,
    registry: Option<&dyn Registry>,
    env: EnvLookup<'_>,
) -> UninstallPlan {
    // Список файлов и маркер владения должны указывать на одну сборку.
    let problem = if marker_guid(install_dir).as_deref() != Some(identity.guid.as_str()) {
        Some("маркер владения не от этой сборки".to_owned())
    } else {
        registry.and_then(|registry| {
            let values = registry.read(&identity.install_key).unwrap_or_default();
            ownership_problem(install_dir, &values, env, identity)
        })
    };
    let here = real_dir(install_dir);
    let (roots, blocked) = library_roots(&here, library_root, identity, problem.as_deref());
    let existing: Vec<String> = roots.into_iter().filter(|root| exists(root)).collect();
    let owned_bytes = |root: &str| {
        if layout::is_inside(root, &here) {
            return tree_size(Path::new(root));
        }
        library_cleanup::owned_entries(root, Some(&here)).map_or(0, |owned| {
            owned
                .targets
                .iter()
                .map(|target| tree_size(Path::new(target)))
                .sum()
        })
    };
    let library_bytes = existing.iter().map(|root| owned_bytes(root)).sum();
    UninstallPlan {
        managed: true,
        install_root: install_dir.to_owned(),
        install_bytes: tree_size(Path::new(install_dir)),
        library_root: library_root.map(str::to_owned),
        library_exists: !existing.is_empty(),
        library_bytes,
        // Переключателя нет из-за защиты: окно говорит почему (other, test, unverified).
        library_blocked: if existing.is_empty() {
            blocked
                .iter()
                .find(|item| exists(&item.root))
                .map(|item| item.why.to_owned())
        } else {
            None
        },
        library_roots: Some(existing),
        desktop_entry: None,
    }
}

/// Причина отказа удалять каталог программы или `None` (`ownershipProblem`).
pub fn ownership_problem(
    install_dir: &str,
    values: &[(String, registry::RegValue)],
    env: EnvLookup<'_>,
    id: &Identity,
) -> Option<String> {
    if let Some(refused) = layout::refused_dir(install_dir, env) {
        return Some(refused);
    }
    let path = win_path();
    let mut current = path.resolve(&[install_dir]);
    loop {
        match std::fs::symlink_metadata(&current) {
            Ok(stat) if stat.file_type().is_symlink() => {
                return Some(format!("ссылка на пути: {current}"));
            }
            Ok(_) => {}
            Err(_) => return Some(format!("нет каталога {current}")),
        }
        let parent = path.dirname(&current);
        if parent == current {
            break;
        }
        current = parent;
    }
    if !exists(&path.join(&[install_dir, layout::EXE])) {
        return Some(format!("нет {}", layout::EXE));
    }
    // Копия каталога (резервная, перенесённая руками) несёт тот же маркер: установка — только та, на
    // которую указывает реестр.
    let location = sz_value(values, "InstallLocation").filter(|location| !location.is_empty());
    if !location
        .as_deref()
        .is_some_and(|location| same_dir(location, install_dir))
    {
        return Some(format!(
            "это копия, а не установка: в реестре каталог {}",
            location.as_deref().unwrap_or("не указан")
        ));
    }
    let install_id = sz_value(values, "HLSRInstallId").unwrap_or_default();
    if !layout::is_install_id(&install_id) {
        return Some("в реестре нет installId установки".to_owned());
    }
    let marker_file = PathBuf::from(path.join(&[install_dir, layout::OWNER_MARKER]));
    let Ok(bytes) = hlsr_core::js::fs::read_file(&marker_file) else {
        return Some("нет маркера владения".to_owned());
    };
    let text = hlsr_core::js::utf8_string(&bytes);
    let marker = text.trim_end_matches(['\r', '\n']);
    if marker != id.owner_marker(&install_id) {
        return Some("маркер владения не совпадает с реестром".to_owned());
    }
    None
}

/// `ownDataDir(dir, env)`: каталог данных HLSR (`hlsr`, `HLSR Updater`…) прямо в `%APPDATA%` или
/// `%LOCALAPPDATA%`, существующий и без ссылок на пути.
pub fn own_data_dir(dir: &str, env: EnvLookup<'_>) -> bool {
    let path = win_path();
    let full = path.resolve(&[dir]);
    let name = path.basename(&full);
    if !name
        .get(..4)
        .is_some_and(|head| head.eq_ignore_ascii_case("hlsr"))
        || layout::refused_dir(&full, env).is_some()
    {
        return false;
    }
    let parent = path.dirname(&full);
    if !["APPDATA", "LOCALAPPDATA"]
        .iter()
        .filter_map(|name| env(name).filter(|root| !root.is_empty()))
        .any(|root| layout::is_same_dir(&root, &parent))
    {
        return false;
    }
    let mut current = full;
    loop {
        match std::fs::symlink_metadata(&current) {
            Ok(stat) if !stat.file_type().is_symlink() => {}
            _ => return false,
        }
        let parent = path.dirname(&current);
        if parent == current {
            return true;
        }
        current = parent;
    }
}

/// Ответ очистки библиотеки для окна (`libraryWarning`).
pub fn library_warning(result: &CleanupLibraryResult) -> Option<String> {
    if result.ok() {
        return None;
    }
    let root = result.root().unwrap_or("null");
    Some(match result.code() {
        cleanup_exit::REFUSED_RUNNING => format!(
            "Игры не удалены: игра ещё запущена. Закройте её и удалите папку вручную: {root}"
        ),
        cleanup_exit::PARTIAL => {
            format!("Часть файлов удалить не удалось. Оставшиеся файлы здесь: {root}")
        }
        code => {
            let reason = result
                .reason()
                .filter(|reason| !reason.is_empty())
                .map_or_else(|| code.to_string(), str::to_owned);
            format!("Игры не удалены: путь библиотеки не прошёл проверки безопасности ({reason}).")
        }
    })
}

/// Итог удаления: `ok` или `partial` (с предупреждениями).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallStatus {
    Ok,
    Partial,
}

impl UninstallStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            UninstallStatus::Ok => "ok",
            UninstallStatus::Partial => "partial",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallResult {
    pub status: UninstallStatus,
    pub warnings: Vec<String>,
    pub games_removed: bool,
    /// Каталог программы, который удалить после выхода; `None` — оставить.
    pub remove_program: Option<String>,
    /// Данные HLSR, которые удалить после выхода.
    pub remove_data: Vec<String>,
    /// Папка вокруг каталога программы: удалить, если опустела.
    pub remove_parent: Option<String>,
    /// Свой маркер владения: каталог с другим скрипт удаления не трогает.
    pub own_marker: Option<String>,
}

impl UninstallResult {
    /// Есть ли что удалять после выхода.
    pub fn pending(&self) -> bool {
        self.remove_program.is_some() || !self.remove_data.is_empty()
    }
}

/// Очистка библиотеки: `cleanupLibrary(root, { purge: true, installDir, ownMarker })` — корень,
/// каталог программы и свой маркер владения.
pub type CleanupLibrary<'a> = &'a dyn Fn(&str, &str, Option<&str>) -> CleanupLibraryResult;

/// Параметры удаления.
pub struct UninstallOptions<'a> {
    pub install_dir: &'a str,
    pub library_root: Option<&'a str>,
    pub remove_games: bool,
    pub registry: &'a dyn Registry,
    pub shortcuts: &'a Shortcuts,
    pub cleanup_library: CleanupLibrary<'a>,
    pub identity: &'a Identity,
    /// `env`: `refusedDir`, `APPDATA` и `LOCALAPPDATA`.
    pub env: EnvLookup<'a>,
    pub log: &'a dyn Logger,
    /// Данные HLSR, которые уносит полное удаление (`dataDirs`).
    pub data_dirs: &'a [String],
    /// `<имя приложения>-updater`: свой кеш обновлений у каждой сборки; `None` — `hlsr-updater`.
    pub updater_cache_name: Option<&'a str>,
    /// `os.tmpdir()`: копии апдейтера; `None` — не искать.
    pub temp_root: Option<&'a Path>,
    /// `app.removeAsDefaultProtocolClient(name)`; `false` — не удалось.
    pub remove_protocol_client: Option<&'a dyn Fn(&str) -> bool>,
    /// Жив ли процесс игры (`runningGames`); `None` — проверка ОС.
    pub is_alive: Option<&'a dyn Fn(u32) -> bool>,
}

/// `^[A-Za-z0-9._-]+-updater$`.
fn is_updater_cache_name(name: &str) -> bool {
    name.strip_suffix("-updater").is_some_and(|stem| {
        !stem.is_empty()
            && stem
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    })
}

/// `uninstall(options)`: полное удаление (игры и данные) — только если маркер владения совпал с
/// реестром; свои ярлыки, ключи этой сборки, протокол Discord, кеш обновлений, свои копии апдейтера;
/// каталог программы, данные и опустевшая папка HLSR — после выхода.
pub fn uninstall(options: UninstallOptions<'_>) -> UninstallResult {
    let log = options.log;
    let id = options.identity;
    let env = options.env;
    let path = win_path();
    let mut warnings = Vec::new();
    let values = options.registry.read(&id.install_key).unwrap_or_default();
    let problem = ownership_problem(options.install_dir, &values, env, id);
    // Полное удаление (игры, данные) и уборка за боевой сборкой — только если маркер владения
    // совпал с реестром: иначе неизвестно, чья это установка.
    let full = options.remove_games && problem.is_none();
    if options.remove_games
        && let Some(problem) = &problem
    {
        warnings.push(format!(
            "Игры и данные не удалены: установку не удалось проверить ({problem})."
        ));
    }
    let production = is_production(id, problem.as_deref());
    // Сравнения с библиотекой и ярлыками — по полному пути (короткие имена 8.3 раскрыты).
    let here = real_dir(options.install_dir);
    let parent = path.dirname(&here);
    let library_root = options.library_root.filter(|root| !root.is_empty());
    // Старые версии разрешали выбрать библиотеку внутри папки программы.
    let nested = library_root.is_some_and(|root| layout::is_inside(root, &here));
    let mut data_dirs: Vec<String> = if full {
        options
            .data_dirs
            .iter()
            .filter(|dir| own_data_dir(dir, env))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let own_marker = own_marker_of(options.install_dir);
    let alive = options.is_alive.unwrap_or(&library_cleanup::is_alive);

    let mut games_removed = false;
    let mut parent_purged = false;
    if full {
        let (roots, blocked) = library_roots(&here, library_root, id, problem.as_deref());
        warnings.extend(blocked.into_iter().map(|item| item.reason));
        for root in roots {
            // Вложенная библиотека прежних версий уходит с папкой программы, а старая библиотека в
            // %APPDATA%\hlsr\library — с данными, если в ней не запущена игра.
            let in_program =
                nested && library_root.is_some_and(|library| layout::is_same_dir(&root, library));
            let in_data = data_dirs
                .iter()
                .position(|dir| layout::is_inside(&root, dir));
            if in_program || in_data.is_some() {
                if in_program && problem.is_some() {
                    continue;
                }
                if library_cleanup::running_games(&root, alive).is_empty() {
                    games_removed = true;
                } else {
                    warnings.extend(library_warning(&CleanupLibraryResult::Refused {
                        code: cleanup_exit::REFUSED_RUNNING,
                        reason: None,
                        root: Some(root.clone()),
                    }));
                    if let Some(at) = in_data {
                        data_dirs.remove(at);
                    }
                }
                continue;
            }
            let result = (options.cleanup_library)(&root, &here, own_marker.as_deref());
            warnings.extend(library_warning(&result));
            let left = result.left();
            if !left.is_empty() {
                warnings.push(format!(
                    "Оставлено в {} ({}), удаление это не трогает: {}{}.",
                    result.root().unwrap_or("null"),
                    left.len(),
                    left[..left.len().min(10)].join(", "),
                    if left.len() > 10 { ", …" } else { "" }
                ));
            }
            if library_root.is_some_and(|library| layout::is_same_dir(&root, library)) {
                games_removed = result.ok();
            }
            if layout::is_same_dir(&root, &parent) {
                parent_purged = result.ok();
            }
        }
    }
    // Даже при отказе очистки игр нельзя затем удалить их вместе с папкой программы.
    let keep_nested_library = nested && !games_removed;

    let exe = path.join(&[&here, layout::EXE]);
    warnings.extend(options.shortcuts.remove_own(&exe));

    // Ключи этой сборки — если маркер совпал с реестром или ключ указывает на этот каталог
    // (InstallLocation); ключи другой установки не трогаем.
    let mut changes = Vec::new();
    for key in [&id.uninstall_key, &id.install_key] {
        let Some(key_values) = options.registry.read(key) else {
            continue;
        };
        let location =
            sz_value(&key_values, "InstallLocation").filter(|location| !location.is_empty());
        if problem.is_none()
            || location
                .as_deref()
                .is_some_and(|location| same_dir(location, options.install_dir))
        {
            changes.push(Change::Remove { key: key.clone() });
        } else {
            warnings.push(format!(
                "Запись реестра другой установки ({}) не тронута: {key}",
                location.as_deref().unwrap_or("каталог не указан")
            ));
        }
    }
    // InstallerOrigin один на все сборки: его убирает только боевая, ключ — если в нём больше
    // ничего нет.
    if production && let Some(origin) = options.registry.read(layout::ORIGIN_KEY) {
        changes.push(
            if origin.iter().all(|(name, _)| name == "InstallerOrigin") {
                Change::Remove {
                    key: layout::ORIGIN_KEY.to_owned(),
                }
            } else {
                Change::Set {
                    key: layout::ORIGIN_KEY.to_owned(),
                    values: vec![("InstallerOrigin".to_owned(), ValueChange::Delete)],
                }
            },
        );
    }
    if !changes.is_empty()
        && let Err(error) = registry::apply(options.registry, &changes)
    {
        warnings.push(format!(
            "Запись в «Программах и компонентах» не удалена: {error}"
        ));
    }
    // Ключ протокола удаляется, только если он ведёт к этому HLSR.exe.
    if let Some(remove) = options.remove_protocol_client
        && !remove(DISCORD_PROTOCOL)
    {
        log.warn(&format!(
            "[uninstall] протокол {DISCORD_PROTOCOL} не удалён"
        ));
    }

    // Кеш обновлений лаунчера — служебный, свой у каждой сборки: удаление QA-сборки не стирает кеш
    // боевого HLSR.
    let cache_name = options
        .updater_cache_name
        .filter(|name| is_updater_cache_name(name))
        .unwrap_or("hlsr-updater");
    if let Some(local) = env("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        let cache = PathBuf::from(path.join(&[&local, cache_name]));
        if let Err(error) = remove_all(&cache) {
            let message =
                hlsr_core::js::errno::fs_message(&error, "rm", &cache.to_string_lossy(), None);
            log.warn(&format!("[uninstall] кеш обновлений: {message}"));
        }
    }
    // Копии HLSRUpdater.exe в TEMP: лаунчер убирает их только через сутки.
    if let Some(temp) = options.temp_root
        && problem.is_none()
    {
        remove_updater_stages(temp, id, log);
    }

    // Профиль Chromium апдейтера 0.4.x служебный; имя у него от боевого HLSR, QA-сборка его только
    // делит — убирает боевая, всегда.
    let mut data = data_dirs;
    if production && let Some(appdata) = env("APPDATA").filter(|value| !value.is_empty()) {
        data.push(path.join(&[&appdata, UPDATER_APP_NAME]));
    }
    let remove_data: Vec<String> = data
        .iter()
        .enumerate()
        .filter(|(index, dir)| {
            own_data_dir(dir, env)
                && data
                    .iter()
                    .position(|other| layout::is_same_dir(other, dir))
                    == Some(*index)
        })
        .map(|(_, dir)| dir.clone())
        .collect();

    if keep_nested_library {
        warnings.push(format!(
            "Папка программы оставлена, чтобы сохранить вложенную библиотеку игр: {}.",
            library_root.unwrap_or_default()
        ));
    } else if let Some(problem) = &problem {
        warnings.push(format!(
            "Каталог программы не удалён ({problem}): {}. Удалите его вручную, если он больше не нужен.",
            options.install_dir
        ));
    }
    log.info(&format!(
        "[uninstall] HLSR удалён{}",
        problem
            .as_ref()
            .map(|problem| format!(", каталог программы оставлен: {problem}"))
            .unwrap_or_default()
    ));
    let remove_program =
        (problem.is_none() && !keep_nested_library).then(|| options.install_dir.to_owned());
    // Папка вокруг каталога программы уходит, только если после него опустела: очищенная
    // библиотека или папка с именем продукта (C:\HLSR).
    let own_parent =
        parent_purged || path.basename(&parent).to_lowercase() == id.product_name.to_lowercase();
    UninstallResult {
        status: if warnings.is_empty() {
            UninstallStatus::Ok
        } else {
            UninstallStatus::Partial
        },
        warnings,
        games_removed,
        remove_parent: (remove_program.is_some() && own_parent).then_some(parent),
        remove_program,
        remove_data,
        own_marker,
    }
}

/// Свои копии апдейтера в TEMP: в копии лежит маркер владения установки. Чужие и копии без маркера
/// остаются; работающую копию держит её `HLSRUpdater.exe` — её тоже не трогаем.
fn remove_updater_stages(temp_root: &Path, id: &Identity, log: &dyn Logger) {
    let Ok(names) = hlsr_core::js::fs::read_dir_names(temp_root) else {
        return;
    };
    for name in names {
        if !is_stage_name(&name) {
            continue;
        }
        let dir = temp_root.join(&name);
        let marker = library_cleanup::owner_marker_of(&dir.to_string_lossy())
            .and_then(|marker| parse_js_json(&marker).ok());
        if marker
            .as_ref()
            .and_then(|marker| marker.get("appId"))
            .and_then(Value::as_str)
            != Some(id.guid.as_str())
        {
            continue;
        }
        // Запущенный exe Windows не даёт открыть на запись.
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join(UPDATER_EXE))
        {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => continue,
        }
        if let Err(error) = remove_all(&dir) {
            let message =
                hlsr_core::js::errno::fs_message(&error, "rm", &dir.to_string_lossy(), None);
            log.warn(&format!("[uninstall] копия апдейтера {name}: {message}"));
        }
    }
}

/// `app.removeAsDefaultProtocolClient(protocol)` Electron в Windows: раздел
/// `HKCU\Software\Classes\<protocol>` удаляется, только если его команда открытия — `"<exe>" "%1"`
/// этого exe; нет раздела или команда чужая — `true`, ничего не тронуто; `false` — не удалился.
pub fn remove_protocol_client(registry: &dyn Registry, exe: &str, protocol: &str) -> bool {
    let key = format!("HKCU\\Software\\Classes\\{protocol}");
    let command = registry
        .read(&format!("{key}\\shell\\open\\command"))
        .and_then(|values| sz_value(&values, ""));
    if command.as_deref() != Some(format!("\"{exe}\" \"%1\"").as_str()) {
        return true;
    }
    registry.delete_tree(&key).is_ok()
}

/// PowerShell ждёт, пока этот процесс выйдет и отпустит файлы, и удаляет каталог программы и данные
/// (`HLSR_REMOVE_DATA` через «|»: в путях Windows его не бывает), а затем папку
/// `HLSR_REMOVE_PARENT`, если та опустела. Пауза — `Start-Sleep` самого PowerShell. Пути передаются
/// переменными окружения, а не текстом команды. `Remove-NoFollow` не заходит в junction и symlink;
/// каталог с маркером владения другой установки (`HLSR_OWN_MARKER` — свой) или с нечитаемым маркером
/// не трогается; read-only снимается только у самого объекта и не у файла с жёсткими ссылками.
/// Удалённый путь уходит из списка: новая установка на его месте не тронется. С `relaunch` после
/// удаления снова открывается установщик («Назад» в окне удаления) — только когда каталога уже нет.
pub const REMOVAL_SCRIPT: &str = concat!(
    "$ErrorActionPreference = 'SilentlyContinue'; ",
    "function Remove-NoFollow($p) { $i = Get-Item -LiteralPath $p -Force; if (-not $i) { return }; ",
    "$link = $i.Attributes -band 1024; ",
    "if ($i.PSIsContainer -and -not $link) { ",
    "$m = Join-Path $p '.hlsr-install-owner.json'; ",
    "if (Test-Path -LiteralPath $m) { try { if ([IO.File]::ReadAllText($m).TrimEnd() -ne $env:HLSR_OWN_MARKER) { return } } catch { return } }; ",
    "Get-ChildItem -LiteralPath $p -Force | ForEach-Object { Remove-NoFollow $_.FullName } }; ",
    "if (($i.Attributes -band 1) -and $i.LinkType -ne 'HardLink') { $i.Attributes = $i.Attributes -band -bnot 1 }; ",
    "try { $i.Delete() } catch {} }; ",
    "for ($i = 0; $i -lt 60 -and $env:HLSR_WAIT_PID -and (Get-Process -Id $env:HLSR_WAIT_PID); $i++) { Start-Sleep -Seconds 1 }; ",
    "$d = $env:HLSR_REMOVE_DIR; ",
    "$all = @(@($d) + @($env:HLSR_REMOVE_DATA -split '\\|') | Where-Object { $_ -and (Test-Path -LiteralPath $_) }); ",
    "for ($i = 0; $i -lt 60 -and $all.Count; $i++) { $all | ForEach-Object { Remove-NoFollow $_ }; ",
    "$all = @($all | Where-Object { Test-Path -LiteralPath $_ }); if ($all.Count) { Start-Sleep -Seconds 1 } }; ",
    "$q = $env:HLSR_REMOVE_PARENT; ",
    "if ($q -and (Test-Path -LiteralPath $q) -and -not (Get-ChildItem -LiteralPath $q -Force)) { Remove-NoFollow $q }; ",
    "if ($env:HLSR_RELAUNCH -and -not ($d -and (Test-Path -LiteralPath $d))) { Start-Process -FilePath $env:HLSR_RELAUNCH }",
);

/// `<SystemRoot|WINDIR|C:\Windows>\System32\WindowsPowerShell\v1.0\powershell.exe`.
pub fn powershell_path(env: &Env) -> String {
    let get = |name: &str| env.get(name).filter(|value| !value.is_empty());
    let root = get("SystemRoot")
        .or_else(|| get("WINDIR"))
        .unwrap_or_else(|| "C:\\Windows".to_owned());
    win_path().join(&[
        &root,
        "System32",
        "WindowsPowerShell",
        "v1.0",
        "powershell.exe",
    ])
}

/// Что ещё удалить после выхода и кого ждать (`{ data, parent, pid, ownMarker }`).
#[derive(Debug, Clone, Copy, Default)]
pub struct RemovalTargets<'a> {
    pub data: &'a [String],
    pub parent: Option<&'a str>,
    pub pid: Option<u32>,
    pub own_marker: Option<&'a str>,
}

/// `removalCommand(installDir, env, relaunch, targets)`: команда отложенного удаления;
/// `install_dir` — `None`, если удалять нужно только данные. Каждый путь — абсолютный путь Windows
/// без переводов строки, `\0` и `|`, иначе ошибка `Недопустимый путь: …`.
pub fn removal_command(
    install_dir: Option<&str>,
    env: &Env,
    relaunch: Option<&str>,
    targets: &RemovalTargets<'_>,
) -> Result<Command, String> {
    let paths: Vec<&str> = install_dir
        .into_iter()
        .chain(targets.data.iter().map(String::as_str))
        .chain(targets.parent)
        .collect();
    if paths.is_empty() {
        return Err("Недопустимый путь: удалять нечего".to_owned());
    }
    for value in &paths {
        if !win_path().is_absolute(value) || value.contains(['\r', '\n', '\0', '|']) {
            return Err(format!("Недопустимый путь: {value}"));
        }
    }
    let mut child_env = env.clone();
    child_env.set("HLSR_REMOVE_DIR", install_dir.unwrap_or_default());
    child_env.set("HLSR_REMOVE_DATA", &targets.data.join("|"));
    child_env.set("HLSR_REMOVE_PARENT", targets.parent.unwrap_or_default());
    let pid = targets.pid.filter(|pid| *pid != 0);
    child_env.set(
        "HLSR_WAIT_PID",
        &pid.map(|pid| pid.to_string()).unwrap_or_default(),
    );
    child_env.set("HLSR_OWN_MARKER", targets.own_marker.unwrap_or_default());
    if let Some(relaunch) = relaunch {
        child_env.set("HLSR_RELAUNCH", relaunch);
    }
    Ok(Command {
        file: powershell_path(env),
        args: [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
            REMOVAL_SCRIPT,
        ]
        .map(str::to_owned)
        .to_vec(),
        cwd: None,
        env: child_env,
        hidden: true,
    })
}

/// `launcherCommand(command)`: как запустить скрипт, чтобы он пережил выход `HLSR.exe`. PowerShell с
/// `DETACHED_PROCESS` выходит с кодом 0, не выполнив `-Command`, и каталог программы не удалялся.
/// Поэтому короткий PowerShell со скрытой консолью запускает скрипт через `Start-Process` (скрипт —
/// `-EncodedCommand`, пути — в окружении) и сразу выходит.
pub fn launcher_command(command: &Command) -> Command {
    let script = command.args.last().map(String::as_str).unwrap_or_default();
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = BASE64_STANDARD.encode(utf16);
    let launch = format!(
        "Start-Process -FilePath $env:HLSR_POWERSHELL -WindowStyle Hidden -ArgumentList '-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-EncodedCommand','{encoded}'"
    );
    let mut env = command.env.clone();
    env.set("HLSR_POWERSHELL", &command.file);
    Command {
        file: command.file.clone(),
        args: [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &launch,
        ]
        .map(str::to_owned)
        .to_vec(),
        cwd: command.cwd.clone(),
        env,
        hidden: true,
    }
}

/// Почему отложенное удаление не запланировано.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    #[error("{0}")]
    BadPath(String),
    /// Код выхода короткого PowerShell; `null` — не запустился или не уложился в 30 с.
    #[error("скрипт удаления не запущен (код {0})")]
    NotStarted(String),
}

/// `scheduleRemoval(installDir, { env, relaunch, data, parent, pid, ownMarker })`: короткий
/// PowerShell без окна, с рабочим каталогом `os.tmpdir()`, запускает скрипт удаления и выходит.
pub fn schedule_removal(
    processes: &dyn Processes,
    install_dir: Option<&str>,
    env: &Env,
    relaunch: Option<&str>,
    targets: &RemovalTargets<'_>,
    temp_dir: &Path,
) -> Result<(), ScheduleError> {
    let removal =
        removal_command(install_dir, env, relaunch, targets).map_err(ScheduleError::BadPath)?;
    let mut command = launcher_command(&removal);
    command.cwd = Some(temp_dir.to_string_lossy().into_owned());
    match processes.run(&command, Some(SCHEDULE_TIMEOUT)) {
        Some(0) => Ok(()),
        status => Err(ScheduleError::NotStarted(
            status.map_or_else(|| "null".to_owned(), |code| code.to_string()),
        )),
    }
}
