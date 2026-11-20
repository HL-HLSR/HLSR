//! Очистка после удаления (`uninstall/library-cleanup.js` 0.4.9): библиотека игр, настройки и
//! профиль. Удаляется только то, что прошло проверки: путь с буквой диска, вне системных каталогов,
//! профиля пользователя и библиотек Steam, без ссылок на пути, с каноническим путём и маркером
//! `.hlsr\library.json` (не ссылкой); ни одна игра не запущена. Удаление не переходит по ссылкам и
//! не трогает каталоги других установок HLSR (маркер владения). С `purge` уходит только своё
//! ([`owned_entries`]), чужое перечисляется в `left`.
//!
//! Маркер библиотеки (запись) — в доставке (ПП2); здесь он только читается. Библиотеки Steam —
//! порт [`SteamLibraries`] (реализация — `SteamLocator` ПП3 в приложении). Пути — по правилам `path`
//! этой ОС, как у 0.4.7 (модуль работал с родным `path`).

use crate::errors::cleanup_exit as codes;
use hlsr_core::js::path::NodePath;
use hlsr_core::json_text::parse_js_json;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// `.hlsr\library.json` (`MARKER_RELATIVE`, разделитель этой ОС).
pub fn marker_relative() -> String {
    NodePath::native().join(&[".hlsr", "library.json"])
}

pub const MARKER_SCHEMA: f64 = 1.0;
pub const MARKER_ID: &str = "hlsr-library";
/// Зоны библиотеки, которые удаляются без `--purge`.
pub const OWNED_ZONES: [&str; 3] = [".hlsr", "Games", "Downloads"];
/// Файлы профиля в `userData`, которые `--keep-profile` сохраняет.
pub const PROFILE_FILES: [&str; 1] = ["game-stats.json"];

/// Окружение: `(options && options.env) || process.env`.
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Библиотеки Steam (`steamZones`): удаление их не трогает.
pub trait SteamLibraries: Send + Sync {
    /// Каталоги библиотек; ошибка поиска Steam — пустой список.
    fn libraries(&self) -> Vec<String>;
}

/// Заданный список библиотек (`options.steamLibraries`).
#[derive(Debug, Clone, Default)]
pub struct StaticSteamLibraries(pub Vec<String>);

impl SteamLibraries for StaticSteamLibraries {
    fn libraries(&self) -> Vec<String> {
        self.0.clone()
    }
}

/// Параметры очистки (`options` 0.4.7). Пустые строки — как отсутствие (`opts.x ? … : null`).
#[derive(Default)]
pub struct CleanupOptions<'a> {
    /// `None` — окружение процесса.
    pub env: Option<EnvLookup<'a>>,
    /// `None` — `USERPROFILE`, затем домашний каталог ОС.
    pub home: Option<String>,
    pub extra_zones: Vec<String>,
    /// `None` — библиотек Steam нет (приложение передаёт `SteamLocator`).
    pub steam: Option<&'a dyn SteamLibraries>,
    pub install_dir: Option<String>,
    pub purge: bool,
    pub user_data: Option<String>,
    pub logs_dir: Option<String>,
    pub updater_cache: Option<String>,
    pub app_name: Option<String>,
    pub preserve_profile: bool,
    /// Жив ли процесс (`process.kill(pid, 0)`); `None` — проверка ОС.
    pub is_alive: Option<&'a dyn Fn(u32) -> bool>,
    /// `ownMarker`: маркер владения своей установки; каталог с другим маркером не удаляется.
    pub own_marker: Option<String>,
}

fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|text| !text.is_empty())
}

impl CleanupOptions<'_> {
    fn get(&self, name: &str) -> Option<String> {
        let lookup = self.env.unwrap_or(&process_env);
        lookup(name).filter(|value| !value.is_empty())
    }
}

fn lower(value: &str) -> String {
    value.to_lowercase()
}

fn path() -> NodePath {
    NodePath::native()
}

