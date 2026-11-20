//! Тренировочные сейвы HL1 (`practice-saves.js`): «load e1» … «load e62». Недостающие докачиваются
//! в `SAVE` игры после установки и при запуске; сейвы игрока с теми же именами не перезаписываются.
//!
//! Архив (7z, до 64 МБ) скачивается с источников манифеста по очереди, проверяется по размеру и
//! SHA-256, распаковывается в staging, нужные сейвы копируются без замены существующих. Кроме `SAVE`
//! мода сейвы кладутся в `SAVE` языковых папок (`valve_WON_russian`): игра с языком Steam, отличным
//! от английского, пишет сейвы туда и ищет там раньше, чем в папке мода. Поэтому сейв набора в языковой
//! папке перекрыл бы одноимённый свой сейв игрока в папке мода — такие имена туда не кладутся.

use crate::archive_service::{ArchiveService, ExtractOptions};
use crate::download_rate_limiter::DownloadRateLimiter;
use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::file_ops::hex;
use crate::http_client::{HttpClient, RequestOptions};
use crate::manifests;
use crate::node_fs::mkdir_recursive;
use crate::paths::{LibraryPaths, assert_inside, assert_inside_with, remove_within_zone};
use crate::ports::{Logger, TracingLogger};
use crate::runs_scan;
use hlsr_core::js::fs::{read_dir, read_dir_names};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{Platform, errno};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::OnceCell;

/// Архив больше этого размера не качается.
pub const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
/// Простой соединения при скачивании архива.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_millis(5 * 60 * 1000);

/// Сейв набора: имя, размер и SHA-256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackFile {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// Источник архива.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub id: String,
    pub url: String,
}

/// Архив с сейвами: имя файла, размер, SHA-256 и источники по порядку.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackArchive {
    pub filename: String,
    pub size: u64,
    pub sha256: String,
    pub providers: Vec<Provider>,
}

/// Набор сейвов (`config/saves/*.manifest.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub pack_id: String,
    pub game_ids: Vec<String>,
    /// Папка `SAVE` мода от папки установки, через `/`.
    pub target: String,
    pub files: Vec<PackFile>,
    pub archive: PackArchive,
}

fn text_of(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn number_of(value: &Value, name: &str) -> u64 {
    value.get(name).and_then(Value::as_u64).unwrap_or(0)
}

impl Pack {
    /// Набор из документа манифеста; чего нет — пусто или 0 (проверка размера и хеша при
    /// скачивании такой архив отвергнет).
    pub fn from_value(value: &Value) -> Pack {
        let list = |parent: &Value, name: &str| -> Vec<Value> {
            parent
                .get(name)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        let archive = value.get("archive").cloned().unwrap_or(Value::Null);
        Pack {
            pack_id: text_of(value, "packId"),
            game_ids: list(value, "gameIds")
                .iter()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect(),
            target: text_of(value, "target"),
            files: list(value, "files")
                .iter()
                .map(|file| PackFile {
                    name: text_of(file, "name"),
                    size: number_of(file, "size"),
                    sha256: text_of(file, "sha256"),
                })
                .collect(),
            archive: PackArchive {
                filename: text_of(&archive, "filename"),
                size: number_of(&archive, "size"),
                sha256: text_of(&archive, "sha256"),
                providers: list(&archive, "providers")
                    .iter()
                    .map(|provider| Provider {
                        id: text_of(provider, "id"),
                        url: text_of(provider, "url"),
                    })
                    .collect(),
            },
        }
    }
}

/// Встроенный набор сейвов HL1 (`config/saves/hl1-practice-saves.manifest.json`).
pub fn hl1_pack() -> &'static Pack {
    static PACK: OnceLock<Pack> = OnceLock::new();
    PACK.get_or_init(|| Pack::from_value(manifests::hl1_practice_saves_manifest().value()))
}

/// `SAVE_NAME`: `/^[A-Za-z0-9_-]+\.sav$/`.
pub fn is_save_name(name: &str) -> bool {
    name.strip_suffix(".sav").is_some_and(|stem| {
        !stem.is_empty()
            && stem
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

/// Чем кончился [`PracticeSaves::ensure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Для игры нет набора или путь установки пуст.
    NotApplicable,
    /// Мода нет (игра не установлена).
    NotInstalled,
    /// Все сейвы на месте.
    Present,
    /// Недостающие сейвы добавлены.
    Installed,
}

impl Status {
    /// `status`: `not-applicable`, `not-installed`, `present` или `installed`.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::NotApplicable => "not-applicable",
            Status::NotInstalled => "not-installed",
            Status::Present => "present",
            Status::Installed => "installed",
        }
    }
}

/// `{ status, added }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: Status,
    pub added: usize,
}

