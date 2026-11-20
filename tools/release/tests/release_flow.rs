//! Выпуск от упаковки до приёмки 0.4.7: артефакты `hlsr-release` → Node-скрипты 0.4.7 с минимальными
//! правками (`tools/release/node`: подпись канала тестовым ключом, чанки, блок `build`) →
//! `tools/compat-047/accept-release.js` (замороженная обновлялка 0.4.7 проверяет готовые байты).
//! Самотест 0.4.7 «signed release is the installer, latest-linux.yml and channel; production rejects
//! test keys» — Linux-часть; Windows-часть — приёмка установщика. Нужен `node` в PATH (как для
//! фикстур `tools/reference`); без него тесты падают, а не пропускаются: пропуск спрятал бы
//! непроверенный выпуск.

mod support;

use hlsr_release::appimage::{self, AppImageOptions, Runner};
use hlsr_release::archive::{self, ArchiveOptions};
use hlsr_release::error::Result;
use hlsr_release::setup::{self, SetupOptions};
use hlsr_release::testing::{png_stub, runtime_stub};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use support::{exe_with_version, linux_tree, sha256, windows_resources, write};

const VERSION: &str = "0.5.0";
const DATE: &str = "2026-10-16T09:00:00.000Z";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("корень репозитория")
        .to_owned()
}

fn node(script: &str, args: &[&str], env: &[(&str, &Path)]) -> Output {
    let mut command = Command::new("node");
    command
        .arg(repo().join(script))
        .args(args)
        .current_dir(repo());
    for (name, value) in env {
        command.env(name, value);
    }
    command
        .output()
        .unwrap_or_else(|error| panic!("не запустился node (нужен в PATH): {error}"))
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct Keys {
    private_key: PathBuf,
    public_key: PathBuf,
    test_keys: PathBuf,
    changelog: PathBuf,
    attest_dir: PathBuf,
}

fn make_keys(dir: &Path) -> Keys {
    let output = node(
        "tools/release/node/tests/support/make-keys.js",
        &[dir.to_str().unwrap(), VERSION],
        &[],
    );
    assert!(output.status.success(), "{}", text(&output));
    let paths: Value = serde_json::from_slice(&output.stdout).unwrap();
    let path = |name: &str| PathBuf::from(paths[name].as_str().unwrap());
    Keys {
        private_key: path("privateKey"),
        public_key: path("publicKey"),
        test_keys: path("testKeys"),
        changelog: path("changelog"),
        attest_dir: path("attestDir"),
    }
}

/// Подпись канала Node-скриптом 0.4.7 (`launcher-release-prepare*.js`): ключи только тестовые.
fn prepare(script: &str, extra: &[&str], keys: &Keys, test_trust: bool) -> Output {
    let mut args = vec!["--version", VERSION];
    args.extend_from_slice(extra);
    if test_trust {
        args.extend(["--test-public-key", keys.public_key.to_str().unwrap()]);
    }
    node(
        script,
        &args,
        &[
            ("HLSR_MANIFEST_PRIVATE_KEY_PATH", &keys.private_key),
            ("HLSR_ATTEST_KEYS_DIR", &keys.attest_dir),
            ("HLSR_CHANGELOG_JSON", &keys.changelog),
        ],
    )
}

fn accept(dist: &Path, platform: &str, keys: Option<&Keys>, extra: &[&str]) -> Output {
    let mut args = vec!["--dist", dist.to_str().unwrap(), "--platform", platform];
    if let Some(keys) = keys {
        args.extend(["--test-keys", keys.test_keys.to_str().unwrap()]);
    }
    let release = env!("CARGO_BIN_EXE_hlsr-release");
    args.extend(["--hlsr-release", release]);
    args.extend_from_slice(extra);
    node("tools/compat-047/accept-release.js", &args, &[])
}

fn failed_steps(report: &str) -> Vec<&str> {
    report
        .lines()
        .filter(|line| line.starts_with("FAIL"))
        .collect()
}

fn step<'a>(report: &'a str, name: &str) -> &'a str {
    report
        .lines()
        .find(|line| line.contains(name))
        .unwrap_or_else(|| panic!("в отчёте нет шага «{name}»:\n{report}"))
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

