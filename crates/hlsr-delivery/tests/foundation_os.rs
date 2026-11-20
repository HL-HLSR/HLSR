//! Основа доставки на настоящей ОС: свободное место и кеш `free-space.js`,
//! «игра запущена» (`file-ops.js isGameRunning`), удаление внутри зоны.

mod support;

use hlsr_delivery::free_space::{CACHE_MS, FreeSpaceCache};
use hlsr_delivery::ports::{FakeFreeSpace, FreeSpace, SystemFreeSpace};
use hlsr_delivery::{errors::Code, errors::Failure, file_ops, paths};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn system_free_space_of_existing_folder() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        SystemFreeSpace
            .free_bytes(dir.path())
            .is_some_and(|bytes| bytes > 0)
    );
}

/// Linux: `statfs` несуществующего каталога — ошибка, `null`. Windows: запасной путь
/// 0.4.7 (`fsutil volume diskfree <корень>`) даёт место на томе.
#[test]
fn system_free_space_of_missing_folder_depends_on_os() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("нет").join("такой");
    let bytes = SystemFreeSpace.free_bytes(&missing);
    if cfg!(windows) {
        assert!(bytes.is_some_and(|b| b > 0), "{bytes:?}");
    } else {
        assert_eq!(bytes, None);
    }
}

#[test]
fn free_space_is_cached_for_five_seconds_per_folder() {
    let fake = Arc::new(FakeFreeSpace::default());
    fake.set(Path::new("/lib"), Some(100));
    let cache = FreeSpaceCache::new(fake.clone());
    let start = Instant::now();

    assert_eq!(
        cache.free_space_bytes_at(Path::new("/lib"), start),
        Some(100)
    );
    fake.set(Path::new("/lib"), Some(50));
    let almost = start + Duration::from_millis(CACHE_MS - 1);
    assert_eq!(
        cache.free_space_bytes_at(Path::new("/lib"), almost),
        Some(100),
        "в пределах 5 с — из кеша"
    );
    let later = start + Duration::from_millis(CACHE_MS);
    assert_eq!(
        cache.free_space_bytes_at(Path::new("/lib"), later),
        Some(50),
        "через 5 с — заново"
    );

    // Неудача тоже кешируется; ключ — строка пути.
    assert_eq!(cache.free_space_bytes_at(Path::new("/none"), start), None);
    fake.set(Path::new("/none"), Some(7));
    assert_eq!(cache.free_space_bytes_at(Path::new("/none"), start), None);
    cache.forget();
    assert_eq!(
        cache.free_space_bytes_at(Path::new("/none"), start),
        Some(7)
    );
    assert_eq!(fake.calls(), 4);
}

#[cfg(target_os = "linux")]
#[test]
fn game_running_is_found_by_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("hl2.exe");
    std::fs::copy("/bin/sleep", &game).unwrap();

    assert_eq!(file_ops::is_game_running(dir.path(), &[]), None);
    let mut child = std::process::Command::new(&game).arg("30").spawn().unwrap();
    let found = (0..50).find_map(|_| {
        std::thread::sleep(Duration::from_millis(20));
        file_ops::is_game_running(dir.path(), &[])
    });
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(
        found.as_deref(),
        Some("hl2.exe"),
        "по умолчанию ищется hl2.exe"
    );
    assert_eq!(
        file_ops::is_game_running(dir.path(), &["other.exe", "hl2.exe"]),
        None
    );
}

#[cfg(windows)]
#[test]
fn game_running_is_found_by_locked_executable() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("hl.exe");
    std::fs::write(&game, "MZ").unwrap();
    assert_eq!(file_ops::is_game_running(dir.path(), &["hl.exe"]), None);
    // Запущенный exe Windows держит без права записи для других.
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&game)
        .unwrap();
    assert_eq!(
        file_ops::is_game_running(dir.path(), &["hl.exe"]).as_deref(),
        Some("hl.exe")
    );
}

#[test]
fn remove_within_zone_deletes_tree_but_not_the_zone() {
    let dir = tempfile::tempdir().unwrap();
    let zone = dir.path().join("Downloads");
    let target = zone.join("hl1-won");
    std::fs::create_dir_all(target.join("sub")).unwrap();
    std::fs::write(target.join("sub").join("a.7z.part"), "x").unwrap();

    paths::remove_within_zone(&zone, &target).unwrap();
    assert!(!target.exists());
    assert!(zone.exists());
    paths::remove_within_zone(&zone, &target).unwrap();

    match paths::remove_within_zone(&zone, &zone) {
        Err(Failure::Delivery(error)) => assert_eq!(error.code, Code::PathInvalid),
        other => panic!("{other:?}"),
    }
    match paths::remove_within_zone(&zone, &dir.path().join("other")) {
        Err(Failure::Delivery(error)) => assert_eq!(error.code, Code::PathInvalid),
        other => panic!("{other:?}"),
    }
}

