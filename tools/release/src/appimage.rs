//! AppImage-установщик Linux (решение Р11): `HLSR-Setup-<версия>-linux-x86_64.AppImage` =
//! рантайм type2 (закреплённый sha256) + squashfs со сжатием zstd. В корне squashfs лежит
//! `resources/payload/` (`payload.json` и архив лаунчера): обновлялка 0.4.7 достаёт его рантаймом
//! самого скачанного AppImage — `--appimage-extract resources/payload` без FUSE
//! (`launcher-update/embedded-payload.js:81-93`). Рядом лежат `hlsr` (он же режим `Bootstrap`
//! установщика: определяется по `APPIMAGE` и `resources/payload` рядом с exe), `AppRun`,
//! `hlsr.desktop` и значок.
//!
//! Собирается только на Linux: нужен `mksquashfs` из squashfs-tools 4.4+ (zstd). Внешние команды
//! идут через [`Runner`], поэтому раскладка и сборка файла проверяются тестами на любой ОС.

use crate::archive::is_linux_x64_elf;
use crate::error::{At, ReleaseError, Result, fail};
use crate::hashes::{self, Digests};
use crate::latest_yml;
use crate::version;
use hlsr_update::linux::embedded_payload;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Запуск внешних команд.
pub trait Runner {
    fn run(&self, program: &str, args: &[String]) -> Result<()>;
}

/// Настоящий запуск процессов.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<()> {
        let status = Command::new(program).args(args).status().map_err(|error| {
            ReleaseError::Message(if error.kind() == io::ErrorKind::NotFound {
                format!("Нет {program}: нужен squashfs-tools 4.4 или новее (zstd)")
            } else {
                format!("{program}: {error}")
            })
        })?;
        if status.success() {
            Ok(())
        } else {
            fail(format!("{program} завершился с {status}"))
        }
    }
}

/// Что собрать.
#[derive(Debug, Clone)]
pub struct AppImageOptions {
    /// Каталог с `payload.json` и архивом (итог `archive`).
    pub payload: PathBuf,
    /// Бинарник лаунчера (Linux x86-64): он же исполняет режим `Bootstrap`.
    pub exe: PathBuf,
    /// Значок PNG для меню и `.DirIcon`.
    pub icon: Option<PathBuf>,
    /// Рантайм type2.
    pub runtime: PathBuf,
    /// Закреплённый sha256 рантайма; пустой или другой — отказ.
    pub runtime_sha256: String,
    /// Итоговый файл: `…/HLSR-Setup-<версия>-linux-x86_64.AppImage`.
    pub out: PathBuf,
    pub mksquashfs: String,
    /// `releaseDate` в `latest-linux.yml`.
    pub release_date: Option<String>,
}

/// Что получилось.
#[derive(Debug, Clone)]
pub struct AppImageOutput {
    pub image: PathBuf,
    pub latest_yml: PathBuf,
    pub digests: Digests,
    pub latest_yml_sha256: String,
}

pub const APP_RUN: &str =
    "#!/bin/sh\nHERE=$(dirname \"$(readlink -f \"$0\")\")\nexec \"$HERE/hlsr\" \"$@\"\n";
pub const DESKTOP: &str = "[Desktop Entry]\nType=Application\nName=HLSR\nExec=hlsr\nIcon=hlsr\nCategories=Game;\nTerminal=false\n";

/// Рантайм type2: ELF с меткой `AI\x02` в `e_ident`.
pub fn is_type2_runtime(head: &[u8]) -> bool {
    head.len() >= 11 && head[..4] == [0x7f, b'E', b'L', b'F'] && head[8..11] == [b'A', b'I', 2]
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).at(path)
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<()> {
    Ok(())
}

fn head(path: &Path, len: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .at(path)?
        .take(len)
        .read_to_end(&mut bytes)
        .at(path)?;
    Ok(bytes)
}

/// Корень squashfs.
fn stage(
    options: &AppImageOptions,
    payload: &embedded_payload::Payload,
    into: &Path,
) -> Result<()> {
    let put = |relative: &str, bytes: &[u8], executable: bool| -> Result<()> {
        let path = relative
            .split('/')
            .fold(into.to_owned(), |path, part| path.join(part));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).at(parent)?;
        }
        fs::write(&path, bytes).at(&path)?;
        if executable {
            make_executable(&path)?;
        }
        Ok(())
    };
    put("AppRun", APP_RUN.as_bytes(), true)?;
    put("hlsr.desktop", DESKTOP.as_bytes(), false)?;
    let exe = fs::read(&options.exe).at(&options.exe)?;
    put("hlsr", &exe, true)?;
    let descriptor = options.payload.join(embedded_payload::DESCRIPTOR);
    let target = format!("resources/{}", embedded_payload::DIRECTORY);
    put(
        &format!("{target}/{}", embedded_payload::DESCRIPTOR),
        &fs::read(&descriptor).at(&descriptor)?,
        false,
    )?;
    let name = payload
        .archive
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    put(
        &format!("{target}/{name}"),
        &fs::read(&payload.archive).at(&payload.archive)?,
        false,
    )?;
    if let Some(icon) = &options.icon {
        let bytes = fs::read(icon).at(icon)?;
        put("hlsr.png", &bytes, false)?;
        put(".DirIcon", &bytes, false)?;
        put("resources/icons/hlsr.png", &bytes, false)?;
    }
    Ok(())
}

