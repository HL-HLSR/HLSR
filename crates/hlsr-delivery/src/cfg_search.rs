//! Где движок ищет cfg игры (`cfg-search.js`).
//!
//! GoldSrc с языком Steam, отличным от английского, ставит папку «<мод>_<язык>»
//! (valve_WON_russian) перед папкой мода и туда же пишет config.cfg при выходе. Её config.cfg
//! перекрывает config.cfg мода, поэтому лаунчер должен читать и править именно его.
//!
//! Какая папка активна, решает язык клиента Steam (`HKCU\Software\Valve\Steam`, значение
//! `Language`): с ним игра запускается. Если язык не прочитать (Linux: игра в префиксе Proton
//! без клиента Steam), активной считается папка, чей config.cfg записан последним: при выходе
//! игра пишет его туда, откуда читает.

use crate::errors::{DeliveryError, Failure, RawError};
use crate::paths::assert_inside_with;
use crate::ports::{Registry, RegistryHive};
use crate::{game_binds, runs_scan};
use hlsr_core::js::fs::{mtime_ms, read_dir_names};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{Platform, errno};
use regress::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

const STEAM_KEY: &str = "Software\\Valve\\Steam";
/// Сколько секунд прочитанный язык Steam не перечитывается: `LANGUAGE_TTL_MS`.
const LANGUAGE_TTL: Duration = Duration::from_millis(30_000);

/// Строка значения в выводе `reg query`: `/^\s*Language\s+REG_\w+\s+(\S+)\s*$/im`
/// (без `i` у букв: флаг без `u` сводит регистр только у ASCII).
static LANGUAGE_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::with_flags(
        r"^\s*[lL][aA][nN][gG][uU][aA][gG][eE]\s+[rR][eE][gG]_\w+\s+(\S+)\s*$",
        "m",
    )
    .expect("выражение 0.4.7 разбирается")
});

/// Язык клиента Steam из реестра с кешем на 30 секунд (`readSteamLanguage`). Кешируется и
/// «не прочитан».
pub struct SteamLanguage {
    registry: Arc<dyn Registry>,
    platform: Platform,
    clock: Box<dyn Fn() -> Instant + Send + Sync>,
    cache: Mutex<Option<(Instant, Option<String>)>>,
}

impl SteamLanguage {
    /// Для этой ОС и настоящих часов.
    pub fn new(registry: Arc<dyn Registry>) -> SteamLanguage {
        SteamLanguage::with(registry, Platform::CURRENT, Box::new(Instant::now))
    }

    /// С другой ОС (`process.platform`) и часами — для проверок.
    pub fn with(
        registry: Arc<dyn Registry>,
        platform: Platform,
        clock: Box<dyn Fn() -> Instant + Send + Sync>,
    ) -> SteamLanguage {
        SteamLanguage {
            registry,
            platform,
            clock,
            cache: Mutex::new(None),
        }
    }

    /// Язык строчными буквами или `None`: не Windows, нет значения, значение не в виде
    /// одного слова.
    pub fn read(&self) -> Option<String> {
        if self.platform != Platform::Win32 {
            return None;
        }
        let now = (self.clock)();
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((at, value)) = cache.as_ref()
            && now.saturating_duration_since(*at) < LANGUAGE_TTL
        {
            return value.clone();
        }
        let value = self
            .registry
            .query(RegistryHive::CurrentUser, STEAM_KEY, "Language")
            .and_then(|output| {
                let found = LANGUAGE_LINE.find(&output)?;
                let range = found.group(1)?;
                Some(output[range].to_lowercase())
            });
        *cache = Some(((self.clock)(), value.clone()));
        value
    }
}

/// Откуда взять язык Steam для поиска папок (`options.language` в 0.4.7).
#[derive(Debug, Clone, Copy)]
pub enum LanguageOption<'a> {
    /// Ключа `language` нет: язык читается из реестра.
    Detect,
    /// Задан явно; `None` — «не прочитан» (`null`).
    Given(Option<&'a str>),
}

/// Поиск cfg: хранит чтение языка Steam.
pub struct CfgSearch {
    steam_language: SteamLanguage,
}

/// Папка мода игры: `<игра>/<папка cfg движка>`; ошибка — `PATH_INVALID`, если путь выходит
/// за папку установки.
pub fn mod_dir_of(
    install_path: &Path,
    game_directory: &str,
    engine: &str,
) -> Result<PathBuf, DeliveryError> {
    let path = NodePath::native();
    let install = install_path.to_string_lossy();
    let joined = path.join(&[&install, &game_directory.replace('/', path.sep())]);
    let game_dir = assert_inside_with(&path, &install, &joined, Some("Папка игры"))?;
    let sub = game_binds::engine_of(engine).cfg_dir;
    Ok(PathBuf::from(if sub.is_empty() {
        game_dir
    } else {
        path.join(&[&game_dir, sub])
    }))
}

/// Имена в каталоге без учёта регистра: игра под Windows и Wine его не различает.
#[derive(Debug)]
struct Listing {
    dir: PathBuf,
    /// Имя в нижнем регистре → имя на диске. Если на диске два имени с одним нижним регистром,
    /// остаётся последнее из списка `readdir`.
    by_lower: HashMap<String, String>,
}