// ------------------------------------------------------------------ Windows

struct Windows {
    dir: tempfile::TempDir,
    keys: Keys,
    dist: PathBuf,
}

/// Выпуск Windows: `hlsr-release setup` → Node `launcher-release-prepare.js` (production-профиль,
/// подпись тестовым ключом, trust root подменён) → каталог, который проверяет приёмка.
fn windows_release() -> Windows {
    let dir = tempfile::tempdir().unwrap();
    let keys = make_keys(&dir.path().join("keys"));
    let exe = write(dir.path(), "build/HLSR.exe", &exe_with_version("0.5.0.0"));
    let resources = dir.path().join("resources-src");
    windows_resources(&resources);
    let dist = dir.path().join("dist").join("windows");
    setup::run(&SetupOptions {
        exe,
        resources: Some(resources),
        version: VERSION.to_owned(),
        qa: false,
        out: dist.clone(),
        release_date: Some(DATE.to_owned()),
        allow_unversioned_exe: false,
    })
    .unwrap();
    let store = dir.path().join("chunk-store");
    let output = prepare(
        "tools/release/node/launcher-release-prepare.js",
        &[
            "--build-dir",
            dist.to_str().unwrap(),
            "--out",
            dist.to_str().unwrap(),
            "--chunk-store",
            store.to_str().unwrap(),
        ],
        &keys,
        true,
    );
    assert!(output.status.success(), "{}", text(&output));
    Windows { dir, keys, dist }
}

#[test]
fn windows_release_passes_the_frozen_0_4_7_acceptance() {
    let release = windows_release();
    let output = accept(&release.dist, "win32", Some(&release.keys), &[]);
    let report = text(&output);
    assert!(output.status.success(), "{report}");
    assert!(failed_steps(&report).is_empty(), "{report}");
    for name in [
        "канал: форма и подпись",
        "версия выпуска — только X.Y.Z",
        "установщик: имя, размер, ровно два адреса",
        "verifyInstallerStream",
        "latest.yml: байт в байт",
        "plan.validate принимает выпуск",
        "без сегмента HLSR",
        "delta: блок и манифест кусков",
        "блок build: присутствует и покрыт подписью",
        "нагрузка установщика",
        "build.executable совпал с HLSR.exe",
        "ProductVersion HLSR.exe",
    ] {
        // По шагу на базу: по умолчанию 0.4.8 (копии 0.4.7) и 0.4.9 (свои копии).
        let passed = report
            .lines()
            .filter(|line| line.starts_with("ok") && line.contains(name))
            .count();
        assert_eq!(passed, 2, "{name}:\n{report}");
    }
    for base in ["база 0.4.8 (набор 0.4.7)", "база 0.4.9 (набор 0.4.9)"] {
        assert!(report.contains(base), "{base}:\n{report}");
    }

    // Канал — то, что подписал Node, и в нём есть всё, что ждут 0.4.7 и сервер.
    let channel: Value =
        serde_json::from_slice(&fs::read(release.dist.join("launcher-channel.json")).unwrap())
            .unwrap();
    let mut keys: Vec<&str> = channel
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "build",
            "delta",
            "installer",
            "latestYmlSha256",
            "minimumSupportedVersion",
            "publishedAt",
            "schemaVersion",
            "signature",
            "updateRequired",
            "version"
        ]
    );
    assert_eq!(channel["build"]["executable"]["path"], "HLSR.exe");
    assert_eq!(channel["build"]["attest"]["v"], 1);
    assert_eq!(channel["installer"]["filename"], "HLSR-Setup-0.5.0.exe");
    assert_eq!(channel["installer"]["urls"].as_array().unwrap().len(), 2);
    // blockmap не публикуется (Р4).
    assert!(!fs::read_dir(&release.dist).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("blockmap")
    }));
    let plan: Value =
        serde_json::from_slice(&fs::read(release.dist.join("publish-plan.json")).unwrap()).unwrap();
    let kinds: Vec<&str> = plan["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "launcher-installer",
            "launcher-chunk-manifest",
            "launcher-latest-yml",
            "launcher-changelog",
            "channel-manifest"
        ]
    );
}

