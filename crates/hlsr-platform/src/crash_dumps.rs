//! Дампы падений (`game-delivery/crash-dumps.js`): свежие `.dmp`/`.mdmp` в папке мода, корне
//! игры, `%LOCALAPPDATA%\CrashDumps` (только `<exe>.<pid>.dmp` этой игры) и
//! `$XDG_DATA_HOME/hlsr/crashdumps` (дампы Wine). Снимок до запуска игры, поиск новых после
//! выхода, ожидание, пока дамп допишется, и проверка пути перед «Показать в папке».

use hlsr_core::js::fs::{mtime_ms, read_dir};
use hlsr_core::js::path::NodePath;
use hlsr_delivery::environment::Environment;
use hlsr_delivery::manifests::{LAUNCH_CONFIG, launch_config_of};
use hlsr_delivery::paths::is_inside_with;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `DUMP_EXTENSIONS`.
pub const DUMP_EXTENSIONS: [&str; 2] = [".dmp", ".mdmp"];
/// `DUMP_SETTLE_MS`: сколько ждать, пока дамп допишется.
pub const DUMP_SETTLE_MS: u64 = 5000;
/// `DUMP_POLL_MS`.
pub const DUMP_POLL_MS: u64 = 500;

/// `isDumpFile(name)`: расширение `.dmp` или `.mdmp` без учёта регистра.
pub fn is_dump_file(name: &str) -> bool {
    let extension = NodePath::native().extname(name).to_lowercase();
    DUMP_EXTENSIONS.contains(&extension.as_str())
}

/// Папка поиска: `{id, dir}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpDir {
    /// `game-mod`, `game-root`, `wer-crashdumps` или `wine-crashdumps`.
    pub id: &'static str,
    pub dir: String,
}

/// Переменная окружения, если она не пустая (`source.X` в условии JS).
fn env_value(env: &Environment, name: &str) -> Option<String> {
    env.var(name).filter(|value| !value.is_empty())
}

/// `dumpDirectories(installPath, env, gameId)`: с `game_id` — только папка мода этой игры
/// (иначе дамп другой игры той же библиотеки выдавался бы за дамп этой).
pub fn dump_directories(
    install_path: Option<&str>,
    env: &Environment,
    game_id: Option<&str>,
) -> Vec<DumpDir> {
    dump_directories_on(&NodePath::native(), install_path, env, game_id)
}

/// `dumpDirectories` с `path` заданной ОС (`path.win32` или `path.posix`).
pub fn dump_directories_on(
    path: &NodePath,
    install_path: Option<&str>,
    env: &Environment,
    game_id: Option<&str>,
) -> Vec<DumpDir> {
    let mut dirs = Vec::new();
    let config = game_id
        .filter(|id| !id.is_empty())
        .and_then(launch_config_of);
    let mods: Vec<&str> = match config.filter(|config| !config.game_directory.is_empty()) {
        Some(config) => vec![config.game_directory],
        None => LAUNCH_CONFIG
            .iter()
            .map(|(_, config)| config.game_directory)
            .filter(|name| !name.is_empty())
            .collect(),
    };

    if let Some(install_path) = install_path.filter(|path| !path.is_empty()) {
        let root = path.resolve(&[install_path]);
        for mod_directory in mods {
            dirs.push(DumpDir {
                id: "game-mod",
                dir: path.join(&[&root, mod_directory]),
            });
        }
        dirs.push(DumpDir {
            id: "game-root",
            dir: root,
        });
    }

    if let Some(local_app_data) = env_value(env, "LOCALAPPDATA") {
        dirs.push(DumpDir {
            id: "wer-crashdumps",
            dir: path.join(&[&local_app_data, "CrashDumps"]),
        });
    }
    if let Some(data_dir) = env_value(env, "XDG_DATA_HOME") {
        dirs.push(DumpDir {
            id: "wine-crashdumps",
            dir: path.join(&[&data_dir, "hlsr", "crashdumps"]),
        });
    }
    dirs
}

