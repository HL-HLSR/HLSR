//! Готовые HUD и прицелы для HL1 из ресурсов раздела на speedrun.com (`goldsrc-packs.js`).
//! Лицензии у паков нет, поэтому лаунчер их не раздаёт: скачивает с Google Drive автора и сверяет
//! с закреплённым SHA-256. Если автор заменит файл, пак не ставится, пока его не проверят и не
//! обновят хеш здесь.
//!
//! Перед каждым запуском папка спрайтов мода сначала возвращается в исходное состояние
//! (оригиналы лежат в `.hlsr/packs/backup`), а затем в неё кладутся выбранные паки. Так смена или
//! отключение пака ничего не оставляет за собой.
//!
//! Адрес Google Drive — константа; подменить его можно только в отладочной сборке и с `dev-hooks`
//! (`PackOptions::drive_download`): тесты ходят на локальный сервер, а не в интернет.

use crate::archive_service::{ArchiveService, ExtractOptions};
use crate::errors::{Code, DeliveryError, Failure, RawError, to_delivery_error};
use crate::file_ops::{hex, rm_sync};
use crate::goldsrc_settings::{
    CHANGER_COLORS, CHANGER_SHAPES, CROSSHAIR_PACKS, HUD_PACKS, PackSelection,
};
use crate::http_client::{HttpClient, RequestOptions, read_body};
use crate::js_value::prop;
use crate::manifests::LaunchConfig;
use crate::node_fs::{self, mkdir_recursive, read_bytes_optional, write_bytes, write_text};
use crate::paths::assert_inside_with;
use crate::ports::Logger;
use hlsr_core::js::fs::read_dir;
use hlsr_core::js::object::JsObject;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, encode_uri_component, is_whitespace};
use hlsr_core::json_text::{js_key_order, json_parse, stringify};
use regress::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

/// Ссылка на раздел ресурсов HL1 (`LINKS.hl1Resources`).
pub const SOURCE_PAGE: &str = "https://www.speedrun.com/hl1/resources";
pub const DRIVE_DOWNLOAD: &str = "https://drive.usercontent.google.com/download";
/// Простой соединения при скачивании пака.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_millis(60_000);
/// Состояние установленных паков (от папки установки).
pub const STATE_DIR: &str = ".hlsr/packs";

/// Закреплённый пак: файл на Drive, его размер и SHA-256, папка с файлами внутри архива.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    /// `hud` или `crosshair`.
    pub kind: String,
    pub title: String,
    pub author: String,
    pub drive_id: String,
    pub size: usize,
    pub sha256: String,
    pub root: String,
}

/// Паки по идентификатору в порядке объявления (`PACKS`).
pub type Catalogue = Vec<(String, Pack)>;

fn pack(
    kind: &str,
    title: &str,
    author: &str,
    drive_id: &str,
    size: usize,
    sha256: &str,
    root: &str,
) -> Pack {
    Pack {
        kind: kind.to_owned(),
        title: title.to_owned(),
        author: author.to_owned(),
        drive_id: drive_id.to_owned(),
        size,
        sha256: sha256.to_owned(),
        root: root.to_owned(),
    }
}

static PACKS: LazyLock<Catalogue> = LazyLock::new(|| {
    vec![
        (
            "ag-hud".to_owned(),
            pack(
                "hud",
                "Big AG HUD",
                "execut4ble",
                "1_nmTVsEVha-bbQwCGnC7eHfshvCSqIw7",
                177_578,
                "462e5714d1a12d65af8c1b1ec45874d91e99d78de91ef2e7c65676d0e037c1e3",
                "big AG HUD",
            ),
        ),
        (
            "execut4ble-crosshair".to_owned(),
            pack(
                "crosshair",
                "Crosshair by execut4ble",
                "execut4ble",
                "1-73LVNJh89pSLGk7e6gmIiAurpIgqIAD",
                5_152,
                "43b9e3a1f79e84924c12b3d7c4c2fa9573cffe0e4dab7b5d09833c9db7ccd2c6",
                "crosshair by execut4ble",
            ),
        ),
        (
            "crosshair-changer".to_owned(),
            pack(
                "crosshair",
                "Half-Life Crosshair Changer",
                "aarongeorge",
                "1cq8baRp4TQrwdl8fx0F3g3UW-L1uLY9-",
                59_902,
                "98594d4cc130187facb8efdfe619e5fed22d83c22b9da398b15cd9dde08d9fbd",
                "Half-Life-Crosshair-Changer-master/crosshairs",
            ),
        ),
    ]
});

