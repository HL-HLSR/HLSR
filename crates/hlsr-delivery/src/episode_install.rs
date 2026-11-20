//! Установка эпизода Ghosting (`episode-install.js` 0.4.9). Эпизод ставится по своему
//! подписанному манифесту, который лежит рядом с каналом Ghosting:
//! `…/ghosting-renewed/channel/stable.json` → `…/ghosting-renewed/episodes/<id>/stable.json`,
//! архив 7z — рядом с манифестом. Подпись — ключами канала ghosting-renewed (`channel_config`).
//! Саму установку ведёт `GameDeliveryService::install_episode`.

use crate::channel_config;
use crate::environment::Environment;
use crate::episodes;
use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::file_ops::rm_sync;
use crate::http_client::{HttpClient, RequestOptions, read_body};
use crate::install_service::rename;
use crate::js_value as jv;
use crate::manifest_signing::{CheckOptions, check};
use crate::node_fs;
use crate::ports::Logger;
use hlsr_core::js;
use hlsr_core::json_text::{json_parse, stringify};
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

pub const GAME_ID: &str = "ghosting-renewed";
pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const TIMEOUT: Duration = Duration::from_millis(15_000);
/// Поля манифеста — ровно эти (по алфавиту).
const FIELDS: [&str; 8] = [
    "archive",
    "episode",
    "gameId",
    "publishedAt",
    "requires",
    "schemaVersion",
    "signature",
    "version",
];
const ARCHIVE_FIELDS: [&str; 3] = ["fileName", "sha256", "size"];
/// `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

fn invalid(message: String) -> DeliveryError {
    DeliveryError::new(
        Code::ManifestInvalid,
        format!("Манифест эпизода: {message}"),
    )
}

static ARCHIVE_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ghosting-renewed-(ep\d+)-[A-Za-z0-9._-]+\.7z$").unwrap());

/// `episodeOfArchive(fileName)`: архив эпизода зовётся `ghosting-renewed-<id>-….7z` — по имени
/// находятся его загрузки в Downloads.
pub fn episode_of_archive(file_name: &str) -> Option<&'static str> {
    let id = ARCHIVE_NAME.captures(file_name)?.get(1)?.as_str();
    episodes::episode_of(id).ok().map(|episode| episode.id)
}

/// Откуда брать манифест эпизода и чьим ключам верить.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestSource {
    pub url: String,
    pub public_keys: Vec<String>,
}

/// `manifestSource(id, env)`: `None` — канал Ghosting не настроен (сборка без профиля выпуска),
/// эпизоды недоступны.
pub fn manifest_source(id: &str, env: &Environment) -> Option<ManifestSource> {
    let config = channel_config::resolve(GAME_ID, env)?;
    let base = url::Url::parse(config.url.as_deref().filter(|url| !url.is_empty())?).ok()?;
    let url = base.join(&format!("../episodes/{id}/stable.json")).ok()?;
    Some(ManifestSource {
        url: url.to_string(),
        public_keys: config.public_keys,
    })
}

fn same_keys(value: &serde_json::Map<String, Value>, keys: &[&str]) -> bool {
    let mut own: Vec<&str> = value.keys().map(String::as_str).collect();
    own.sort_unstable();
    own == keys
}