/// `ZONE_RANK`: незнакомая зона — 9.
fn zone_rank(zone: &str) -> u32 {
    match zone {
        "game-mod" => 0,
        "game-root" => 1,
        "wer-crashdumps" | "wine-crashdumps" => 2,
        _ => 9,
    }
}

/// `SHARED_ZONES`: папки, куда пишут дампы и другие программы.
fn is_shared_zone(zone: &str) -> bool {
    matches!(zone, "wer-crashdumps" | "wine-crashdumps")
}

/// `processPrefixOf(gameId)`: `<exe>.` в нижнем регистре — так Windows называет дампы в
/// CrashDumps (`<exe>.<pid>.dmp`).
fn process_prefix_of(path: &NodePath, game_id: Option<&str>) -> Option<String> {
    let config = game_id
        .filter(|id| !id.is_empty())
        .and_then(launch_config_of)?;
    let exe = path.basename(&config.executable.replace('\\', "/"));
    (!exe.is_empty()).then(|| exe.to_lowercase() + ".")
}

/// `belongsToGame(zone, name, prefix)`.
fn belongs_to_game(zone: &str, name: &str, prefix: Option<&str>) -> bool {
    match prefix {
        None => true,
        Some(prefix) => !is_shared_zone(zone) || name.to_lowercase().starts_with(prefix),
    }
}

/// Сведения о дампе в снимке: размер, время изменения (`mtimeMs`) и зона.
#[derive(Debug, Clone, PartialEq)]
pub struct DumpInfo {
    pub size: u64,
    pub mtime_ms: f64,
    pub zone: &'static str,
}

/// Снимок дампов (`Map` путь → сведения в порядке первого появления).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DumpSnapshot {
    entries: Vec<(String, DumpInfo)>,
}

impl DumpSnapshot {
    pub fn get(&self, file: &str) -> Option<&DumpInfo> {
        self.entries
            .iter()
            .find(|(path, _)| path == file)
            .map(|(_, info)| info)
    }

    /// `map.set`: у существующего ключа место прежнее.
    fn set(&mut self, file: String, info: DumpInfo) {
        match self.entries.iter_mut().find(|(path, _)| *path == file) {
            Some(entry) => entry.1 = info,
            None => self.entries.push((file, info)),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &DumpInfo)> {
        self.entries
            .iter()
            .map(|(path, info)| (path.as_str(), info))
    }
}

/// `snapshotDumps(installPath, env, gameId)`.
pub fn snapshot_dumps(
    install_path: Option<&str>,
    env: &Environment,
    game_id: Option<&str>,
) -> DumpSnapshot {
    let path = NodePath::native();
    let mut snapshot = DumpSnapshot::default();
    let prefix = process_prefix_of(&path, game_id);

    for DumpDir { id, dir } in dump_directories_on(&path, install_path, env, game_id) {
        let Ok(entries) = read_dir(Path::new(&dir)) else {
            continue;
        };
        for entry in entries {
            if !entry.file_type.is_file()
                || !is_dump_file(&entry.name)
                || !belongs_to_game(id, &entry.name, prefix.as_deref())
            {
                continue;
            }
            let full = path.join(&[&dir, &entry.name]);
            // Файл исчез между readdir и stat.
            let Ok(meta) = std::fs::metadata(&full) else {
                continue;
            };
            let Ok(mtime_ms) = mtime_ms(&meta) else {
                continue;
            };
            snapshot.set(
                full,
                DumpInfo {
                    size: meta.len(),
                    mtime_ms,
                    zone: id,
                },
            );
        }
    }
    snapshot
}

/// Найденный дамп: `{path, size, mtimeMs, zone}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Dump {
    pub path: String,
    pub size: u64,
    pub mtime_ms: f64,
    pub zone: &'static str,
}