impl Outcome {
    fn of(status: Status) -> Outcome {
        Outcome { status, added: 0 }
    }

    /// Объект результата, как его видит вызывающий.
    pub fn to_value(&self) -> Value {
        json!({"status": self.status.as_str(), "added": self.added})
    }
}

/// Какой сейв куда положить: `source` — готовый файл в папке мода (копия в языковую папку на месте),
/// без него файл берётся из распакованного архива.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCopy {
    pub name: String,
    pub dir: PathBuf,
    pub source: Option<PathBuf>,
}

type Job = Arc<OnceCell<Result<Outcome, Failure>>>;

/// Докачка тренировочных сейвов.
pub struct PracticeSaves {
    paths: LibraryPaths,
    archive: Option<ArchiveService>,
    packs: Vec<Pack>,
    logger: Arc<dyn Logger>,
    http: HttpClient,
    rate_limiter: Option<Arc<DownloadRateLimiter>>,
    jobs: Mutex<HashMap<String, Job>>,
}

impl PracticeSaves {
    /// Сейвы HL1 по умолчанию; архив скачивается в `Downloads` библиотеки, распаковывается в staging.
    pub fn new(paths: &LibraryPaths) -> PracticeSaves {
        PracticeSaves {
            paths: paths.clone(),
            archive: None,
            packs: vec![hl1_pack().clone()],
            logger: Arc::new(TracingLogger),
            http: HttpClient::new(),
            rate_limiter: None,
            jobs: Mutex::new(HashMap::new()),
        }
    }

    /// Распаковщик 7z; без него докачать нечем (`EXTRACT_FAILED`).
    pub fn with_archive(mut self, archive: ArchiveService) -> PracticeSaves {
        self.archive = Some(archive);
        self
    }

    /// Другие наборы вместо сейвов HL1.
    pub fn with_packs(mut self, packs: Vec<Pack>) -> PracticeSaves {
        self.packs = packs;
        self
    }

    pub fn with_logger(mut self, logger: Arc<dyn Logger>) -> PracticeSaves {
        self.logger = logger;
        self
    }

    pub fn with_http_client(mut self, http: HttpClient) -> PracticeSaves {
        self.http = http;
        self
    }

    /// Общий ограничитель скорости загрузок.
    pub fn with_rate_limiter(mut self, limiter: Arc<DownloadRateLimiter>) -> PracticeSaves {
        self.rate_limiter = Some(limiter);
        self
    }

    /// `packFor(gameId)`.
    pub fn pack_for(&self, game_id: &str) -> Option<&Pack> {
        self.packs
            .iter()
            .find(|pack| pack.game_ids.iter().any(|id| id == game_id))
    }