/// `forbiddenZones(options)`: системные каталоги, каталоги профиля, дополнительные зоны.
pub fn forbidden_zones(options: &CleanupOptions<'_>) -> Vec<String> {
    let path = path();
    let home = non_empty(&options.home)
        .map(str::to_owned)
        .or_else(|| options.get("USERPROFILE"))
        .or_else(|| std::env::home_dir().map(|dir| dir.to_string_lossy().into_owned()))
        .filter(|home| !home.is_empty());
    let mut zones: Vec<String> = Vec::new();
    let mut add = |value: Option<String>| {
        let Some(value) = value.filter(|value| !value.is_empty()) else {
            return;
        };
        let resolved = path.resolve(&[&value]);
        if zones.iter().any(|zone| lower(zone) == lower(&resolved)) {
            return;
        }
        zones.push(resolved);
    };
    add(options.get("SystemRoot").or_else(|| options.get("windir")));
    for name in [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
        "PUBLIC",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
    ] {
        add(options.get(name));
    }
    if let Some(home) = &home {
        for name in [
            "Desktop",
            "Documents",
            "Downloads",
            "Pictures",
            "Videos",
            "Music",
            "OneDrive",
            "AppData",
        ] {
            add(Some(path.join(&[home, name])));
        }
    }
    for extra in &options.extra_zones {
        add(Some(extra.clone()));
    }
    zones
}

/// `steamZones(options)`.
pub fn steam_zones(options: &CleanupOptions<'_>) -> Vec<String> {
    let path = path();
    options
        .steam
        .map(|steam| {
            steam
                .libraries()
                .iter()
                .map(|library| path.resolve(&[library]))
                .collect()
        })
        .unwrap_or_default()
}

/// `isSameOrInside(parent, candidate)`: без учёта регистра.
pub fn is_same_or_inside(parent: &str, candidate: &str) -> bool {
    let path = path();
    let a = lower(&path.resolve(&[parent]));
    let b = lower(&path.resolve(&[candidate]));
    if a == b {
        return true;
    }
    let sep = path.sep();
    let prefix = if a.ends_with(sep) {
        a
    } else {
        format!("{a}{sep}")
    };
    b.starts_with(&prefix)
}

/// `pathChain(target)`: от корня до самого пути.
pub fn path_chain(target: &str) -> Vec<String> {
    let path = path();
    let mut chain = Vec::new();
    let mut current = path.resolve(&[target]);
    loop {
        chain.insert(0, current.clone());
        let parent = path.dirname(&current);
        if parent == current {
            break;
        }
        current = parent;
    }
    chain
}

/// `findReparsePoint(target)`: первая ссылка (symlink, junction) на пути или нечитаемый элемент;
/// несуществующие пропускаются.
pub fn find_reparse_point(target: &str) -> Option<String> {
    for part in path_chain(target) {
        match std::fs::symlink_metadata(&part) {
            Ok(stat) if stat.file_type().is_symlink() => return Some(part),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Some(part),
        }
    }
    None
}

