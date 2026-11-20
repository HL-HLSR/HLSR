//! Нагрузка `HLSR-Setup.exe` (Р1): хвост, проверка целостности, чтение файлов списка, установка из
//! нагрузки. Exe для упаковки — копия любого exe (здесь — сам тестовый бинарник).

mod support;

use hlsr_core::json_formats::compact;
use hlsr_update::windows::file_list;
use hlsr_update::windows::layout::{self, App};
use hlsr_update::windows::payload::{
    Payload, PayloadError, Source, TRAILER_LEN, Trailer, has_trailer, read_list, read_trailer,
};
use hlsr_update::windows::setup;
use std::io::Read;
use std::path::{Path, PathBuf};
use support::payload::pack_for_tests;

/// Сборка: `HLSR.exe` — копия тестового бинарника, плюс ресурсы.
fn build(root: &Path) -> (PathBuf, PathBuf) {
    build_as(root, &App::production())
}

/// Сборка с удостоверением `app` (боевая или QA, `hlsr-release setup --qa`).
fn build_as(root: &Path, app: &App) -> (PathBuf, PathBuf) {
    let tree = root.join("win-unpacked");
    std::fs::create_dir_all(tree.join("resources").join("7zip")).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), tree.join(layout::EXE)).unwrap();
    std::fs::write(tree.join("resources").join("7zip").join("7za.exe"), b"7za").unwrap();
    std::fs::write(tree.join("resources").join("empty.bin"), b"").unwrap();
    std::fs::write(
        tree.join("resources").join("Лицензия.txt"),
        "текст".repeat(1000),
    )
    .unwrap();
    let list = file_list::build(&tree, "0.5.0", app).unwrap();
    std::fs::write(tree.join(file_list::RELATIVE), compact(&list.to_value())).unwrap();
    let setup = root.join("HLSR-Setup-0.5.0.exe");
    pack_for_tests(&tree, &setup);
    (tree, setup)
}

#[test]
fn trailer_round_trips_and_rejects_foreign_tails() {
    let trailer = Trailer {
        offset: 1000,
        length: 200,
        sha256: [7; 32],
    };
    let bytes = trailer.to_bytes();
    assert_eq!(bytes.len() as u64, TRAILER_LEN);
    assert_eq!(&bytes[..12], b"HLSR-SETUP-1");
    assert_eq!(
        Trailer::parse(&bytes, 1000 + 200 + TRAILER_LEN),
        Some(trailer)
    );
    // Длина файла не сходится — дописано ещё что-то или файл обрезан.
    assert_eq!(Trailer::parse(&bytes, 1000 + 200 + TRAILER_LEN + 1), None);
    let mut foreign = bytes;
    foreign[0] = b'X';
    assert_eq!(Trailer::parse(&foreign, 1000 + 200 + TRAILER_LEN), None);
    let empty_exe = Trailer {
        offset: 0,
        length: 200,
        sha256: [0; 32],
    };
    assert_eq!(
        Trailer::parse(&empty_exe.to_bytes(), 200 + TRAILER_LEN),
        None
    );
}

#[test]
fn plain_exe_has_no_payload() {
    let exe = std::env::current_exe().unwrap();
    assert!(!has_trailer(&exe));
    assert!(matches!(Payload::open(&exe), Err(PayloadError::Missing)));
    let dir = tempfile::tempdir().unwrap();
    let tiny = dir.path().join("tiny.exe");
    std::fs::write(&tiny, b"MZ").unwrap();
    assert!(!has_trailer(&tiny));
    assert!(!has_trailer(&dir.path().join("missing.exe")));
}

#[test]
fn payload_serves_every_file_of_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let (tree, setup) = build(dir.path());
    assert!(has_trailer(&setup));
    let trailer = read_trailer(&setup).unwrap().unwrap();
    assert_eq!(
        trailer.offset,
        std::fs::metadata(tree.join(layout::EXE)).unwrap().len()
    );
    let payload = Payload::open(&setup).unwrap();
    let source = Source::Payload(Box::new(payload.clone()));
    assert_eq!(source.location(), dir.path());
    let list = source.read_list().unwrap();
    assert_eq!(list, file_list::read(&tree).unwrap());
    for item in &list.files {
        let mut bytes = Vec::new();
        source.open(item).unwrap().read_to_end(&mut bytes).unwrap();
        let mut expected_path = tree.clone();
        for part in item.path.split('/') {
            expected_path.push(part);
        }
        assert_eq!(
            bytes,
            std::fs::read(expected_path).unwrap(),
            "{}",
            item.path
        );
    }
}

#[test]
fn damaged_payload_is_refused_before_installation() {
    let dir = tempfile::tempdir().unwrap();
    let (_tree, setup) = build(dir.path());
    let trailer = read_trailer(&setup).unwrap().unwrap();
    let mut bytes = std::fs::read(&setup).unwrap();
    let at = (trailer.offset + trailer.length - 1) as usize;
    bytes[at] ^= 0xff;
    let flipped = dir.path().join("flipped.exe");
    std::fs::write(&flipped, &bytes).unwrap();
    assert!(matches!(
        Payload::open(&flipped),
        Err(PayloadError::Damaged)
    ));

    let truncated = dir.path().join("truncated.exe");
    let original = std::fs::read(&setup).unwrap();
    std::fs::write(&truncated, &original[..original.len() - 10]).unwrap();
    assert!(!has_trailer(&truncated));
    assert!(matches!(
        Payload::open(&truncated),
        Err(PayloadError::Missing)
    ));
}