/// `validateManifest(data, id, publicKeys)`: подпись ключами канала, поля строго.
pub fn validate_manifest(
    data: &Value,
    id: &str,
    public_keys: &[String],
) -> Result<Value, DeliveryError> {
    let Some(object) = data.as_object() else {
        return Err(invalid("не JSON-объект".to_owned()));
    };
    check(
        data,
        &CheckOptions {
            required: true,
            public_keys,
        },
    )
    .map_err(|error| {
        DeliveryError::new(
            Code::SignatureInvalid,
            format!("Подпись манифеста эпизода не принята: {}", error.message),
        )
    })?;
    let show = |key: &str| jv::template(data.get(key));
    if !same_keys(object, &FIELDS) {
        let keys: Vec<&str> = object.keys().map(String::as_str).collect();
        return Err(invalid(format!("поля {}", keys.join(", "))));
    }
    if data.get("schemaVersion").and_then(Value::as_f64) != Some(1.0) {
        return Err(invalid(format!("schemaVersion {}", show("schemaVersion"))));
    }
    if data.get("gameId").and_then(Value::as_str) != Some(GAME_ID) {
        return Err(invalid(format!("gameId {}", show("gameId"))));
    }
    if data.get("episode").and_then(Value::as_str) != Some(id) {
        return Err(invalid(format!(
            "описывает {}, запрошен {id}",
            show("episode")
        )));
    }
    let version_ok = data
        .get("version")
        .and_then(Value::as_str)
        .is_some_and(|v| {
            let parts: Vec<&str> = v.split('.').collect();
            parts.len() == 3
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        });
    if !version_ok {
        return Err(invalid(format!("version {}", show("version"))));
    }
    let published_ok = data
        .get("publishedAt")
        .and_then(Value::as_str)
        .is_some_and(|p| {
            let b = p.as_bytes();
            b.len() >= 11
                && b[..4].iter().all(u8::is_ascii_digit)
                && b[4] == b'-'
                && b[5..7].iter().all(u8::is_ascii_digit)
                && b[7] == b'-'
                && b[8..10].iter().all(u8::is_ascii_digit)
                && b[10] == b'T'
                && js::date_parse(p).is_some()
        });
    if !published_ok {
        return Err(invalid(format!("publishedAt {}", show("publishedAt"))));
    }
    let requires = episodes::episode_of(id)?.requires;
    if data.get("requires") != Some(&json!(requires)) {
        let shown = data
            .get("requires")
            .map_or("undefined".to_owned(), |v| stringify(v, ""));
        return Err(invalid(format!("requires {shown}")));
    }

    let archive = data.get("archive");
    let Some(fields) = archive
        .and_then(Value::as_object)
        .filter(|fields| same_keys(fields, &ARCHIVE_FIELDS))
    else {
        return Err(invalid("archive".to_owned()));
    };
    let file_name = jv::template(fields.get("fileName"));
    if episode_of_archive(&file_name) != Some(id) {
        return Err(invalid(format!("archive.fileName {file_name}")));
    }
    let size_ok = fields
        .get("size")
        .and_then(Value::as_f64)
        .is_some_and(|size| size.fract() == 0.0 && size > 0.0 && size <= MAX_SAFE_INTEGER);
    if !size_ok {
        return Err(invalid(format!(
            "archive.size {}",
            jv::template(fields.get("size"))
        )));
    }
    let sha_ok = fields
        .get("sha256")
        .and_then(Value::as_str)
        .is_some_and(|sha| {
            sha.len() == 64
                && sha
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
    if !sha_ok {
        return Err(invalid("archive.sha256".to_owned()));
    }
    Ok(data.clone())
}

/// `fetchManifest(id, env)`: подписанный манифест эпизода и его адрес.
pub async fn fetch_manifest(
    id: &str,
    env: &Environment,
    http: &HttpClient,
) -> Result<(Value, String), Failure> {
    let Some(source) = manifest_source(id, env) else {
        return Err(DeliveryError::new(
            Code::ChannelUnavailable,
            "Канал Ghosting не настроен: эпизоды недоступны",
        )
        .with_details(json!({ "id": id }))
        .into());
    };
    let options = RequestOptions {
        timeout: Some(TIMEOUT),
        ..RequestOptions::default()
    };
    let response = http.request(&source.url, &options).await?;
    let status = response.status();
    if status != 200 {
        drop(response);
        return Err(DeliveryError::new(
            Code::ChannelUnavailable,
            format!("Манифест эпизода {id}: HTTP {status}"),
        )
        .with_details(json!({ "statusCode": status }))
        .into());
    }
    let body = read_body(response, Some(MAX_BODY_BYTES), None).await?;
    let data = json_parse(&String::from_utf8_lossy(&body))
        .map_err(|error| invalid(format!("не JSON: {error}")))?;
    let manifest = validate_manifest(&data, id, &source.public_keys)?;
    Ok((manifest, source.url))
}

/// `providerDescriptor(manifest, manifestUrl)`: архив рядом с манифестом. QA-канал на 127.0.0.1
/// отдаёт по http — это `local-http` (флаг разрешает его только вне production).
pub fn provider_descriptor(manifest: &Value, manifest_url: &str) -> Result<Value, DeliveryError> {
    let file_name = jv::template(manifest.get("archive").and_then(|a| a.get("fileName")));
    let url = url::Url::parse(manifest_url)
        .and_then(|base| base.join(&file_name))
        .map_err(|error| invalid(format!("адрес архива: {error}")))?;
    let (id, kind) = if url.scheme() == "http" {
        ("qa-local", "local-http")
    } else {
        ("hlsr-primary", "https-direct")
    };
    Ok(json!({
        "id": id,
        "type": kind,
        "status": "permanent",
        "priority": 10,
        "url": url.to_string(),
    }))
}

/// `stockCfgOf(episodeDir)`: стоковые cfg эпизода (chapter*.cfg, skill_episodic.cfg) в нижнем
/// регистре — их `sync_config_in` не перезаписывает.
pub fn stock_cfg_of(episode_dir: &Path) -> Result<Vec<String>, RawError> {
    let dir = episode_dir.join("cfg");
    let entries = match js::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(RawError::fs(&error, "scandir", &dir)),
    };
    let mut names: Vec<String> = entries
        .into_iter()
        .filter(|entry| entry.file_type.is_file() && entry.name.to_lowercase().ends_with(".cfg"))
        .map(|entry| entry.name.to_lowercase())
        .collect();
    names.sort();
    Ok(names)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(suffix);
    PathBuf::from(text)
}

/// Что подменяет `swap_in`.
pub struct Swap<'a> {
    pub install_path: &'a Path,
    pub id: &'a str,
    /// Свежая папка эпизода (в staging).
    pub fresh: &'a Path,
    /// `InstallService::episodes_park_path`.
    pub park: &'a Path,
    pub marker: &'a Value,
    /// `Date.now()` для имени `.stale-…`.
    pub now_ms: i64,
    pub logger: &'a dyn Logger,
}

