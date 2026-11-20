//! `hlsr-release stage`, `qa-identity`, `version`: раскладка ресурсов, удостоверение сборки,
//! версия рабочего дерева.

mod support;

use hlsr_core::json_text::parse_js_json;
use hlsr_release::qa_identity::identity_json;
use hlsr_release::stage::{self, Platform};
use hlsr_release::tree;
use hlsr_release::version::{require_release, workspace_version};
use support::write;

fn repo(umu: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for file in [
        "LICENSE",
        "build/license-en.txt",
        "build/license-ru.txt",
        "build/THIRD-PARTY-NOTICES.txt",
        "build/SOURCE-SDK-LICENSE.txt",
        "build/thirdpartylegalnotices.txt",
        "build/licenses/CascadiaCode-OFL.txt",
        "build/licenses/Rubik-OFL.txt",
        "build/licenses/FontAwesome-LICENSE.txt",
        "build/licenses/HLSRSymbols-OFL.txt",
        "build/licenses/NotoColorEmoji-OFL.txt",
        "build/licenses/umu-launcher-LICENSE.txt",
        "build/icons/256x256.png",
        "resources/7zip/LICENSE.txt",
        "resources/7zip/win/x64/7za.exe",
        "resources/7zip/linux/x64/7za",
        "resources/livesplit-overlay/proxy-d3d9.dll",
        "resources/livesplit-overlay/proxy-opengl32.dll",
        "resources/livesplit-overlay/hlsr-overlay64.exe",
        "resources/livesplit-overlay/hlsr-share64.dll",
        "resources/livesplit-components/LiveSplit.BunnySplit.dll",
        "resources/livesplit-fonts/Carlito-Regular.ttf",
        "resources/livesplit-fixes/LiveSplit.SourceSplit.dll",
        "resources/presence-relay/hlsr-presence-relay.exe",
        "resources/linux/install-userns-profile.sh",
    ] {
        write(root, file, file.as_bytes());
    }
    if umu {
        write(root, "build/umu/umu-run", b"umu");
    }
    dir
}

fn names(root: &std::path::Path) -> Vec<String> {
    tree::walk_sorted(root)
        .unwrap()
        .into_iter()
        .filter(|entry| !entry.dir)
        .map(|entry| entry.rel)
        .collect()
}

#[test]
fn windows_resources_follow_decision_r6() {
    let repo = repo(false);
    let out = repo.path().join("stage");
    let staged = stage::run(Platform::Windows, repo.path(), &out).unwrap();
    assert!(staged.skipped.is_empty());
    assert_eq!(
        names(&out),
        [
            "7zip/LICENSE.txt",
            "7zip/win/x64/7za.exe",
            "licenses/CascadiaCode-OFL.txt",
            "licenses/FontAwesome-LICENSE.txt",
            "licenses/HLSR-LICENSE.txt",
            "licenses/HLSRSymbols-OFL.txt",
            "licenses/LICENSE_en.txt",
            "licenses/LICENSE_ru.txt",
            "licenses/NotoColorEmoji-OFL.txt",
            "licenses/Rubik-OFL.txt",
            "licenses/SOURCE-SDK-LICENSE.txt",
            "licenses/THIRD-PARTY-NOTICES.txt",
            "licenses/thirdpartylegalnotices.txt",
            "livesplit-components/LiveSplit.BunnySplit.dll",
            "livesplit-overlay/proxy-d3d9.dll",
            "livesplit-overlay/proxy-opengl32.dll",
        ]
    );
}

#[test]
fn linux_resources_include_what_0_4_7_extra_resources_listed() {
    let repo = repo(true);
    let out = repo.path().join("stage");
    let staged = stage::run(Platform::Linux, repo.path(), &out).unwrap();
    assert!(staged.skipped.is_empty());
    let list = names(&out);
    for needed in [
        "icons/hlsr.png",
        "7zip/linux/x64/7za",
        "umu/umu-run",
        "livesplit-fonts/Carlito-Regular.ttf",
        "livesplit-fixes/LiveSplit.SourceSplit.dll",
        "presence-relay/hlsr-presence-relay.exe",
        "linux/install-userns-profile.sh",
        "livesplit-overlay/proxy-d3d9.dll",
        "livesplit-overlay/proxy-opengl32.dll",
        "livesplit-overlay/hlsr-overlay64.exe",
        "livesplit-overlay/hlsr-share64.dll",
        "licenses/CascadiaCode-OFL.txt",
        "licenses/Rubik-OFL.txt",
        "licenses/FontAwesome-LICENSE.txt",
        "licenses/HLSRSymbols-OFL.txt",
        "licenses/NotoColorEmoji-OFL.txt",
        "licenses/umu-launcher-LICENSE.txt",
    ] {
        assert!(list.iter().any(|name| name == needed), "{needed}: {list:?}");
    }
    assert!(
        !list.iter().any(|name| name.starts_with("7zip/win")),
        "7za.exe Windows не нужен"
    );
    assert_eq!(
        std::fs::read(out.join("icons/hlsr.png")).unwrap(),
        b"build/icons/256x256.png"
    );
}

