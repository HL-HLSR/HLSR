//! Установка версий лаунчера в управляемый корень (`launcher-update/ArchiveInstall.js` 0.4.7).
//!
//! Корень по умолчанию — `~/.local/opt/hlsr`: `versions/X.Y.Z-<16hex>/` (версия и начало SHA-256
//! архива), ссылки `current` и `previous` на них, скрипт `start`, маркер `.hlsr-install.json`
//! (`{"schema":1}\n`). Подготовка (`prepare`) под блокировкой `.update-lock` (`wx 0600`,
//! `{pid, identity: "<boot_id>:<starttime>"}`) сверяет архив с описанием, распаковывает его в
//! `versions/.staging-*` и проверяет состав (`verify_stage`). `commit` переименовывает каталог версии,
//! переключает `previous`/`current` через временную ссылку и `rename`, удаляет позапрошлые версии;
//! `rollback` возвращает прежнюю. Ссылки и `/proc` — только Unix/Linux; на прочих ОС подготовка работает
//! (тесты), а `commit` отвечает ошибкой.

use super::archive::{self, ExtractOptions};
use crate::cancel::CancelToken;
use crate::channel::{self, VerifyOptions, field};
use crate::chunks::{self, AllowUrl, AssembleOptions, Assembled, ChunkFetch, Manifest};
use crate::ports::{Clock, Logger, SystemClock};
use serde_json::Value;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Имя маркера установки.
pub const INSTALL_MARKER: &str = ".hlsr-install.json";
/// Описание сборки рядом с бинарником.
pub const RELEASE_FILE: &str = ".hlsr-release.json";
/// Файл блокировки.
pub const LOCK_FILE: &str = ".update-lock";
/// Брошенная блокировка без читаемого владельца старше этого — чужая (мс).
pub const STALE_LOCK_MS: f64 = 30_000.0;

/// Скрипт `start`: запускает текущую версию с `--no-sandbox` без переменных AppImage.
pub const START_SCRIPT: &str = "#!/bin/sh\nROOT=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd) || exit 1\nunset APPIMAGE APPDIR ARGV0 OWD\nexec \"$ROOT/current/hlsr\" --no-sandbox \"$@\"\n";

/// `~/.local/opt/hlsr`.
pub fn default_root(home: &Path) -> PathBuf {
    home.join(".local").join("opt").join("hlsr")
}