/// `PACKS`.
pub fn catalogue() -> &'static Catalogue {
    &PACKS
}

/// `defaultPacks()`.
pub fn default_packs() -> PackSelection {
    PackSelection {
        hud: "default".to_owned(),
        crosshair: "default".to_owned(),
        changer_shape: "Default".to_owned(),
        changer_color: "green".to_owned(),
    }
}

fn one_of(value: Option<&Value>, allowed: &[&str], fallback: &str) -> String {
    match value.and_then(Value::as_str) {
        Some(text) if allowed.contains(&text) => text.to_owned(),
        _ => fallback.to_owned(),
    }
}

/// `sanitizePacks(raw)`: выбор паков из настроек; всё незнакомое — умолчания.
pub fn sanitize_packs(raw: &Value) -> PackSelection {
    let base = default_packs();
    PackSelection {
        hud: one_of(prop(raw, "hud"), &HUD_PACKS, &base.hud),
        crosshair: one_of(prop(raw, "crosshair"), &CROSSHAIR_PACKS, &base.crosshair),
        changer_shape: one_of(
            prop(raw, "changerShape"),
            &CHANGER_SHAPES,
            &base.changer_shape,
        ),
        changer_color: one_of(
            prop(raw, "changerColor"),
            &CHANGER_COLORS,
            &base.changer_color,
        ),
    }
}

/// `catalog()`: что показывает окно — страница-источник, паки без хешей, формы и цвета Changer.
pub fn catalog() -> Value {
    let packs: Vec<Value> = PACKS
        .iter()
        .map(|(id, pack)| {
            json!({"id": id, "kind": pack.kind, "title": pack.title, "author": pack.author})
        })
        .collect();
    json!({
        "sourcePage": SOURCE_PAGE,
        "packs": packs,
        "changerShapes": CHANGER_SHAPES,
        "changerColors": CHANGER_COLORS,
    })
}

