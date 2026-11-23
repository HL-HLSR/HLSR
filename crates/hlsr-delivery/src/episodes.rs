//! Эпизоды Ghosting (`episodes.js` 0.4.9): Episode One (`episodic\`) и Episode Two (`ep2\`)
//! ставятся в папку установленной ghosting-renewed рядом с `ghostingmod\` и запускаются тем же
//! hl2.exe с `-game episodic|ep2`. Маркеры лежат внутри папки эпизода: при обновлении Ghosting
//! она переезжает целиком (`InstallService::park_episodes`).

use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::file_ops::{hex, rm_sync};
use crate::install_service::rename;
use crate::js_value as jv;
use crate::manifests::LaunchConfig;
use crate::node_fs;
use crate::ports::Logger;
use hlsr_core::json_store::read_json_safe;
use hlsr_core::json_text::stringify;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Эпизод: id, папка в папке Ghosting и без каких эпизодов он не запустится.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Episode {
    pub id: &'static str,
    pub dir: &'static str,
    pub requires: &'static [&'static str],
}

/// `EPISODES`. Порядок папок — `mutable_paths::EPISODE_DIRS` (сверяет тест).
pub const EPISODES: [Episode; 2] = [
    Episode {
        id: "ep1",
        dir: "episodic",
        requires: &[],
    },
    // Episode Two монтирует episodic\ и episodic\ep1_pak.vpk: без Episode One не запустится.
    Episode {
        id: "ep2",
        dir: "ep2",
        requires: &["ep1"],
    },
];

pub const MARKER: &str = ".hlsr-episode.json";
pub const SYNC: &str = ".hlsr-sync.json";
const GHOSTING_CFG: [&str; 2] = ["ghostingmod", "cfg"];
/// Файлы игрока, которые идут в эпизод всегда: его конфиг лаунчера и config.cfg игры (бинды).
const PLAYER_FILES: [&str; 2] = ["autoexec.cfg", "config.cfg"];

/// Данные игрока в папке эпизода (как ghostingmod в `USER_DATA`, без cfg — его каждый запуск
/// кладёт `sync_config_in`) и демо в её корне: при переустановке и после краха подмены они
/// переезжают rename из прежней копии (`episode_install::swap_in`, `unpark_episodes`).
pub const PLAYER_ENTRIES: [&str; 7] = [
    "save",
    "runs",
    "screenshots",
    "custom",
    "synclogs",
    "download",
    "downloadlists",
];

/// `isPlayerEntry(name)`.
pub fn is_player_entry(name: &str) -> bool {
    let lower = name.to_lowercase();
    PLAYER_ENTRIES.contains(&lower.as_str()) || lower.ends_with(".dem")
}

/// `isEpisode(id)`.
pub fn is_episode(id: &str) -> bool {
    EPISODES.iter().any(|episode| episode.id == id)
}

fn unknown(id: &str) -> DeliveryError {
    DeliveryError::new(Code::LaunchConfigInvalid, format!("Нет эпизода {id}"))
        .with_details(json!({ "id": id }))
}

/// `episodeOf(id)`: нет такого — `LAUNCH_CONFIG_INVALID`.
pub fn episode_of(id: &str) -> Result<&'static Episode, DeliveryError> {
    EPISODES
        .iter()
        .find(|episode| episode.id == id)
        .ok_or_else(|| unknown(id))
}

/// `episodeIdOf(id)` из IPC: только строка (массив `["ep1"]` не эпизод).
pub fn episode_id_of(id: &Value) -> Result<&'static str, DeliveryError> {
    match id {
        Value::String(id) => episode_of(id).map(|episode| episode.id),
        other => Err(unknown(&jv::template(Some(other)))),
    }
}

fn dir_of(install_path: &Path, id: &str) -> Result<PathBuf, DeliveryError> {
    Ok(install_path.join(episode_of(id)?.dir))
}

fn sha256_of(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// `readMarker(installPath, id)`: маркер эпизода или `null`.
pub fn read_marker(install_path: &Path, id: &str) -> Value {
    dir_of(install_path, id).map_or(Value::Null, |dir| read_json_safe(&dir.join(MARKER)))
}

/// Есть ли в каталоге мода `gameinfo.txt` без учёта регистра имени (на Linux в архивах
/// эпизодов лежит `GameInfo.txt`).
pub fn has_gameinfo(dir: &Path) -> bool {
    if dir.join("gameinfo.txt").exists() || dir.join("GameInfo.txt").exists() {
        return true;
    }
    hlsr_core::js::fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .into_iter()
            .any(|entry| entry.name.eq_ignore_ascii_case("gameinfo.txt"))
    })
}