/// `^[0-9]+\.[0-9]+\.[0-9]+-[a-f0-9]{16}$` — имя каталога версии.
pub fn is_version_dir(name: &str) -> bool {
    let Some((version, hash)) = name.rsplit_once('-') else {
        return false;
    };
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && hash.len() == 16
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `readCurrent(root)`: цель ссылки `current` (`versions/X.Y.Z-<16hex>`), `None` — ссылки нет.
pub fn read_current(root: &Path) -> Result<Option<String>, String> {
    let link = root.join("current");
    match std::fs::read_link(&link) {
        Ok(target) => {
            let target = target.to_string_lossy().into_owned();
            match target.strip_prefix("versions/") {
                Some(name) if is_version_dir(name) => Ok(Some(target)),
                _ => Err("Некорректный указатель установки HLSR".to_owned()),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(super::fs_error(&error, "readlink", &link)),
    }
}

fn read_json(file: &Path) -> Option<Value> {
    let text = std::fs::read(file).ok()?;
    hlsr_core::json_text::json_parse(&String::from_utf8_lossy(&text)).ok()
}

/// `managedRoot(executable)`: корень установки, из которой запущен бинарник (по `realpath`):
/// `<корень>/versions/<версия>/hlsr`, маркер `{"schema":1}` и `.hlsr-release.json` Linux x64 рядом.
/// `None` — бинарник не из управляемой установки (обновления выключены).
pub fn managed_root(executable: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(executable).ok()?;
    let dir = real.parent()?;
    let versions = dir.parent().unwrap_or(dir);
    let root = versions.parent().unwrap_or(versions).to_owned();
    let marker = read_json(&root.join(INSTALL_MARKER))?;
    if field(&marker, "schema").and_then(Value::as_f64) != Some(1.0)
        || versions.file_name().and_then(|name| name.to_str()) != Some("versions")
    {
        return None;
    }
    let release = read_json(&dir.join(RELEASE_FILE))?;
    let is =
        |key: &str, expected: &str| field(&release, key).and_then(Value::as_str) == Some(expected);
    (is("platform", "linux")
        && is("arch", "x64")
        && real.file_name().and_then(|name| name.to_str()) == Some("hlsr"))
    .then_some(root)
}

/// `processIdentity(pid)`: `<boot_id>:<starttime>` из `/proc` (только Linux; иначе `None`).
pub fn process_identity(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after = stat.get(stat.rfind(')')? + 2..)?;
        let start = after.split_whitespace().nth(19)?;
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        Some(format!("{}:{start}", boot.trim()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// `fs.symlinkSync(target, link)`; без Unix ссылок нет — ошибка.
fn symlink(target: &str, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "ссылки установки HLSR есть только в Linux",
        ))
    }
}

/// `switchLink(root, name, target)`: временная ссылка `.<имя>-<16hex>` и `rename` поверх прежней.
fn switch_link(root: &Path, name: &str, target: &str) -> Result<(), String> {
    let temp = root.join(format!(".{name}-{}", super::random_hex(8)));
    symlink(target, &temp)
        .map_err(|error| super::fs_error2(&error, "symlink", Path::new(target), &temp))?;
    let destination = root.join(name);
    let renamed = std::fs::rename(&temp, &destination)
        .map_err(|error| super::fs_error2(&error, "rename", &temp, &destination));
    let _ = std::fs::remove_file(&temp);
    renamed
}

/// Inode файла (Unix); на прочих ОС сравнение не делается.
fn inode(metadata: &std::fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

/// Сборка версии из чанков (`prepareFromChunks` options).
pub struct ChunkStage {
    /// Кеш скачанных кусков (`<загрузка>/chunks`).
    pub cache_dir: PathBuf,
    pub chunk_base_urls: Vec<String>,
    pub fetch: Arc<dyn ChunkFetch>,
    pub is_allowed: AllowUrl,
    pub cancel: Option<CancelToken>,
    pub on_progress: Option<chunks::OnProgress>,
    pub log: Option<Arc<dyn Logger>>,
    pub clock: Arc<dyn Clock>,
}

impl ChunkStage {
    pub fn new(
        cache_dir: PathBuf,
        chunk_base_urls: Vec<String>,
        fetch: Arc<dyn ChunkFetch>,
    ) -> ChunkStage {
        ChunkStage {
            cache_dir,
            chunk_base_urls,
            fetch,
            is_allowed: chunks::production_allow(),
            cancel: None,
            on_progress: None,
            log: None,
            clock: Arc::new(SystemClock),
        }
    }
}

/// `ArchiveInstall`: одна установка (подготовка, переключение, откат) в корне.
#[derive(Debug)]
pub struct ArchiveInstall {
    root: PathBuf,
    stage: Option<PathBuf>,
    locked: bool,
    lock_file: Option<File>,
    original: Option<String>,
    relative: Option<String>,
    chunk_result: Option<Assembled>,
}

const ABORTED: &str = "Распаковка отменена";

fn aborted(cancel: Option<&CancelToken>) -> Result<(), String> {
    if cancel.is_some_and(CancelToken::is_cancelled) {
        return Err(ABORTED.to_owned());
    }
    Ok(())
}

impl ArchiveInstall {
    /// `new ArchiveInstall(root)`: корень через `path.resolve`.
    pub fn new(root: &Path) -> ArchiveInstall {
        ArchiveInstall {
            root: super::resolve(root),
            stage: None,
            locked: false,
            lock_file: None,
            original: None,
            relative: None,
            chunk_result: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Каталог подготовленной версии (`versions/.staging-*`).
    pub fn stage(&self) -> Option<&Path> {
        self.stage.as_deref()
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Итог сборки из чанков (`chunkResult`).
    pub fn chunk_result(&self) -> Option<&Assembled> {
        self.chunk_result.as_ref()
    }

    /// `lock()`: `.update-lock` создаётся только новым; брошенный (процесса нет или он другой, а у
    /// нечитаемого — старше 30 с) удаляется и создаётся заново.
    pub fn lock(&mut self) -> Result<(), String> {
        super::make_dirs(&self.root, 0o777)
            .map_err(|error| super::fs_error(&error, "mkdir", &self.root))?;
        let root_meta = std::fs::symlink_metadata(&self.root)
            .map_err(|error| super::fs_error(&error, "lstat", &self.root))?;
        if root_meta.file_type().is_symlink() {
            return Err("Каталог установки не должен быть ссылкой".to_owned());
        }
        let file = self.root.join(LOCK_FILE);
        let acquire = || -> io::Result<File> {
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(&file)
        };
        let handle = match acquire() {
            Ok(handle) => handle,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let stat = std::fs::symlink_metadata(&file)
                    .map_err(|error| super::fs_error(&error, "lstat", &file))?;
                let owner = std::fs::read(&file).ok().and_then(|bytes| {
                    hlsr_core::json_text::json_parse(&String::from_utf8_lossy(&bytes)).ok()
                });
                let stale = match owner {
                    Some(owner) if !owner.is_null() => {
                        let pid =
                            channel::is_integer(field(&owner, "pid")).filter(|pid| *pid > 0.0);
                        match (pid, field(&owner, "identity").and_then(Value::as_str)) {
                            (Some(pid), Some(identity)) => {
                                process_identity(pid as u32).as_deref() != Some(identity)
                            }
                            _ => false,
                        }
                    }
                    _ => {
                        let modified = hlsr_core::js::fs::mtime_ms(&stat).unwrap_or(0.0);
                        SystemClock.now_ms() as f64 - modified > STALE_LOCK_MS
                    }
                };
                let same = std::fs::symlink_metadata(&file)
                    .map(|now| inode(&now) == inode(&stat))
                    .unwrap_or(false);
                if !stale || !same {
                    return Err("Другая установка уже выполняется".to_owned());
                }
                std::fs::remove_file(&file)
                    .map_err(|error| super::fs_error(&error, "unlink", &file))?;
                acquire().map_err(|error| super::fs_error(&error, "open", &file))?
            }
            Err(error) => return Err(super::fs_error(&error, "open", &file)),
        };
        self.locked = true;
        let owner = serde_json::json!({
            "pid": std::process::id(),
            "identity": process_identity(std::process::id()),
        });
        use std::io::Write as _;
        let mut handle = handle;
        handle
            .write_all(owner.to_string().as_bytes())
            .map_err(|error| super::fs_error(&error, "write", &file))?;
        self.lock_file = Some(handle);
        Ok(())
    }

    /// Начало подготовки (`stageWith` до `fill`): блокировка, уборка `.staging-*`, каталог `.staging-*`.
    fn begin(&mut self, manifest: &Value, cancel: Option<&CancelToken>) -> Result<PathBuf, String> {
        self.discard();
        self.lock()?;
        let result = (|| {
            self.clean_orphan_staging()?;
            aborted(cancel)?;
            self.original = read_current(&self.root)?;
            let version = hlsr_core::js::to_string(&manifest["version"]);
            let sha = hlsr_core::js::to_string(&manifest["installer"]["sha256"]);
            let prefix: String = sha.chars().take(16).collect();
            self.relative = Some(format!("versions/{version}-{prefix}"));
            let versions = self.root.join("versions");
            super::make_dirs(&versions, 0o777)
                .map_err(|error| super::fs_error(&error, "mkdir", &versions))?;
            let prefix = versions.join(".staging-");
            let stage = super::mkdtemp(&prefix).map_err(|error| {
                super::fs_error(
                    &error,
                    "mkdtemp",
                    &PathBuf::from(format!("{}XXXXXX", prefix.display())),
                )
            })?;
            self.stage = Some(stage.clone());
            Ok(stage)
        })();
        if result.is_err() {
            self.discard();
        }
        result
    }

    /// Конец подготовки: отмена и `verifyStage`; при ошибке — `discard`.
    fn finish(
        &mut self,
        filled: Result<(), String>,
        manifest: &Value,
        cancel: Option<&CancelToken>,
    ) -> Result<(), String> {
        let result = filled
            .and_then(|()| aborted(cancel))
            .and_then(|()| self.verify_stage(manifest));
        if result.is_err() {
            self.discard();
        }
        result
    }

    /// `prepare(archive, manifest)`: проверить архив по описанию (размер, SHA-256, SHA-512) и распаковать
    /// в `versions/.staging-*` (предел и ожидаемый размер — `unpackedSize`).
    pub async fn prepare(
        &mut self,
        archive: &Path,
        manifest: &Value,
        cancel: Option<&CancelToken>,
    ) -> Result<(), String> {
        let stage = self.begin(manifest, cancel)?;
        let filled = async {
            let installer = &manifest["installer"];
            let verdict = channel::verify_installer_stream(
                archive,
                installer,
                &VerifyOptions {
                    max_bytes: None,
                    cancel: cancel.cloned(),
                },
            )
            .await;
            aborted(cancel)?;
            if !verdict.ok {
                return Err(verdict.problems.join("; "));
            }
            let unpacked = installer["unpackedSize"]
                .as_f64()
                .filter(|size| *size > 0.0)
                .map(|size| size as u64);
            let (archive, stage, cancel) = (archive.to_owned(), stage.clone(), cancel.cloned());
            tokio::task::spawn_blocking(move || {
                archive::extract(
                    &archive,
                    &stage,
                    ExtractOptions {
                        max_bytes: unpacked,
                        expected_bytes: unpacked,
                        cancel,
                        on_progress: None,
                    },
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())?
        }
        .await;
        self.finish(filled, manifest, cancel)
    }

    /// `prepareFromChunks(chunkManifest, channelManifest)`: новая версия из чанков — совпавшее с
    /// установленной версией берётся с диска, остальное скачивается. Каталог версии называется по
    /// установщику подписанного канала.
    pub async fn prepare_from_chunks(
        &mut self,
        chunk_manifest: Manifest,
        channel_manifest: &Value,
        options: ChunkStage,
    ) -> Result<(), String> {
        let cancel = options.cancel.clone();
        let stage = self.begin(channel_manifest, cancel.as_ref())?;
        let source = self
            .original
            .as_ref()
            .map(|original| self.root.join(original));
        let mut assemble = AssembleOptions::new(
            chunk_manifest,
            stage,
            options.cache_dir,
            options.chunk_base_urls,
            options.fetch,
            options.is_allowed,
        );
        assemble.source_root = source;
        assemble.cancel = options.cancel;
        assemble.on_progress = options.on_progress;
        assemble.log = options.log;
        assemble.clock = options.clock;
        let filled = match chunks::assemble(assemble).await {
            Ok(result) => {
                self.chunk_result = Some(result);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        };
        self.finish(filled, channel_manifest, cancel.as_ref())
    }

    /// `verifyStage(manifest)`: `.hlsr-release.json` совпадает с каналом (версия, Linux x64, `hlsr`),
    /// есть `hlsr`, `resources/app.asar`, `resources/icons/hlsr.png`, `hlsr` — ELF64 LE x86-64; `chmod 0755`.
    pub fn verify_stage(&self, manifest: &Value) -> Result<(), String> {
        let stage = self.stage.as_ref().ok_or("Нет подготовленной версии")?;
        let release_file = stage.join(RELEASE_FILE);
        let bytes = std::fs::read(&release_file)
            .map_err(|error| super::fs_error(&error, "open", &release_file))?;
        let release = hlsr_core::json_text::json_parse(&String::from_utf8_lossy(&bytes))
            .map_err(|error| error.to_string())?;
        if release.is_null() {
            return Err("Cannot read properties of null (reading 'version')".to_owned());
        }
        let text = |key: &str| field(&release, key).and_then(Value::as_str);
        if field(&release, "version") != Some(&manifest["version"])
            || text("platform") != Some("linux")
            || text("arch") != Some("x64")
            || text("executable") != Some("hlsr")
        {
            return Err("Версия или платформа архива не совпала с подписанным каналом".to_owned());
        }
        for name in ["hlsr", "resources/app.asar", "resources/icons/hlsr.png"] {
            let file = name
                .split('/')
                .fold(stage.clone(), |path, part| path.join(part));
            let metadata =
                std::fs::metadata(&file).map_err(|error| super::fs_error(&error, "stat", &file))?;
            if !metadata.is_file() {
                return Err(format!("В архиве отсутствует {name}"));
            }
        }
        let binary = stage.join("hlsr");
        let mut header = [0u8; 20];
        let mut handle =
            File::open(&binary).map_err(|error| super::fs_error(&error, "open", &binary))?;
        let mut filled = 0;
        while filled < header.len() {
            match handle.read(&mut header[filled..]) {
                Ok(0) => break,
                Ok(count) => filled += count,
                Err(error) => return Err(super::fs_error(&error, "read", &binary)),
            }
        }
        if header[..4] != [0x7f, 0x45, 0x4c, 0x46]
            || header[4] != 2
            || header[5] != 1
            || u16::from_le_bytes([header[18], header[19]]) != 62
        {
            return Err("В архиве нет Linux x64 бинарника".to_owned());
        }
        super::chmod(&binary, 0o755).map_err(|error| super::fs_error(&error, "chmod", &binary))
    }

    /// `commit()`: подготовленная версия становится текущей. Возвращает путь к её `hlsr`.
    pub fn commit(&mut self) -> Result<PathBuf, String> {
        let (Some(stage), true) = (self.stage.clone(), self.locked) else {
            return Err("Нет подготовленной версии".to_owned());
        };
        if read_current(&self.root)? != self.original {
            return Err("Установка HLSR изменилась во время обновления".to_owned());
        }
        let relative = self.relative.clone().unwrap_or_default();
        let final_dir = self.root.join(&relative);
        if final_dir.exists() {
            return Err("Эта версия уже есть в каталоге versions".to_owned());
        }
        let start_temp = self.root.join(format!(".start-{}", std::process::id()));
        super::write_file(&start_temp, START_SCRIPT.as_bytes(), 0o755, false)
            .map_err(|error| super::fs_error(&error, "open", &start_temp))?;
        let start = self.root.join("start");
        std::fs::rename(&start_temp, &start)
            .map_err(|error| super::fs_error2(&error, "rename", &start_temp, &start))?;
        let marker = self.root.join(INSTALL_MARKER);
        super::write_file(&marker, b"{\"schema\":1}\n", 0o666, false)
            .map_err(|error| super::fs_error(&error, "open", &marker))?;
        std::fs::rename(&stage, &final_dir)
            .map_err(|error| super::fs_error2(&error, "rename", &stage, &final_dir))?;
        self.stage = None;
        let switched = match &self.original {
            Some(original) => switch_link(&self.root, "previous", original),
            None => Ok(()),
        }
        .and_then(|()| switch_link(&self.root, "current", &relative));
        if let Err(error) = switched {
            let _ = super::remove_all(&final_dir);
            return Err(error);
        }
        self.prune_versions(&[Some(relative.as_str()), self.original.as_deref()]);
        self.unlock();
        Ok(final_dir.join("hlsr"))
    }

    /// `rollback()`: `current` снова на прежнюю версию (или убрать, если её не было), новая — удалить.
    pub fn rollback(&mut self) -> Result<(), String> {
        match &self.original {
            Some(original) => switch_link(&self.root, "current", original)?,
            None => {
                let current = self.root.join("current");
                std::fs::remove_file(&current)
                    .map_err(|error| super::fs_error(&error, "unlink", &current))?;
            }
        }
        if let Some(relative) = &self.relative {
            let _ = super::remove_all(&self.root.join(relative));
        }
        Ok(())
    }

    /// `unlock()`. Ошибка удаления `.update-lock` наружу не идёт (у 0.4.7 `unlinkSync` бросал, и
    /// `commit` после переключения ссылок сообщал бы об ошибке при готовой установке), но блокировка
    /// остаётся «взятой»: следующий `discard()`/`unlock()` повторит удаление. Иначе брошенный файл
    /// со своим pid и идентичностью (не «устаревший») блокировал бы каждую подготовку до перезапуска.
    pub fn unlock(&mut self) {
        if !self.locked {
            return;
        }
        self.lock_file = None;
        match std::fs::remove_file(self.root.join(LOCK_FILE)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return,
        }
        self.locked = false;
    }

    /// `pruneVersions(keep)`: на диске остаются новая и прежняя версии, позапрошлые удаляются;
    /// ошибка удаления не мешает обновлению.
    pub fn prune_versions(&self, keep: &[Option<&str>]) {
        let kept: Vec<String> = keep
            .iter()
            .flatten()
            .filter_map(|relative| Path::new(relative).file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        let Ok(names) = hlsr_core::js::fs::read_dir_names(&self.root.join("versions")) else {
            return;
        };
        for name in names {
            if name.starts_with('.') || kept.contains(&name) || !is_version_dir(&name) {
                continue;
            }
            let _ = super::remove_all(&self.root.join("versions").join(&name));
        }
    }

    /// `cleanOrphanStaging()`: брошенные `versions/.staging-*` упавших установок (блокировка уже наша).
    pub fn clean_orphan_staging(&self) -> Result<(), String> {
        let dir = self.root.join("versions");
        let names = match hlsr_core::js::fs::read_dir_names(&dir) {
            Ok(names) => names,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(super::fs_error(&error, "scandir", &dir)),
        };
        for name in names {
            if !name.starts_with(".staging-") {
                continue;
            }
            let target = dir.join(&name);
            if self.stage.as_deref() == Some(target.as_path()) {
                continue;
            }
            if let Err(error) = super::remove_all(&target)
                && error.kind() != io::ErrorKind::NotFound
            {
                return Err(super::fs_error(&error, "rm", &target));
            }
        }
        Ok(())
    }

    /// `discard()`: удалить подготовленную версию и снять блокировку.
    pub fn discard(&mut self) {
        if let Some(stage) = self.stage.take() {
            let _ = super::remove_all(&stage);
        }
        self.unlock();
    }
}