#[test]
fn production_acceptance_rejects_a_test_key_and_the_node_verifier_agrees() {
    let release = windows_release();
    // Без --test-keys trust root — вшитые ключи 0.4.7: тестовая подпись не проходит.
    let output = accept(&release.dist, "win32", None, &[]);
    let report = text(&output);
    assert!(!output.status.success(), "{report}");
    assert!(
        step(&report, "канал: форма и подпись").starts_with("FAIL"),
        "{report}"
    );
    assert!(
        step(&report, "plan.validate принимает выпуск").starts_with("FAIL"),
        "{report}"
    );

    let verify = |public_key: Option<&Path>| {
        let mut args: Vec<String> = [
            "--channel",
            release.dist.join("launcher-channel.json").to_str().unwrap(),
            "--installer",
            release.dist.join("HLSR-Setup-0.5.0.exe").to_str().unwrap(),
            "--latest-yml",
            release.dist.join("latest.yml").to_str().unwrap(),
            "--expected-version",
            VERSION,
        ]
        .map(str::to_owned)
        .to_vec();
        if let Some(public_key) = public_key {
            args.extend([
                "--public-key".to_owned(),
                public_key.to_str().unwrap().to_owned(),
            ]);
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        node("tools/release/node/verify-launcher-release.js", &args, &[])
    };
    let rejected = verify(None);
    assert!(!rejected.status.success());
    assert!(
        text(&rejected).contains("trust root"),
        "{}",
        text(&rejected)
    );
    let accepted = verify(Some(&release.keys.public_key));
    assert!(accepted.status.success(), "{}", text(&accepted));

    // Боевой профиль тоже не подпишет тестовым ключом без явной подмены trust root.
    let again = prepare(
        "tools/release/node/launcher-release-prepare.js",
        &[
            "--build-dir",
            release.dist.to_str().unwrap(),
            "--out",
            release.dir.path().join("second").to_str().unwrap(),
            "--no-chunks",
        ],
        &release.keys,
        false,
    );
    assert_eq!(again.status.code(), Some(1), "{}", text(&again));
    assert!(
        text(&again).contains("PRODUCTION-канал не может быть подписан"),
        "{}",
        text(&again)
    );
}

#[test]
fn tampered_artifacts_fail_the_acceptance() {
    let release = windows_release();
    let check = |name: &str, tamper: &dyn Fn(&Path), failing: &[&str]| {
        let copy = release.dir.path().join(format!("case-{name}"));
        copy_dir(&release.dist, &copy);
        tamper(&copy);
        let output = accept(&copy, "win32", Some(&release.keys), &[]);
        let report = text(&output);
        assert!(!output.status.success(), "{name}:\n{report}");
        for step_name in failing {
            assert!(
                step(&report, step_name).starts_with("FAIL"),
                "{name} / {step_name}:\n{report}"
            );
        }
    };
    // Лишний байт установщика: размер и хеши канала не сходятся.
    check(
        "installer",
        &|dir| {
            let path = dir.join("HLSR-Setup-0.5.0.exe");
            let mut bytes = fs::read(&path).unwrap();
            bytes.push(0);
            fs::write(path, bytes).unwrap();
        },
        &["установщик: имя, размер", "verifyInstallerStream"],
    );
    // latest.yml другой сборки: хеш из подписанного канала не совпал.
    check(
        "latest-yml",
        &|dir| {
            let path = dir.join("latest.yml");
            let yml = fs::read_to_string(&path).unwrap();
            fs::write(path, yml.replace("size:", "size:  ")).unwrap();
        },
        &["latest.yml: байт в байт"],
    );
    // Правка блока build в канале ломает подпись: блок покрыт ею (Р3).
    check(
        "build-block",
        &|dir| {
            let path = dir.join("launcher-channel.json");
            let mut channel: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            channel["build"]["executable"]["sha256"] = Value::from("0".repeat(64));
            fs::write(&path, serde_json::to_string_pretty(&channel).unwrap()).unwrap();
        },
        &["канал: форма и подпись"],
    );
    // Версия не X.Y.Z в имени каталога выпуска.
    check(
        "installer-name",
        &|dir| {
            fs::rename(
                dir.join("HLSR-Setup-0.5.0.exe"),
                dir.join("HLSR-Setup-0.5.exe"),
            )
            .unwrap();
        },
        &["установщик: имя, размер"],
    );
}

// ------------------------------------------------------------------ Linux

/// Вместо `mksquashfs` — файл с любыми байтами: настоящий образ собирается на Linux-машине.
struct FakeSquashfs;

impl Runner for FakeSquashfs {
    fn run(&self, _program: &str, args: &[String]) -> Result<()> {
        fs::write(&args[1], b"fake squashfs bytes").unwrap();
        Ok(())
    }
}

struct Linux {
    dir: tempfile::TempDir,
    keys: Keys,
    dist: PathBuf,
    tree: PathBuf,
}

/// Выпуск Linux: `archive` → `appimage` (с подменой `mksquashfs`) → Node
/// `launcher-release-prepare-linux.js`; `payload.json` и архив лежат рядом с AppImage, как их оставляет
/// распаковка рантайма.
fn linux_release() -> Linux {
    let dir = tempfile::tempdir().unwrap();
    let keys = make_keys(&dir.path().join("keys"));
    let tree = dir.path().join("linux-unpacked");
    linux_tree(&tree);
    let dist = dir.path().join("dist").join("linux");
    let payload = dist.join("payload");
    archive::run(&ArchiveOptions {
        tree: tree.clone(),
        version: VERSION.to_owned(),
        out: payload.clone(),
        executables: Vec::new(),
        mtime: Some(0),
    })
    .unwrap();
    let launcher = write(
        dir.path(),
        "target/hlsr",
        &hlsr_release::testing::elf_stub("launcher"),
    );
    let runtime = write(dir.path(), "runtime-x86_64", &runtime_stub());
    let image = dist.join(format!("HLSR-Setup-{VERSION}-linux-x86_64.AppImage"));
    appimage::run(
        &AppImageOptions {
            payload: payload.clone(),
            exe: launcher,
            icon: Some(write(dir.path(), "icon.png", &png_stub())),
            runtime,
            runtime_sha256: sha256(&runtime_stub()),
            out: image,
            mksquashfs: "mksquashfs".to_owned(),
            release_date: Some(DATE.to_owned()),
        },
        &FakeSquashfs,
    )
    .unwrap();
    for name in ["payload.json", "HLSR-0.5.0-linux-x86_64.tar.gz"] {
        fs::copy(payload.join(name), dist.join(name)).unwrap();
    }
    let store = dir.path().join("chunk-store");
    let output = prepare(
        "tools/release/node/launcher-release-prepare-linux.js",
        &[
            "--build-dir",
            dist.to_str().unwrap(),
            "--out",
            dist.to_str().unwrap(),
            "--tree",
            tree.to_str().unwrap(),
            "--chunk-store",
            store.to_str().unwrap(),
        ],
        &keys,
        true,
    );
    assert!(output.status.success(), "{}", text(&output));
    Linux {
        dir,
        keys,
        dist,
        tree,
    }
}

#[test]
fn linux_release_is_the_installer_latest_linux_yml_and_a_signed_channel() {
    let release = linux_release();
    let channel: Value =
        serde_json::from_slice(&fs::read(release.dist.join("launcher-channel.json")).unwrap())
            .unwrap();
    let mut keys: Vec<&str> = channel
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "build",
            "delta",
            "installer",
            "latestYmlSha256",
            "minimumSupportedVersion",
            "publishedAt",
            "schemaVersion",
            "signature",
            "updateRequired",
            "version"
        ]
    );
    assert_eq!(channel["version"], VERSION);
    assert_eq!(
        channel["installer"]["filename"],
        "HLSR-Setup-0.5.0-linux-x86_64.AppImage"
    );
    assert_eq!(channel["build"]["executable"]["path"], "hlsr");
    let yml = fs::read_to_string(release.dist.join("latest-linux.yml")).unwrap();
    assert!(
        yml.starts_with(
            "version: 0.5.0\nfiles:\n  - url: HLSR-Setup-0.5.0-linux-x86_64.AppImage\n"
        ),
        "{yml}"
    );
    assert!(yml.contains(&format!(
        "sha512: {}",
        channel["installer"]["sha512"].as_str().unwrap()
    )));
    assert_eq!(
        sha256(yml.as_bytes()),
        channel["latestYmlSha256"].as_str().unwrap()
    );

    // Приёмка Linux-части на этой ОС: канал, хеши, latest-linux.yml, gate с локального HTTP,
    // разбор вшитого архива; рантайм AppImage и commit — только Linux (skip с причиной).
    let output = accept(&release.dist, "linux", Some(&release.keys), &[]);
    let report = text(&output);
    assert!(output.status.success(), "{report}");
    assert!(failed_steps(&report).is_empty(), "{report}");
    assert!(
        report.contains("Linux, база 0.4.9 (набор 0.4.9)"),
        "{report}"
    );
    for name in [
        "канал: форма и подпись",
        "latest-linux.yml: байт в байт",
        "LauncherUpdateGate.evaluate с локального HTTP",
        "вшитый архив: payload.json рядом с AppImage",
    ] {
        assert!(step(&report, name).starts_with("ok"), "{name}:\n{report}");
    }

    // Проверка выпуска Node-скриптом 0.4.7 (с подменой запуска рантайма — его здесь нет).
    let verify = node(
        "tools/release/node/tests/support/verify-linux.js",
        &[
            release.dist.to_str().unwrap(),
            release.keys.public_key.to_str().unwrap(),
        ],
        &[],
    );
    assert!(verify.status.success(), "{}", text(&verify));
    // Испорченный AppImage ловится по размеру и хешам канала.
    let copy = release.dir.path().join("broken");
    copy_dir(&release.dist, &copy);
    let image = copy.join("HLSR-Setup-0.5.0-linux-x86_64.AppImage");
    let mut bytes = fs::read(&image).unwrap();
    bytes.extend_from_slice(b"bad");
    fs::write(&image, bytes).unwrap();
    let broken = node(
        "tools/release/node/tests/support/verify-linux.js",
        &[
            copy.to_str().unwrap(),
            release.keys.public_key.to_str().unwrap(),
        ],
        &[],
    );
    assert!(!broken.status.success());
    assert!(text(&broken).contains("размер"), "{}", text(&broken));
}

