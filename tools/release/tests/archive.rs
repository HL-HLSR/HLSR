//! `hlsr-release archive`: строгий USTAR `.tar.gz` для обновлялки 0.4.7, `payload.json`,
//! `.hlsr-release.json`. Архив распаковывает перенос `archive.js` (`hlsr_update::linux::archive`);
//! замороженный `archive.js` на тех же архивах гоняют Node-тесты (`tools/compat-047`). Порядок и
//! поля записей сверяются с GNU tar, которым 0.4.7 собирала архив
//! (`tar --format=ustar --sort=name --owner=0 --group=0 --numeric-owner`).

mod support;

use flate2::read::GzDecoder;
use hlsr_core::json_formats::pretty2;
use hlsr_release::archive::{self, ArchiveOptions};
use hlsr_release::testing::{elf_stub, png_stub};
use hlsr_update::linux::archive::{ExtractOptions, extract};
use hlsr_update::linux::embedded_payload;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use support::{linux_tree, sha256, write};

struct Fixture {
    _dir: tempfile::TempDir,
    tree: PathBuf,
    out: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    linux_tree(&tree);
    let out = dir.path().join("out");
    Fixture {
        _dir: dir,
        tree,
        out,
    }
}

fn options(f: &Fixture, version: &str) -> ArchiveOptions {
    ArchiveOptions {
        tree: f.tree.clone(),
        version: version.to_owned(),
        out: f.out.clone(),
        executables: vec!["resources/linux/install-userns-profile.sh".to_owned()],
        mtime: Some(0),
    }
}

/// Записи tar: `(имя, тип, размер, режим, uid, gid, уже ли USTAR)`.
fn entries(archive: &Path) -> Vec<(String, u8, u64, u32, u64, u64)> {
    let mut gz = Vec::new();
    GzDecoder::new(fs::File::open(archive).unwrap())
        .read_to_end(&mut gz)
        .unwrap();
    let mut out = Vec::new();
    for entry in tar::Archive::new(&gz[..]).entries().unwrap() {
        let entry = entry.unwrap();
        let header = entry.header();
        assert_eq!(&header.as_bytes()[257..263], b"ustar\0", "USTAR");
        assert_eq!(&header.as_bytes()[263..265], b"00");
        out.push((
            String::from_utf8(entry.path_bytes().to_vec()).unwrap(),
            header.entry_type().as_byte(),
            header.size().unwrap(),
            header.mode().unwrap(),
            header.uid().unwrap(),
            header.gid().unwrap(),
        ));
    }
    out
}

