//! `hlsr-release appimage`: раскладка корня squashfs, команда `mksquashfs`, сборка файла
//! «рантайм + squashfs», `latest-linux.yml`. Настоящий `mksquashfs` и рантайм type2 здесь не
//! запускаются (Linux-машина, строки «Linux — отложено»): команды идут через подмену [`Runner`].

mod support;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use hlsr_release::appimage::{self, AppImageOptions, Runner, mksquashfs_args};
use hlsr_release::archive::{self, ArchiveOptions};
use hlsr_release::error::Result;
use hlsr_release::testing::{elf_stub, png_stub, runtime_stub};
use hlsr_release::tree;
use sha2::{Digest, Sha512};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use support::{linux_tree, sha256, write};

const DATE: &str = "2026-10-16T09:00:00.000Z";

/// Записывает команды и состояние корня squashfs в момент вызова, а вместо `mksquashfs` создаёт файл.
#[derive(Default)]
struct Recorder {
    calls: RefCell<Vec<(String, Vec<String>)>>,
    root: RefCell<Vec<(String, u64, bool)>>,
    fail: bool,
}

impl Runner for Recorder {
    fn run(&self, program: &str, args: &[String]) -> Result<()> {
        self.calls
            .borrow_mut()
            .push((program.to_owned(), args.to_vec()));
        let root = PathBuf::from(&args[0]);
        *self.root.borrow_mut() = tree::walk_sorted(&root)
            .unwrap()
            .into_iter()
            .map(|entry| (entry.rel, entry.size, entry.dir))
            .collect();
        if self.fail {
            return hlsr_release::error::fail("mksquashfs: ошибка");
        }
        fs::write(&args[1], b"fake squashfs bytes").unwrap();
        Ok(())
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    payload: PathBuf,
    exe: PathBuf,
    runtime: PathBuf,
    runtime_sha256: String,
    out: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    linux_tree(&tree);
    let payload = dir.path().join("payload");
    archive::run(&ArchiveOptions {
        tree,
        version: "0.5.0".to_owned(),
        out: payload.clone(),
        executables: Vec::new(),
        mtime: Some(0),
    })
    .unwrap();
    let exe = write(dir.path(), "dist/hlsr", &elf_stub("bootstrap"));
    let runtime_bytes = runtime_stub();
    let runtime = write(dir.path(), "runtime-x86_64", &runtime_bytes);
    let out = dir
        .path()
        .join("out/HLSR-Setup-0.5.0-linux-x86_64.AppImage");
    Fixture {
        payload,
        exe,
        runtime,
        runtime_sha256: sha256(&runtime_bytes),
        out,
        dir,
    }
}

fn options(f: &Fixture) -> AppImageOptions {
    AppImageOptions {
        payload: f.payload.clone(),
        exe: f.exe.clone(),
        icon: Some(write(f.dir.path(), "icon.png", &png_stub())),
        runtime: f.runtime.clone(),
        runtime_sha256: f.runtime_sha256.clone(),
        out: f.out.clone(),
        mksquashfs: "mksquashfs".to_owned(),
        release_date: Some(DATE.to_owned()),
    }
}

#[test]
fn squashfs_root_carries_resources_payload_and_the_launcher() {
    let f = fixture();
    let recorder = Recorder::default();
    appimage::run(&options(&f), &recorder).unwrap();
    let root: Vec<String> = recorder
        .root
        .borrow()
        .iter()
        .filter(|(_, _, dir)| !dir)
        .map(|(path, ..)| path.clone())
        .collect();
    // `--appimage-extract resources/payload` достаёт payload.json и архив; рядом — запуск.
    assert_eq!(
        root,
        [
            ".DirIcon",
            "AppRun",
            "hlsr",
            "hlsr.desktop",
            "hlsr.png",
            "resources/icons/hlsr.png",
            "resources/payload/HLSR-0.5.0-linux-x86_64.tar.gz",
            "resources/payload/payload.json",
        ]
    );
    let calls = recorder.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "mksquashfs");
    let args = &calls[0].1;
    assert!(args.windows(2).any(|pair| pair == ["-comp", "zstd"]));
    assert!(args.iter().any(|arg| arg == "-noappend"));
    assert!(args.iter().any(|arg| arg == "-root-owned"));
}

