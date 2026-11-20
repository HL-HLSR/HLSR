//! `hlsr-release setup`: установщик Windows с нагрузкой, `latest.yml`, `install-files.json`.
//! Установщик открывается кодом, которым его читает `HLSR-Setup.exe` при установке
//! (`hlsr_update::windows::payload`), файлы из нагрузки сверяются с собранными.

mod support;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use hlsr_release::setup::{self, SetupOptions};
use hlsr_update::windows::file_list::{self, sha256_file};
use hlsr_update::windows::layout;
use hlsr_update::windows::payload::{Payload, Source, Trailer, read_trailer};
use hlsr_update::windows::setup::app_name_of;
use serde_json::Value;
use sha2::{Digest, Sha512};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use support::{exe_with_version, sha256, windows_resources, write};

const DATE: &str = "2026-10-16T09:00:00.000Z";

struct Fixture {
    _dir: tempfile::TempDir,
    exe: PathBuf,
    resources: PathBuf,
    out: PathBuf,
}

fn fixture(product: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let exe = write(dir.path(), "build/HLSR.exe", &exe_with_version(product));
    let resources = dir.path().join("resources-src");
    windows_resources(&resources);
    let out = dir.path().join("dist");
    Fixture {
        _dir: dir,
        exe,
        resources,
        out,
    }
}

fn options(f: &Fixture, version: &str) -> SetupOptions {
    SetupOptions {
        exe: f.exe.clone(),
        resources: Some(f.resources.clone()),
        version: version.to_owned(),
        qa: false,
        out: f.out.clone(),
        release_date: Some(DATE.to_owned()),
        allow_unversioned_exe: false,
    }
}

fn read(source: &Source, path: &str, size: u64) -> Vec<u8> {
    let list = source.read_list().unwrap();
    let item = list.files.iter().find(|item| item.path == path).unwrap();
    assert_eq!(item.size, size, "{path}");
    let mut bytes = Vec::new();
    source.open(item).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn installer_is_exe_plus_payload_plus_trailer_and_latest_yml_has_the_0_4_7_text() {
    let f = fixture("0.5.0.0");
    let output = setup::run(&options(&f, "0.5.0")).unwrap();
    assert_eq!(output.setup, f.out.join("HLSR-Setup-0.5.0.exe"));

    // Хвост: сигнатура, смещение = длина HLSR.exe, нагрузка кончается ровно перед хвостом.
    let exe = fs::read(&f.exe).unwrap();
    let setup_bytes = fs::read(&output.setup).unwrap();
    let trailer = read_trailer(&output.setup).unwrap().expect("хвост");
    assert_eq!(trailer.offset, exe.len() as u64);
    assert_eq!(
        trailer.offset + trailer.length + Trailer::to_bytes(&trailer).len() as u64,
        setup_bytes.len() as u64
    );
    assert_eq!(
        &setup_bytes[..exe.len()],
        &exe[..],
        "HLSR.exe — первые N байт"
    );
    assert_eq!(output.installer.size, setup_bytes.len() as u64);

    // latest.yml: текст build-windows-setup.js:92-113.
    let sha512 = BASE64_STANDARD.encode(Sha512::digest(&setup_bytes));
    let expected = format!(
        "version: 0.5.0\nfiles:\n  - url: HLSR-Setup-0.5.0.exe\n    sha512: {sha512}\n    size: {}\n\
         path: HLSR-Setup-0.5.0.exe\nsha512: {sha512}\nreleaseDate: '{DATE}'\n",
        setup_bytes.len()
    );
    let yml = fs::read_to_string(&output.latest_yml).unwrap();
    assert_eq!(yml, expected);
    assert_eq!(output.installer.sha512, sha512);
    assert_eq!(output.latest_yml_sha256, sha256(expected.as_bytes()));
    assert_eq!(output.installer.sha256, sha256(&setup_bytes));
}

#[test]
fn files_read_back_from_the_payload_are_the_staged_files() {
    let f = fixture("0.5.0.0");
    let output = setup::run(&options(&f, "0.5.0")).unwrap();
    let source = Source::Payload(Box::new(Payload::open(&output.setup).unwrap()));
    let list = source.read_list().unwrap();
    assert_eq!(list.version, "0.5.0");
    assert_eq!(list.app, layout::App::production());
    let paths: Vec<&str> = list.files.iter().map(|item| item.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "HLSR.exe",
            "resources/7zip/LICENSE.txt",
            "resources/7zip/win/x64/7za.exe",
            "resources/licenses/HLSR-LICENSE.txt",
            "resources/livesplit-components/LiveSplit.BunnySplit.dll",
            "resources/livesplit-overlay/hlsr-overlay.dll",
        ],
        "порядок — по строкам JS, список файла не содержит сам себя"
    );
    for item in &list.files {
        let bytes = read(&source, &item.path, item.size);
        assert_eq!(sha256(&bytes), item.sha256, "{}", item.path);
        let on_disk = item
            .path
            .split('/')
            .fold(output.unpacked.clone(), |path, part| path.join(part));
        assert_eq!(bytes, fs::read(on_disk).unwrap(), "{}", item.path);
    }
    // Каждый файл списка совпал с win-unpacked по хешу.
    for item in &list.files {
        let on_disk = item
            .path
            .split('/')
            .fold(output.unpacked.clone(), |path, part| path.join(part));
        assert_eq!(sha256_file(&on_disk).unwrap(), item.sha256);
    }
}