/// `findNewDumps(installPath, snapshot, startedAtMs, env, gameId)`: новые и изменённые после
/// снимка дампы (не старше `started_at_ms` − 2 с); сначала папка мода, затем корень игры, затем
/// общие папки, внутри зоны — свежие первыми.
pub fn find_new_dumps(
    install_path: Option<&str>,
    snapshot: Option<&DumpSnapshot>,
    started_at_ms: Option<f64>,
    env: &Environment,
    game_id: Option<&str>,
) -> Vec<Dump> {
    let current = snapshot_dumps(install_path, env, game_id);
    let mut found = Vec::new();

    for (file, info) in current.iter() {
        let before = snapshot.and_then(|snapshot| snapshot.get(file));
        let changed = match before {
            None => true,
            Some(before) => before.size != info.size || before.mtime_ms != info.mtime_ms,
        };
        if !changed {
            continue;
        }
        // `startedAtMs && …`: 0 и отсутствие — без отсечки по времени.
        if let Some(started) = started_at_ms.filter(|started| *started != 0.0)
            && info.mtime_ms < started - 2000.0
        {
            continue;
        }
        found.push(Dump {
            path: file.to_owned(),
            size: info.size,
            mtime_ms: info.mtime_ms,
            zone: info.zone,
        });
    }

    hlsr_core::js::sort::sort_by_comparator(&mut found, |a, b| {
        let rank = f64::from(zone_rank(a.zone)) - f64::from(zone_rank(b.zone));
        if rank != 0.0 {
            rank
        } else {
            b.mtime_ms - a.mtime_ms
        }
    });
    found
}

/// Снимок перед запуском игры (`dumpSnapshots.set(gameId, …)`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DumpContext {
    pub snapshot: DumpSnapshot,
    pub game_id: Option<String>,
    pub install_path: Option<String>,
    pub started_at_ms: Option<f64>,
}

/// `leftNewDump(context, env)`: игра под Proton при падении часто выходит с кодом 0 (Wine не
/// передаёт код исключения), а Source успевает записать минидамп — свежий дамп после «штатного»
/// выхода и есть падение.
pub fn left_new_dump(context: Option<&DumpContext>, env: &Environment) -> bool {
    let Some(context) = context else {
        return false;
    };
    let Some(install_path) = context
        .install_path
        .as_deref()
        .filter(|path| !path.is_empty())
    else {
        return false;
    };
    !find_new_dumps(
        Some(install_path),
        Some(&context.snapshot),
        context.started_at_ms,
        env,
        context.game_id.as_deref(),
    )
    .is_empty()
}

/// Параметры `waitForDump` (`options`).
#[derive(Debug, Clone, Default)]
pub struct WaitOptions {
    /// `settleMs`; по умолчанию [`DUMP_SETTLE_MS`].
    pub settle_ms: Option<u64>,
    /// `pollMs`; по умолчанию [`DUMP_POLL_MS`].
    pub poll_ms: Option<u64>,
    /// `env`; по умолчанию — окружение процесса.
    pub env: Option<Environment>,
    pub game_id: Option<String>,
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_millis() as f64)
}

/// `waitForDump(installPath, snapshot, startedAtMs, options)`: дамп готов, когда его размер
/// не изменился между двумя проверками; к сроку — дампы ненулевого размера.
pub async fn wait_for_dump(
    install_path: Option<&str>,
    snapshot: Option<&DumpSnapshot>,
    started_at_ms: Option<f64>,
    options: &WaitOptions,
) -> Vec<Dump> {
    let settle_ms = options.settle_ms.unwrap_or(DUMP_SETTLE_MS);
    let poll_ms = options.poll_ms.unwrap_or(DUMP_POLL_MS);
    let env = options.env.clone().unwrap_or_else(Environment::process);
    let game_id = options.game_id.as_deref().filter(|id| !id.is_empty());
    let deadline = now_ms() + settle_ms as f64;
    let mut last_size: Option<u64> = None;

    loop {
        let found = find_new_dumps(install_path, snapshot, started_at_ms, &env, game_id);
        if let Some(first) = found.first()
            && last_size == Some(first.size)
            && first.size > 0
        {
            return found;
        }
        last_size = found.first().map(|dump| dump.size);
        if now_ms() >= deadline {
            return found.into_iter().filter(|dump| dump.size > 0).collect();
        }
        tokio::time::sleep(Duration::from_millis(poll_ms)).await;
    }
}

