//! Командная строка `hlsr-release`. Разбор аргументов без внешних крейтов: `--имя значение`,
//! `--флаг`, неизвестное — отказ с подсказкой.

use crate::appimage::{self, AppImageOptions, SystemRunner};
use crate::archive::{self, ArchiveOptions};
use crate::error::{ReleaseError, Result, fail};
use crate::latest_yml;
use crate::pe_version;
use crate::qa_identity;
use crate::setup::{self, SetupOptions};
use crate::stage::{self, Platform};
use crate::version;
use hlsr_core::json_formats::compact;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub const USAGE: &str = "\
hlsr-release — инструменты выпуска (подпроект 6)

  hlsr-release setup --exe <HLSR.exe> --version <X.Y.Z> --out <каталог> [--resources <каталог>]
                     [--qa] [--release-date <ISO>] [--allow-unversioned-exe]
      HLSR-Setup-<v>.exe с нагрузкой, latest.yml, win-unpacked/ (install-files.json внутри)
  hlsr-release archive --tree <каталог> --version <X.Y.Z> --out <каталог>
                       [--exec <путь>]... [--mtime <секунды>]
      строгий USTAR .tar.gz, <архив>.json, payload.json (Linux); дописывает в дерево
      .hlsr-release.json и заглушку resources/app.asar
  hlsr-release appimage --payload <каталог> --exe <hlsr> --runtime <файл> --runtime-sha256 <hex>
                        --out <.../HLSR-Setup-<v>-linux-x86_64.AppImage> [--icon <png>]
                        [--mksquashfs <команда>] [--release-date <ISO>]
      AppImage type2 + latest-linux.yml рядом (только Linux: mksquashfs 4.4+ с zstd)
  hlsr-release latest-linux-yml --image <AppImage> --version <X.Y.Z> --out <файл> [--release-date <ISO>]
  hlsr-release stage --platform <windows|linux> --repo <корень> --out <каталог resources>
  hlsr-release qa-identity [--production] [--version <X.Y.Z>]
  hlsr-release version [--manifest <Cargo.toml>] [--release]
  hlsr-release exe-version <файл.exe>
";

struct Args {
    values: HashMap<&'static str, Vec<String>>,
    flags: HashSet<&'static str>,
    rest: Vec<String>,
}