#[test]
fn install_files_json_is_json_stringify_plus_newline() {
    let f = fixture("0.5.0.0");
    let output = setup::run(&options(&f, "0.5.0")).unwrap();
    let text = fs::read_to_string(output.unpacked.join("resources/install-files.json")).unwrap();
    assert!(
        text.starts_with(
            "{\"schema\":1,\"version\":\"0.5.0\",\"app\":{\"appId\":\"com.hlsrteam.hlsr\",\"productName\":\"HLSR\"},\"files\":[{\"path\":\"HLSR.exe\",\"size\":"
        ),
        "{text}"
    );
    assert!(text.ends_with("}]}\n"));
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert!(file_list::problems_of(&parsed).is_empty());
    // Это же чтение, которое делает установка из распакованной сборки (путь В 0.4.7).
    let list = file_list::read(&output.unpacked).unwrap();
    assert_eq!(list.files.len(), 6);
    assert_eq!(output.files, 6);
}

#[test]
fn qa_build_gets_its_own_identity() {
    let f = fixture("0.5.0.0");
    let mut qa = options(&f, "0.5.0");
    qa.qa = true;
    let output = setup::run(&qa).unwrap();
    assert_eq!(output.app.app_id, "com.hlsrteam.hlsr.qa");
    let list = file_list::read(&output.unpacked).unwrap();
    assert_eq!(list.app.app_id, "com.hlsrteam.hlsr.qa");
    assert_eq!(list.app.product_name, "HLSR-QA");
    let identity = layout::identity(Some(&list.app)).unwrap();
    assert_eq!(identity.guid, "a4c160bc-6860-53ec-9475-5b670bd2800e");
    assert_eq!(identity.default_install_dir, "C:\\HLSR-QA\\Launcher");
    // Имя установщика то же: канал и апдейтер 0.4.7 требуют ровно HLSR-Setup-<v>.exe.
    assert!(output.setup.ends_with("HLSR-Setup-0.5.0.exe"));
}

/// Упаковка `--qa` → лаунчер узнаёт QA-сборку (`app.getName()` — `hlsr-qa`, как у QA-сборки 0.4.7)
/// и из установщика (окно установки, тихая установка), и из распакованной сборки; боевая — `hlsr`.
#[test]
fn packaged_build_knows_its_app_name() {
    let f = fixture("0.5.0.0");
    let mut qa = options(&f, "0.5.0");
    qa.qa = true;
    let output = setup::run(&qa).unwrap();
    assert_eq!(app_name_of(&output.setup), "hlsr-qa");
    assert_eq!(app_name_of(&output.unpacked.join(layout::EXE)), "hlsr-qa");

    let f = fixture("0.5.0.0");
    let output = setup::run(&options(&f, "0.5.0")).unwrap();
    assert_eq!(app_name_of(&output.setup), "hlsr");
    assert_eq!(app_name_of(&output.unpacked.join(layout::EXE)), "hlsr");
}

/// Установка из упакованных установщиков (тихая установка, подделанные реестр и ярлыки): QA-лаунчер
/// из каталога программы берёт свой профиль `%APPDATA%\hlsr-qa` и журналы
/// `%LOCALAPPDATA%\hlsr-qa\logs`, боевой — прежние `hlsr`. Пути Windows на настоящей ФС — только
/// Windows.
#[cfg(windows)]
#[test]
fn installed_launcher_takes_the_profile_of_its_build() {
    use hlsr_core::logger::MemoryLogger;
    use hlsr_core::paths::Paths;
    use hlsr_update::windows::install::{self, InstallOptions};
    use hlsr_update::windows::known_folders::FakeKnownFolders;
    use hlsr_update::windows::registry::FakeRegistry;
    use hlsr_update::windows::shortcuts::{FakeShellLinks, Shortcuts};
    use std::sync::Arc;

    for (qa, name, dir_name) in [(true, "hlsr-qa", "HLSR-QA"), (false, "hlsr", "HLSR")] {
        let f = fixture("0.5.0.0");
        let mut packed = options(&f, "0.5.0");
        packed.qa = qa;
        let output = setup::run(&packed).unwrap();
        let source = Source::Payload(Box::new(Payload::open(&output.setup).unwrap()));
        let list = source.read_list().unwrap();
        let id = layout::identity(Some(&list.app)).unwrap();
        let root = f._dir.path();
        let registry = FakeRegistry::new();
        let shortcuts = Shortcuts::new(
            Arc::new(FakeShellLinks::new()),
            &FakeKnownFolders::under(root),
            &id,
        )
        .unwrap();
        let log = MemoryLogger::default();
        let install_dir = root.join(dir_name).join("Launcher");
        let install_dir = install_dir.to_string_lossy().into_owned();
        let library = root.join("Library").to_string_lossy().into_owned();
        let mut install = InstallOptions::new(&source, &install_dir, &registry, &shortcuts, &log);
        install.env = &|_| None;
        install.library_root = &library;
        install.sleep = &|_| {};
        install::install(install).unwrap();

        let exe = PathBuf::from(&install_dir).join(layout::EXE);
        assert_eq!(app_name_of(&exe), name, "qa={qa}");
        let roaming = root.join("AppData").join("Roaming");
        let local = root.join("AppData").join("Local");
        let paths = Paths::windows_from(&roaming, &local, app_name_of(&exe));
        assert_eq!(paths.data_dir, roaming.join(name));
        assert_eq!(paths.logs_dir, local.join(name).join("logs"));
        assert_eq!(paths.app_name, name);
    }
}

