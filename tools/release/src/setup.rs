//! Установщик Windows (решение Р1): `HLSR-Setup-<версия>.exe` — сам `HLSR.exe`, за которым
//! лежит нагрузка (`install-files.json` и файлы сборки) и хвост (`hlsr_update::windows::payload`).
//! Вместо `build-windows-setup.js` + electron-builder (`--dir`, portable-exe, blockmap):
//!
//! 1. сборка раскладывается в `<out>/win-unpacked` (`HLSR.exe`, `resources/…`) — туда же 0.4.7
//!    складывал `win-unpacked`, и `launcher-release-prepare.js` режет на куски именно её;
//! 2. `resources/install-files.json` — `JSON.stringify(list) + "\n"` (`windows-install/file-list.js`),
//!    с `appId` и именем продукта сборки (боевые или QA, решение 18);
//! 3. установщик = exe + нагрузка + хвост; после записи он открывается тем же кодом, что читает его
//!    при установке, и хеш нагрузки сверяется;
//! 4. `latest.yml` — текст `build-windows-setup.js:92-113`; sha256 этого файла идёт в канал
//!    (`latestYmlSha256`). blockmap не публикуется (Р4): 0.4.7 качает установщик целиком.
//!
//! Версия — только `X.Y.Z` (Р9) и должна совпасть с `ProductVersion` самого `HLSR.exe`: именно её
//! сверяет апдейтер 0.4.7 после установки (`verifyInstalled`).

use crate::error::{At, Result, fail};
use crate::hashes::{self, Digests};
use crate::latest_yml::{self, Latest};
use crate::pe_version;
use crate::tree;
use crate::version;
use hlsr_update::channel::MAX_INSTALLER_BYTES;
use hlsr_update::windows::file_list::{self, FileList};
use hlsr_update::windows::layout::{self, App};
use hlsr_update::windows::payload::{Payload, Trailer};
use sha2::{Digest as _, Sha256};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Что собирать.
#[derive(Debug, Clone)]
pub struct SetupOptions {
    /// Собранный `HLSR.exe`.
    pub exe: PathBuf,
    /// Каталог, содержимое которого станет `resources/` (7zip, оверлей, лицензии…).
    pub resources: Option<PathBuf>,
    pub version: String,
    /// QA-сборка со своим `appId`, именем и GUID (решение 18).
    pub qa: bool,
    pub out: PathBuf,
    /// `releaseDate` в `latest.yml` (по умолчанию — сейчас).
    pub release_date: Option<String>,
    /// Пропустить требование ресурса версии у `HLSR.exe` (пробные exe без VERSIONINFO).
    pub allow_unversioned_exe: bool,
}

/// Что получилось.
#[derive(Debug, Clone)]
pub struct SetupOutput {
    pub setup: PathBuf,
    pub latest_yml: PathBuf,
    pub unpacked: PathBuf,
    pub installer: Digests,
    pub latest_yml_sha256: String,
    pub app: App,
    pub files: usize,
    /// Сумма размеров файлов списка.
    pub bytes: u64,
}

struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
    written: u64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(data)?;
        self.hasher.update(&data[..count]);
        self.written += count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn list_error(error: file_list::FileListError) -> crate::error::ReleaseError {
    crate::error::ReleaseError::Message(error.to_string())
}

/// Версия `HLSR.exe` против версии выпуска.
fn check_exe_version(exe: &Path, version: &str, allow_unversioned: bool) -> Result<()> {
    match pe_version::product_version(exe)? {
        Some(found) => match pe_version::version_triple(&found) {
            Some(triple) if triple == version => Ok(()),
            _ => fail(format!(
                "ProductVersion {found} у {} не совпадает с версией выпуска {version}: апдейтер 0.4.7 \
                 отвергнет установку (verifyInstalled)",
                exe.display()
            )),
        },
        None if allow_unversioned => Ok(()),
        None => fail(format!(
            "У {} нет ресурса версии (ProductVersion): сборка без VERSIONINFO не пройдёт проверку \
             апдейтера 0.4.7",
            exe.display()
        )),
    }
}

/// Раскладка `win-unpacked`: старая сборка стирается, только если это прежняя раскладка.
fn stage(options: &SetupOptions, unpacked: &Path) -> Result<()> {
    if !tree::is_empty_or_missing(unpacked)? {
        if !unpacked.join(file_list::RELATIVE).is_file() {
            return fail(format!(
                "{} не пуст и не похож на прежнюю раскладку (нет {}): стирать не буду",
                unpacked.display(),
                file_list::RELATIVE
            ));
        }
        fs::remove_dir_all(unpacked).at(unpacked)?;
    }
    fs::create_dir_all(unpacked).at(unpacked)?;
    let exe = unpacked.join(layout::EXE);
    fs::copy(&options.exe, &exe).at(&options.exe)?;
    let resources = unpacked.join("resources");
    fs::create_dir_all(&resources).at(&resources)?;
    if let Some(source) = &options.resources {
        tree::copy_tree(source, &resources)?;
    }
    if resources.join(file_list::FILE).exists() {
        return fail(format!(
            "В ресурсах уже есть {}: список файлов пишет упаковщик",
            file_list::FILE
        ));
    }
    Ok(())
}