/// Файл подменён после построения списка: в нагрузке другие длины — нагрузка не открывается
/// (в распакованной сборке тот же случай — `CORRUPT` при копировании).
#[test]
fn tampered_build_gives_a_damaged_payload() {
    let dir = tempfile::tempdir().unwrap();
    let (tree, _setup) = build(dir.path());
    std::fs::write(tree.join("resources").join("7zip").join("7za.exe"), b"7za!").unwrap();
    let setup = dir.path().join("tampered.exe");
    pack_for_tests(&tree, &setup);
    assert!(matches!(Payload::open(&setup), Err(PayloadError::Damaged)));
}

/// Список нагрузки без хеша: тот же, что у [`Payload::open`]; испорченные файлы после списка его не
/// портят (их отвергнет установщик), испорченный список — ошибка.
#[test]
fn read_list_takes_the_list_without_the_payload_hash() {
    let dir = tempfile::tempdir().unwrap();
    let (tree, setup) = build(dir.path());
    let list = read_list(&setup).unwrap();
    assert_eq!(&list, Payload::open(&setup).unwrap().list());
    assert_eq!(list, file_list::read(&tree).unwrap());

    let trailer = read_trailer(&setup).unwrap().unwrap();
    let original = std::fs::read(&setup).unwrap();
    let mut bytes = original.clone();
    bytes[(trailer.offset + trailer.length - 1) as usize] ^= 0xff;
    let flipped = dir.path().join("flipped.exe");
    std::fs::write(&flipped, &bytes).unwrap();
    assert!(matches!(
        Payload::open(&flipped),
        Err(PayloadError::Damaged)
    ));
    assert_eq!(read_list(&flipped).unwrap(), list);

    let mut bytes = original.clone();
    // Первый байт JSON списка (после u64 длины).
    bytes[(trailer.offset + 8) as usize] = b'x';
    let broken = dir.path().join("broken-list.exe");
    std::fs::write(&broken, &bytes).unwrap();
    assert!(matches!(read_list(&broken), Err(PayloadError::List(_))));

    let mut bytes = original;
    bytes[trailer.offset as usize..trailer.offset as usize + 8]
        .copy_from_slice(&u64::MAX.to_le_bytes());
    let too_long = dir.path().join("too-long.exe");
    std::fs::write(&too_long, &bytes).unwrap();
    assert!(matches!(read_list(&too_long), Err(PayloadError::Damaged)));

    let exe = std::env::current_exe().unwrap();
    assert!(matches!(read_list(&exe), Err(PayloadError::Missing)));
}

/// Имя приложения (`app.getName()`) — по списку своей сборки: нагрузка установщика или
/// `resources\install-files.json` рядом с exe. QA-сборка — `hlsr-qa`; боевая, сборка без списка
/// (отладка) и с битым списком — `hlsr`.
#[test]
fn app_name_comes_from_the_build_list() {
    let production = tempfile::tempdir().unwrap();
    let (tree, setup_exe) = build(production.path());
    assert_eq!(setup::app_name_of(&tree.join(layout::EXE)), "hlsr");
    assert_eq!(setup::app_name_of(&setup_exe), "hlsr");

    let qa = tempfile::tempdir().unwrap();
    let (tree, setup_exe) = build_as(qa.path(), &App::qa());
    assert_eq!(setup::app_name_of(&tree.join(layout::EXE)), "hlsr-qa");
    assert_eq!(setup::app_name_of(&setup_exe), "hlsr-qa");
    // Список в каталоге exe битый — боевое имя, как боевые ключи у `build_of`.
    std::fs::write(tree.join(file_list::RELATIVE), b"{").unwrap();
    assert_eq!(setup::app_name_of(&tree.join(layout::EXE)), "hlsr");

    let bare = tempfile::tempdir().unwrap();
    let exe = bare.path().join(layout::EXE);
    std::fs::write(&exe, b"MZ").unwrap();
    assert_eq!(setup::app_name_of(&exe), "hlsr");
    assert_eq!(
        setup::app_name_of(&std::env::current_exe().unwrap()),
        "hlsr"
    );
}

/// Установка из нагрузки — пути Windows на настоящей ФС (`path.win32`): только Windows.
#[cfg(windows)]
mod install_from_payload {
    use super::*;
    use hlsr_core::logger::MemoryLogger;
    use hlsr_update::windows::install::{self, InstallOptions};
    use hlsr_update::windows::known_folders::FakeKnownFolders;
    use hlsr_update::windows::registry::FakeRegistry;
    use hlsr_update::windows::shortcuts::{FakeShellLinks, Shortcuts};
    use std::sync::Arc;