/// `markerLinked(root)`: `.hlsr` или `library.json` — ссылка (symlink, junction). Такой маркер не
/// считается: он мог бы прийти из чужой папки.
pub fn marker_linked(root: &str) -> bool {
    let path = path();
    for part in [
        path.join(&[root, ".hlsr"]),
        path.join(&[root, &marker_relative()]),
    ] {
        match std::fs::symlink_metadata(&part) {
            Ok(stat) if stat.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

/// `readMarker(root)`: маркер с `schema: 1` и `id: "hlsr-library"`; BOM допускается; маркер-ссылка
/// не считается.
pub fn read_marker(root: &str) -> Option<Value> {
    if marker_linked(root) {
        return None;
    }
    let file = PathBuf::from(path().join(&[root, &marker_relative()]));
    let bytes = hlsr_core::js::fs::read_file(&file).ok()?;
    let text = hlsr_core::js::utf8_string(&bytes);
    let data = parse_js_json(text.strip_prefix('\u{feff}').unwrap_or(&text)).ok()?;
    if !data.is_object() {
        return None;
    }
    if data.get("id").and_then(Value::as_str) != Some(MARKER_ID) {
        return None;
    }
    if data.get("schema").and_then(Value::as_f64) != Some(MARKER_SCHEMA)
        || !data["schema"].is_number()
    {
        return None;
    }
    Some(data)
}

/// Итог `checkLibraryRoot`.
#[derive(Debug, Clone, PartialEq)]
pub struct RootCheck {
    pub ok: bool,
    pub code: i32,
    pub reason: Option<String>,
    pub root: Option<String>,
    pub marker: Option<Value>,
}

impl RootCheck {
    fn refuse(code: i32, reason: impl Into<String>, root: Option<String>) -> RootCheck {
        RootCheck {
            ok: false,
            code,
            reason: Some(reason.into()),
            root,
            marker: None,
        }
    }

    /// Вид 0.4.7: `{ok, code, reason?, root?, marker?}`.
    pub fn to_value(&self) -> Value {
        let mut out = json!({"ok": self.ok, "code": self.code});
        if let Some(reason) = &self.reason {
            out["reason"] = json!(reason);
        }
        if let Some(root) = &self.root {
            out["root"] = json!(root);
        }
        if let Some(marker) = &self.marker {
            out["marker"] = marker.clone();
        }
        out
    }
}

fn is_dangerous(c: char) -> bool {
    matches!(c, '*' | '?' | '"' | '<' | '>' | '|' | '\0')
}

/// `/^[A-Za-z]:[\\/]/`.
fn has_drive_root(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

/// `fs.realpathSync.native(root)`: канонический путь без префикса `\\?\`.
pub fn real_path(root: &str) -> std::io::Result<String> {
    let real = std::fs::canonicalize(root)?;
    let text = real.to_string_lossy().into_owned();
    Ok(match text.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => text
            .strip_prefix(r"\\?\")
            .map_or(text.clone(), str::to_owned),
    })
}

/// `checkLibraryRoot(rawPath, options)`.
pub fn check_library_root(raw_path: &str, options: &CleanupOptions<'_>) -> RootCheck {
    let raw = hlsr_core::js::trim(raw_path);
    if raw.is_empty() {
        return RootCheck::refuse(codes::REFUSED_PATH, "путь библиотеки не задан", None);
    }
    if raw.chars().any(is_dangerous) {
        return RootCheck::refuse(
            codes::REFUSED_PATH,
            "путь содержит недопустимые символы",
            None,
        );
    }
    if raw.starts_with("\\\\") || raw.starts_with("//") {
        return RootCheck::refuse(
            codes::REFUSED_PATH,
            "сетевые и device-пути не поддерживаются",
            None,
        );
    }
    if raw.split(['\\', '/']).any(|part| part == "..") {
        return RootCheck::refuse(codes::REFUSED_PATH, "путь содержит ..", None);
    }
    if !has_drive_root(raw) {
        return RootCheck::refuse(
            codes::REFUSED_PATH,
            "нужен абсолютный путь с буквой диска",
            None,
        );
    }
    let path = path();
    let root = path.resolve(&[raw]);
    if path.dirname(&root) == root {
        return RootCheck::refuse(
            codes::REFUSED_PATH,
            "корень диска библиотекой быть не может",
            None,
        );
    }

    let mut zones = forbidden_zones(options);
    zones.extend(steam_zones(options));

    if let Some(install_dir) = non_empty(&options.install_dir) {
        let install_dir = path.resolve(&[install_dir]);
        let owned_launcher_subdir = lower(&path.dirname(&install_dir)) == lower(&root)
            && lower(&path.basename(&install_dir)) == "launcher";
        if !owned_launcher_subdir
            && (is_same_or_inside(&install_dir, &root) || is_same_or_inside(&root, &install_dir))
        {
            return RootCheck::refuse(
                codes::REFUSED_PATH,
                format!("путь пересекается с каталогом лаунчера {install_dir}"),
                Some(root),
            );
        }
    }
    for zone in &zones {
        if is_same_or_inside(zone, &root) || is_same_or_inside(&root, zone) {
            return RootCheck::refuse(
                codes::REFUSED_PATH,
                format!("путь пересекается с системным каталогом {zone}"),
                Some(root),
            );
        }
    }

    if !Path::new(&root).exists() {
        return RootCheck::refuse(
            codes::NOTHING_TO_DO,
            "каталог библиотеки не существует",
            Some(root),
        );
    }
    if let Some(reparse) = find_reparse_point(&root) {
        return RootCheck::refuse(
            codes::REFUSED_REPARSE,
            format!("в пути есть символьная ссылка или junction: {reparse}"),
            Some(root),
        );
    }
    let real = match real_path(&root) {
        Ok(real) => real,
        Err(error) => {
            return RootCheck::refuse(
                codes::REFUSED_PATH,
                format!("путь не разрешается: {error}"),
                Some(root),
            );
        }
    };
    if lower(&real) != lower(&root) {
        return RootCheck::refuse(
            codes::REFUSED_REPARSE,
            format!("каноничный путь отличается: {real}"),
            Some(root),
        );
    }
    match std::fs::metadata(&root) {
        Ok(stat) if stat.is_dir() => {}
        Ok(_) => {
            return RootCheck::refuse(
                codes::REFUSED_PATH,
                "путь не является каталогом",
                Some(root),
            );
        }
        Err(error) => return RootCheck::refuse(codes::REFUSED_PATH, error.to_string(), Some(root)),
    }
    let Some(marker) = read_marker(&root) else {
        return RootCheck::refuse(
            codes::REFUSED_MARKER,
            format!("нет маркера {} с ожидаемой схемой и id", marker_relative()),
            Some(root),
        );
    };
    RootCheck {
        ok: true,
        code: codes::OK,
        reason: None,
        root: Some(root),
        marker: Some(marker),
    }
}

/// `isAlive(pid)` (`process.kill(pid, 0)`): процесс есть; нет прав на него — тоже «жив».
pub fn is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: открытие процесса по номеру; дескриптор закрывается ниже.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            // SAFETY: код ошибки только что завершившегося вызова этого потока.
            return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        // SAFETY: дескриптор открыт с правом запроса сведений.
        let ok = unsafe { GetExitCodeProcess(handle, &mut code) } != 0;
        // SAFETY: дескриптор получен от OpenProcess и закрывается один раз.
        unsafe { CloseHandle(handle) };
        ok && code == STILL_ACTIVE
    }
    #[cfg(not(windows))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

/// Запущенная игра из `.hlsr\state\*.process.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningGame {
    pub game_id: String,
    pub pid: u32,
}

/// `Number(value)` для `pid` и проверка `Number.isInteger(pid) && pid > 0`.
fn pid_of(value: &Value) -> Option<u32> {
    let number = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => hlsr_core::js::to_number(text),
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        _ => return None,
    };
    (number.is_finite() && number.fract() == 0.0 && number > 0.0 && number <= f64::from(u32::MAX))
        .then_some(number as u32)
}

/// `runningGames(root)`.
pub fn running_games(root: &str, is_alive: &dyn Fn(u32) -> bool) -> Vec<RunningGame> {
    let state_dir = PathBuf::from(path().join(&[root, ".hlsr", "state"]));
    let Ok(names) = hlsr_core::js::fs::read_dir_names(&state_dir) else {
        return Vec::new();
    };
    let mut running = Vec::new();
    for name in names {
        let Some(stem) = name.strip_suffix(".process.json") else {
            continue;
        };
        let Ok(bytes) = hlsr_core::js::fs::read_file(&state_dir.join(&name)) else {
            continue;
        };
        let Ok(record) = parse_js_json(&hlsr_core::js::utf8_string(&bytes)) else {
            continue;
        };
        if !record.is_object() {
            continue;
        }
        let pid_value = record.get("pid").cloned().unwrap_or(Value::Null);
        if !hlsr_core::js::truthy(&pid_value) {
            continue;
        }
        let state = record.get("state").and_then(Value::as_str);
        if !matches!(state, Some("running" | "starting" | "stopping")) {
            continue;
        }
        let Some(pid) = pid_of(&pid_value) else {
            continue;
        };
        if !is_alive(pid) {
            continue;
        }
        let game_id = record
            .get("gameId")
            .filter(|value| hlsr_core::js::truthy(value))
            .map(hlsr_core::js::to_string)
            .unwrap_or_else(|| stem.to_owned());
        running.push(RunningGame { game_id, pid });
    }
    running
}

/// Объект, который не удалился: путь и код ошибки (или текст).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub path: String,
    pub error: String,
}