#[test]
fn mksquashfs_arguments_are_reproducible_zstd() {
    let args = mksquashfs_args(Path::new("/root"), Path::new("/out.squashfs"));
    assert_eq!(args[0], "/root");
    assert_eq!(args[1], "/out.squashfs");
    assert_eq!(
        args[2..],
        [
            "-noappend",
            "-root-owned",
            "-no-xattrs",
            "-comp",
            "zstd",
            "-Xcompression-level",
            "19",
            "-all-time",
            "0",
            "-mkfs-time",
            "0",
            "-quiet"
        ]
    );
}

#[test]
fn image_is_runtime_followed_by_squashfs_and_latest_yml_describes_it() {
    let f = fixture();
    let output = appimage::run(&options(&f), &Recorder::default()).unwrap();
    let image = fs::read(&f.out).unwrap();
    let runtime = fs::read(&f.runtime).unwrap();
    assert_eq!(&image[..runtime.len()], &runtime[..]);
    assert_eq!(&image[runtime.len()..], b"fake squashfs bytes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&f.out).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    let sha512 = BASE64_STANDARD.encode(Sha512::digest(&image));
    let expected = format!(
        "version: 0.5.0\nfiles:\n  - url: HLSR-Setup-0.5.0-linux-x86_64.AppImage\n    sha512: {sha512}\n    size: {}\n\
         path: HLSR-Setup-0.5.0-linux-x86_64.AppImage\nsha512: {sha512}\nreleaseDate: '{DATE}'\n",
        image.len()
    );
    let yml = fs::read_to_string(f.out.parent().unwrap().join("latest-linux.yml")).unwrap();
    assert_eq!(yml, expected);
    assert_eq!(output.latest_yml_sha256, sha256(expected.as_bytes()));
    assert_eq!(output.digests.sha256, sha256(&image));
    // Рабочий каталог убран.
    let names: Vec<String> = fs::read_dir(f.out.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[test]
fn runtime_must_be_the_pinned_one() {
    let f = fixture();
    let mut o = options(&f);
    o.runtime_sha256 = "0".repeat(64);
    let error = appimage::run(&o, &Recorder::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("вместо закреплённого"), "{error}");
    o.runtime_sha256 = String::new();
    let error = appimage::run(&o, &Recorder::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("закреплённый sha256"), "{error}");
    assert!(!f.out.exists());
    // Рантайм верный по хешу, но не type2.
    let mut plain = runtime_stub();
    plain[8..11].fill(0);
    let path = write(f.dir.path(), "runtime-type1", &plain);
    let mut o = options(&f);
    o.runtime = path;
    o.runtime_sha256 = sha256(&plain);
    let error = appimage::run(&o, &Recorder::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("не type2"), "{error}");
}

#[test]
fn payload_must_match_its_descriptor() {
    let f = fixture();
    let archive = f.payload.join("HLSR-0.5.0-linux-x86_64.tar.gz");
    let mut bytes = fs::read(&archive).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&archive, bytes).unwrap();
    let error = appimage::run(&options(&f), &Recorder::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("не совпадает с payload.json"), "{error}");
}

#[test]
fn name_exe_and_failures_are_checked() {
    let f = fixture();
    let mut o = options(&f);
    o.out = f.dir.path().join("out/HLSR.AppImage");
    assert!(
        appimage::run(&o, &Recorder::default())
            .unwrap_err()
            .to_string()
            .contains("должен называться")
    );
    let mut o = options(&f);
    o.exe = write(f.dir.path(), "dist/not-elf", b"text");
    assert!(
        appimage::run(&o, &Recorder::default())
            .unwrap_err()
            .to_string()
            .contains("не ELF")
    );
    // Сбой mksquashfs: образа нет, рабочий каталог убран.
    let failing = Recorder {
        fail: true,
        ..Recorder::default()
    };
    assert!(appimage::run(&options(&f), &failing).is_err());
    assert!(!f.out.exists());
    let leftovers: Vec<_> = fs::read_dir(f.out.parent().unwrap())
        .map(|entries| entries.map(|entry| entry.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