/// Настоящий репозиторий: тексты лицензий вшитых шрифтов (и umu на Linux) уходят в выпуск как есть,
/// у OFL — строка Copyright шрифта (таблица `name`), уведомления называют каждый файл.
#[test]
fn real_repository_ships_font_and_umu_licenses() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("корень репозитория")
        .to_owned();
    let notices = std::fs::read_to_string(root.join("build/THIRD-PARTY-NOTICES.txt")).unwrap();
    let licenses = [
        (
            "CascadiaCode-OFL.txt",
            "Copyright (c) 2019 - Present, Microsoft Corporation,",
        ),
        ("Rubik-OFL.txt", "Copyright 2015 The Rubik Project Authors"),
        (
            "FontAwesome-LICENSE.txt",
            "Fonticons, Inc. (https://fontawesome.com)",
        ),
        (
            "HLSRSymbols-OFL.txt",
            "Copyright 2016 The Inter Project Authors (https://github.com/rsms/inter)",
        ),
        ("NotoColorEmoji-OFL.txt", "Copyright 2022 Google Inc."),
        ("umu-launcher-LICENSE.txt", "GNU GENERAL PUBLIC LICENSE"),
    ];
    let out = tempfile::tempdir().unwrap();
    for platform in [Platform::Windows, Platform::Linux] {
        let stage = out.path().join(format!("{platform:?}"));
        stage::run(platform, &root, &stage).unwrap();
        for (name, first) in licenses {
            let staged = stage.join("licenses").join(name);
            if platform == Platform::Windows && name.starts_with("umu-") {
                assert!(!staged.exists(), "{name}: umu только в Linux");
                continue;
            }
            let text = std::fs::read_to_string(&staged).unwrap();
            assert_eq!(
                text,
                std::fs::read_to_string(root.join("build/licenses").join(name)).unwrap()
            );
            assert!(text.trim_start().starts_with(first), "{name}");
            if name.contains("-OFL") {
                assert!(
                    text.contains("SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007"),
                    "{name}"
                );
            }
            assert!(
                notices.contains(&format!("licenses/{name}")),
                "{name} нет в THIRD-PARTY-NOTICES.txt"
            );
        }
    }
}

#[test]
fn missing_files_are_reported_and_umu_is_optional() {
    let repo = repo(false);
    let out = repo.path().join("stage");
    let staged = stage::run(Platform::Linux, repo.path(), &out).unwrap();
    assert_eq!(staged.skipped, ["build/umu/umu-run"]);
    std::fs::remove_file(repo.path().join("build/license-ru.txt")).unwrap();
    let out = repo.path().join("stage2");
    let error = stage::run(Platform::Windows, repo.path(), &out)
        .unwrap_err()
        .to_string();
    assert!(error.contains("build/license-ru.txt"), "{error}");
    assert!(
        !out.exists(),
        "ничего не копируется, пока не хватает файлов"
    );
    // Не пустой каталог — отказ.
    let busy = repo.path().join("busy");
    write(&busy, "x", b"x");
    assert!(stage::run(Platform::Windows, repo.path(), &busy).is_err());
}

#[test]
fn production_and_qa_identities() {
    let production = identity_json(false, Some("0.5.0")).unwrap();
    assert_eq!(production["guid"], "3f824379-6b41-5725-b424-59cb87f2acae");
    assert_eq!(production["appId"], "com.hlsrteam.hlsr");
    assert_eq!(production["defaultInstallDir"], "C:\\HLSR\\Launcher");
    assert_eq!(production["installerName"], "HLSR-Setup-0.5.0.exe");
    assert_eq!(production["appName"], "hlsr");
    let qa = identity_json(true, None).unwrap();
    assert_eq!(qa["qa"], true);
    assert_eq!(qa["appId"], "com.hlsrteam.hlsr.qa");
    assert_eq!(qa["productName"], "HLSR-QA");
    // Имя приложения QA-сборки 0.4.7 (`extraMetadata.name`): свой профиль `%APPDATA%\hlsr-qa`.
    assert_eq!(qa["appName"], "hlsr-qa");
    // GUID из 0.4.7 (`windows-install/layout.js`, `guidFor`; фикстура windows-layout.json).
    assert_eq!(qa["guid"], "a4c160bc-6860-53ec-9475-5b670bd2800e");
    assert_eq!(qa["defaultInstallDir"], "C:\\HLSR-QA\\Launcher");
    assert_eq!(
        qa["installKey"],
        "HKCU\\Software\\a4c160bc-6860-53ec-9475-5b670bd2800e"
    );
    assert!(qa.get("installerName").is_none());
    // JSON читается как обычный.
    parse_js_json(&hlsr_core::json_formats::compact(&qa)).unwrap();
}

#[test]
fn release_versions_and_workspace_manifest() {
    require_release("0.5.0").unwrap();
    require_release("10.20.300").unwrap();
    for bad in [
        "0.5.0-dev",
        "0.5.0-beta.1",
        "0.5.0+1",
        "0.5",
        "1.2.3.4",
        " 0.5.0",
        "",
    ] {
        assert!(require_release(bad).is_err(), "{bad:?}");
    }
    let manifest = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.5.0-dev\"\nedition = \"2024\"\n\n[package]\nversion = \"9.9.9\"\n";
    assert_eq!(workspace_version(manifest).as_deref(), Some("0.5.0-dev"));
    assert_eq!(workspace_version("[package]\nversion = \"1.0.0\"\n"), None);
}