impl Args {
    fn parse(
        args: &[String],
        value_options: &[&'static str],
        flag_options: &[&'static str],
    ) -> Result<Args> {
        let mut out = Args {
            values: HashMap::new(),
            flags: HashSet::new(),
            rest: Vec::new(),
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if let Some(name) = value_options.iter().find(|name| **name == arg) {
                let Some(value) = iter.next() else {
                    return fail(format!("У {arg} нет значения"));
                };
                out.values.entry(name).or_default().push(value.clone());
            } else if let Some(name) = flag_options.iter().find(|name| **name == arg) {
                out.flags.insert(name);
            } else if arg.starts_with("--") {
                return fail(format!("Неизвестный аргумент: {arg}"));
            } else {
                out.rest.push(arg.clone());
            }
        }
        Ok(out)
    }

    fn one(&self, name: &str) -> Option<String> {
        self.values.get(name).and_then(|list| list.last()).cloned()
    }

    fn required(&self, name: &str) -> Result<String> {
        self.one(name)
            .ok_or_else(|| ReleaseError::Message(format!("Не задан {name}")))
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        self.required(name).map(PathBuf::from)
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

fn say(line: &str) {
    println!("{line}");
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
}

fn run_setup(args: &[String]) -> Result<()> {
    let args = Args::parse(
        args,
        &[
            "--exe",
            "--version",
            "--out",
            "--resources",
            "--release-date",
        ],
        &["--qa", "--allow-unversioned-exe"],
    )?;
    let output = setup::run(&SetupOptions {
        exe: args.path("--exe")?,
        resources: args.one("--resources").map(PathBuf::from),
        version: args.required("--version")?,
        qa: args.flag("--qa"),
        out: args.path("--out")?,
        release_date: args.one("--release-date"),
        allow_unversioned_exe: args.flag("--allow-unversioned-exe"),
    })?;
    // Строка о списке — как у build-windows-setup.js (`finalizeUnpacked`).
    say(&format!(
        "install-files.json: {} файлов, {}, {}",
        output.files,
        mib(output.bytes),
        output.app.app_id
    ));
    say(&format!(
        "{}: {}, sha256 {}",
        output.setup.display(),
        mib(output.installer.size),
        output.installer.sha256
    ));
    say(&format!(
        "{}: sha256 {}",
        output.latest_yml.display(),
        output.latest_yml_sha256
    ));
    Ok(())
}

fn run_archive(args: &[String]) -> Result<()> {
    let args = Args::parse(
        args,
        &["--tree", "--version", "--out", "--exec", "--mtime"],
        &[],
    )?;
    let mtime =
        match args.one("--mtime") {
            Some(text) => Some(text.parse::<u64>().map_err(|_| {
                ReleaseError::Message(format!("--mtime — секунды, получено {text}"))
            })?),
            None => None,
        };
    let output = archive::run(&ArchiveOptions {
        tree: args.path("--tree")?,
        version: args.required("--version")?,
        out: args.path("--out")?,
        executables: args.values.get("--exec").cloned().unwrap_or_default(),
        mtime,
    })?;
    say(&format!(
        "{}: {} записей, {} распакованных, sha256 {}",
        output.archive.display(),
        output.entries,
        mib(output.unpacked_size),
        output.digests.sha256
    ));
    say(&format!("{}", output.descriptor.display()));
    Ok(())
}

fn run_appimage(args: &[String]) -> Result<()> {
    let args = Args::parse(
        args,
        &[
            "--payload",
            "--exe",
            "--runtime",
            "--runtime-sha256",
            "--out",
            "--icon",
            "--mksquashfs",
            "--release-date",
        ],
        &[],
    )?;
    let output = appimage::run(
        &AppImageOptions {
            payload: args.path("--payload")?,
            exe: args.path("--exe")?,
            icon: args.one("--icon").map(PathBuf::from),
            runtime: args.path("--runtime")?,
            runtime_sha256: args.required("--runtime-sha256")?,
            out: args.path("--out")?,
            mksquashfs: args
                .one("--mksquashfs")
                .unwrap_or_else(|| "mksquashfs".to_owned()),
            release_date: args.one("--release-date"),
        },
        &SystemRunner,
    )?;
    say(&format!(
        "{}: {}, sha256 {}",
        output.image.display(),
        mib(output.digests.size),
        output.digests.sha256
    ));
    say(&format!(
        "{}: sha256 {}",
        output.latest_yml.display(),
        output.latest_yml_sha256
    ));
    Ok(())
}

fn run_latest_linux_yml(args: &[String]) -> Result<()> {
    let args = Args::parse(
        args,
        &["--image", "--version", "--out", "--release-date"],
        &[],
    )?;
    let version = args.required("--version")?;
    version::require_release(&version)?;
    let (digests, sha256) = latest_yml::write_for_image(
        &args.path("--image")?,
        &version,
        args.one("--release-date").as_deref(),
        &args.path("--out")?,
    )?;
    say(&format!(
        "{}: sha512 {}…, latest-linux.yml sha256 {sha256}",
        mib(digests.size),
        &digests.sha512[..16]
    ));
    Ok(())
}

fn run_stage(args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["--platform", "--repo", "--out"], &[])?;
    let platform = args.required("--platform")?;
    let Some(platform) = Platform::parse(&platform) else {
        return fail(format!(
            "--platform — windows или linux, получено {platform}"
        ));
    };
    let staged = stage::run(platform, &args.path("--repo")?, &args.path("--out")?)?;
    say(&format!("ресурсы: {} записей", staged.copied.len()));
    for skipped in &staged.skipped {
        eprintln!("нет необязательного файла: {skipped}");
    }
    Ok(())
}

fn run_qa_identity(args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["--version"], &["--production"])?;
    let version = args.one("--version");
    if let Some(version) = &version {
        version::require_release(version)?;
    }
    let identity = qa_identity::identity_json(!args.flag("--production"), version.as_deref())?;
    say(&compact(&identity));
    Ok(())
}

fn run_version(args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["--manifest"], &["--release"])?;
    let manifest = args
        .one("--manifest")
        .unwrap_or_else(|| "Cargo.toml".to_owned());
    let text = std::fs::read_to_string(&manifest)
        .map_err(|error| ReleaseError::Message(format!("{manifest}: {error}")))?;
    let Some(found) = version::workspace_version(&text) else {
        return fail(format!("В {manifest} нет version в [workspace.package]"));
    };
    if args.flag("--release") {
        version::require_release(&found)?;
    }
    say(&found);
    Ok(())
}

fn run_exe_version(args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [file] = args.rest.as_slice() else {
        return fail("exe-version: нужен один файл");
    };
    match pe_version::product_version(std::path::Path::new(file))? {
        Some(version) => {
            say(&version);
            Ok(())
        }
        None => fail(format!("В {file} нет ProductVersion")),
    }
}

/// Выполнить команду; `Err` — текст для пользователя.
pub fn run(argv: &[String]) -> Result<()> {
    let Some((command, rest)) = argv.split_first() else {
        say(USAGE);
        return fail("Не задана команда");
    };
    match command.as_str() {
        "setup" => run_setup(rest),
        "archive" => run_archive(rest),
        "appimage" => run_appimage(rest),
        "latest-linux-yml" => run_latest_linux_yml(rest),
        "stage" => run_stage(rest),
        "qa-identity" => run_qa_identity(rest),
        "version" => run_version(rest),
        "exe-version" => run_exe_version(rest),
        "--help" | "-h" | "help" => {
            say(USAGE);
            Ok(())
        }
        other => {
            say(USAGE);
            fail(format!("Неизвестная команда: {other}"))
        }
    }
}