#[test]
fn only_x_y_z_versions_are_released() {
    for bad in ["0.5.0-dev", "0.5.0-beta.1", "0.5", "v0.5.0", "0.5.0.0", ""] {
        let f = fixture("0.5.0.0");
        let error = setup::run(&options(&f, bad)).unwrap_err().to_string();
        assert!(error.contains("X.Y.Z"), "{bad}: {error}");
        assert!(!f.out.exists(), "{bad}: ничего не пишется");
    }
}

#[test]
fn exe_product_version_has_to_match_the_release() {
    let f = fixture("0.4.0.0");
    let error = setup::run(&options(&f, "0.5.0")).unwrap_err().to_string();
    assert!(error.contains("ProductVersion 0.4.0.0"), "{error}");
    // Хвост в четвёртом числе не мешает: сравниваются три.
    let f = fixture("0.5.0.7");
    setup::run(&options(&f, "0.5.0")).unwrap();
}

#[test]
fn exe_without_version_resource_needs_explicit_consent() {
    let dir = tempfile::tempdir().unwrap();
    let exe = write(dir.path(), "HLSR.exe", b"MZ but no resources");
    let mut o = SetupOptions {
        exe,
        resources: None,
        version: "0.5.0".to_owned(),
        qa: false,
        out: dir.path().join("dist"),
        release_date: Some(DATE.to_owned()),
        allow_unversioned_exe: false,
    };
    assert!(setup::run(&o).is_err(), "файл не PE — отказ, а не молчание");
    // Настоящий PE без ресурса версии.
    let bare = hlsr_release::testing::pe_with_version(None);
    o.exe = write(dir.path(), "bare.exe", &bare);
    let error = setup::run(&o).unwrap_err().to_string();
    assert!(error.contains("нет ресурса версии"), "{error}");
    o.allow_unversioned_exe = true;
    setup::run(&o).unwrap();
}

#[test]
fn same_inputs_give_the_same_bytes() {
    let a = fixture("0.5.0.0");
    let b = fixture("0.5.0.0");
    let first = setup::run(&options(&a, "0.5.0")).unwrap();
    let second = setup::run(&options(&b, "0.5.0")).unwrap();
    assert_eq!(first.installer, second.installer);
    assert_eq!(
        fs::read(&first.latest_yml).unwrap(),
        fs::read(&second.latest_yml).unwrap()
    );
    // Повторный запуск в тот же каталог заменяет прежнюю раскладку и даёт те же байты.
    let again = setup::run(&options(&a, "0.5.0")).unwrap();
    assert_eq!(again.installer, first.installer);
}

#[test]
fn a_foreign_unpacked_directory_is_not_erased() {
    let f = fixture("0.5.0.0");
    write(&f.out, "win-unpacked/precious.txt", b"not ours");
    let error = setup::run(&options(&f, "0.5.0")).unwrap_err().to_string();
    assert!(error.contains("стирать не буду"), "{error}");
    assert!(f.out.join("win-unpacked/precious.txt").exists());
}

#[test]
fn resources_must_not_bring_their_own_list() {
    let f = fixture("0.5.0.0");
    write(&f.resources, "install-files.json", b"{}");
    let error = setup::run(&options(&f, "0.5.0")).unwrap_err().to_string();
    assert!(error.contains("install-files.json"), "{error}");
}

#[test]
fn installer_has_no_blockmap_and_no_electron_leftovers() {
    let f = fixture("0.5.0.0");
    setup::run(&options(&f, "0.5.0")).unwrap();
    let mut names: Vec<String> = fs::read_dir(&f.out)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    // Р4: blockmap не публикуется; рядом только установщик, latest.yml и раскладка для нарезки.
    assert_eq!(
        names,
        ["HLSR-Setup-0.5.0.exe", "latest.yml", "win-unpacked"]
    );
}