fn listing(dir: &Path) -> Result<Listing, RawError> {
    let names = match read_dir_names(dir) {
        Ok(names) => names,
        Err(error) => {
            if !matches!(errno::code(&error).as_deref(), Some("ENOENT" | "ENOTDIR")) {
                return Err(RawError::fs(&error, "scandir", dir));
            }
            Vec::new()
        }
    };
    Ok(Listing {
        dir: dir.to_path_buf(),
        by_lower: names
            .into_iter()
            .map(|name| (name.to_lowercase(), name))
            .collect(),
    })
}

/// `mtimeMs` config.cfg в каталоге; `-1` — файла нет или он не читается.
fn config_time(entry: &Listing) -> f64 {
    let Some(real) = entry.by_lower.get("config.cfg") else {
        return -1.0;
    };
    std::fs::metadata(entry.dir.join(real))
        .and_then(|meta| mtime_ms(&meta))
        .unwrap_or(-1.0)
}

/// Файлы в папках поиска: путь, откуда движок выполнит cfg с этим именем, а для нового
/// файла — путь в папке мода.
#[derive(Debug)]
pub struct Files {
    pub dirs: Vec<PathBuf>,
    /// Последняя папка поиска: сюда создаются новые файлы.
    pub mod_dir: PathBuf,
    entries: Vec<Listing>,
}

impl Files {
    fn find(&self, name: &str) -> Option<&Listing> {
        let lower = name.to_lowercase();
        self.entries
            .iter()
            .find(|entry| entry.by_lower.contains_key(&lower))
    }

    /// Файл с этим именем (без учёта регистра) есть в одной из папок.
    pub fn exists(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// Путь файла `name`: из первой папки, где он есть, иначе — в папке мода.
    pub fn path_of(&self, name: &str) -> PathBuf {
        let path = NodePath::native();
        match self.find(name) {
            Some(hit) => {
                let real = &hit.by_lower[&name.to_lowercase()];
                PathBuf::from(path.join(&[&hit.dir.to_string_lossy(), real]))
            }
            None => PathBuf::from(path.join(&[&self.mod_dir.to_string_lossy(), name])),
        }
    }
}

/// `files(dirs)`: список файлов каждой папки (нет папки — пустой).
pub fn files(dirs: Vec<PathBuf>) -> Result<Files, RawError> {
    let entries = dirs
        .iter()
        .map(|dir| listing(dir))
        .collect::<Result<Vec<_>, _>>()?;
    let mod_dir = dirs.last().cloned().unwrap_or_default();
    Ok(Files {
        dirs,
        mod_dir,
        entries,
    })
}

/// `/^[a-z]+$/`.
fn is_language_word(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_lowercase())
}

impl CfgSearch {
    pub fn new(steam_language: SteamLanguage) -> CfgSearch {
        CfgSearch { steam_language }
    }

    /// Папки cfg в порядке поиска движка: языковая (если игра сейчас пишет туда), затем папка
    /// мода. Новые файлы создаются в последней.
    pub fn search_dirs(
        &self,
        install_path: &Path,
        game_directory: &str,
        engine: &str,
        language: LanguageOption,
    ) -> Result<Vec<PathBuf>, Failure> {
        let mod_dir = mod_dir_of(install_path, game_directory, engine)?;
        if engine != "goldsrc" {
            return Ok(vec![mod_dir]);
        }

        let language = match language {
            LanguageOption::Detect => self.steam_language.read(),
            LanguageOption::Given(value) => value.map(str::to_owned),
        };
        if let Some(language) = language.as_deref().filter(|l| is_language_word(l)) {
            if language == "english" {
                return Ok(vec![mod_dir]);
            }
            // Папку игра создаёт при первом выходе с этим языком; пока её нет, движок
            // читает config.cfg из папки мода.
            let wanted = format!(
                "{}_{language}",
                NodePath::native().basename(&game_directory.replace('\\', "/"))
            )
            .to_lowercase();
            let posix = NodePath::new(Platform::Linux, "");
            let found = runs_scan::language_dirs(install_path, game_directory)
                .into_iter()
                .find(|relative| posix.basename(relative).to_lowercase() == wanted);
            return Ok(match found {
                Some(relative) => vec![mod_dir_of(install_path, &relative, engine)?, mod_dir],
                None => vec![mod_dir],
            });
        }

        let mut best: Option<PathBuf> = None;
        let mut best_time = config_time(&listing(&mod_dir)?);
        for relative in runs_scan::language_dirs(install_path, game_directory) {
            let dir = mod_dir_of(install_path, &relative, engine)?;
            let time = config_time(&listing(&dir)?);
            if time > best_time {
                best = Some(dir);
                best_time = time;
            }
        }
        Ok(match best {
            Some(best) => vec![best, mod_dir],
            None => vec![mod_dir],
        })
    }

    /// `filesOf`: файлы в папках поиска игры.
    pub fn files_of(
        &self,
        install_path: &Path,
        game_directory: &str,
        engine: &str,
        language: LanguageOption,
    ) -> Result<Files, Failure> {
        let dirs = self.search_dirs(install_path, game_directory, engine, language)?;
        Ok(files(dirs)?)
    }
}