/// `status(installPath)`: `{ep1: {installed, version}, ep2: …}`. Установлен — есть и маркер с
/// версией-строкой, и `gameinfo.txt`.
pub fn status(install_path: Option<&Path>) -> Map<String, Value> {
    EPISODES
        .iter()
        .map(|episode| {
            let version = install_path
                .map(|root| read_marker(root, episode.id))
                .and_then(|marker| {
                    marker
                        .get("version")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .filter(|_| {
                    install_path.is_some_and(|root| has_gameinfo(&root.join(episode.dir)))
                });
            (
                episode.id.to_owned(),
                json!({ "installed": version.is_some(), "version": version }),
            )
        })
        .collect()
}

/// `dependentsOf(id)`: эпизоды, которые без этого не запустятся (ep1 → ep2).
pub fn dependents_of(id: &str) -> Vec<&'static str> {
    EPISODES
        .iter()
        .filter(|episode| episode.requires.contains(&id))
        .map(|episode| episode.id)
        .collect()
}

/// Аргументы запуска эпизодов: у `LaunchConfig` они `&'static`, каждый набор живёт до конца процесса.
static ARGUMENTS: Mutex<Vec<&'static [&'static str]>> = Mutex::new(Vec::new());

fn static_arguments(arguments: Vec<&'static str>) -> &'static [&'static str] {
    let mut known = ARGUMENTS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(found) = known.iter().find(|known| **known == arguments.as_slice()) {
        return found;
    }
    let leaked: &'static [&'static str] = Box::leak(arguments.into_boxed_slice());
    known.push(leaked);
    leaked
}

/// `launchConfig(config, id)`: доверенная конфигурация запуска Ghosting с каталогом мода
/// эпизода (`-game ghostingmod` → `-game <dir>`); исходная не меняется.
pub fn launch_config(config: &LaunchConfig, id: &str) -> Result<LaunchConfig, DeliveryError> {
    let episode = episode_of(id)?;
    let at = config
        .arguments
        .iter()
        .position(|argument| *argument == "-game")
        .filter(|at| at + 1 < config.arguments.len())
        .ok_or_else(|| {
            DeliveryError::new(
                Code::LaunchConfigInvalid,
                "В конфигурации запуска нет -game",
            )
            .with_details(json!({ "id": id }))
        })?;
    let mut arguments = config.arguments.to_vec();
    arguments[at + 1] = episode.dir;
    Ok(LaunchConfig {
        game_directory: episode.dir,
        arguments: static_arguments(arguments),
        ..config.clone()
    })
}

fn hash_of(file: &Path) -> Option<String> {
    node_fs::read_bytes(file)
        .ok()
        .map(|bytes| sha256_of(&bytes))
}

/// Запись через `<файл>.hlsr-episode-tmp` и rename; сбой — временный файл убран.
fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), RawError> {
    let mut temp = target.as_os_str().to_owned();
    temp.push(".hlsr-episode-tmp");
    let temp = PathBuf::from(temp);
    node_fs::write_bytes(&temp, bytes)
        .and_then(|()| rename(&temp, target))
        .inspect_err(|_| {
            let _ = rm_sync(&temp, false, 0, Duration::ZERO);
        })
}

/// Итог `syncConfigIn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncIn {
    pub copied: Vec<String>,
    pub config_sha256: Option<String>,
}

/// `syncConfigIn(installPath, id, {ghostingStock})`: конфиг Ghosting в cfg эпизода перед
/// запуском. Идут `autoexec.cfg`, `config.cfg` и cfg игрока — те, которых нет в сборке Ghosting
/// (`ghosting_stock`: имена файлов `ghostingmod/cfg` из её files.json; `None` — сборка
/// неизвестна, только они двое). Стоковые cfg Ghosting (chapter*.cfg HL2, skill.cfg) в эпизод не
/// попадают: там свои.
pub fn sync_config_in(
    install_path: &Path,
    id: &str,
    ghosting_stock: Option<&[String]>,
) -> Result<SyncIn, Failure> {
    let episode_dir = dir_of(install_path, id)?;
    if !episode_dir.exists() {
        return Ok(SyncIn {
            copied: Vec::new(),
            config_sha256: None,
        });
    }
    // Прошлый запуск не вернул config.cfg (крах): сначала вернуть, потом перезаписывать.
    if episode_dir.join(SYNC).exists() {
        sync_config_back(install_path, id)?;
    }

    let stock: Option<Vec<String>> =
        ghosting_stock.map(|names| names.iter().map(|name| name.to_lowercase()).collect());
    let from = install_path.join(GHOSTING_CFG[0]).join(GHOSTING_CFG[1]);
    let to = episode_dir.join("cfg");
    let names: Vec<String> = match hlsr_core::js::fs::read_dir(&from) {
        Ok(entries) => entries
            .into_iter()
            .filter(|entry| entry.file_type.is_file())
            .map(|entry| entry.name)
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(RawError::fs(&error, "scandir", &from).into()),
    };
    let copied: Vec<String> = names
        .into_iter()
        .filter(|name| {
            let lower = name.to_lowercase();
            lower.ends_with(".cfg")
                && (PLAYER_FILES.contains(&lower.as_str())
                    || stock.as_ref().is_some_and(|stock| !stock.contains(&lower)))
        })
        .collect();
    node_fs::mkdir_recursive(&to)?;
    // Файлы игрока — под именами, которые читает игра (Config.cfg → config.cfg).
    for name in &copied {
        let lower = name.to_lowercase();
        let target = if PLAYER_FILES.contains(&lower.as_str()) {
            lower
        } else {
            name.clone()
        };
        node_fs::copy_file(&from.join(name), &to.join(target))?;
    }

    let config_sha256 = hash_of(&to.join("config.cfg"));
    // srcSha256 — config.cfg Ghosting в момент копии: вернуть можно, только если его с тех пор не меняли.
    let src_sha256 = hash_of(&from.join("config.cfg"));
    let sync = json!({ "configSha256": config_sha256, "srcSha256": src_sha256 });
    write_atomic(&episode_dir.join(SYNC), stringify(&sync, "  ").as_bytes())?;
    Ok(SyncIn {
        copied,
        config_sha256,
    })
}