/// Файл «только для чтения» удаляется: в Linux решают права каталога, в Windows 0.4.9 снимает
/// атрибут перед удалением (`makeWritableSync`; в 0.4.7 был `FILE_LOCKED` с EPERM).
#[test]
fn remove_file_and_readonly_attribute() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("game.cfg");
    std::fs::write(&file, "x").unwrap();
    let mut permissions = std::fs::metadata(&file).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&file, permissions).unwrap();
    file_ops::remove_file_sync(&file, Some("game.cfg")).unwrap();
    assert!(!file.exists());
    file_ops::remove_file_sync(&file, None).unwrap();
    file_ops::make_writable(&file);
}

/// Обёртки без `_with` берут правила `path` этой ОС и текущий каталог процесса.
#[test]
fn native_wrappers_follow_this_os() {
    use hlsr_delivery::library_path::default_library_path;
    let dir = tempfile::tempdir().unwrap();
    let library = paths::resolve_library(&dir.path().join("HLSR").to_string_lossy()).unwrap();
    let game = paths::game_install_path(&library, "hl1-won").unwrap();
    assert_eq!(game, library.games.join("hl1-won"));
    assert!(paths::is_inside(&library.root, &game));
    assert!(!paths::is_inside(&game, &library.root));
    let cfg = file_ops::resolve_inside(&game, "valve/config.cfg", None).unwrap();
    assert_eq!(cfg, game.join("valve").join("config.cfg"));
    assert!(file_ops::resolve_inside(&game, "../hl2-4260/x", None).is_err());
    assert_eq!(
        paths::assert_inside(&library.root, &library.root.join("Games").join(".."), None).unwrap(),
        library.root
    );
    if cfg!(windows) {
        let upper = std::path::PathBuf::from(library.root.to_string_lossy().to_uppercase());
        assert!(
            paths::is_inside(&library.root, &upper.join("GAMES")),
            "в Windows регистр не важен"
        );
        assert_eq!(default_library_path(), std::path::PathBuf::from("C:\\HLSR"));
    } else {
        let home = std::env::home_dir().unwrap();
        assert_eq!(default_library_path(), home.join("Games").join("HLSR"));
    }
}

