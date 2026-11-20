//! Формы файлов выпуска против 0.4.7, байт в байт: фикстура `release-forms.json` из
//! `tools/reference/update/release-forms.js` — настоящие `writeUpdateInfo` (`build-windows-setup.js`),
//! `latestLinuxYml`, `embeddedPayload.describe`, `buildArchive` (`build-launcher-archive.js`, GNU tar),
//! `fileList.build` эталона на тех же входах, что получает `hlsr-release`.

mod support;

use hlsr_core::json_formats::pretty2;
use hlsr_release::archive::{self, ArchiveOptions};
use hlsr_release::latest_yml::{self, Latest};
use hlsr_release::setup::{self, SetupOptions};
use hlsr_update::linux::embedded_payload::{self, Describe};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static FORMS: LazyLock<Value> = LazyLock::new(|| {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("release-forms.json");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    hlsr_core::json_text::parse_js_json(&text).unwrap()
});

fn text(value: &Value) -> &str {
    value.as_str().expect("строка")
}

fn list(value: &Value) -> &Vec<Value> {
    value.as_array().expect("массив")
}

/// Дерево из описания фикстуры: путь и байты в hex.
fn write_tree(root: &Path, spec: &Value) {
    for file in list(spec) {
        let path = text(&file["path"])
            .split('/')
            .fold(root.to_owned(), |path, part| path.join(part));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let hex = text(&file["hex"]);
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
            .collect();
        fs::write(path, bytes).unwrap();
    }
}

#[test]
fn latest_yml_texts_match_the_real_writers_of_0_4_7() {
    let cases = list(&FORMS["latestYml"]);
    assert!(cases.len() >= 3);
    for case in cases {
        let render = |filename: &str| {
            latest_yml::render(&Latest {
                version: text(&case["version"]),
                filename,
                sha512: text(&case["sha512"]),
                size: case["size"].as_u64().unwrap(),
                release_date: text(&case["releaseDate"]),
            })
        };
        let name = text(&case["name"]);
        // electron-builder: build-windows-setup.js, writeUpdateInfo.
        assert_eq!(
            render(text(&case["windows"]["filename"])),
            text(&case["windows"]["text"]),
            "latest.yml / {name}"
        );
        assert_eq!(
            latest_yml::windows_installer_name(text(&case["version"])),
            text(&case["windows"]["filename"])
        );
        // launcher-release-prepare-linux.js, latestLinuxYml.
        assert_eq!(
            render(text(&case["linux"]["filename"])),
            text(&case["linux"]["text"]),
            "latest-linux.yml / {name}"
        );
        assert_eq!(
            latest_yml::linux_installer_name(text(&case["version"])),
            text(&case["linux"]["filename"])
        );
    }
}

#[test]
fn payload_json_text_matches_describe_and_the_bootstrap_writer() {
    for case in list(&FORMS["payloadJson"]) {
        let input = &case["input"];
        let descriptor = embedded_payload::describe(&Describe {
            version: text(&input["version"]),
            size: input["size"].as_u64().unwrap(),
            sha256: text(&input["sha256"]),
            sha512: text(&input["sha512"]),
            unpacked_size: input["unpackedSize"].as_u64().unwrap(),
        });
        // build-bootstrap-app.js:41 — JSON.stringify(descriptor, null, 2) + "\n".
        assert_eq!(pretty2(&descriptor) + "\n", text(&case["text"]));
        assert!(embedded_payload::problems_of(&descriptor).is_empty());
    }
}

/// Записи архива так же, как их разбирал бы любой tar: `(имя без «./», тип, размер, uid, gid)`.
fn archive_entries(archive: &Path) -> Vec<(String, String, u64, u64, u64)> {
    use flate2::read::GzDecoder;
    use std::io::Read as _;
    let mut raw = Vec::new();
    GzDecoder::new(fs::File::open(archive).unwrap())
        .read_to_end(&mut raw)
        .unwrap();
    tar::Archive::new(&raw[..])
        .entries()
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let header = entry.header();
            assert_eq!(&header.as_bytes()[257..263], b"ustar\0", "USTAR");
            let name = String::from_utf8(entry.path_bytes().to_vec()).unwrap();
            let name = match name.strip_prefix("./") {
                Some("") | None => name,
                Some(rest) => rest.to_owned(),
            };
            (
                name,
                (header.entry_type().as_byte() as char).to_string(),
                header.size().unwrap(),
                header.uid().unwrap(),
                header.gid().unwrap(),
            )
        })
        .collect()
}

#[test]
fn archive_matches_build_launcher_archive_of_0_4_7() {
    let archive_form = &FORMS["archive"];
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("linux-unpacked");
    write_tree(&tree, &archive_form["tree"]);
    let out = dir.path().join("out");
    let version = text(&archive_form["version"]);
    let output = archive::run(&ArchiveOptions {
        tree: tree.clone(),
        version: version.to_owned(),
        out,
        executables: Vec::new(),
        mtime: Some(0),
    })
    .unwrap();

    // build-launcher-archive.js: маркер, `<архив>.json`, сумма размеров (маркер в неё входит).
    assert_eq!(
        fs::read_to_string(tree.join(".hlsr-release.json")).unwrap(),
        text(&archive_form["releaseMarker"])
    );
    assert_eq!(
        fs::read_to_string(&output.info).unwrap(),
        text(&archive_form["info"])
    );
    assert_eq!(
        output.unpacked_size,
        archive_form["unpackedSize"].as_u64().unwrap()
    );

    // GNU tar `--format=ustar --sort=name --owner=0 --group=0 --numeric-owner`: тот же порядок,
    // типы, размеры и владелец 0/0 (режимы зависят от ОС снятия и не сверяются).
    let theirs: Vec<(String, String, u64, u64, u64)> = list(&archive_form["entries"])
        .iter()
        .map(|entry| {
            (
                text(&entry["name"]).to_owned(),
                text(&entry["type"]).to_owned(),
                entry["size"].as_u64().unwrap(),
                entry["uid"].as_u64().unwrap(),
                entry["gid"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(archive_entries(&output.archive), theirs);
}

fn tree_text(root: &Path, relative: &str) -> String {
    let path = relative
        .split('/')
        .fold(root.to_owned(), |path, part| path.join(part));
    fs::read_to_string(path).unwrap()
}

#[test]
fn install_files_json_matches_file_list_of_0_4_7() {
    let forms = &FORMS["installFiles"];
    for case in list(&forms["cases"]) {
        let dir = tempfile::tempdir().unwrap();
        let source: PathBuf = dir.path().join("source");
        write_tree(&source, &forms["tree"]);
        let out = dir.path().join("dist");
        let qa = text(&case["name"]) == "qa";
        let output = setup::run(&SetupOptions {
            exe: source.join("HLSR.exe"),
            resources: Some(source.join("resources")),
            version: text(&case["version"]).to_owned(),
            qa,
            out,
            release_date: Some("2026-10-16T09:00:00.000Z".to_owned()),
            allow_unversioned_exe: true,
        })
        .unwrap();
        assert_eq!(
            output.app.app_id,
            text(&case["app"]["appId"]),
            "{}",
            text(&case["name"])
        );
        // build-windows-setup.js, finalizeUnpacked: JSON.stringify(list) + "\n".
        assert_eq!(
            tree_text(&output.unpacked, "resources/install-files.json"),
            text(&case["text"]),
            "{}",
            text(&case["name"])
        );
    }
}