/// Итог `syncConfigBack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncBack {
    pub returned: bool,
    pub conflict: bool,
}

/// `syncConfigBack(installPath, id)`: после выхода из эпизода config.cfg, который игра
/// переписала (бинды из меню), — обратно в Ghosting. Не вернётся, если игра его не меняла, файл
/// пуст (игра упала на записи) или config.cfg Ghosting успели поменять — тогда новее он. Без
/// отметки ничего не делает.
pub fn sync_config_back(install_path: &Path, id: &str) -> Result<SyncBack, Failure> {
    let episode_dir = dir_of(install_path, id)?;
    let sync_file = episode_dir.join(SYNC);
    if !sync_file.exists() {
        return Ok(SyncBack::default());
    }
    let sync = read_json_safe(&sync_file);
    let mut result = SyncBack::default();
    if jv::truthy(Some(&sync)) {
        let bytes = node_fs::read_bytes(&episode_dir.join("cfg").join("config.cfg")).ok();
        let target = install_path
            .join(GHOSTING_CFG[0])
            .join(GHOSTING_CFG[1])
            .join("config.cfg");
        if let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty())
            && sync.get("configSha256").and_then(Value::as_str) != Some(&sha256_of(&bytes))
        {
            // `hashOf(target) !== (sync.srcSha256 || null)`.
            let expected = sync.get("srcSha256").filter(|v| jv::truthy(Some(v)));
            let same = match (hash_of(&target), expected) {
                (None, None) => true,
                (Some(hash), Some(Value::String(expected))) => hash == *expected,
                _ => false,
            };
            if same {
                if let Some(parent) = target.parent() {
                    node_fs::mkdir_recursive(parent)?;
                }
                write_atomic(&target, &bytes)?;
                result.returned = true;
            } else {
                result.conflict = true;
            }
        }
    }
    // Битая отметка тоже снимается: иначе висела бы вечно.
    rm_sync(&sync_file, false, 0, Duration::ZERO)?;
    Ok(result)
}

/// `pendingSync(installPath)`: эпизоды, у которых осталась отметка синхронизации (игра упала
/// или лаунчер закрылся раньше).
pub fn pending_sync(install_path: Option<&Path>) -> Vec<&'static str> {
    let Some(root) = install_path else {
        return Vec::new();
    };
    EPISODES
        .iter()
        .filter(|episode| root.join(episode.dir).join(SYNC).exists())
        .map(|episode| episode.id)
        .collect()
}

/// `returnPending(installPath, {logger, ids})`: `sync_config_back` для `ids` (по умолчанию — всех
/// с отметкой). Зовут на выходе игры, на старте лаунчера и перед запуском Ghosting, когда игра
/// не запущена. Не бросает: сбой — в журнал.
pub fn return_pending(install_path: &Path, logger: &dyn Logger, ids: Option<&[&str]>) {
    let pending;
    let ids = match ids {
        Some(ids) => ids,
        None => {
            pending = pending_sync(Some(install_path));
            &pending
        }
    };
    for id in ids {
        match sync_config_back(install_path, id) {
            Ok(back) if back.returned => logger.info(&format!(
                "[episodes] {id}: config.cfg эпизода вернулся в ghostingmod/cfg"
            )),
            Ok(back) if back.conflict => logger.warn(&format!(
                "[episodes] {id}: config.cfg эпизода не вернулся — config.cfg Ghosting с тех пор менялся"
            )),
            Ok(_) => {}
            Err(error) => logger.warn(&format!(
                "[episodes] {id}: config.cfg эпизода не вернулся: {error}"
            )),
        }
    }
}

/// `remove(installPath, id)`: удаляет эпизод и те, что без него не запустятся; папки удалённых.
pub fn remove(install_path: &Path, id: &str) -> Result<Vec<String>, Failure> {
    episode_of(id)?;
    let mut removed = Vec::new();
    for each in dependents_of(id).into_iter().chain([id]) {
        let dir = dir_of(install_path, each)?;
        if !dir.exists() {
            continue;
        }
        rm_sync(&dir, true, 0, Duration::ZERO)?;
        removed.push(episode_of(each)?.dir.to_owned());
    }
    Ok(removed)
}