/// Параметры `mksquashfs`: корень принадлежит root, без xattr, zstd, время 0 (повторяемые байты).
pub fn mksquashfs_args(source: &Path, image: &Path) -> Vec<String> {
    let mut args = vec![
        source.to_string_lossy().into_owned(),
        image.to_string_lossy().into_owned(),
    ];
    args.extend(
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
            "-quiet",
        ]
        .map(str::to_owned),
    );
    args
}

/// Собрать AppImage и `latest-linux.yml` рядом с ним.
pub fn run(options: &AppImageOptions, runner: &dyn Runner) -> Result<AppImageOutput> {
    let payload = embedded_payload::read(&options.payload).map_err(ReleaseError::Message)?;
    let version = payload.version();
    version::require_release(&version)?;
    // Архив должен быть тем, что описано: размер, sha256, sha512.
    let installer = &payload.manifest["installer"];
    let archive = hashes::digests(&payload.archive)?;
    let described = |key: &str| installer[key].as_str().unwrap_or_default().to_owned();
    if installer["size"].as_u64() != Some(archive.size)
        || described("sha256") != archive.sha256
        || described("sha512") != archive.sha512
    {
        return fail(
            "Архив в payload не совпадает с payload.json (размер или хеши): соберите заново",
        );
    }

    let expected = options.runtime_sha256.trim().to_ascii_lowercase();
    if expected.len() != 64 {
        return fail("Не задан закреплённый sha256 рантайма AppImage (64 hex-символа)");
    }
    let runtime = hashes::digests(&options.runtime)?;
    if runtime.sha256 != expected {
        return fail(format!(
            "Рантайм AppImage {}: sha256 {} вместо закреплённого {expected}",
            options.runtime.display(),
            runtime.sha256
        ));
    }
    if !is_type2_runtime(&head(&options.runtime, 16)?) {
        return fail("Рантайм AppImage — не type2 (нет метки AI\\x02)");
    }
    if !is_linux_x64_elf(&head(&options.exe, 20)?) {
        return fail("hlsr — не ELF x86-64 для Linux");
    }
    let expected_name = latest_yml::linux_installer_name(&version);
    if options.out.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return fail(format!(
            "AppImage должен называться {expected_name}, а не {}",
            options.out.display()
        ));
    }

    let dir = options
        .out
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_owned();
    fs::create_dir_all(&dir).at(&dir)?;
    let work = dir.join(format!(".{expected_name}.work"));
    let _ = fs::remove_dir_all(&work);
    let root = work.join("root");
    fs::create_dir_all(&root).at(&root)?;
    let squashfs = work.join("payload.squashfs");
    let partial = work.join("image.partial");
    let built = (|| -> Result<()> {
        stage(options, &payload, &root)?;
        runner.run(&options.mksquashfs, &mksquashfs_args(&root, &squashfs))?;
        // Рантайм, сразу за ним squashfs: так type2 находит файловую систему.
        let mut out = BufWriter::new(File::create(&partial).at(&partial)?);
        io::copy(
            &mut File::open(&options.runtime).at(&options.runtime)?,
            &mut out,
        )
        .at(&partial)?;
        io::copy(&mut File::open(&squashfs).at(&squashfs)?, &mut out).at(&partial)?;
        out.flush().at(&partial)?;
        drop(out);
        make_executable(&partial)?;
        let _ = fs::remove_file(&options.out);
        fs::rename(&partial, &options.out).at(&options.out)
    })();
    let _ = fs::remove_dir_all(&work);
    built?;

    let yml = dir.join("latest-linux.yml");
    let (digests, latest_yml_sha256) = latest_yml::write_for_image(
        &options.out,
        &version,
        options.release_date.as_deref(),
        &yml,
    )?;
    Ok(AppImageOutput {
        image: options.out.clone(),
        latest_yml: yml,
        digests,
        latest_yml_sha256,
    })
}
