//! Папка библиотеки по умолчанию (`shared/library-path.js`).

use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use std::path::PathBuf;

/// `DEFAULT_LIBRARY_PATH`: `C:\HLSR` в Windows, `~/Games/HLSR` в Linux
/// (домашний каталог — как `os.homedir()`: `$HOME`, иначе из базы пользователей).
pub fn default_library_path() -> PathBuf {
    // Живая проверка (dev-hooks): своя папка вместо `C:\HLSR` — иначе пустая библиотека прогона
    // уходит на настоящие игры владельца (`recover_library_path`).
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    if let Some(path) = std::env::var_os("HLSR_DEFAULT_LIBRARY").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    let home = std::env::home_dir().unwrap_or_default();
    PathBuf::from(default_library_path_for(
        Platform::CURRENT,
        &home.to_string_lossy(),
    ))
}

/// `DEFAULT_LIBRARY_PATH` для ОС `platform` и домашнего каталога `home`.
pub fn default_library_path_for(platform: Platform, home: &str) -> String {
    match platform {
        Platform::Win32 => "C:\\HLSR".to_owned(),
        Platform::Linux => NodePath::new(platform, "").join(&[home, "Games", "HLSR"]),
    }
}