/// Windows: те же случаи, что `tools/reference/node-semantics-win32.js` снял с
/// Electron 0.4.7 под Wine (`fixtures/fs-win32.json`): занятый exe, «только для
/// чтения», непустой каталог, коды и тексты ошибок, в том числе путь `\\?\…` в
/// кодовой странице ANSI у `fs.rmSync`.
#[cfg(windows)]
#[test]
fn fs_errors_like_electron_under_wine() {
    use hlsr_delivery::errors::RawError;
    use serde_json::{Value, json};
    use std::os::windows::fs::OpenOptionsExt;

    let fixture = support::fixture("fs-win32.json");
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path().to_string_lossy().into_owned();
    let root = temp.path().join("Игры HLSR");
    let at = |rel: &str| rel.split('/').fold(root.clone(), |p, part| p.join(part));
    for (rel, text) in [
        ("game/hl.exe", "MZ"),
        ("plain.txt", "p"),
        ("plaindir/sub/file.txt", "p"),
        ("source.txt", "s"),
        ("readonly.txt", "r"),
        ("rodir/readonly.txt", "r"),
        ("full/file.txt", "f"),
    ] {
        std::fs::create_dir_all(at(rel).parent().unwrap()).unwrap();
        std::fs::write(at(rel), text).unwrap();
    }
    let set_readonly = |rel: &str, on: bool| {
        let mut permissions = std::fs::metadata(at(rel)).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(on);
        std::fs::set_permissions(at(rel), permissions).unwrap();
    };
    set_readonly("readonly.txt", true);
    set_readonly("rodir/readonly.txt", true);
    // Как exe запущенной игры: читать можно, писать и удалять нельзя.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(at("game/hl.exe"))
        .unwrap();

    let raw = |error: RawError| json!({"code": error.code.unwrap_or_default(), "syscall": error.syscall, "message": error.message});
    let outcome = |result: Result<(), Failure>| match result {
        Ok(()) => json!({"ok": true}),
        Err(Failure::Raw(error)) => raw(error),
        Err(Failure::Delivery(error)) => json!({"delivery": error.to_public()}),
    };
    let fs_error =
        |result: std::io::Result<()>, syscall: &str, path: &Path, dest: Option<&Path>| match result
        {
            Ok(()) => json!({"ok": true}),
            Err(error) => raw(RawError::fs_with_dest(&error, syscall, path, dest)),
        };
    let actual: Vec<(&str, Value)> = vec![
        (
            "open r+ занятого exe",
            fs_error(
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(at("game/hl.exe"))
                    .map(drop),
                "open",
                &at("game/hl.exe"),
                None,
            ),
        ),
        (
            "copyFile поверх занятого exe",
            fs_error(
                std::fs::copy(at("source.txt"), at("game/hl.exe")).map(drop),
                "copyfile",
                &at("source.txt"),
                Some(&at("game/hl.exe")),
            ),
        ),
        (
            "unlink занятого exe",
            fs_error(
                std::fs::remove_file(at("game/hl.exe")),
                "unlink",
                &at("game/hl.exe"),
                None,
            ),
        ),
        (
            "rmSync обычного файла",
            outcome(file_ops::remove_file_sync(&at("plain.txt"), None)),
        ),
        (
            "rmSync обычного каталога",
            outcome(paths::remove_within_zone(&root, &at("plaindir"))),
        ),
        (
            "rmSync занятого exe",
            outcome(paths::remove_within_zone(&root, &at("game/hl.exe"))),
        ),
        (
            "rmSync каталога с занятым exe",
            outcome(paths::remove_within_zone(&root, &at("game"))),
        ),
        (
            "rmSync файла только для чтения",
            outcome(paths::remove_within_zone(&root, &at("readonly.txt"))),
        ),
        (
            "rmSync каталога с файлом только для чтения",
            outcome(paths::remove_within_zone(&root, &at("rodir"))),
        ),
        (
            "rmSync непустого каталога без recursive",
            outcome(file_ops::remove_file_sync(&at("full"), None)),
        ),
        (
            "rmSync нет файла",
            outcome(file_ops::remove_file_sync(&at("missing.txt"), None)),
        ),
        (
            "rmdir непустого каталога",
            fs_error(std::fs::remove_dir(at("full")), "rmdir", &at("full"), None),
        ),
        (
            "open нет каталога",
            fs_error(
                std::fs::File::open(at("missing/x.txt")).map(drop),
                "open",
                &at("missing/x.txt"),
                None,
            ),
        ),
        (
            "open недопустимое имя",
            fs_error(
                std::fs::File::open(at("bad<name>.txt")).map(drop),
                "open",
                &at("bad<name>.txt"),
                None,
            ),
        ),
        (
            "mkdir существующего",
            fs_error(std::fs::create_dir(at("full")), "mkdir", &at("full"), None),
        ),
        (
            "readFile каталога",
            fs_error(
                hlsr_core::js::fs::read_file(&at("full")).map(drop),
                "read",
                &at("full"),
                None,
            ),
        ),
    ];
    let escaped = serde_json::to_string(&work).unwrap();
    let work_in_json = escaped[1..escaped.len() - 1].to_owned();
    // C++-часть rmSync пишет путь в кодовой странице ANSI. Фикстура снята при своей
    // (CP1251 у Wine с русской локалью): при другой системной странице кириллица
    // «Игры» выглядит иначе — тогда эта часть пути не сравнивается.
    // SAFETY: GetACP не принимает аргументов и не может завершиться ошибкой.
    let acp = unsafe { windows_sys::Win32::Globalization::GetACP() };
    let same_code_page = fixture["acp"].as_u64() == Some(u64::from(acp));
    let mask = |text: String| -> String {
        if same_code_page {
            return text;
        }
        let mut out = String::new();
        let mut rest = text.as_str();
        while let Some(at) = rest.find("<work>\\\\") {
            let start = at + "<work>\\\\".len();
            out.push_str(&rest[..start]);
            rest = &rest[start..];
            if let Some(end) = rest.find(" HLSR") {
                out.push_str("<ANSI>");
                rest = &rest[end..];
            }
        }
        out.push_str(rest);
        out
    };
    let mut wrong = Vec::new();
    for ((name, value), expected) in actual.iter().zip(fixture["scenarios"].as_array().unwrap()) {
        assert_eq!(*name, support::str_of(&expected["name"]));
        let mut value: Value =
            serde_json::from_str(&mask(value.to_string().replace(&work_in_json, "<work>")))
                .unwrap();
        let mut expected: Value = serde_json::from_str(&mask(expected.to_string())).unwrap();
        expected.as_object_mut().unwrap().remove("name");
        if *name == "readFile каталога" {
            // Node читает каталог без пути в тексте: сравнивается код.
            value = json!({"code": value["code"]});
            expected = json!({"code": expected["code"]});
        }
        if value != expected {
            wrong.push(format!("{name}:\n- {expected}\n+ {value}"));
        }
    }
    let exists = json!({
        "game/hl.exe": at("game/hl.exe").exists(),
        "plain.txt": at("plain.txt").exists(),
        "plaindir": at("plaindir").exists(),
        "readonly.txt": at("readonly.txt").exists(),
        "rodir/readonly.txt": at("rodir/readonly.txt").exists(),
    });
    if exists != fixture["existsAfter"] {
        wrong.push(format!(
            "existsAfter:\n- {}\n+ {exists}",
            fixture["existsAfter"]
        ));
    }
    assert!(
        SystemFreeSpace.free_bytes(&root).is_some(),
        "statfs каталога"
    );
    assert!(
        SystemFreeSpace.free_bytes(&at("source.txt")).is_some(),
        "statfs файла — его каталог"
    );
    drop(lock);
    for rel in ["readonly.txt", "rodir/readonly.txt"] {
        if at(rel).exists() {
            set_readonly(rel, false);
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}