/// Записать установщик: exe, нагрузка (длина списка, список, файлы в порядке списка кроме
/// `HLSR.exe`), хвост.
fn write_setup(unpacked: &Path, list: &FileList, setup: &Path) -> Result<()> {
    let partial = setup.with_extension("exe.partial");
    let exe_path = unpacked.join(layout::EXE);
    let list_path = unpacked.join("resources").join(file_list::FILE);
    let list_bytes = fs::read(&list_path).at(&list_path)?;
    let result = (|| -> Result<()> {
        let mut out = BufWriter::new(File::create(&partial).at(&partial)?);
        let mut exe = File::open(&exe_path).at(&exe_path)?;
        let exe_len = io::copy(&mut exe, &mut out).at(&partial)?;
        let mut payload = HashingWriter {
            inner: &mut out,
            hasher: Sha256::new(),
            written: 0,
        };
        payload
            .write_all(&(list_bytes.len() as u64).to_le_bytes())
            .at(&partial)?;
        payload.write_all(&list_bytes).at(&partial)?;
        for item in list.files.iter().filter(|item| item.path != layout::EXE) {
            let path = item
                .path
                .split('/')
                .fold(unpacked.to_owned(), |path, part| path.join(part));
            let mut file = File::open(&path).at(&path)?;
            let copied = io::copy(&mut file, &mut payload).at(&path)?;
            if copied != item.size {
                return fail(format!(
                    "{} изменился во время упаковки ({copied} байт вместо {})",
                    item.path, item.size
                ));
            }
        }
        let trailer = Trailer {
            offset: exe_len,
            length: payload.written,
            sha256: payload.hasher.finalize().into(),
        };
        out.write_all(&trailer.to_bytes()).at(&partial)?;
        out.flush().at(&partial)?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    let _ = fs::remove_file(setup);
    fs::rename(&partial, setup).at(setup)
}

/// Собрать установщик и `latest.yml`.
pub fn run(options: &SetupOptions) -> Result<SetupOutput> {
    version::require_release(&options.version)?;
    if !options.exe.is_file() {
        return fail(format!("Нет HLSR.exe: {}", options.exe.display()));
    }
    check_exe_version(
        &options.exe,
        &options.version,
        options.allow_unversioned_exe,
    )?;

    let app = if options.qa {
        App::qa()
    } else {
        App::production()
    };
    fs::create_dir_all(&options.out).at(&options.out)?;
    let unpacked = options.out.join("win-unpacked");
    stage(options, &unpacked)?;

    let list = file_list::build(&unpacked, &options.version, &app).map_err(list_error)?;
    let problems = file_list::problems_of(&list.to_value());
    if !problems.is_empty() {
        return fail(format!(
            "Список файлов сборки некорректен: {}",
            problems.join("; ")
        ));
    }
    file_list::write_new(&unpacked, &list).at(&unpacked.join(file_list::RELATIVE))?;

    let name = latest_yml::windows_installer_name(&options.version);
    let setup = options.out.join(&name);
    write_setup(&unpacked, &list, &setup)?;
    // Тот же код, что читает установщик при запуске: хвост, хеш нагрузки, раскладка файлов.
    let opened = Payload::open(&setup).map_err(|error| {
        crate::error::ReleaseError::Message(format!(
            "Упакованный установщик не открывается: {error}"
        ))
    })?;
    if opened.list().files.len() != list.files.len() {
        return fail("Список файлов в нагрузке не совпал со списком сборки");
    }

    let installer = hashes::digests(&setup)?;
    if installer.size > MAX_INSTALLER_BYTES {
        return fail(format!(
            "Установщик {} байт больше предела {MAX_INSTALLER_BYTES}: канал его не примет",
            installer.size
        ));
    }
    let release_date = options
        .release_date
        .clone()
        .unwrap_or_else(|| hlsr_core::js::iso_string(SystemTime::now()));
    let text = latest_yml::render(&Latest {
        version: &options.version,
        filename: &name,
        sha512: &installer.sha512,
        size: installer.size,
        release_date: &release_date,
    });
    let latest_yml_path = options.out.join("latest.yml");
    fs::write(&latest_yml_path, &text).at(&latest_yml_path)?;

    Ok(SetupOutput {
        setup,
        latest_yml: latest_yml_path,
        unpacked,
        installer,
        latest_yml_sha256: hashes::sha256_bytes(text.as_bytes()),
        app,
        files: list.files.len(),
        bytes: list.total_bytes(),
    })
}