    /// Сколько загрузок идёт сейчас (`jobs.size`).
    pub fn pending(&self) -> usize {
        self.jobs.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Одна загрузка на игру: повторный вызов, пока сейвы качаются, ждёт её же и получает тот же итог.
    pub async fn ensure(&self, game_id: &str, install_path: &Path) -> Result<Outcome, Failure> {
        let Some(pack) = self.pack_for(game_id) else {
            return Ok(Outcome::of(Status::NotApplicable));
        };
        if install_path.as_os_str().is_empty() {
            return Ok(Outcome::of(Status::NotApplicable));
        }
        let key = format!(
            "{}\n{}",
            pack.pack_id,
            NodePath::native().resolve(&[&install_path.to_string_lossy()])
        );
        let job: Job = self
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(key.clone())
            .or_default()
            .clone();
        job.get_or_init(|| async {
            let result = self.run(pack, install_path).await;
            // `.finally`: запись убирается, как только работа кончилась.
            let mut jobs = self.jobs.lock().unwrap_or_else(|p| p.into_inner());
            if jobs
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, &job))
            {
                jobs.remove(&key);
            }
            result
        })
        .await
        .clone()
    }

    async fn run(&self, pack: &Pack, install_path: &Path) -> Result<Outcome, Failure> {
        let Some(plan) = plan_copies(install_path, pack)? else {
            return Ok(Outcome::of(Status::NotInstalled));
        };
        if plan.is_empty() {
            return Ok(Outcome::of(Status::Present));
        }

        self.logger.info(&format!(
            "[saves] не хватает тренировочных сейвов: {} в {}",
            plan.len(),
            dirs_of(install_path, &plan)
        ));

        // Сейвы, которые уже лежат в папке мода, копируются в языковые папки на месте; архив
        // нужен, только если какого-то файла нет нигде.
        if plan.iter().all(|copy| copy.source.is_some()) {
            return self.copy(&plan, install_path, None);
        }

        let archive_path = self.paths.downloads.join(&pack.archive.filename);
        let staging = self.paths.staging.join(format!("saves-{}", pack.pack_id));
        let result = self
            .fetch_and_copy(pack, install_path, &archive_path, &staging)
            .await;
        let _ = fs::remove_file(&archive_path);
        let _ = remove_within_zone(&self.paths.root, &staging);
        result
    }

    async fn fetch_and_copy(
        &self,
        pack: &Pack,
        install_path: &Path,
        archive_path: &Path,
        staging: &Path,
    ) -> Result<Outcome, Failure> {
        mkdir_recursive(&self.paths.downloads)?;
        self.download(&pack.archive, archive_path).await?;

        remove_within_zone(&self.paths.root, staging)?;
        mkdir_recursive(staging)?;

        let Some(archive) = &self.archive else {
            return Err(DeliveryError::new(Code::ExtractFailed, "Распаковщик не подключён").into());
        };
        archive
            .extract(archive_path, staging, ExtractOptions::default())
            .await?;

        // Пока шла загрузка, игру могли удалить, а игрок — сохраниться: раскладку считаем
        // заново, папку мода заново не создаём.
        let Some(fresh) = plan_copies(install_path, pack)? else {
            return Ok(Outcome::of(Status::NotInstalled));
        };
        self.copy(&fresh, install_path, Some(staging))
    }

    fn copy(
        &self,
        plan: &[PlannedCopy],
        install_path: &Path,
        staging: Option<&Path>,
    ) -> Result<Outcome, Failure> {
        let mut added = 0;
        for copy in plan {
            let source = match (&copy.source, staging) {
                (Some(source), _) => Some(source.clone()),
                (None, Some(staging)) => {
                    let source = staging.join(&copy.name);
                    assert_inside(staging, &source, None)?;
                    Some(source)
                }
                // Без готового файла и без распаковки брать нечего.
                (None, None) => None,
            };
            if !source.as_ref().is_some_and(|source| source.exists()) {
                return Err(DeliveryError::new(
                    Code::ExtractFailed,
                    format!("В архиве сейвов нет {}", copy.name),
                )
                .into());
            }
            let Some(source) = source else { continue };
            mkdir_recursive(&copy.dir)?;
            match copy_file_exclusive(&source, &copy.dir.join(&copy.name)) {
                Ok(()) => added += 1,
                Err(error) if error.code.as_deref() == Some("EEXIST") => {}
                Err(error) => return Err(error.into()),
            }
        }
        self.logger.info(&format!(
            "[saves] добавлено тренировочных сейвов: {added} → {}",
            dirs_of(install_path, plan)
        ));
        Ok(Outcome {
            status: Status::Installed,
            added,
        })
    }

    async fn download(&self, descriptor: &PackArchive, target: &Path) -> Result<(), Failure> {
        let mut last_error: Option<Failure> = None;
        for provider in &descriptor.providers {
            match self.download_from(&provider.url, descriptor, target).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    self.logger.warn(&format!(
                        "[saves] источник {} не сработал: {error}",
                        provider.id
                    ));
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            DeliveryError::new(
                Code::ManifestInvalid,
                "У архива сейвов нет ни одного источника",
            )
            .into()
        }))
    }

    async fn download_from(
        &self,
        url: &str,
        descriptor: &PackArchive,
        target: &Path,
    ) -> Result<(), Failure> {
        if descriptor.size == 0
            || descriptor.size > MAX_ARCHIVE_BYTES
            || !is_sha256_hex(&descriptor.sha256)
        {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                "У архива сейвов неверный размер или SHA-256",
            )
            .into());
        }

        let mut part = target.as_os_str().to_owned();
        part.push(".part");
        let part = PathBuf::from(part);

        match self.fetch(url, descriptor, &part, target).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = fs::remove_file(&part);
                // Своя ошибка идёт дальше как есть, чужая (сеть, диск) — это NETWORK_ERROR.
                Err(match error {
                    Failure::Raw(raw) => DeliveryError::new(Code::NetworkError, raw.message).into(),
                    delivery => delivery,
                })
            }
        }
    }

    async fn fetch(
        &self,
        url: &str,
        descriptor: &PackArchive,
        part: &Path,
        target: &Path,
    ) -> Result<(), Failure> {
        let options = RequestOptions {
            timeout: Some(DOWNLOAD_TIMEOUT),
            ..RequestOptions::default()
        };
        let mut response = self.http.request(url, &options).await?;
        if response.status() != 200 {
            let status = response.status();
            drop(response);
            return Err(DeliveryError::new(
                Code::NetworkError,
                format!("Источник ответил {status}"),
            )
            .into());
        }

        let mut file = fs::File::create(part).map_err(|e| RawError::fs(&e, "open", part))?;
        let mut hash = Sha256::new();
        let mut received: u64 = 0;
        while let Some(chunk) = response.chunk().await? {
            if let Some(limiter) = &self.rate_limiter {
                limiter.acquire(chunk.len()).await;
            }
            received += chunk.len() as u64;
            if received > descriptor.size {
                return Err(DeliveryError::new(
                    Code::SizeMismatch,
                    "Архив сейвов длиннее объявленного",
                )
                .into());
            }
            hash.update(&chunk);
            file.write_all(&chunk).map_err(|e| write_error(&e))?;
        }
        drop(file);

        if received != descriptor.size {
            return Err(DeliveryError::new(
                Code::SizeMismatch,
                format!(
                    "Размер архива сейвов не совпал: {received} вместо {}",
                    descriptor.size
                ),
            )
            .into());
        }
        if hex(&hash.finalize()) != descriptor.sha256 {
            return Err(DeliveryError::new(
                Code::ChecksumMismatch,
                "SHA-256 архива сейвов не совпал с объявленным",
            )
            .into());
        }

        fs::rename(part, target)
            .map_err(|e| RawError::fs_with_dest(&e, "rename", part, Some(target)))?;
        Ok(())
    }
}