fn tree_files(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, relative: &str, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(root.join(relative)).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let child = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            if entry.file_type().unwrap().is_dir() {
                walk(root, &child, out);
            } else {
                out.push((child.clone(), fs::read(root.join(&child)).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, "", &mut out);
    out.sort();
    out
}

#[test]
fn archive_unpacks_with_the_clients_strict_extractor_into_the_same_tree() {
    let f = fixture();
    let output = archive::run(&options(&f, "0.5.0")).unwrap();
    assert_eq!(output.archive, f.out.join("HLSR-0.5.0-linux-x86_64.tar.gz"));
    let target = f._dir.path().join("unpacked");
    fs::create_dir_all(&target).unwrap();
    let done = extract(
        &output.archive,
        &target,
        ExtractOptions {
            max_bytes: Some(output.unpacked_size),
            expected_bytes: Some(output.unpacked_size),
            ..ExtractOptions::default()
        },
    )
    .unwrap();
    assert_eq!(done.bytes, output.unpacked_size);
    assert_eq!(done.entries, output.entries);
    assert_eq!(tree_files(&target), tree_files(&f.tree));
    let names: Vec<String> = tree_files(&target)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for needed in [
        ".hlsr-release.json",
        "hlsr",
        "resources/app.asar",
        "resources/icons/hlsr.png",
    ] {
        assert!(
            names.iter().any(|name| name == needed),
            "{needed}: {names:?}"
        );
    }
}

#[test]
fn release_marker_stub_and_side_files_have_the_0_4_7_forms() {
    let f = fixture();
    let output = archive::run(&options(&f, "0.5.0")).unwrap();
    // build-launcher-archive.js:12 — JSON.stringify(...) + "\n".
    assert_eq!(
        fs::read_to_string(f.tree.join(".hlsr-release.json")).unwrap(),
        "{\"version\":\"0.5.0\",\"platform\":\"linux\",\"arch\":\"x64\",\"executable\":\"hlsr\"}\n"
    );
    assert_eq!(
        fs::read_to_string(&output.info).unwrap(),
        format!(
            "{{\"version\":\"0.5.0\",\"unpackedSize\":{}}}\n",
            output.unpacked_size
        )
    );
    assert!(f.tree.join("resources/app.asar").is_file(), "заглушка");
    // payload.json: описание с отступом 2 (build-bootstrap-app.js:41) и строгая проверка клиента.
    let text = fs::read_to_string(&output.descriptor).unwrap();
    let data: Value = serde_json::from_str(&text).unwrap();
    assert!(embedded_payload::problems_of(&data).is_empty());
    assert_eq!(text, pretty2(&data) + "\n");
    assert_eq!(data["version"], "0.5.0");
    assert_eq!(data["installer"]["format"], "tar.gz");
    assert_eq!(
        data["installer"]["filename"],
        "HLSR-0.5.0-linux-x86_64.tar.gz"
    );
    assert_eq!(data["installer"]["size"], output.digests.size);
    assert_eq!(data["installer"]["sha256"], output.digests.sha256);
    assert_eq!(data["installer"]["sha512"], output.digests.sha512);
    assert_eq!(data["installer"]["unpackedSize"], output.unpacked_size);
    let bytes = fs::read(&output.archive).unwrap();
    assert_eq!(sha256(&bytes), output.digests.sha256);
    // Сумма размеров файлов == unpackedSize (archive.js: expectedBytes).
    let sum: u64 = tree_files(&f.tree)
        .iter()
        .map(|(_, b)| b.len() as u64)
        .sum();
    assert_eq!(sum, output.unpacked_size);
}

#[test]
fn entries_are_plain_ustar_in_gnu_tar_sort_order() {
    let f = fixture();
    let output = archive::run(&options(&f, "0.5.0")).unwrap();
    let list = entries(&output.archive);
    // Корень, затем записи в глубину по байтам имени; каталог — перед содержимым.
    let names: Vec<&str> = list.iter().map(|entry| entry.0.as_str()).collect();
    assert_eq!(names[0], "./");
    assert_eq!(names[1], ".hlsr-release.json");
    assert_eq!(names[2], "hlsr");
    assert_eq!(names[3], "resources/");
    let mut sorted = names[1..].to_vec();
    sorted.sort_by(|a, b| a.trim_end_matches('/').cmp(b.trim_end_matches('/')));
    // Порядок «в глубину» совпадает с сортировкой по полному пути, пока имена не содержат
    // символов меньше '/'; в тестовом дереве их нет.
    assert_eq!(names[1..], sorted[..]);
    for (name, kind, size, mode, uid, gid) in &list {
        assert!(
            matches!(*kind, b'0' | b'5'),
            "{name}: только файлы и каталоги"
        );
        assert_eq!((*uid, *gid), (0, 0), "{name}: владелец 0/0");
        if *kind == b'5' {
            assert_eq!((*size, *mode), (0, 0o755), "{name}");
            assert!(name.ends_with('/'));
        }
    }
    let by_name = |wanted: &str| list.iter().find(|entry| entry.0 == wanted).unwrap();
    assert_eq!(by_name("hlsr").3, 0o755, "hlsr исполняемый всегда");
    assert_eq!(
        by_name("resources/linux/install-userns-profile.sh").3,
        0o755
    );
    assert_eq!(by_name("resources/app.asar").3, 0o644);
    // Имя длиннее 100 байт уходит в поле prefix, а не в расширение GNU или PAX.
    let long = list
        .iter()
        .find(|entry| entry.0.ends_with("file.bin"))
        .unwrap();
    assert!(long.0.len() > 100, "{}", long.0);
    assert!(!names.iter().any(|name| name.contains("LongLink")));
}

/// GNU tar: `HLSR_GNU_TAR`, `tar` из PATH или tar из Git for Windows (в PATH Windows первым стоит bsdtar).
fn gnu_tar() -> Option<String> {
    let candidates = [
        std::env::var("HLSR_GNU_TAR").unwrap_or_default(),
        "tar".to_owned(),
        r"C:\Program Files\Git\usr\bin\tar.exe".to_owned(),
    ];
    candidates
        .into_iter()
        .filter(|path| !path.is_empty())
        .find(|path| {
            Command::new(path)
                .arg("--version")
                .output()
                .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("GNU tar"))
        })
}

#[test]
fn entry_list_matches_gnu_tar_of_0_4_7() {
    // 0.4.7: tar --format=ustar --sort=name --owner=0 --group=0 --numeric-owner -czf … -C <дерево> .
    let f = fixture();
    let output = archive::run(&options(&f, "0.5.0")).unwrap();
    let Some(tar) = gnu_tar() else {
        eprintln!("нет GNU tar: сверка пропущена");
        return;
    };
    let gnu = Command::new(tar)
        .args([
            "--format=ustar",
            "--sort=name",
            "--owner=0",
            "--group=0",
            "--numeric-owner",
            "-cf",
            "-",
            "-C",
        ])
        .arg(&f.tree)
        .arg(".")
        .output()
        .unwrap();
    assert!(
        gnu.status.success(),
        "{}",
        String::from_utf8_lossy(&gnu.stderr)
    );
    let mut theirs = Vec::new();
    for entry in tar::Archive::new(&gnu.stdout[..]).entries().unwrap() {
        let entry = entry.unwrap();
        let name = String::from_utf8(entry.path_bytes().to_vec()).unwrap();
        let name = name.strip_prefix("./").unwrap_or(&name).to_owned();
        theirs.push((
            if name.is_empty() {
                "./".to_owned()
            } else {
                name
            },
            entry.header().entry_type().as_byte(),
            entry.header().size().unwrap(),
        ));
    }
    let ours: Vec<_> = entries(&output.archive)
        .into_iter()
        .map(|(name, kind, size, ..)| (name, kind, size))
        .collect();
    assert_eq!(ours, theirs, "имена, типы и размеры совпадают с GNU tar");
}

#[test]
fn same_tree_gives_the_same_archive_bytes() {
    let f = fixture();
    let first = archive::run(&options(&f, "0.5.0")).unwrap();
    let before = fs::read(&first.archive).unwrap();
    let again = archive::run(&options(&f, "0.5.0")).unwrap();
    assert_eq!(fs::read(&again.archive).unwrap(), before);
    assert_eq!(again.digests, first.digests);
}

#[test]
fn only_x_y_z_versions_are_archived() {
    for bad in ["0.5.0-dev", "0.5.0-beta.1", "0.5", ""] {
        let f = fixture();
        let error = archive::run(&options(&f, bad)).unwrap_err().to_string();
        assert!(error.contains("X.Y.Z"), "{bad}: {error}");
        assert!(!f.out.exists());
    }
}

#[test]
fn tree_that_0_4_7_would_not_install_is_refused() {
    // Нет hlsr.
    let f = fixture();
    fs::remove_file(f.tree.join("hlsr")).unwrap();
    assert!(
        archive::run(&options(&f, "0.5.0"))
            .unwrap_err()
            .to_string()
            .contains("нет hlsr")
    );
    // hlsr не ELF x86-64.
    let f = fixture();
    write(&f.tree, "hlsr", b"#!/bin/sh\n");
    assert!(
        archive::run(&options(&f, "0.5.0"))
            .unwrap_err()
            .to_string()
            .contains("не ELF")
    );
    // ELF, но не x86-64 (e_machine 183 — aarch64).
    let f = fixture();
    let mut arm = elf_stub("");
    arm[18..20].copy_from_slice(&183u16.to_le_bytes());
    write(&f.tree, "hlsr", &arm);
    assert!(
        archive::run(&options(&f, "0.5.0"))
            .unwrap_err()
            .to_string()
            .contains("не ELF")
    );
    // Нет значка или он не PNG.
    let f = fixture();
    fs::remove_file(f.tree.join("resources/icons/hlsr.png")).unwrap();
    assert!(
        archive::run(&options(&f, "0.5.0"))
            .unwrap_err()
            .to_string()
            .contains("hlsr.png")
    );
    let f = fixture();
    write(&f.tree, "resources/icons/hlsr.png", b"GIF89a");
    assert!(
        archive::run(&options(&f, "0.5.0"))
            .unwrap_err()
            .to_string()
            .contains("не PNG")
    );
    // Нет дерева.
    let f = fixture();
    let mut o = options(&f, "0.5.0");
    o.tree = f.tree.join("missing");
    assert!(archive::run(&o).is_err());
    let _ = png_stub();
}

#[cfg(unix)]
#[test]
fn symlinks_in_the_tree_are_refused() {
    let f = fixture();
    std::os::unix::fs::symlink("hlsr", f.tree.join("link")).unwrap();
    let error = archive::run(&options(&f, "0.5.0")).unwrap_err().to_string();
    assert!(error.contains("Ссылка в сборке: link"), "{error}");
}

#[test]
fn path_that_does_not_fit_ustar_is_refused_not_extended() {
    let f = fixture();
    // 160 байт имени каталога и 120 имени файла: ни name (100), ни prefix (155) не вместят.
    let deep = format!("resources/{}/{}", "p".repeat(160), "q".repeat(120));
    write(&f.tree, &format!("{deep}/x"), b"x");
    let error = archive::run(&options(&f, "0.5.0")).unwrap_err().to_string();
    assert!(error.contains("USTAR"), "{error}");
    assert!(
        !f.out
            .join("HLSR-0.5.0-linux-x86_64.tar.gz.partial")
            .exists()
    );
}
