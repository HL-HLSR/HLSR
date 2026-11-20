//! Библиотека игр и загрузки (`AppManager.js` 0.4.9): `withHlsrFolder`, `validateInstallPath`,
//! `ensureDefaultInstallPath`, `getFreeSpaceMb`, каналы `select-install-folder`,
//! `download-settings-*`, `validate-install-path`, `open-games-folder`, `get-library-path`,
//! `set-library-path`.

use super::{DeliveryApi, PLATFORM, lock};
use crate::library_path::default_library_path;
use crate::paths::{is_inside, mark_created_library, prepare_library_root, resolve_library};
use crate::ports::OpenDialog;
use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use hlsr_core::store::Store;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// `LIBRARY_FOLDER_NAME`: папка библиотеки внутри выбранной.
pub const LIBRARY_FOLDER_NAME: &str = "HLSR";

/// Папки Windows, в которые библиотеку не ставим (`validateInstallPath`).
const FORBIDDEN_LIBRARY: [&str; 4] = [
    "\\windows",
    "\\program files",
    "\\programdata",
    "\\system32",
];
/// Папки Windows, которые не открывает «Папка игр».
const FORBIDDEN_GAMES: [&str; 3] = ["\\windows", "\\program files", "\\programdata"];

/// `library.json` (`Store` с умолчаниями `StoreDefaults.library`).
pub trait LibraryStore: Send + Sync {
    fn get(&self, key: &str) -> Option<Value>;

    /// `false` — файл не записан (значение в памяти всё равно меняется).
    fn set(&self, key: &str, value: Value) -> bool;
}

impl LibraryStore for Mutex<Store> {
    fn get(&self, key: &str) -> Option<Value> {
        lock(self).get(key).cloned()
    }

    fn set(&self, key: &str, value: Value) -> bool {
        lock(self).set(key, value)
    }
}

/// `withHlsrFolder(selected)`: выбранная папка и `HLSR` в ней (если это не она сама).
pub fn with_hlsr_folder(selected: &str) -> String {
    with_hlsr_folder_on(&NodePath::native(), selected)
}

fn with_hlsr_folder_on(path: &NodePath, selected: &str) -> String {
    let value = hlsr_core::js::trim(selected);
    if value.is_empty() {
        return value.to_owned();
    }
    let normalized = path.resolve(&[value]);
    if path.basename(&normalized).to_lowercase() == LIBRARY_FOLDER_NAME.to_lowercase() {
        return normalized;
    }
    path.join(&[&normalized, LIBRARY_FOLDER_NAME])
}

/// Есть ли кириллица (`/[Ѐ-ӿ]/`).
fn has_cyrillic(text: &str) -> bool {
    text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
}