/// Счёт удаления (`{ removed, failed, kept }`): `kept` — каталоги других установок HLSR.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removal {
    pub removed: u64,
    pub failed: Vec<Failed>,
    pub kept: Vec<String>,
}

impl Removal {
    fn fail(&mut self, path: &Path, error: &std::io::Error) {
        self.failed.push(Failed {
            path: path.to_string_lossy().into_owned(),
            error: hlsr_core::js::errno::code(error)
                .map_or_else(|| error.to_string(), |code| code.into_owned()),
        });
    }

    fn failed_value(&self) -> Value {
        Value::Array(
            self.failed
                .iter()
                .map(|item| json!({"path": item.path, "error": item.error}))
                .collect(),
        )
    }
}

/// Маркер владения Windows-установки HLSR (`windows::layout::OWNER_MARKER`).
pub const INSTALL_OWNER_MARKER: &str = ".hlsr-install-owner.json";

/// Нечитаемый маркер владения (занят, нет прав): считается чужим, не равен ни одному настоящему.
pub const UNREADABLE_MARKER: &str = "\0нечитаемый маркер";

/// `ownerMarkerOf(dir)`: маркер владения без переводов строки в конце; `None` — его нет, при любой
/// другой ошибке чтения — [`UNREADABLE_MARKER`].
pub fn owner_marker_of(dir: &str) -> Option<String> {
    let file = PathBuf::from(path().join(&[dir, INSTALL_OWNER_MARKER]));
    match hlsr_core::js::fs::read_file(&file) {
        Ok(bytes) => Some(
            hlsr_core::js::utf8_string(&bytes)
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(UNREADABLE_MARKER.to_owned()),
    }
}

/// `removeTreeNoFollow(target, result, ownMarker)`: ссылка удаляется сама (без перехода), каталог —
/// после содержимого, файл — `unlink`. Каталог с маркером владения не `own_marker` (чужая установка
/// HLSR) не удаляется никогда: он и его предки остаются (`kept`).
pub fn remove_tree_no_follow(target: &Path, out: &mut Removal, own_marker: Option<&str>) {
    let stat = match std::fs::symlink_metadata(target) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            out.fail(target, &error);
            return;
        }
    };
    if stat.file_type().is_symlink() {
        match std::fs::remove_file(target).or_else(|_| std::fs::remove_dir(target)) {
            Ok(()) => out.removed += 1,
            Err(error) => out.fail(target, &error),
        }
        return;
    }
    if stat.is_dir() {
        let text = target.to_string_lossy();
        let marker = owner_marker_of(&text);
        if marker.is_some() && marker.as_deref() != own_marker {
            out.kept.push(text.into_owned());
            return;
        }
        let names = match hlsr_core::js::fs::read_dir_names(target) {
            Ok(names) => names,
            Err(error) => {
                out.fail(target, &error);
                return;
            }
        };
        let kept = out.kept.len();
        for name in names {
            remove_tree_no_follow(&target.join(name), out, own_marker);
        }
        if out.kept.len() > kept {
            return;
        }
        match std::fs::remove_dir(target) {
            Ok(()) => out.removed += 1,
            Err(error) => out.fail(target, &error),
        }
        return;
    }
    match std::fs::remove_file(target) {
        Ok(()) => out.removed += 1,
        Err(error) => out.fail(target, &error),
    }
}