/// `swapIn(…)`: подмена папки эпизода. Прежняя уходит в park: упадёт лаунчер посреди подмены —
/// `unpark_episodes` на старте вернёт её туда, где папки нет. Свежая (маркер уже внутри) встаёт
/// одним rename, данные игрока переезжают в неё rename из прежней, и только потом прежняя
/// удаляется. Ошибка — всё возвращается как было.
pub fn swap_in(swap: &Swap) -> Result<(), Failure> {
    let Swap {
        install_path,
        id,
        fresh,
        park,
        marker,
        now_ms,
        logger,
    } = *swap;
    // Служебные файлы лаунчера в папке эпизода — только его собственные, не из архива.
    rm_sync(&fresh.join(episodes::SYNC), false, 0, Duration::ZERO)?;
    node_fs::write_bytes(
        &fresh.join(episodes::MARKER),
        stringify(marker, "  ").as_bytes(),
    )?;
    let dir = episodes::episode_of(id)?.dir;
    let target = install_path.join(dir);
    let parked = park.join(dir);
    let had = target.exists();
    if had {
        // Отложенная раньше копия не вернулась (папка уже стояла) — в сторону, как в park_episodes.
        if parked.exists() {
            rename(
                &parked,
                &with_suffix(
                    &parked,
                    &format!(".stale-{}", js::base36(now_ms.max(0) as u128)),
                ),
            )?;
        }
        node_fs::mkdir_recursive(park)?;
        rename(&target, &parked)?;
    }

    let mut placed = false;
    let mut carried: Vec<String> = Vec::new();
    let moved = (|| -> Result<(), Failure> {
        rename(fresh, &target)?;
        placed = true;
        if !had {
            return Ok(());
        }
        let names = js::fs::read_dir_names(&parked)
            .map_err(|error| RawError::fs(&error, "scandir", &parked))?;
        for name in names
            .into_iter()
            .filter(|name| episodes::is_player_entry(name))
        {
            // Данные игрока главнее одноимённого из архива; SAVE и save на Windows — одно имя.
            let present = js::fs::read_dir_names(&target)
                .map_err(|error| RawError::fs(&error, "scandir", &target))?;
            for same in present
                .iter()
                .filter(|other| other.to_lowercase() == name.to_lowercase())
            {
                rm_sync(&target.join(same), true, 0, Duration::ZERO)?;
            }
            rename(&parked.join(&name), &target.join(&name))?;
            carried.push(name);
        }
        Ok(())
    })();
    if let Err(error) = moved {
        let back = (|| -> Result<(), RawError> {
            for name in &carried {
                rename(&target.join(name), &parked.join(name))?;
            }
            if placed {
                rename(&target, fresh)?;
            }
            if had {
                rename(&parked, &target)?;
            }
            Ok(())
        })();
        if let Err(back) = back {
            logger.error(&format!(
                "[game-delivery] эпизод {dir} не вернулся на место, прежняя копия в {}: {}",
                parked.display(),
                back.message
            ));
        }
        return Err(error);
    }

    if had && let Err(error) = rm_sync(&parked, true, 0, Duration::ZERO) {
        logger.warn(&format!(
            "[game-delivery] прежняя копия эпизода {dir} не удалена из {}: {}",
            parked.display(),
            error.message
        ));
    }
    Ok(())
}
