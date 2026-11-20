//! Оверлей LiveSplit над настоящей ОС (Windows): прокси в папке запущенной игры заблокирован —
//! `copyTo` оставляет прежнюю копию и пишет предупреждение (`livesplit-overlay.js`), а после
//! выгрузки заменяет её. Тест загружает в сам тестовый процесс копию системной DLL с меткой прокси
//! в конце файла.

#![cfg(windows)]

use hlsr_core::js::Platform;
use hlsr_core::logger::MemoryLogger;
use hlsr_platform::livesplit_overlay::{
    self as lso, LiveSplitOverlay, OverlayOptions, PROXY_OPENGL32, Target,
};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows_sys::Win32::Foundation::FreeLibrary;
use windows_sys::Win32::System::LibraryLoader::{DONT_RESOLVE_DLL_REFERENCES, LoadLibraryExW};

/// Системная DLL с меткой прокси в конце: образ грузится, а лаунчер узнаёт в ней свой прокси.
fn marked_system_dll(name: &str) -> Vec<u8> {
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    let mut bytes = std::fs::read(PathBuf::from(system).join("System32").join(name)).unwrap();
    bytes.extend_from_slice(lso::PROXY_MARKER);
    bytes
}

/// Загруженная в процесс DLL (образ отображён в память, зависимости не разрешаются, `DllMain` не
/// вызывается): файл заблокирован, как у прокси в запущенной игре.
struct Loaded(windows_sys::Win32::Foundation::HMODULE);

impl Loaded {
    fn new(path: &Path) -> Loaded {
        let wide = hlsr_core::win32::wide(path);
        let module = unsafe {
            LoadLibraryExW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                DONT_RESOLVE_DLL_REFERENCES,
            )
        };
        assert!(
            !module.is_null(),
            "DLL не загружена: {}",
            std::io::Error::last_os_error()
        );
        Loaded(module)
    }
}

impl Drop for Loaded {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.0) };
    }
}

#[test]
fn locked_proxy_keeps_the_previous_copy_and_is_replaced_after_unload() {
    let root = tempfile::tempdir().unwrap();
    let resources = root.path().join("resources").join(lso::RESOURCE_DIRECTORY);
    std::fs::create_dir_all(&resources).unwrap();
    let (first, second) = (
        marked_system_dll("version.dll"),
        marked_system_dll("msimg32.dll"),
    );
    std::fs::write(resources.join(PROXY_OPENGL32.resource), &first).unwrap();
    std::fs::write(resources.join(lso::PROXY_D3D9.resource), "d3d9 v1").unwrap();
    let install = root.path().join("game");
    std::fs::create_dir_all(install.join("Half-Life")).unwrap();
    let target = Target {
        install_path: &install,
        executable: "Half-Life/hl.exe",
        engine: Some("goldsrc"),
    };
    let proxy = install.join("Half-Life").join(PROXY_OPENGL32.name);
    let logger = Arc::new(MemoryLogger::default());
    let service = LiveSplitOverlay::new(OverlayOptions {
        base_dir: Some(root.path().join("data")),
        resources_path: Some(root.path().join("resources")),
        logger: Some(logger.clone()),
        platform: Some(Platform::Win32),
    });
    let settings = json!({"liveSplitOverlay": {"enabled": true, "x": 7}});

    let prepared = service
        .prepare("hl1-won", &settings, Some(&target), false)
        .unwrap()
        .unwrap();
    assert_eq!(prepared.placed, [install.join("Half-Life")]);
    assert_eq!(std::fs::read(&proxy).unwrap(), first);

    // Игра запущена: её прокси загружен. В ресурсах — другая сборка прокси.
    let loaded = Loaded::new(&proxy);
    std::fs::write(resources.join(PROXY_OPENGL32.resource), &second).unwrap();
    let again = service
        .prepare(
            "hl1-won",
            &json!({"liveSplitOverlay": {"enabled": true, "x": 8}}),
            Some(&target),
            false,
        )
        .expect("заблокированный прокси — не ошибка")
        .unwrap();
    assert_eq!(again.placed, prepared.placed, "папка по-прежнему с прокси");
    assert_eq!(
        std::fs::read(&proxy).unwrap(),
        first,
        "остаётся прежняя копия"
    );
    let lines = logger.take();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0].0, "warn");
    assert!(
        lines[0]
            .1
            .starts_with("[livesplit-overlay] opengl32.dll не обновлён: "),
        "{}",
        lines[0].1
    );
    let leftovers: Vec<String> = std::fs::read_dir(install.join("Half-Life"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "временный файл не остаётся: {leftovers:?}"
    );
    let ini = std::fs::read_to_string(again.dir.join("overlay.ini")).unwrap();
    assert!(
        ini.contains("\r\nx=8\r\n"),
        "настройки пишутся, пока прокси занят: {ini:?}"
    );

    // Игра закрыта — следующий запуск заменяет прокси.
    drop(loaded);
    service
        .prepare("hl1-won", &settings, Some(&target), false)
        .unwrap();
    assert_eq!(std::fs::read(&proxy).unwrap(), second);
    assert!(logger.take().is_empty());
}