fn sha256_of(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// `inside(root, relative)`: путь внутри корня (`/` в относительном — разделитель ОС), иначе
/// `PATH_INVALID`.
fn inside(root: &Path, relative: &str) -> Result<PathBuf, DeliveryError> {
    let path = NodePath::native();
    let root_text = root.to_string_lossy();
    let joined = path.join(&[&root_text, &relative.replace('/', path.sep())]);
    assert_inside_with(&path, &root_text, &joined, Some("Пак")).map(PathBuf::from)
}

fn join_names(root: &Path, names: &[&str]) -> PathBuf {
    let path = NodePath::native();
    let mut parts = vec![root.to_string_lossy().into_owned()];
    parts.extend(names.iter().map(|name| (*name).to_owned()));
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    PathBuf::from(path.join(&refs))
}

// --------------------------------------------------------------------- состояние

/// Записи `state.files` в порядке `Object.keys`: имя файла и запись о нём (любое значение JSON).
type StateFiles = Vec<(String, Value)>;

/// `readState(file)`: записанные паки; нет файла, не JSON или нет `files` — пустой список.
fn read_state(file: &Path) -> StateFiles {
    let Ok(bytes) = hlsr_core::js::fs::read_file(file) else {
        return Vec::new();
    };
    let Ok(Value::Object(state)) = json_parse(&js::utf8_string(&bytes)) else {
        return Vec::new();
    };
    match state.get("files") {
        Some(Value::Object(files)) => js_key_order(files)
            .into_iter()
            .map(|(name, entry)| (name.clone(), entry.clone()))
            .collect(),
        Some(Value::Array(files)) => files
            .iter()
            .enumerate()
            .map(|(index, entry)| (index.to_string(), entry.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// `writeState(file, state)`: JSON с отступом в два пробела, без перевода строки в конце;
/// через `<файл>.tmp` и переименование.
fn write_state(file: &Path, state: &Value) -> Result<(), RawError> {
    if let Some(parent) = file.parent() {
        mkdir_recursive(parent)?;
    }
    let mut temp = file.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    write_text(
        &temp,
        &stringify(state, "  "),
        crate::game_binds::Encoding::Utf8,
    )?;
    std::fs::rename(&temp, file)
        .map_err(|error| RawError::fs_with_dest(&error, "rename", &temp, Some(file)))
}

// --------------------------------------------------------- скачивание и распаковка

/// Адрес файла на Drive: `<base>?id=<id>&export=download`.
pub fn download_url(base: &str, drive_id: &str) -> String {
    format!(
        "{base}?id={}&export=download",
        encode_uri_component(drive_id)
    )
}

/// Скачивает пак в кеш, если там нет файла с тем же размером и хешем.
async fn fetch_pack(
    pack: &Pack,
    cache_file: &Path,
    client: &HttpClient,
    base: &str,
) -> Result<(), Failure> {
    if let Some(cached) = read_bytes_optional(cache_file)?
        && cached.len() == pack.size
        && sha256_of(&cached) == pack.sha256
    {
        return Ok(());
    }

    let response = client
        .request(
            &download_url(base, &pack.drive_id),
            &RequestOptions {
                timeout: Some(DOWNLOAD_TIMEOUT),
                ..RequestOptions::default()
            },
        )
        .await?;
    if response.status() != 200 {
        let status = response.status();
        drop(response);
        return Err(DeliveryError::new(
            Code::HttpError,
            format!("HTTP {status} для пака {}", pack.title),
        )
        .into());
    }

    // Google Drive отдаёт HTML-страницу, если файл удалён или закрыт: она не пройдёт ни по
    // размеру, ни по хешу.
    let body = read_body(response, Some(pack.size + 1), None).await?;
    let actual = sha256_of(&body);
    if body.len() != pack.size || actual != pack.sha256 {
        return Err(DeliveryError::new(
            Code::ChecksumMismatch,
            format!(
                "Пак {} на Google Drive изменился или недоступен",
                pack.title
            ),
        )
        .with_details(json!({"expected": pack.sha256, "actual": actual}))
        .into());
    }

    if let Some(parent) = cache_file.parent() {
        mkdir_recursive(parent)?;
    }
    write_bytes(cache_file, &body)?;
    Ok(())
}

async fn unpack(
    cache_file: &Path,
    destination: &Path,
    archive: Option<&ArchiveService>,
) -> Result<(), Failure> {
    let Some(archive) = archive else {
        return Err(
            DeliveryError::new(Code::ExtractFailed, "Нет 7-Zip для распаковки пака").into(),
        );
    };
    rm_sync(destination, true, 0, Duration::from_millis(100))?;
    mkdir_recursive(destination)?;
    archive
        .extract(cache_file, destination, ExtractOptions::default())
        .await?;
    Ok(())
}

/// `listFiles(dir)`: имена файлов папки (не каталогов и не ссылок) в порядке `readdir`.
fn list_files(dir: &Path) -> Result<Vec<String>, RawError> {
    let entries = read_dir(dir).map_err(|error| RawError::fs(&error, "scandir", dir))?;
    Ok(entries
        .into_iter()
        .filter(|entry| entry.file_type.is_file())
        .map(|entry| entry.name)
        .collect())
}

// --------------------------------------------- что положить в папку спрайтов

/// `/^\s*(crosshair|autoaim|zoom|zoom_autoaim)\s/i`.
static CROSSHAIR_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::with_flags(r"^\s*(crosshair|autoaim|zoom|zoom_autoaim)\s", "i")
        .expect("выражение 0.4.7 разбирается")
});

/// `/^weapon_.*\.txt$/i`.
static WEAPON_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::with_flags(r"^weapon_.*\.txt$", "i").expect("выражение 0.4.7 разбирается")
});

/// `linesOf(buffer)`: строки latin1 без BOM и без пробелов в конце.
fn lines_of(bytes: &[u8]) -> Vec<String> {
    let text = js::latin1_string(bytes);
    let text = text.strip_prefix("\u{EF}\u{BB}\u{BF}").unwrap_or(&text);
    text.split('\n')
        .map(|line| line.trim_end_matches(is_whitespace).to_owned())
        .collect()
}

/// Пак execut4ble несёт свои weapon_*.txt, но иконки оружия в них размечены под Steam-версию: у
/// мины в WON был бы чужой значок. Поэтому из файлов пака берутся только строки прицела, а иконки
/// остаются из файлов игры.
pub fn merge_crosshair_lines(base: &[u8], pack: &[u8]) -> Vec<u8> {
    let keep = lines_of(base)
        .into_iter()
        .skip(1)
        .filter(|line| !js::trim(line).is_empty() && CROSSHAIR_LINE.find(line).is_none());
    let cross = lines_of(pack)
        .into_iter()
        .skip(1)
        .filter(|line| CROSSHAIR_LINE.find(line).is_some());
    let body: Vec<String> = keep.chain(cross).collect();
    let mut text = vec![body.len().to_string()];
    text.extend(body);
    js::latin1_bytes(&(text.join("\r\n") + "\r\n"))
}

/// Распакованный пак: имена файлов верхнего уровня его папки и сама папка.
struct Source {
    names: Vec<String>,
    root: PathBuf,
}

impl Source {
    fn read(&self, name: &str) -> Result<Vec<u8>, Failure> {
        Ok(node_fs::read_bytes(&inside(&self.root, name)?)?)
    }
}

fn source_of<'a>(sources: &'a [(String, Source)], id: &str) -> &'a Source {
    &sources
        .iter()
        .find(|(source_id, _)| source_id == id)
        .expect("выбранный пак распакован")
        .1
}