    /// Установка из нагрузки: установленный `HLSR.exe` — первые N байт установщика, файлы — байт в байт.
    #[test]
    fn install_from_payload_writes_the_exe_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let (tree, setup) = build(dir.path());
        let source = Source::Payload(Box::new(Payload::open(&setup).unwrap()));
        let registry = FakeRegistry::new();
        let id = layout::identity(None).unwrap();
        let shortcuts = Shortcuts::new(
            Arc::new(FakeShellLinks::new()),
            &FakeKnownFolders::under(dir.path()),
            &id,
        )
        .unwrap();
        let log = MemoryLogger::default();
        let install_dir = dir.path().join("HLSR").join("Launcher");
        let install_dir = install_dir.to_string_lossy().into_owned();
        let library = dir.path().join("Library").to_string_lossy().into_owned();
        let mut options = InstallOptions::new(&source, &install_dir, &registry, &shortcuts, &log);
        options.env = &|_| None;
        options.library_root = &library;
        options.sleep = &|_| {};
        let result = install::install(options).unwrap();
        assert_eq!(result.version, "0.5.0");
        let installed = PathBuf::from(&install_dir);
        assert_eq!(
            std::fs::read(installed.join(layout::EXE)).unwrap(),
            std::fs::read(tree.join(layout::EXE)).unwrap()
        );
        assert_eq!(
            std::fs::read(installed.join("resources").join("Лицензия.txt")).unwrap(),
            std::fs::read(tree.join("resources").join("Лицензия.txt")).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(installed.join(file_list::RELATIVE)).unwrap(),
            file_list::read(&tree).unwrap().to_text()
        );
        assert!(!has_trailer(&installed.join(layout::EXE)));
    }

    /// Установленная QA-сборка узнаёт себя по списку в каталоге программы: имя `hlsr-qa`, свой
    /// профиль `%APPDATA%\hlsr-qa` и журналы `%LOCALAPPDATA%\hlsr-qa\logs`, как QA-сборка 0.4.7.
    #[test]
    fn installed_qa_build_takes_its_own_profile() {
        let dir = tempfile::tempdir().unwrap();
        let (_tree, setup_exe) = build_as(dir.path(), &App::qa());
        let source = Source::Payload(Box::new(Payload::open(&setup_exe).unwrap()));
        let registry = FakeRegistry::new();
        let id = layout::identity(Some(&App::qa())).unwrap();
        let shortcuts = Shortcuts::new(
            Arc::new(FakeShellLinks::new()),
            &FakeKnownFolders::under(dir.path()),
            &id,
        )
        .unwrap();
        let log = MemoryLogger::default();
        let install_dir = dir.path().join("HLSR-QA").join("Launcher");
        let install_dir = install_dir.to_string_lossy().into_owned();
        let library = dir.path().join("Library").to_string_lossy().into_owned();
        let mut options = InstallOptions::new(&source, &install_dir, &registry, &shortcuts, &log);
        options.env = &|_| None;
        options.library_root = &library;
        options.sleep = &|_| {};
        let result = install::install(options).unwrap();
        assert_eq!(result.identity.app_id, layout::QA_APP_ID);

        let exe = PathBuf::from(&install_dir).join(layout::EXE);
        let name = setup::app_name_of(&exe);
        assert_eq!(name, "hlsr-qa");
        let roaming = dir.path().join("Roaming");
        let local = dir.path().join("Local");
        let paths = hlsr_core::paths::Paths::windows_from(&roaming, &local, name);
        assert_eq!(paths.data_dir, roaming.join("hlsr-qa"));
        assert_eq!(paths.logs_dir, local.join("hlsr-qa").join("logs"));
    }

    /// Exe с нагрузкой внутри каталога установки — `BAD_DIR` (замена каталога заняла бы сам установщик);
    /// каталог установки рядом с exe (в его каталоге) допустим — выше.
    #[test]
    fn setup_inside_the_install_dir_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (_tree, setup) = build(dir.path());
        let install_dir = dir.path().join("HLSR").join("Launcher");
        std::fs::create_dir_all(&install_dir).unwrap();
        let inside = install_dir.join("HLSR-Setup-0.5.0.exe");
        std::fs::rename(&setup, &inside).unwrap();
        let source = Source::Payload(Box::new(Payload::open(&inside).unwrap()));
        let registry = FakeRegistry::new();
        let id = layout::identity(None).unwrap();
        let shortcuts = Shortcuts::new(
            Arc::new(FakeShellLinks::new()),
            &FakeKnownFolders::under(dir.path()),
            &id,
        )
        .unwrap();
        let log = MemoryLogger::default();
        let install_dir = install_dir.to_string_lossy().into_owned();
        // Библиотека — во временном каталоге, не `C:\HLSR`, даже если проверка пропустит.
        let library = dir.path().join("Library").to_string_lossy().into_owned();
        let mut options = InstallOptions::new(&source, &install_dir, &registry, &shortcuts, &log);
        options.env = &|_| None;
        options.library_root = &library;
        options.sleep = &|_| {};
        let error = install::install(options).unwrap_err();
        assert_eq!(error.code.as_deref(), Some("BAD_DIR"));
        assert_eq!(error.message, "Установщик запущен из каталога установки");
        assert!(registry.ops().is_empty());
    }
}