/// Итог `cleanupLibrary`.
#[derive(Debug, Clone, PartialEq)]
pub enum CleanupLibraryResult {
    /// Отказ: проверка пути, запущенная игра, нечитаемый каталог.
    Refused {
        code: i32,
        reason: Option<String>,
        root: Option<String>,
    },
    Done {
        code: i32,
        root: String,
        zones: Vec<String>,
        root_removed: bool,
        removal: Removal,
        /// Не своё: чужие файлы и папки и каталоги других установок HLSR (от корня).
        left: Vec<String>,
    },
}

impl CleanupLibraryResult {
    pub fn ok(&self) -> bool {
        matches!(self, CleanupLibraryResult::Done { removal, .. } if removal.failed.is_empty())
    }

    pub fn code(&self) -> i32 {
        match self {
            CleanupLibraryResult::Refused { code, .. }
            | CleanupLibraryResult::Done { code, .. } => *code,
        }
    }

    pub fn root(&self) -> Option<&str> {
        match self {
            CleanupLibraryResult::Refused { root, .. } => root.as_deref(),
            CleanupLibraryResult::Done { root, .. } => Some(root),
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            CleanupLibraryResult::Refused { reason, .. } => reason.as_deref(),
            CleanupLibraryResult::Done { .. } => None,
        }
    }

    /// Удалено объектов.
    pub fn removed(&self) -> u64 {
        match self {
            CleanupLibraryResult::Done { removal, .. } => removal.removed,
            CleanupLibraryResult::Refused { .. } => 0,
        }
    }

    /// Оставленное чужое (`left`); у отказа — ничего.
    pub fn left(&self) -> &[String] {
        match self {
            CleanupLibraryResult::Done { left, .. } => left,
            CleanupLibraryResult::Refused { .. } => &[],
        }
    }

    pub fn root_removed(&self) -> bool {
        matches!(
            self,
            CleanupLibraryResult::Done {
                root_removed: true,
                ..
            }
        )
    }

    /// Вид 0.4.7.
    pub fn to_value(&self) -> Value {
        match self {
            CleanupLibraryResult::Refused { code, reason, root } => {
                let mut out = json!({"ok": false, "code": code});
                if let Some(reason) = reason {
                    out["reason"] = json!(reason);
                }
                out["root"] = json!(root);
                out
            }
            CleanupLibraryResult::Done {
                code,
                root,
                zones,
                root_removed,
                removal,
                left,
            } => json!({
                "ok": removal.failed.is_empty(), "code": code, "root": root, "zones": zones,
                "rootRemoved": root_removed, "removed": removal.removed, "failed": removal.failed_value(),
                "left": left,
            }),
        }
    }
}