fn desired_files(
    selection: &PackSelection,
    sources: &[(String, Source)],
    mut read_base: impl FnMut(&str) -> Result<Option<Vec<u8>>, Failure>,
) -> Result<JsObject<Vec<u8>>, Failure> {
    let mut files = JsObject::new();

    if selection.hud == "ag-hud" {
        let dir = source_of(sources, "ag-hud");
        for name in &dir.names {
            files.insert(name.as_str(), dir.read(name)?);
        }
    }

    if selection.crosshair == "execut4ble-crosshair" {
        let dir = source_of(sources, "execut4ble-crosshair");
        for name in &dir.names {
            if WEAPON_FILE.find(name).is_some() {
                let base = read_base(name)?;
                let content = match base {
                    Some(base) => merge_crosshair_lines(&base, &dir.read(name)?),
                    None => dir.read(name)?,
                };
                files.insert(name.as_str(), content);
            } else {
                files.insert(name.as_str(), dir.read(name)?);
            }
        }
    }

    if selection.crosshair == "crosshair-changer" {
        let dir = source_of(sources, "crosshair-changer");
        files.insert(
            "crosshairs.spr",
            dir.read(&format!(
                "{}/{}.spr",
                selection.changer_shape, selection.changer_color
            ))?,
        );
    }

    Ok(files)
}

// ----------------------------------------- возврат к исходному состоянию и установка

/// `entry.<name>` у записи состояния: у `null` — `TypeError` V8; у остальных значений, не
/// объектов, поля нет.
fn entry_field<'a>(entry: &'a Value, name: &str) -> Result<Option<&'a Value>, RawError> {
    if entry.is_null() {
        return Err(RawError::new(format!(
            "Cannot read properties of null (reading '{name}')"
        )));
    }
    Ok(prop(entry, name))
}

/// Возвращает оригиналы файлов, которые лаунчер подменял. Файл, который с тех пор изменился
/// (например, его обновила сборка), уже чужой: его не трогаем, а старую копию оригинала
/// выбрасываем.
fn restore(sprites_dir: &Path, backup_dir: &Path, state: &StateFiles) -> Result<(), Failure> {
    let retry = Duration::from_millis(100);
    for (name, entry) in state {
        let target = inside(sprites_dir, name)?;
        let backup = inside(backup_dir, name)?;
        if let Some(current) = read_bytes_optional(&target)?
            && entry_field(entry, "sha256")?.and_then(Value::as_str)
                == Some(sha256_of(&current).as_str())
        {
            if js::truthy(entry_field(entry, "backup")?.unwrap_or(&Value::Null)) && backup.exists()
            {
                node_fs::copy_file(&backup, &target)?;
            } else {
                rm_sync(&target, false, 0, retry)?;
            }
        }
        rm_sync(&backup, false, 0, retry)?;
    }
    Ok(())
}

/// Итог [`apply_packs`] (`{ status, packs?, error? }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// У игры нет папки спрайтов для паков.
    Skipped,
    /// Выбраны стандартные HUD и прицел: игра в исходном состоянии.
    Default,
    /// Выбранные паки положены в спрайты.
    Applied { packs: Vec<String> },
    /// Паки не применены (нет сети, пак подменён, ошибка файла): игра остаётся с прошлыми.
    Failed {
        packs: Vec<String>,
        code: Code,
        message: String,
    },
}