/// `CRASH_ARCHIVE_RE` (`/_crash-.*\.zip$/i`): архив сбоя, который собирает сама игра.
fn is_crash_archive(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".zip") && lower[..lower.len() - 4].contains("_crash-")
}

/// `crashArchiveOf(installPath, startedAtMs, gameId, env)`: Ghosting пакует
/// `ghosting_crash-<время>.txt` и `.dmp` в `.zip` с тем же именем в папке мода. Ищется не по
/// дампу: если дамп не записался, в архиве один лог. Самый новый после запуска (−2 с) или `None`.
pub fn crash_archive_of(
    install_path: Option<&str>,
    started_at_ms: Option<f64>,
    game_id: Option<&str>,
    env: &Environment,
) -> Option<String> {
    let path = NodePath::native();
    let mut newest: Option<(String, f64)> = None;
    for DumpDir { id, dir } in dump_directories_on(&path, install_path, env, game_id) {
        if id != "game-mod" {
            continue;
        }
        let Ok(entries) = read_dir(Path::new(&dir)) else {
            continue;
        };
        for entry in entries {
            if !is_crash_archive(&entry.name) {
                continue;
            }
            let full = path.join(&[&dir, &entry.name]);
            // Файл исчез между readdir и stat.
            let Ok(meta) = std::fs::metadata(&full) else {
                continue;
            };
            let Ok(mtime) = mtime_ms(&meta) else {
                continue;
            };
            let early = started_at_ms
                .filter(|started| *started != 0.0)
                .is_some_and(|started| mtime < started - 2000.0);
            if !meta.is_file() || meta.len() == 0 || early {
                continue;
            }
            if newest.as_ref().is_none_or(|(_, at)| mtime > *at) {
                newest = Some((full, mtime));
            }
        }
    }
    newest.map(|(file, _)| file)
}

/// `crashLogOf(dumpPath)`: текстовый лог краша рядом с дампом под тем же именем (Ghosting пишет
/// пару `ghosting_crash-<время>.txt` и `.dmp`). Его и просят прислать — он читается без отладчика.
/// Нет такого файла — `None` (тогда присылают сам дамп).
pub fn crash_log_of(dump_path: &str) -> Option<String> {
    if !is_dump_file(dump_path) {
        return None;
    }
    let extension = NodePath::native().extname(dump_path);
    let log = format!("{}.txt", &dump_path[..dump_path.len() - extension.len()]);
    std::fs::metadata(&log)
        .is_ok_and(|meta| meta.is_file())
        .then_some(log)
}

/// `fs.realpathSync`: путь с развёрнутыми ссылками, в Windows без `\\?\`.
fn real_path(path: &str) -> Option<String> {
    let real = std::fs::canonicalize(path).ok()?;
    let text = real.to_string_lossy().into_owned();
    if cfg!(windows) {
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return Some(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return Some(rest.to_owned());
        }
    }
    Some(text)
}

/// `isAllowedDumpPath(candidate, installPath, env)`: файл дампа внутри одной из папок поиска
/// (по настоящему пути, после ссылок), но не сама папка.
pub fn is_allowed_dump_path(
    candidate: &str,
    install_path: Option<&str>,
    env: &Environment,
) -> bool {
    if candidate.is_empty() {
        return false;
    }
    let Some(real) = real_path(candidate) else {
        return false;
    };
    let path = NodePath::native();
    if !is_dump_file(&real) && !is_crash_archive(&path.basename(&real)) {
        return false;
    }
    dump_directories_on(&path, install_path, env, None)
        .iter()
        .any(|DumpDir { dir, .. }| {
            let root = if PathBuf::from(dir).exists() {
                match real_path(dir) {
                    Some(root) => root,
                    None => return false,
                }
            } else {
                dir.clone()
            };
            is_inside_with(&path, &root, &real) && real != *dir
        })
}