/// `/^[0-9a-f]{64}$/`.
fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Ошибка записи потока Node без пути: `ENOSPC: no space left on device, write`.
fn write_error(error: &io::Error) -> RawError {
    match errno::code(error) {
        Some(code) => RawError {
            message: format!("{code}: {}, write", errno::description(&code)),
            code: Some(code.into_owned()),
            syscall: Some("write".to_owned()),
            ..RawError::default()
        },
        None => RawError::new(error.to_string()),
    }
}

/// `fs.copyFileSync(from, to, COPYFILE_EXCL)`: файл создаётся только новый (`EEXIST`, если есть),
/// права берутся с источника, при сбое недописанный файл удаляется.
fn copy_file_exclusive(from: &Path, to: &Path) -> Result<(), RawError> {
    let failed = |error: &io::Error| RawError::fs_with_dest(error, "copyfile", from, Some(to));
    let mut source = fs::File::open(from).map_err(|e| failed(&e))?;
    let permissions = source.metadata().map_err(|e| failed(&e))?.permissions();
    let mut dest = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|e| failed(&e))?;
    let copied = io::copy(&mut source, &mut dest).and_then(|_| dest.set_permissions(permissions));
    drop(dest);
    if let Err(error) = copied {
        let _ = fs::remove_file(to);
        return Err(failed(&error));
    }
    Ok(())
}

/// Папки плана от папки установки, без повторов, через запятую.
fn dirs_of(install_path: &Path, plan: &[PlannedCopy]) -> String {
    let path = NodePath::native();
    let from = path.resolve(&[&install_path.to_string_lossy()]);
    let mut dirs: Vec<String> = Vec::new();
    for copy in plan {
        let to = path.resolve(&[&copy.dir.to_string_lossy()]);
        let relative = to
            .strip_prefix(&from)
            .map_or(to.as_str(), |rest| rest.trim_start_matches(path.sep()))
            .to_owned();
        if !dirs.contains(&relative) {
            dirs.push(relative);
        }
    }
    dirs.join(", ")
}