impl Outcome {
    /// `status`: `skipped`, `default`, `applied` или `failed`.
    pub fn status(&self) -> &'static str {
        match self {
            Outcome::Skipped => "skipped",
            Outcome::Default => "default",
            Outcome::Applied { .. } => "applied",
            Outcome::Failed { .. } => "failed",
        }
    }

    /// Объект результата, как его видит окно.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("status".into(), json!(self.status()));
        match self {
            Outcome::Skipped => {}
            Outcome::Default => {
                object.insert("packs".into(), json!([]));
            }
            Outcome::Applied { packs } => {
                object.insert("packs".into(), json!(packs));
            }
            Outcome::Failed {
                packs,
                code,
                message,
            } => {
                object.insert("packs".into(), json!(packs));
                object.insert(
                    "error".into(),
                    json!({"code": code.as_str(), "message": message}),
                );
            }
        }
        Value::Object(object)
    }
}

/// Зависимости и подмены `applyPacks` (`options` в 0.4.7).
#[derive(Default)]
pub struct PackOptions<'a> {
    /// HTTP-клиент; без него — настоящий.
    pub client: Option<&'a HttpClient>,
    /// 7-Zip для распаковки; без него паки не ставятся (`EXTRACT_FAILED`).
    pub archive: Option<&'a ArchiveService>,
    pub logger: Option<&'a dyn Logger>,
    /// Другие паки вместо закреплённых (тесты).
    pub catalogue: Option<&'a Catalogue>,
    /// Другой адрес скачивания вместо Google Drive: локальный сервер тестов.
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    pub drive_download: Option<&'a str>,
}

impl PackOptions<'_> {
    /// Адрес скачивания: Google Drive или, в отладочной сборке, подмена.
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    fn base(&self) -> &str {
        self.drive_download.unwrap_or(DRIVE_DOWNLOAD)
    }

    #[cfg(not(any(debug_assertions, feature = "dev-hooks")))]
    fn base(&self) -> &str {
        DRIVE_DOWNLOAD
    }
}

/// `applyPacks(installPath, config, settings, options)`: приводит папку спрайтов в соответствие
/// выбору паков игры. Сбой скачивания и распаковки — результат `failed`, а не ошибка: запуск игры
/// из-за него не отменяется. Ошибка — только выход путей за папку установки и сбой уборки.
pub async fn apply_packs(
    install_path: &Path,
    config: &LaunchConfig,
    settings: &Value,
    options: &PackOptions<'_>,
) -> Result<Outcome, Failure> {
    match &config.packs {
        Some(packs) => {
            apply_packs_in(
                install_path,
                packs.sprites_dir,
                packs.base_sprites_dir,
                settings,
                options,
            )
            .await
        }
        None => Ok(Outcome::Skipped),
    }
}

/// То же по папкам спрайтов мода и игры (от папки установки).
pub async fn apply_packs_in(
    install_path: &Path,
    sprites_dir: &str,
    base_sprites_dir: &str,
    settings: &Value,
    options: &PackOptions<'_>,
) -> Result<Outcome, Failure> {
    struct Silent;
    impl Logger for Silent {
        fn info(&self, _: &str) {}
        fn warn(&self, _: &str) {}
        fn error(&self, _: &str) {}
    }
    let silent = Silent;
    let logger: &dyn Logger = options.logger.unwrap_or(&silent);
    let selection = sanitize_packs(prop(settings, "packs").unwrap_or(&Value::Null));
    let catalogue = options.catalogue.unwrap_or(&PACKS);

    let sprites_dir = inside(install_path, sprites_dir)?;
    let base_sprites_dir = inside(install_path, base_sprites_dir)?;
    let state_root = inside(install_path, STATE_DIR)?;
    let state_file = join_names(&state_root, &["applied.json"]);
    let backup_dir = join_names(&state_root, &["backup"]);

    let mut wanted: Vec<String> = Vec::new();
    if selection.hud != "default" {
        wanted.push(selection.hud.clone());
    }
    if selection.crosshair != "default" {
        wanted.push(selection.crosshair.clone());
    }

    let temp_root = join_names(&state_root, &[&format!("unpacked-{}", std::process::id())]);
    let result = apply_wanted(
        &selection,
        &wanted,
        catalogue,
        &sprites_dir,
        &base_sprites_dir,
        &state_root,
        &temp_root,
        (&state_file, &backup_dir),
        options,
        logger,
    )
    .await;
    let outcome = match result {
        Ok(()) if wanted.is_empty() => Outcome::Default,
        Ok(()) => Outcome::Applied {
            packs: wanted.clone(),
        },
        Err(error) => {
            let delivery = to_delivery_error(error, Some(Code::NetworkError));
            logger.warn(&format!(
                "[launch] паки HL1 не применены: {} {}",
                delivery.code, delivery.message
            ));
            Outcome::Failed {
                packs: wanted,
                code: delivery.code,
                message: delivery.message,
            }
        }
    };
    rm_sync(&temp_root, true, 0, Duration::from_millis(100))?;
    Ok(outcome)
}