#[test]
fn linux_production_profile_rejects_a_test_key() {
    let release = linux_release();
    let output = prepare(
        "tools/release/node/launcher-release-prepare-linux.js",
        &[
            "--build-dir",
            release.dist.to_str().unwrap(),
            "--out",
            release.dir.path().join("production").to_str().unwrap(),
            "--tree",
            release.tree.to_str().unwrap(),
            "--no-chunks",
        ],
        &release.keys,
        false,
    );
    assert!(!output.status.success());
    assert!(
        text(&output).contains("Production-подпись не прошла проверку"),
        "{}",
        text(&output)
    );
    // Без аттестации выпуск не собирается тихо: ключа версии нет — отказ с пояснением.
    let no_key = tempfile::tempdir().unwrap();
    let missing = node(
        "tools/release/node/launcher-release-prepare-linux.js",
        &[
            "--version",
            VERSION,
            "--build-dir",
            release.dist.to_str().unwrap(),
            "--out",
            release.dir.path().join("no-attest").to_str().unwrap(),
            "--tree",
            release.tree.to_str().unwrap(),
            "--no-chunks",
            "--test-public-key",
            release.keys.public_key.to_str().unwrap(),
        ],
        &[
            ("HLSR_MANIFEST_PRIVATE_KEY_PATH", &release.keys.private_key),
            ("HLSR_ATTEST_KEYS_DIR", no_key.path()),
        ],
    );
    assert!(!missing.status.success());
    assert!(
        text(&missing).contains("Нет ключа аттестации"),
        "{}",
        text(&missing)
    );
}

#[test]
fn pre_release_versions_are_not_prepared() {
    let release = windows_release();
    for script in [
        "tools/release/node/launcher-release-prepare.js",
        "tools/release/node/launcher-release-prepare-linux.js",
    ] {
        let output = node(
            script,
            &[
                "--version",
                "0.5.0-dev",
                "--build-dir",
                release.dist.to_str().unwrap(),
            ],
            &[],
        );
        assert!(!output.status.success(), "{script}");
        assert!(
            text(&output).contains("только X.Y.Z"),
            "{script}: {}",
            text(&output)
        );
    }
}