/// Сейвы папки в нижнем регистре → имя на диске: игра под Windows и Proton регистра не различает.
fn present_saves(save_dir: &Path) -> Result<HashMap<String, String>, RawError> {
    match read_dir_names(save_dir) {
        Ok(names) => Ok(names
            .into_iter()
            .map(|name| (name.to_lowercase(), name))
            .collect()),
        Err(error) if errno::code(&error).as_deref() == Some("ENOENT") => Ok(HashMap::new()),
        Err(error) => Err(RawError::fs(&error, "scandir", save_dir)),
    }
}

fn is_pack_file(file_path: &Path, file: &PackFile) -> bool {
    let Ok(metadata) = fs::metadata(file_path) else {
        return false;
    };
    if metadata.len() != file.size {
        return false;
    }
    fs::read(file_path).is_ok_and(|bytes| hex(&Sha256::digest(&bytes)) == file.sha256)
}

/// Какие сейвы куда положить; `None`, если самого мода нет (игра не установлена).
pub fn plan_copies(install_path: &Path, pack: &Pack) -> Result<Option<Vec<PlannedCopy>>, Failure> {
    let Some(mod_save) = resolve_save_dir(install_path, &pack.target)? else {
        return Ok(None);
    };

    let target = pack.target.replace('\\', "/");
    let posix = NodePath::new(Platform::Linux, "");
    let base = posix.basename(&target);
    let mut language_dirs = Vec::new();
    for dir in runs_scan::language_dirs(install_path, &posix.dirname(&target)) {
        if let Some(save_dir) = resolve_save_dir(install_path, &format!("{dir}/{base}"))? {
            language_dirs.push(save_dir);
        }
    }
    let mut language_saves = Vec::new();
    for dir in language_dirs {
        let present = present_saves(&dir)?;
        language_saves.push((dir, present));
    }

    let in_mod = present_saves(&mod_save)?;
    let mut copies = Vec::new();
    for file in &pack.files {
        if !is_save_name(&file.name) {
            continue;
        }
        let key = file.name.to_lowercase();
        let mod_file = in_mod.get(&key).map(|name| mod_save.join(name));
        if mod_file.is_none() {
            copies.push(PlannedCopy {
                name: file.name.clone(),
                dir: mod_save.clone(),
                source: None,
            });
        }

        let missing: Vec<&PathBuf> = language_saves
            .iter()
            .filter(|(_, present)| !present.contains_key(&key))
            .map(|(dir, _)| dir)
            .collect();
        if missing.is_empty() {
            continue;
        }
        if mod_file
            .as_ref()
            .is_some_and(|path| !is_pack_file(path, file))
        {
            continue;
        }
        for dir in missing {
            copies.push(PlannedCopy {
                name: file.name.clone(),
                dir: dir.clone(),
                source: mod_file.clone(),
            });
        }
    }
    Ok(Some(copies))
}

/// Папка `SAVE` внутри мода; `None`, если самого мода нет (игра не установлена). В Linux регистр
/// имён важен, а Wine мог создать «save»: берётся существующая.
pub fn resolve_save_dir(
    install_path: &Path,
    target: &str,
) -> Result<Option<PathBuf>, DeliveryError> {
    let path = NodePath::native();
    let install = install_path.to_string_lossy();
    let wanted = assert_inside_with(
        &path,
        &install,
        &path.join(&[&install, target]),
        Some("Папка сейвов"),
    )?;
    let parent = path.dirname(&wanted);
    let name = path.basename(&wanted);

    let Ok(entries) = read_dir(Path::new(&parent)) else {
        return Ok(None);
    };
    if entries
        .iter()
        .any(|entry| entry.file_type.is_dir() && entry.name == name)
    {
        return Ok(Some(PathBuf::from(wanted)));
    }
    let lower = name.to_lowercase();
    let other = entries
        .iter()
        .find(|entry| entry.file_type.is_dir() && entry.name.to_lowercase() == lower);
    Ok(Some(match other {
        Some(entry) => PathBuf::from(path.join(&[&parent, &entry.name])),
        None => PathBuf::from(wanted),
    }))
}