/// `/^[A-Za-z]:[\\/]/`.
fn has_drive_prefix(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

/// `/^[A-Za-z]:[\\/]?$/`.
fn is_drive_root(text: &str) -> bool {
    let bytes = text.as_bytes();
    (bytes.len() == 2 || (bytes.len() == 3 && matches!(bytes[2], b'\\' | b'/')))
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
}

impl DeliveryApi {
    fn library_store(&self) -> Option<&dyn LibraryStore> {
        self.options.library_store.as_deref()
    }

    fn default_install_path(&self) -> String {
        default_library_path().to_string_lossy().into_owned()
    }

    /// `currentLibraryPath()`: путь из `library.json` или папка по умолчанию.
    pub fn current_library_path(&self) -> String {
        self.library_store()
            .and_then(|store| store.get("libraryPath"))
            .filter(hlsr_core::js::truthy)
            .map_or_else(
                || self.default_install_path(),
                |path| crate::js_value::template(Some(&path)),
            )
    }

    /// `ensureDefaultInstallPath()`: у новой установки — папка по умолчанию, у старой
    /// (`<userData>/library` с играми) — она.
    pub fn ensure_default_install_path(&self) {
        let Some(store) = self.library_store() else {
            return;
        };
        if store
            .get("libraryPath")
            .is_some_and(|path| hlsr_core::js::truthy(&path))
        {
            return;
        }
        let legacy = self
            .user_data_path()
            .map(|dir| dir.join("library"))
            .unwrap_or_default();
        let installed = store.get("installed").filter(hlsr_core::js::truthy);
        let has_games = match &installed {
            Some(Value::Object(map)) => !map.is_empty(),
            Some(Value::Array(list)) => !list.is_empty(),
            Some(Value::String(text)) => !text.is_empty(),
            _ => false,
        };
        if has_games && !legacy.as_os_str().is_empty() && legacy.exists() {
            let legacy = legacy.to_string_lossy().into_owned();
            store.set("libraryPath", json!(legacy));
            store.set("libraryConfirmed", json!(true));
            self.logger()
                .info(&format!("Existing game library detected: {legacy}"));
            return;
        }
        let default = self.default_install_path();
        store.set("libraryPath", json!(default));
        self.logger()
            .info(&format!("Default game library path proposed: {default}"));
    }

    /// `spaceProbePath(target)`: ближайший существующий каталог.
    fn space_probe_path(&self, target: &str) -> PathBuf {
        let path = NodePath::native();
        let start = if target.is_empty() {
            self.default_install_path()
        } else {
            target.to_owned()
        };
        let mut current = path.resolve(&[&start]);
        loop {
            if Path::new(&current).exists() {
                return PathBuf::from(current);
            }
            let parent = path.dirname(&current);
            if parent == current {
                return PathBuf::from(current);
            }
            current = parent;
        }
    }

    /// `getFreeSpaceMb(target)`: свободные МБ тома; `null` — не узнать.
    fn free_space_mb(&self, target: &str) -> Value {
        let probe = self.space_probe_path(target);
        self.ports()
            .free_space
            .free_bytes(&probe)
            .map_or(Value::Null, |bytes| json!(bytes / (1024 * 1024)))
    }

    /// `validateInstallPath(rawPath)`: `{ok, path, freeSpaceMb, hasCyrillic}` или
    /// `{ok: false, error: "invalid" | "no-access"}`.
    pub fn validate_install_path(&self, raw: &Value) -> Value {
        let invalid = || json!({"ok": false, "error": "invalid"});
        let Some(raw) = raw.as_str() else {
            return invalid();
        };
        let trimmed = hlsr_core::js::trim(raw);
        if trimmed.is_empty() {
            return invalid();
        }
        let path = NodePath::native();
        let target = path.normalize(trimmed);
        if PLATFORM == Platform::Linux {
            if !path.is_absolute(trimmed) || target == "/" {
                return invalid();
            }
        } else {
            if !has_drive_prefix(&target) {
                return invalid();
            }
            let lower = target.to_lowercase();
            if FORBIDDEN_LIBRARY.iter().any(|part| lower.contains(part)) {
                return invalid();
            }
            if is_drive_root(&target) {
                return invalid();
            }
        }
        // Созданную здесь папку запоминает `paths`: при сохранении выбора она получит отметку
        // «создал лаунчер».
        if let Err(error) = prepare_library_root(Path::new(&target)) {
            let raw = crate::errors::RawError::fs(&error, "mkdir", Path::new(&target));
            self.logger()
                .warn(&format!("Library path is not creatable: {}", raw.message));
            return json!({"ok": false, "error": "no-access"});
        }
        let probe = Path::new(&target).join(format!(".hlsr-write-test-{}", self.now_ms()));
        let written = std::fs::write(&probe, b"")
            .map_err(|error| crate::errors::RawError::fs(&error, "open", &probe))
            .and_then(|()| {
                std::fs::remove_file(&probe)
                    .map_err(|error| crate::errors::RawError::fs(&error, "unlink", &probe))
            });
        if let Err(error) = written {
            self.logger()
                .warn(&format!("Library path is not writable: {}", error.message));
            return json!({"ok": false, "error": "no-access"});
        }
        json!({
            "ok": true,
            "path": target,
            "freeSpaceMb": self.free_space_mb(&target),
            "hasCyrillic": has_cyrillic(&target),
        })
    }

    /// `select-install-folder`: выбранная папка и `HLSR` в ней; отмена — `null`.
    pub async fn select_install_folder(&self, current: &Value) -> Value {
        let default_path = current
            .as_str()
            .filter(|current| !current.is_empty())
            .map(str::to_owned);
        let picked = self
            .ports()
            .dialogs
            .show_open_dialog(OpenDialog {
                properties: vec!["openDirectory", "createDirectory"],
                default_path,
                ..OpenDialog::default()
            })
            .await;
        match picked.into_iter().next() {
            Some(folder) => json!(with_hlsr_folder(&folder.to_string_lossy())),
            None => Value::Null,
        }
    }

    /// `downloadSettings.rateLimiter.bytesPerSecond` (диагностика): действующий предел скорости,
    /// байт/с; `None` — настроек загрузки нет.
    pub fn download_limit_bps(&self) -> Option<f64> {
        let settings = self.options.download_settings.as_ref()?;
        Some(lock(settings).rate_limiter().bytes_per_second())
    }

    /// `download-settings-get`.
    pub fn download_settings_get(&self) -> Value {
        match &self.options.download_settings {
            Some(settings) => lock(settings).snapshot().to_json(),
            None => json!({"speedLimit": 0, "initialized": false}),
        }
    }

    /// `download-settings-set`.
    pub fn download_settings_set(&self, speed_limit: &Value) -> Value {
        let Some(settings) = &self.options.download_settings else {
            return json!({"ok": false, "code": "SETTINGS_SAVE_FAILED"});
        };
        match lock(settings).save(speed_limit) {
            Ok(snapshot) => {
                let mut out = json!({"ok": true});
                if let (Value::Object(out), Value::Object(fields)) = (&mut out, snapshot.to_json())
                {
                    out.extend(fields);
                }
                out
            }
            Err(error) => json!({"ok": false, "code": error.code()}),
        }
    }

    /// `open-games-folder`: папка `Games` библиотеки (создаётся, если её нет).
    pub async fn open_games_folder(&self) -> Value {
        let library_path = self.current_library_path();
        let paths = match resolve_library(&library_path) {
            Ok(paths) => paths,
            Err(error) => {
                self.logger().warn(&format!(
                    "[games-folder] недопустимый путь библиотеки: {}",
                    error.message
                ));
                return json!({"status": "error", "code": "LIBRARY_UNAVAILABLE"});
            }
        };
        let games = paths.games;
        if !is_inside(&paths.root, &games) || games == paths.root {
            return json!({"status": "error", "code": "PATH_INVALID"});
        }
        let lower = games.to_string_lossy().to_lowercase();
        if PLATFORM != Platform::Linux && FORBIDDEN_GAMES.iter().any(|part| lower.contains(part)) {
            return json!({"status": "error", "code": "PATH_INVALID"});
        }
        if !games.exists()
            && let Err(error) = std::fs::create_dir_all(&games)
        {
            let raw = crate::errors::RawError::fs(&error, "mkdir", &games);
            self.logger().warn(&format!(
                "[games-folder] не удалось создать {}: {}",
                games.display(),
                raw.message
            ));
            return json!({"status": "error", "code": "PATH_NOT_WRITABLE"});
        }
        let failure = self.ports().shell.open_path(&games).await;
        if !failure.is_empty() {
            self.logger().warn(&format!(
                "[games-folder] не удалось открыть {}: {failure}",
                games.display()
            ));
            return json!({"status": "error", "code": "OPEN_FAILED"});
        }
        json!({"status": "ok", "path": games.to_string_lossy()})
    }

    /// `get-library-path`.
    pub fn get_library_path(&self) -> Value {
        let library_path = self.current_library_path();
        let confirmed = self
            .library_store()
            .and_then(|store| store.get("libraryConfirmed"))
            .is_some_and(|confirmed| hlsr_core::js::truthy(&confirmed));
        json!({
            "path": library_path,
            "exists": Path::new(&library_path).exists(),
            "confirmed": confirmed,
            "freeSpaceMb": self.free_space_mb(&library_path),
            "isDefault": library_path == self.default_install_path(),
        })
    }

    /// `set-library-path`: проверка, новая служба доставки (во время загрузки, установки, игры
    /// или запуска — отказ `busy`, папка не меняется и не запоминается), запись в `library.json`.
    pub fn set_library_path_channel(&self, raw: &Value) -> Value {
        let check = self.validate_install_path(raw);
        if check.get("ok") != Some(&json!(true)) {
            return check;
        }
        let path = crate::js_value::template(check.get("path"));
        if let Some(refused) = self.set_library_path(&path) {
            return refused;
        }
        if let Some(store) = self.library_store() {
            store.set("libraryPath", json!(path));
            store.set("libraryConfirmed", json!(true));
        }
        // Папку создал сам лаунчер (её не было до проверки): полное удаление унесёт её целиком.
        let now =
            SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(self.now_ms().max(0) as u64);
        mark_created_library(Path::new(&path), now);
        self.logger()
            .info(&format!("Game library path set to: {path}"));
        check
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_hlsr_folder_appends_once() {
        let posix = NodePath::new(Platform::Linux, "/home/u");
        assert_eq!(with_hlsr_folder_on(&posix, "  "), "");
        assert_eq!(with_hlsr_folder_on(&posix, "/mnt/games"), "/mnt/games/HLSR");
        assert_eq!(
            with_hlsr_folder_on(&posix, "/mnt/games/hlsr/"),
            "/mnt/games/hlsr"
        );
        assert_eq!(with_hlsr_folder_on(&posix, "games"), "/home/u/games/HLSR");
        let win = NodePath::new(Platform::Win32, "C:\\Users\\u");
        assert_eq!(with_hlsr_folder_on(&win, "D:\\"), "D:\\HLSR");
        assert_eq!(
            with_hlsr_folder_on(&win, "D:/Games/Hlsr"),
            "D:\\Games\\Hlsr"
        );
    }

    #[test]
    fn drive_patterns() {
        assert!(has_drive_prefix("C:\\x"));
        assert!(!has_drive_prefix("C:x"));
        assert!(is_drive_root("C:"));
        assert!(is_drive_root("d:\\"));
        assert!(!is_drive_root("d:\\a"));
        assert!(has_cyrillic("/home/Игры"));
        assert!(!has_cyrillic("/home/games"));
    }
}