/// Что в корне библиотеки создаёт HLSR (`game-delivery/paths.js` LAYOUT; `Demos` — архивы демок).
/// Целиком их полное удаление уносит, только если корень — папка HLSR, которую создал HLSR.
pub const OWNED_ENTRIES: [&str; 5] = [".hlsr", "Tools", "Downloads", "Backups", "Demos"];
const HLSR_FOLDER: &str = "hlsr";
const GAMES_DIR: &str = "Games";
/// Игры доставки (`LAUNCH_CONFIG` из `game-delivery/manifests.js` 0.4.9). shortcut: список
/// вписан здесь, а не взят из `hlsr-delivery` (крейт от него не зависит); новая игра — дописать.
pub const KNOWN_GAME_IDS: [&str; 5] = [
    "ghosting-renewed",
    "hl2-4260",
    "hl2-2707",
    "hl2-5288",
    "hl1-won",
];

/// `GAME_ID_RE`: `^[a-z0-9]+(-[a-z0-9]+)*$`.
fn is_game_id(name: &str) -> bool {
    !name.is_empty()
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

fn is_empty_dir(dir: &str) -> bool {
    std::fs::symlink_metadata(dir).is_ok_and(|stat| stat.is_dir() && !stat.file_type().is_symlink())
        && hlsr_core::js::fs::read_dir_names(Path::new(dir)).is_ok_and(|names| names.is_empty())
}

/// `gameId` из `installed.json` папки игры.
fn installed_game_id(dir: &str) -> Option<String> {
    let bytes =
        hlsr_core::js::fs::read_file(&PathBuf::from(path().join(&[dir, "installed.json"]))).ok()?;
    let data = parse_js_json(&hlsr_core::js::utf8_string(&bytes)).ok()?;
    data.get("gameId")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Свои записи в корне библиотеки (`targets`), чужие (`left`, имена) и имя папки игр (`games`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedEntries {
    pub targets: Vec<String>,
    pub left: Vec<String>,
    pub games: String,
}

/// `ownedEntries(root, installDir)`: в папке HLSR (имя `HLSR` и маркер с `createdBy`
/// `installer`/`launcher`) — `.hlsr`, Tools, Downloads, Backups, Demos, остатки замены программы,
/// папки игр (`Games\<известный id>` или с `installed.json` этой игры, в корне — только с
/// `installed.json`); в другой библиотеке — `.hlsr`, `Games\<id>` с `installed.json` и пустые папки
/// раскладки. Ошибка — каталог не читается.
pub fn owned_entries(root: &str, install_dir: Option<&str>) -> std::io::Result<OwnedEntries> {
    let path = path();
    let created_by = read_marker(root).and_then(|marker| {
        marker
            .get("createdBy")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let hlsr_folder = lower(&path.basename(&path.resolve(&[root]))) == HLSR_FOLDER
        && matches!(created_by.as_deref(), Some("installer" | "launcher"));
    let own_names: &[&str] = if hlsr_folder {
        &OWNED_ENTRIES
    } else {
        &[".hlsr"]
    };
    let mut targets = Vec::new();
    let mut left = Vec::new();
    let mut games = GAMES_DIR.to_owned();
    for name in hlsr_core::js::fs::read_dir_names(Path::new(root))? {
        let full = path.join(&[root, &name]);
        if install_dir.is_some_and(|dir| is_same_or_inside(&full, dir)) {
            continue;
        }
        if lower(&name) == lower(GAMES_DIR) {
            games = name.clone();
            let stat = std::fs::symlink_metadata(&full)?;
            // Ссылка или файл на месте Games: удаляется сама ссылка, без перехода.
            if !stat.is_dir() || stat.file_type().is_symlink() {
                targets.push(full);
                continue;
            }
            for game in hlsr_core::js::fs::read_dir_names(Path::new(&full))? {
                let dir = path.join(&[&full, &game]);
                let own = is_game_id(&game)
                    && ((hlsr_folder && KNOWN_GAME_IDS.contains(&game.as_str()))
                        || installed_game_id(&dir).as_deref() == Some(&game));
                if own {
                    targets.push(dir);
                } else {
                    left.push(path.join(&[&name, &game]));
                }
            }
            continue;
        }
        // Пустые Tools, Downloads, Backups создаёт в любой библиотеке сам HLSR.
        let own = own_names.iter().any(|entry| lower(entry) == lower(&name))
            || (OWNED_ENTRIES
                .iter()
                .any(|entry| lower(entry) == lower(&name))
                && is_empty_dir(&full))
            || (hlsr_folder
                && (crate::windows::install::leftover_name(&name).is_some()
                    || (is_game_id(&name) && installed_game_id(&full).as_deref() == Some(&name))));
        if own {
            targets.push(full);
        } else {
            left.push(name);
        }
    }
    Ok(OwnedEntries {
        targets,
        left,
        games,
    })
}

/// `cleanupLibrary(rawPath, options)`: без `purge` — зоны `.hlsr`, `Games`, `Downloads`; с `purge`
/// — своё по [`owned_entries`] (каталог лаунчера внутри не трогается). Пустой корень удаляется.
pub fn cleanup_library(raw_path: &str, options: &CleanupOptions<'_>) -> CleanupLibraryResult {
    let check = check_library_root(raw_path, options);
    if !check.ok {
        return CleanupLibraryResult::Refused {
            code: check.code,
            reason: check.reason,
            root: check.root,
        };
    }
    let root = check.root.unwrap_or_default();
    let alive = options.is_alive.unwrap_or(&is_alive);
    let running = running_games(&root, alive);
    if !running.is_empty() {
        let list: Vec<String> = running
            .iter()
            .map(|game| format!("{} PID {}", game.game_id, game.pid))
            .collect();
        return CleanupLibraryResult::Refused {
            code: codes::REFUSED_RUNNING,
            reason: Some(format!("игра запущена ({})", list.join(", "))),
            root: Some(root),
        };
    }

    let path = path();
    let install_dir = non_empty(&options.install_dir).map(|dir| path.resolve(&[dir]));
    let mut owned = None;
    let targets: Vec<String> = if options.purge {
        match owned_entries(&root, install_dir.as_deref()) {
            Ok(entries) => owned.insert(entries).targets.clone(),
            Err(error) => {
                return CleanupLibraryResult::Refused {
                    code: codes::REFUSED_PATH,
                    reason: Some(format!(
                        "каталог библиотеки не читается: {}",
                        hlsr_core::js::errno::fs_message(&error, "scandir", &root, None)
                    )),
                    root: Some(root),
                };
            }
        }
    } else {
        OWNED_ZONES
            .iter()
            .map(|zone| path.join(&[&root, zone]))
            .collect()
    };

    let mut removal = Removal::default();
    let mut zones = Vec::new();
    for target in targets {
        if !is_same_or_inside(&root, &target) || path.resolve(&[&target]) == root {
            continue;
        }
        if install_dir
            .as_deref()
            .is_some_and(|dir| is_same_or_inside(&target, dir))
        {
            continue;
        }
        if !Path::new(&target).exists() {
            continue;
        }
        remove_tree_no_follow(
            Path::new(&target),
            &mut removal,
            options.own_marker.as_deref(),
        );
        zones.push(target);
    }
    // Games уходит, только если в ней не осталось чужого.
    if let Some(owned) = &owned {
        let _ = std::fs::remove_dir(path.join(&[&root, &owned.games]));
    }

    let root_removed = hlsr_core::js::fs::read_dir_names(Path::new(&root))
        .is_ok_and(|names| names.is_empty())
        && std::fs::remove_dir(&root).is_ok();
    let code = if !removal.failed.is_empty() {
        codes::PARTIAL
    } else if zones.is_empty() {
        codes::NOTHING_TO_DO
    } else {
        codes::OK
    };
    let mut left = owned.map(|owned| owned.left).unwrap_or_default();
    let prefix = format!("{root}{}", path.sep());
    left.extend(removal.kept.iter().map(|dir| {
        dir.strip_prefix(&prefix)
            .map_or_else(|| dir.clone(), str::to_owned)
    }));
    CleanupLibraryResult::Done {
        code,
        root,
        zones,
        root_removed,
        removal,
        left,
    }
}

/// `settingsTargets(options)`: `userData`, журналы, кеш обновлений и каталоги по имени
/// приложения в `%APPDATA%` и `%LOCALAPPDATA%` (без повторов без учёта регистра).
pub fn settings_targets(options: &CleanupOptions<'_>) -> Vec<String> {
    let path = path();
    let mut targets: Vec<String> = Vec::new();
    let mut add = |value: Option<String>| {
        let Some(value) = value.filter(|value| !value.is_empty()) else {
            return;
        };
        let resolved = path.resolve(&[&value]);
        if targets
            .iter()
            .any(|target| lower(target) == lower(&resolved))
        {
            return;
        }
        targets.push(resolved);
    };
    add(options.user_data.clone());
    add(options.logs_dir.clone());
    add(options.updater_cache.clone());
    if let Some(app_name) = non_empty(&options.app_name) {
        if let Some(appdata) = options.get("APPDATA") {
            add(Some(path.join(&[&appdata, app_name])));
        }
        if let Some(local) = options.get("LOCALAPPDATA") {
            add(Some(path.join(&[&local, &format!("{app_name}-updater")])));
        }
    }
    targets
}

/// Итог очистки настроек или профиля.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataCleanup {
    pub code: i32,
    pub targets: Vec<String>,
    pub removal: Removal,
}

impl DataCleanup {
    pub fn ok(&self) -> bool {
        self.removal.failed.is_empty()
            && self.code != codes::REFUSED_PATH
            && self.code != codes::REFUSED_REPARSE
    }

    fn refused(code: i32) -> DataCleanup {
        DataCleanup {
            code,
            targets: Vec::new(),
            removal: Removal::default(),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "ok": self.ok(), "code": self.code, "targets": self.targets,
            "removed": self.removal.removed, "failed": self.removal.failed_value(),
        })
    }
}

/// `cleanupSettings(options)`: каталоги настроек, кроме запретных зон, пересечений с каталогом
/// лаунчера и путей со ссылками. С `preserveProfile` в `userData` остаётся `game-stats.json`
/// (только обычный файл).
pub fn cleanup_settings(options: &CleanupOptions<'_>) -> DataCleanup {
    let path = path();
    let zones = forbidden_zones(options);
    let user_data = non_empty(&options.user_data).map(|dir| path.resolve(&[dir]));
    let install_dir = non_empty(&options.install_dir).map(|dir| path.resolve(&[dir]));
    let mut removal = Removal::default();
    let mut touched = Vec::new();
    for target in settings_targets(options) {
        if path.dirname(&target) == target {
            continue;
        }
        if zones.iter().any(|zone| lower(zone) == lower(&target)) {
            continue;
        }
        if zones.iter().any(|zone| is_same_or_inside(&target, zone)) {
            continue;
        }
        if install_dir
            .as_deref()
            .is_some_and(|dir| is_same_or_inside(dir, &target) || is_same_or_inside(&target, dir))
        {
            continue;
        }
        if find_reparse_point(&target).is_some() {
            continue;
        }
        if !Path::new(&target).exists() {
            continue;
        }
        touched.push(target.clone());
        let is_user_data = user_data
            .as_deref()
            .is_some_and(|dir| lower(dir) == lower(&target));
        if options.preserve_profile && is_user_data {
            let names = match hlsr_core::js::fs::read_dir_names(Path::new(&target)) {
                Ok(names) => names,
                Err(error) => {
                    removal.fail(Path::new(&target), &error);
                    continue;
                }
            };
            for name in names {
                let entry = PathBuf::from(path.join(&[&target, &name]));
                if PROFILE_FILES.iter().any(|file| lower(file) == lower(&name))
                    && std::fs::symlink_metadata(&entry)
                        .is_ok_and(|stat| stat.is_file() && !stat.file_type().is_symlink())
                {
                    continue;
                }
                remove_tree_no_follow(&entry, &mut removal, None);
            }
        } else {
            remove_tree_no_follow(Path::new(&target), &mut removal, None);
        }
    }
    let code = if !removal.failed.is_empty() {
        codes::PARTIAL
    } else if touched.is_empty() {
        codes::NOTHING_TO_DO
    } else {
        codes::OK
    };
    DataCleanup {
        code,
        targets: touched,
        removal,
    }
}

/// `cleanupProfile(options)`: только `game-stats.json` в `userData`.
pub fn cleanup_profile(options: &CleanupOptions<'_>) -> DataCleanup {
    let Some(user_data) = non_empty(&options.user_data) else {
        return DataCleanup::refused(codes::NOTHING_TO_DO);
    };
    let path = path();
    let user_data = path.resolve(&[user_data]);
    let install_dir = non_empty(&options.install_dir).map(|dir| path.resolve(&[dir]));
    let zones = forbidden_zones(options);
    if path.dirname(&user_data) == user_data
        || zones.iter().any(|zone| lower(zone) == lower(&user_data))
        || zones.iter().any(|zone| is_same_or_inside(&user_data, zone))
        || install_dir.as_deref().is_some_and(|dir| {
            is_same_or_inside(dir, &user_data) || is_same_or_inside(&user_data, dir)
        })
    {
        return DataCleanup::refused(codes::REFUSED_PATH);
    }
    if find_reparse_point(&user_data).is_some() {
        return DataCleanup::refused(codes::REFUSED_REPARSE);
    }
    let target = path.join(&[&user_data, PROFILE_FILES[0]]);
    if !Path::new(&target).exists() {
        return DataCleanup::refused(codes::NOTHING_TO_DO);
    }
    let mut removal = Removal::default();
    remove_tree_no_follow(Path::new(&target), &mut removal, None);
    DataCleanup {
        code: if removal.failed.is_empty() {
            codes::OK
        } else {
            codes::PARTIAL
        },
        targets: vec![target],
        removal,
    }
}