#[allow(clippy::too_many_arguments)]
async fn apply_wanted(
    selection: &PackSelection,
    wanted: &[String],
    catalogue: &Catalogue,
    sprites_dir: &Path,
    base_sprites_dir: &Path,
    state_root: &Path,
    temp_root: &Path,
    (state_file, backup_dir): (&Path, &Path),
    options: &PackOptions<'_>,
    logger: &dyn Logger,
) -> Result<(), Failure> {
    let pack_of = |id: &str| -> Result<&Pack, Failure> {
        catalogue
            .iter()
            .find(|(pack_id, _)| pack_id == id)
            .map(|(_, pack)| pack)
            .ok_or_else(|| {
                RawError::new("Cannot read properties of undefined (reading 'driveId')").into()
            })
    };

    // Сначала скачиваем: если сети нет, игра остаётся с прошлыми паками, а не без них.
    let mut sources: Vec<(String, Source)> = Vec::new();
    let own_client = (options.client.is_none() && !wanted.is_empty()).then(HttpClient::new);
    for id in wanted {
        let pack = pack_of(id)?;
        let cache_file = join_names(state_root, &["cache", &format!("{id}.zip")]);
        let client = options
            .client
            .or(own_client.as_ref())
            .expect("клиент есть, когда есть что скачивать");
        fetch_pack(pack, &cache_file, client, options.base()).await?;
        let unpacked = join_names(temp_root, &[id]);
        unpack(&cache_file, &unpacked, options.archive).await?;
        let root = inside(&unpacked, &pack.root)?;
        let names = if id == "crosshair-changer" {
            Vec::new()
        } else {
            list_files(&root)?
        };
        sources.push((id.clone(), Source { names, root }));
    }

    mkdir_recursive(sprites_dir)?;
    let state = read_state(state_file);
    restore(sprites_dir, backup_dir, &state)?;
    write_state(state_file, &json!({"files": {}}))?;

    let files = desired_files(selection, &sources, |name| {
        let from_mod = read_bytes_optional(&inside(sprites_dir, name)?)?;
        match from_mod {
            Some(bytes) => Ok(Some(bytes)),
            None => Ok(read_bytes_optional(&inside(base_sprites_dir, name)?)?),
        }
    })?;

    // Оригиналы и запись о них сохраняются до подмены: если запуск оборвётся посередине,
    // следующий вернёт всё как было.
    let mut recorded: JsObject<Value> = JsObject::new();
    for (name, content) in files.iter() {
        let target = inside(sprites_dir, name)?;
        let backup = inside(backup_dir, name)?;
        let had_original = target.exists();
        if had_original {
            if let Some(parent) = backup.parent() {
                mkdir_recursive(parent)?;
            }
            node_fs::copy_file(&target, &backup)?;
        }
        recorded.insert(
            name,
            json!({"sha256": sha256_of(content), "backup": had_original}),
        );
    }
    let mut next = Map::new();
    next.insert("selection".into(), selection.to_value());
    next.insert(
        "files".into(),
        Value::Object(
            recorded
                .iter()
                .map(|(name, entry)| (name.to_owned(), entry.clone()))
                .collect(),
        ),
    );
    write_state(state_file, &Value::Object(next))?;

    for (name, content) in files.iter() {
        write_bytes(&inside(sprites_dir, name)?, content)?;
    }

    if !wanted.is_empty() {
        let titles: Vec<&str> = wanted
            .iter()
            .map(|id| pack_of(id).map(|pack| pack.title.as_str()))
            .collect::<Result<_, _>>()?;
        logger.info(&format!(
            "[launch] паки HL1: {}, файлов {}",
            titles.join(", "),
            files.len()
        ));
    }
    Ok(())
}
