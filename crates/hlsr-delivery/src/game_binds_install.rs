//! Бинды игры на диске (`game-binds-install.js`): чтение итоговых биндов для клавиатуры в
//! настройках и запись переназначений лаунчера в hlsr-binds.cfg (см. `game_binds`). Файлы
//! ищутся так же, как в игре: сначала языковая папка GoldSrc (valve_WON_russian), потом папка
//! мода (см. `cfg_search`).
//!
//! Тексты cfg HL1 читаются и пишутся побайтно (latin1), CRLF сохраняется, как у 0.4.7.
//! Ошибки файловых вызовов — [`RawError`] с кодом Node: по нему вызывающий пишет журнал.
//!
//! 0.4.9 пишет всё в файл игрока ([`apply_player_binds`], [`import_player_config`],
//! [`undo_import`], [`export_plan`], [`check_files`]). Путь 0.4.7 (`apply_binds` с hlsr-binds.cfg,
//! `install_import`, `remove_import`, `sync_import_trailer`) остаётся только для запуска
//! `hlsr-platform` и его тестов, пока запуск не перейдёт на `prepare_for_launch` (задача F2l).

use crate::bind_presets;
use crate::cfg_document;
use crate::cfg_import::{self, Output};
use crate::cfg_search::{CfgSearch, Files, LanguageOption};
use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::file_ops::rm_sync;
use crate::game_binds::{self, BindRead, Encoding};
use crate::hl1_guide;
use crate::node_fs::{self, decode};
use crate::paths::assert_inside_with;
use crate::player_config;
use hlsr_core::js::fs::{read_dir_names, read_file};
use hlsr_core::js::object::JsObject;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::json_text::{parse_js_json, stringify};
use regress::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

/// Хвост файла импорта: строки лаунчера ещё раз после импортированного конфига.
pub const IMPORT_TRAILER_MARK: &str =
    "// HLSR: launcher files once more, after the imported config";
/// Текст конфига без имени файла (`installImport` строкой).
const PLAIN_IMPORT_FILE: &str = "player.cfg";
/// Где лежит запись о подключённом импорте (от папки установки).
const IMPORT_RECORD: [&str; 2] = [".hlsr", "cfg-import.json"];
/// Где лежат копии файлов, которые импорт заменил.
const IMPORT_BACKUP: [&str; 2] = [".hlsr", "cfg-import-backup"];

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("выражение 0.4.7 разбирается")
}

/// Импорт прошлых версий лаунчера: `/^hlsr-import(-[a-z0-9_-]+)?\.cfg$/i`.
static LEGACY_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"^[hH][lL][sS][rR]-[iI][mM][pP][oO][rR][tT](-[a-zA-Z0-9_-]+)?\.[cC][fF][gG]$")
});
/// Строка exec файла лаунчера: `/^exec\s+hlsr-[a-z-]+\.cfg$/i`.
static LAUNCHER_EXEC: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^[eE][xX][eE][cC]\s+[hH][lL][sS][rR]-[a-zA-Z-]+\.[cC][fF][gG]$"));
/// Строка exec с файлом: `/^exec\s+"?([^";]+?)"?\s*$/i`.
static EXEC_FILE: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"^[eE][xX][eE][cC]\s+"?([^";]+?)"?\s*$"#));
/// Строка exec userconfig.cfg в config.cfg: `/^\s*exec\s+"?userconfig\.cfg"?\s*$/im`.
static EXEC_USERCONFIG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::with_flags(
        r#"^\s*[eE][xX][eE][cC]\s+"?[uU][sS][eE][rR][cC][oO][nN][fF][iI][gG]\.[cC][fF][gG]"?\s*$"#,
        "m",
    )
    .expect("выражение 0.4.7 разбирается")
});

pub(crate) fn is_launcher_exec(line: &str) -> bool {
    LAUNCHER_EXEC.find(line).is_some()
}

fn native() -> NodePath {
    NodePath::native()
}

fn join(parts: &[&str]) -> PathBuf {
    PathBuf::from(native().join(parts))
}

fn text_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// ------------------------------------------------------------------- чтение

/// Файл импорта по пути в папке мода. Игра под Windows и Wine не различает регистр: уже
/// лежащий scripts/jump.cfg — тот же файл, что scripts/Jump.cfg. Выход за папку мода —
/// `PATH_INVALID`.
pub(crate) fn import_target(files: &Files, relative: &str) -> Result<PathBuf, Failure> {
    if !relative.contains('/') {
        return Ok(files.path_of(relative));
    }
    let path = native();
    let mut dir = text_of(&files.mod_dir);
    for part in relative.split('/') {
        // Папки ещё нет — не беда.
        let names = read_dir_names(Path::new(&dir)).unwrap_or_default();
        let lower = part.to_lowercase();
        let chosen = names
            .iter()
            .find(|name| name.to_lowercase() == lower)
            .map_or(part, String::as_str);
        dir = path.join(&[&dir, chosen]);
    }
    let inside = assert_inside_with(&path, &text_of(&files.mod_dir), &dir, Some("Файл импорта"))?;
    Ok(PathBuf::from(inside))
}

/// Бинды игрока в той кодировке, в какой их пишет игра.
pub fn read_install_binds(
    search: &CfgSearch,
    install_path: &Path,
    game_directory: &str,
    engine: &str,
) -> BindRead {
    let profile = game_binds::engine_of(engine);
    let Ok(files) = search.files_of(install_path, game_directory, engine, LanguageOption::Detect)
    else {
        return game_binds::read_binds(|_| None, profile.roots);
    };
    game_binds::read_binds(
        |name| {
            let normalized = name.replace('\\', "/");
            let nested = normalized.trim_start_matches('/');
            if !nested.contains('/') && !files.exists(name) {
                return None;
            }
            // exec scripts/jump.cfg — файл в подпапке мода (импорт под своими путями).
            let file = if nested.contains('/') {
                import_target(&files, nested).ok()?
            } else {
                files.path_of(name)
            };
            let stat = std::fs::metadata(&file).ok()?;
            if !stat.is_file() || stat.len() > 1024 * 1024 {
                return None;
            }
            Some(decode(&read_file(&file).ok()?, profile.encoding))
        },
        profile.roots,
    )
}

// ------------------------------------------------------------------- hlsr-binds.cfg

/// Строка exec добавляется в конец файла один раз. Остальное в файле игрока лаунчер не
/// трогает. `true` — строка добавлена.
pub fn ensure_exec_line(
    file: &Path,
    exec_line: &str,
    encoding: Encoding,
) -> Result<bool, RawError> {
    let text = node_fs::read_text_optional(file, encoding)?.unwrap_or_default();
    if text
        .split('\n')
        .any(|line| js::trim(line).to_lowercase() == exec_line)
    {
        return Ok(false);
    }
    let eol = if text.contains("\r\n") || encoding == Encoding::Latin1 {
        "\r\n"
    } else {
        "\n"
    };
    let suffix = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        eol
    };
    node_fs::write_text(file, &format!("{text}{suffix}{exec_line}{eol}"), encoding)?;
    Ok(true)
}

/// Alias назначенных скриптов. Скриптам гайда HL1 подставляются чувствительность и m_pitch
/// игрока из config.cfg, а тем, что работают через game.cfg, — пустой auto_alias: до нажатия
/// клавиши скрипта загрузка карты ничего не делает.
pub(crate) fn script_aliases(
    search: &CfgSearch,
    install_path: &Path,
    game_id: &str,
    game_directory: &str,
    engine: &str,
    clean: &game_binds::Overrides,
    platform: Platform,
) -> (Vec<(String, String)>, bool) {
    let aliases = bind_presets::aliases_for(game_id, platform, clean);
    let auto_alias = bind_presets::needs_auto_alias(game_id, platform, clean);
    if !aliases
        .iter()
        .any(|(_, value)| hl1_guide::has_player_token(value))
        && !auto_alias
    {
        return (aliases, auto_alias);
    }
    let cvars = read_install_binds(search, install_path, game_directory, engine).cvars;
    let mut result: Vec<(String, String)> = aliases
        .iter()
        .map(|(name, value)| (name.clone(), hl1_guide::with_player_cvars(value, &cvars)))
        .collect();
    if auto_alias {
        result.insert(0, (hl1_guide::AUTO_ALIAS.to_owned(), String::new()));
    }
    (result, auto_alias)
}

/// Итог [`apply_binds`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyBinds {
    /// Игра без биндов или без папки игры.
    Skipped,
    /// hlsr-binds.cfg записан; `appended` — строка exec добавлена в userconfig.cfg или
    /// autoexec.cfg.
    Applied { file: PathBuf, appended: bool },
}

/// Пишет hlsr-binds.cfg и подключает его последней строкой userconfig.cfg (GoldSrc) или
/// autoexec.cfg (Source). `overrides` — переназначения из настроек игры, как их хранит JSON.
pub fn apply_binds(
    search: &CfgSearch,
    install_path: &Path,
    game_id: &str,
    game_directory: &str,
    overrides: &Value,
) -> Result<ApplyBinds, Failure> {
    let Some(engine) = bind_presets::engine_for(game_id) else {
        return Ok(ApplyBinds::Skipped);
    };
    if game_directory.is_empty() {
        return Ok(ApplyBinds::Skipped);
    }
    let profile = game_binds::engine_of(engine);
    let files = search.files_of(install_path, game_directory, engine, LanguageOption::Detect)?;
    node_fs::mkdir_recursive(&files.mod_dir)?;
    let file = files.path_of(game_binds::BINDS_FILE);
    let clean = game_binds::sanitize_overrides(overrides);
    let (aliases, auto_alias) = script_aliases(
        search,
        install_path,
        game_id,
        game_directory,
        engine,
        &clean,
        Platform::CURRENT,
    );
    let text = game_binds::to_cfg_lines(&clean, Some(engine), &aliases);
    node_fs::write_text(&file, &text, profile.encoding)?;
    let appended = ensure_exec_line(
        &files.path_of(profile.exec_file),
        game_binds::BINDS_EXEC_LINE,
        profile.encoding,
    )?;
    // game.cfg движок выполняет при каждой загрузке карты. Строку лаунчер только добавляет:
    // без назначенных скриптов пустой alias ничего не делает.
    if auto_alias {
        ensure_exec_line(
            &files.path_of(hl1_guide::GAME_CFG),
            hl1_guide::AUTO_ALIAS,
            profile.encoding,
        )?;
    }
    Ok(ApplyBinds::Applied { file, appended })
}

// ------------------------------------------------------------------- импорт cfg

/// Файл, подключённый импортом: путь от папки мода; `created` — файла не было до импорта,
/// иначе прежний лежит в `.hlsr/cfg-import-backup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedFile {
    pub path: String,
    pub created: bool,
}

/// Что подключено импортом.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRecord {
    pub files: Vec<ImportedFile>,
    pub execs: Vec<String>,
}

fn legacy_import_files(dir: &Path) -> Result<Vec<String>, RawError> {
    let names = match read_dir_names(dir) {
        Ok(names) => names,
        Err(error) if hlsr_core::js::errno::code(&error).as_deref() == Some("ENOENT") => Vec::new(),
        Err(error) => return Err(RawError::fs(&error, "scandir", dir)),
    };
    Ok(names
        .into_iter()
        .filter(|name| LEGACY_IMPORT.find(name).is_some())
        .collect())
}

fn record_path(install_path: &Path) -> PathBuf {
    join(&[&text_of(install_path), IMPORT_RECORD[0], IMPORT_RECORD[1]])
}

fn backup_root(install_path: &Path) -> PathBuf {
    join(&[&text_of(install_path), IMPORT_BACKUP[0], IMPORT_BACKUP[1]])
}

/// Что подключено импортом; `None` — импорта нет (записи нет, она не читается или не той
/// формы). Из записи берутся только безопасные имена файлов.
pub fn read_import_record(install_path: &Path) -> Option<ImportRecord> {
    let bytes = read_file(&record_path(install_path)).ok()?;
    let text = js::utf8_string(&bytes);
    // JSON.parse не принимает метку порядка байтов, в отличие от разбора файлов Store.
    if text.starts_with('\u{FEFF}') {
        return None;
    }
    let record = parse_js_json(&text).ok()?;
    let files = record.get("files")?.as_array()?;
    let execs = record.get("execs")?.as_array()?;
    let files: Vec<ImportedFile> = files
        .iter()
        .filter_map(|item| {
            let path = item.get("path")?.as_str()?;
            (cfg_import::is_safe_import_name(path) || cfg_import::is_engine_run(path)).then(|| {
                ImportedFile {
                    path: path.to_owned(),
                    created: item.get("created").is_some_and(js::truthy),
                }
            })
        })
        .collect();
    let execs = execs
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| files.iter().any(|item| item.path == *name))
        .map(str::to_owned)
        .collect();
    Some(ImportRecord { files, execs })
}

fn exec_line_of(name: &str) -> String {
    format!("exec \"{name}\"")
}

/// Строка — exec файла импорта: одного из подключённых или hlsr-import.cfg прошлых версий.
fn is_import_exec(line: &str, execs: &[String]) -> bool {
    let line = js::trim(line);
    let Some(found) = EXEC_FILE.find(line) else {
        return false;
    };
    let Some(group) = found.group(1) else {
        return false;
    };
    let name = line[group].replace('\\', "/").to_lowercase();
    execs.iter().any(|item| item.to_lowercase() == name) || name == game_binds::IMPORT_FILE
}

/// Что импортировать: готовые файлы из [`cfg_import::build_import`] или текст одного конфига.
#[derive(Debug, Clone, Copy)]
pub enum ImportSource<'a> {
    Text(&'a str),
    Outputs(&'a [Output]),
}

/// Итог [`install_import`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallImport {
    /// Не HL1 или нет папки игры.
    Unsupported,
    /// Набор файлов не годится.
    Invalid,
    /// Импорт записан; файлы — пути от папки мода.
    Ok { files: Vec<String> },
}

/// Импорт готового конфига HL1 (см. `cfg_import`). `execs` — что подключить из userconfig.cfg
/// (строка ставится перед строками лаунчера: его настройки и переназначения выполняются после
/// импорта); у [`ImportSource::Text`] не используется — подключается сам файл текста, как в
/// 0.4.7 (`options.execs` для строки не читается). Прошлый импорт сначала убирается.
pub fn install_import(
    search: &CfgSearch,
    install_path: &Path,
    game_id: &str,
    game_directory: &str,
    source: ImportSource,
    execs: &[String],
) -> Result<InstallImport, Failure> {
    if bind_presets::engine_for(game_id) != Some("goldsrc") || game_directory.is_empty() {
        return Ok(InstallImport::Unsupported);
    }
    let plain;
    let (list, execs): (&[Output], Vec<String>) = match source {
        ImportSource::Text(text) => {
            plain = [Output {
                file: PLAIN_IMPORT_FILE.to_owned(),
                text: game_binds::import_file_text(text, false),
                root: true,
            }];
            (&plain, vec![PLAIN_IMPORT_FILE.to_owned()])
        }
        ImportSource::Outputs(outputs) => (outputs, execs.to_vec()),
    };
    let names: Vec<String> = list.iter().map(|item| item.file.to_lowercase()).collect();
    let distinct: std::collections::HashSet<&String> = names.iter().collect();
    if list.is_empty()
        || distinct.len() != names.len()
        || !list.iter().all(|item| {
            cfg_import::is_safe_import_name(&item.file) || cfg_import::is_engine_run(&item.file)
        })
        || !list.iter().any(|item| !js::trim(&item.text).is_empty())
        || !execs
            .iter()
            .all(|name| names.contains(&name.to_lowercase()))
        || list
            .iter()
            .map(|item| item.text.encode_utf16().count())
            .sum::<usize>()
            > game_binds::MAX_IMPORT_BYTES
    {
        return Ok(InstallImport::Invalid);
    }

    remove_import(search, install_path, game_id, game_directory)?;

    let profile = game_binds::engine_of("goldsrc");
    let files = search.files_of(
        install_path,
        game_directory,
        "goldsrc",
        LanguageOption::Detect,
    )?;
    node_fs::mkdir_recursive(&files.mod_dir)?;
    let userconfig = files.path_of(profile.exec_file);
    // Строки лаунчера из userconfig.cfg: импортированный userconfig.cfg без них не выполнил
    // бы бинды и настройки лаунчера.
    let launcher_lines: Vec<String> = node_fs::read_text_optional(&userconfig, profile.encoding)?
        .unwrap_or_default()
        .replace("\r\n", "\n")
        .split('\n')
        .map(|line| js::trim(line).to_owned())
        .filter(|line| is_launcher_exec(line))
        .collect();
    let mut record_files: Vec<ImportedFile> = Vec::new();
    let backups = backup_root(install_path);
    let path = native();
    for item in list {
        let relative = item.file.replace('\\', "/");
        let target = import_target(&files, &relative)?;
        let previous = node_fs::read_text_optional(&target, Encoding::Latin1)?;
        if let Some(previous) = &previous {
            let backup = assert_inside_with(
                &path,
                &text_of(&backups),
                &path.join(&[
                    &text_of(&backups),
                    &relative.split('/').collect::<Vec<_>>().join(path.sep()),
                ]),
                Some("Копия файла"),
            )?;
            if let Some(dir) = Path::new(&backup).parent() {
                node_fs::mkdir_recursive(dir)?;
            }
            node_fs::write_text(Path::new(&backup), previous, Encoding::Latin1)?;
        }
        let mut text = item.text.replace("\r\n", "\n").replace('\n', "\r\n");
        let base = relative
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_lowercase();
        if base == "userconfig.cfg" {
            text = format!(
                "{}\r\n{}",
                text.trim_end_matches(js::is_whitespace),
                launcher_lines
                    .iter()
                    .map(|line| format!("{line}\r\n"))
                    .collect::<String>()
            );
        } else if base == "config.cfg" && EXEC_USERCONFIG.find(&text).is_none() {
            // config.cfg игры последней строкой выполняет userconfig.cfg: без неё не
            // выполнились бы файлы лаунчера.
            text = format!(
                "{}\r\nexec userconfig.cfg\r\n",
                text.trim_end_matches(js::is_whitespace)
            );
        }
        if let Some(dir) = target.parent() {
            node_fs::mkdir_recursive(dir)?;
        }
        node_fs::write_text(&target, &text, Encoding::Latin1)?;
        record_files.push(ImportedFile {
            path: relative,
            created: previous.is_none(),
        });
    }

    if !execs.is_empty() {
        let current =
            node_fs::read_text_optional(&userconfig, profile.encoding)?.unwrap_or_default();
        let mut lines: Vec<String> = current
            .replace("\r\n", "\n")
            .split('\n')
            .map(str::to_owned)
            .collect();
        let added: Vec<String> = execs.iter().map(|name| exec_line_of(name)).collect();
        match lines
            .iter()
            .position(|line| is_launcher_exec(js::trim(line)))
        {
            None => {
                while lines.last().is_some_and(String::is_empty) {
                    lines.pop();
                }
                lines.extend(added);
                lines.push(String::new());
            }
            Some(at) => {
                lines.splice(at..at, added);
            }
        }
        node_fs::write_text(&userconfig, &lines.join("\r\n"), profile.encoding)?;
    }
    let record = json!({
        "version": 1,
        "files": record_files
            .iter()
            .map(|item| json!({"path": item.path, "created": item.created}))
            .collect::<Vec<_>>(),
        "execs": execs,
    });
    let record_file = record_path(install_path);
    if let Some(dir) = record_file.parent() {
        node_fs::mkdir_recursive(dir)?;
    }
    node_fs::write_text(&record_file, &stringify(&record, "  "), Encoding::Utf8)?;
    sync_import_trailer(search, install_path, game_id, game_directory)?;
    Ok(InstallImport::Ok {
        files: record_files.into_iter().map(|item| item.path).collect(),
    })
}

/// cfg, который не помещается в свободную часть буфера команд, GoldSrc выполняет построчно и
/// перед каждой строкой доделывает остальной буфер (Cmd_Exec_f, в ReHLDS с пометкой «doesn't
/// obey the rule»). Для большого импорта это значит, что строки userconfig.cfg после exec
/// импорта выполняются раньше импорта, и его default_fov, прицел и бинды перекрывают
/// настройки лаунчера. Поэтому эти строки повторяются в конце последнего подключённого файла
/// импорта. Хвост обновляется при импорте и перед каждым запуском. `true` — файл переписан.
pub fn sync_import_trailer(
    search: &CfgSearch,
    install_path: &Path,
    game_id: &str,
    game_directory: &str,
) -> Result<bool, Failure> {
    if bind_presets::engine_for(game_id) != Some("goldsrc") || game_directory.is_empty() {
        return Ok(false);
    }
    let record = read_import_record(install_path);
    let profile = game_binds::engine_of("goldsrc");
    let files = search.files_of(
        install_path,
        game_directory,
        "goldsrc",
        LanguageOption::Detect,
    )?;
    let last = record
        .as_ref()
        .and_then(|record| record.execs.last())
        .map(String::as_str);
    let import_file = import_target(&files, last.unwrap_or(game_binds::IMPORT_FILE))?;
    let execs: Vec<String> = match &record {
        Some(record) => record.execs.clone(),
        None => vec![game_binds::IMPORT_FILE.to_owned()],
    };
    let text = node_fs::read_text_optional(&import_file, Encoding::Latin1)?;
    let exec_text =
        node_fs::read_text_optional(&files.path_of(profile.exec_file), profile.encoding)?;
    let (Some(text), Some(exec_text)) = (text, exec_text) else {
        return Ok(false);
    };
    let exec_lines: Vec<String> = exec_text
        .replace("\r\n", "\n")
        .split('\n')
        .map(|line| js::trim(line).to_owned())
        .collect();
    let at = exec_lines
        .iter()
        .rposition(|line| is_import_exec(line, &execs));
    let after: Vec<&str> = match at {
        None => Vec::new(),
        Some(at) => exec_lines[at + 1..]
            .iter()
            .map(String::as_str)
            .filter(|line| is_launcher_exec(line))
            .collect(),
    };
    let normalized = text.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mark = lines
        .iter()
        .position(|line| js::trim(line) == IMPORT_TRAILER_MARK);
    let body_lines = match mark {
        Some(mark) => &lines[..mark],
        None => &lines[..],
    };
    let body = body_lines.join("\n");
    let body = body.trim_end_matches(js::is_whitespace);
    let mut parts: Vec<&str> = vec![body];
    if !after.is_empty() {
        parts.push("");
        parts.push(IMPORT_TRAILER_MARK);
        parts.extend(after);
    }
    let next = format!("{}\r\n", parts.join("\r\n"));
    if next == text {
        return Ok(false);
    }
    node_fs::write_text(&import_file, &next, Encoding::Latin1)?;
    Ok(true)
}

/// Итог [`remove_import`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveImport {
    /// Не HL1 или нет папки игры.
    Unsupported,
    /// Импорт убран. `restored_userconfig` — только если userconfig.cfg игрока читался
    /// (`Some(true)`: прежний userconfig.cfg вернулся из копии).
    Ok { restored_userconfig: Option<bool> },
}

/// «Убрать конфиг»: файлы, созданные импортом, удаляются, заменённые возвращаются из копий,
/// строки exec уходят из userconfig.cfg.
pub fn remove_import(
    search: &CfgSearch,
    install_path: &Path,
    game_id: &str,
    game_directory: &str,
) -> Result<RemoveImport, Failure> {
    if bind_presets::engine_for(game_id) != Some("goldsrc") || game_directory.is_empty() {
        return Ok(RemoveImport::Unsupported);
    }
    let profile = game_binds::engine_of("goldsrc");
    let files = search.files_of(
        install_path,
        game_directory,
        "goldsrc",
        LanguageOption::Detect,
    )?;
    let record = read_import_record(install_path);
    let backups = backup_root(install_path);
    let path = native();
    let restored_userconfig = record.as_ref().is_some_and(|record| {
        record
            .files
            .iter()
            .any(|item| item.path.to_lowercase() == "userconfig.cfg" && !item.created)
    });
    for item in record.iter().flat_map(|record| record.files.iter()) {
        let target = import_target(&files, &item.path)?;
        let backup = assert_inside_with(
            &path,
            &text_of(&backups),
            &path.join(&[
                &text_of(&backups),
                &item.path.split('/').collect::<Vec<_>>().join(path.sep()),
            ]),
            Some("Копия файла"),
        )?;
        let backup = PathBuf::from(backup);
        if item.created {
            rm_sync(&target, false, 0, Duration::from_millis(100))?;
        } else if backup.exists() {
            if let Some(dir) = target.parent() {
                node_fs::mkdir_recursive(dir)?;
            }
            node_fs::copy_file(&backup, &target)?;
        }
    }
    rm_sync(&backups, true, 0, Duration::from_millis(100))?;
    rm_sync(
        &record_path(install_path),
        false,
        0,
        Duration::from_millis(100),
    )?;
    for name in legacy_import_files(&files.mod_dir)? {
        node_fs::unlink(&files.mod_dir.join(&name))?;
    }

    let file = files.path_of(profile.exec_file);
    let Some(current) = node_fs::read_text_optional(&file, profile.encoding)? else {
        return Ok(RemoveImport::Ok {
            restored_userconfig: None,
        });
    };
    let execs: Vec<String> = record.map(|record| record.execs).unwrap_or_default();
    let normalized = current.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let kept: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| !is_import_exec(line, &execs))
        .collect();
    if kept.len() != lines.len() {
        node_fs::write_text(&file, &kept.join("\r\n"), profile.encoding)?;
    }
    // Прежний userconfig.cfg мог быть без строк лаунчера: их вернёт следующее сохранение
    // настроек или запуск (apply_binds).
    Ok(RemoveImport::Ok {
        restored_userconfig: Some(restored_userconfig),
    })
}

// ------------------------------------------------------------------- файл игрока (0.4.9)

/// Что пишет [`apply_player_binds`] кроме биндов (`options` `applyBinds` 0.4.9).
#[derive(Debug, Clone, Copy, Default)]
pub struct ApplyOptions<'a> {
    /// Прежние переназначения: пишется только разница с ними (R15).
    pub previous: Option<&'a Value>,
    /// Удалить и правленые руками блоки скриптов, которые больше не нужны.
    pub remove_edited: bool,
    /// Настройки после сохранения: снимок для скриптов гайда и персонализация.
    pub settings: Option<&'a Value>,
    /// Настройки до сохранения: с ними персонализация идёт в ту же запись.
    pub previous_settings: Option<&'a Value>,
    /// Отпечаток файла, как его видело окно (`None` — `undefined`).
    pub expected: Option<Option<&'a str>>,
    /// «Сбросить» в «Персонализации»: умолчания пишутся явными строками.
    pub explicit_personalization: bool,
}

/// Итог [`apply_player_binds`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerBinds {
    /// Игра без биндов или без папки игры.
    Skipped,
    Applied {
        file: PathBuf,
        /// Блоки скриптов, правленые руками и оставленные в файле.
        kept_edited: Vec<String>,
        /// Изменённые строки персонализации.
        changed: Vec<String>,
        sha256: Option<String>,
    },
}

fn alias_line(name: &str, value: &str) -> String {
    if value.is_empty() {
        format!("alias {name}")
    } else {
        format!("alias {name} \"{value}\"")
    }
}

/// Строка alias с токенами игрока как выражение: токен — любое число.
fn line_matcher(line: &str) -> regex::Regex {
    let mut source = String::from("^");
    let mut rest = line;
    loop {
        let next = ["{sensitivity}", "{m_pitch}"]
            .iter()
            .filter_map(|token| rest.find(token).map(|at| (at, token.len())))
            .min();
        let Some((at, length)) = next else {
            source.push_str(&regex::escape(rest));
            break;
        };
        source.push_str(&regex::escape(&rest[..at]));
        source.push_str("-?[0-9.]+");
        rest = &rest[at + length..];
    }
    source.push('$');
    regex::Regex::new(&source).expect("экранированная строка разбирается")
}

/// `applyBinds` 0.4.9: правит файл игрока на месте — по строке на бинд, по блоку на alias
/// скрипта. Пишется только разница с `previous` (ruling R15): клавиша, чьё переназначение не
/// менялось, в файле не трогается. Пропавшее переназначение уходит из файла, а пропавший возврат
/// (`command = base`) — нет. Блок, который больше не нужен, но правлен руками, остаётся
/// (`kept_edited`), пока не передан `remove_edited`. Персонализация (если передана) — в ту же
/// запись.
pub fn apply_player_binds(
    ctx: &player_config::Ctx,
    overrides: &Value,
    options: &ApplyOptions,
) -> Result<PlayerBinds, player_config::Error> {
    let Some(engine) = bind_presets::engine_for(ctx.game_id) else {
        return Ok(PlayerBinds::Skipped);
    };
    if ctx.game_directory.is_empty() {
        return Ok(PlayerBinds::Skipped);
    }
    let profile = game_binds::engine_of(engine);
    let current = player_config::read(ctx)?;
    let clean = game_binds::sanitize_overrides(overrides);
    let before = game_binds::sanitize_overrides(options.previous.unwrap_or(&Value::Null));
    let key_name = |key: &str| {
        if key == ";" && engine != "goldsrc" {
            "semicolon".to_owned()
        } else {
            key.to_owned()
        }
    };
    let doc_key = |key: &str| format!("bind:{}", key_name(key).to_lowercase());

    let mut doc = cfg_document::parse(&current.text);
    for (key, entry) in before.iter() {
        if !clean.contains_key(key) && entry.command != entry.base {
            doc = cfg_document::remove_key(&doc, &doc_key(key));
        }
    }
    for (key, entry) in clean.iter() {
        if before
            .get(key)
            .is_some_and(|previous| previous.command == entry.command)
        {
            continue;
        }
        if entry.command.is_empty() {
            doc = cfg_document::insert_lines(
                &cfg_document::remove_key(&doc, &doc_key(key)),
                "binds",
                &[format!("unbind \"{}\"", key_name(key))],
            );
        } else {
            doc = cfg_document::set(
                &doc,
                &doc_key(key),
                &player_config::to_file_text(ctx.game_id, &entry.command),
                "binds",
            );
        }
    }

    let (aliases, auto_alias) = script_aliases(
        ctx.search,
        ctx.install,
        ctx.game_id,
        ctx.game_directory,
        engine,
        &clean,
        ctx.platform,
    );
    let mut wanted: Vec<(String, String)> = Vec::new();
    for (name, value) in &aliases {
        let line = alias_line(name, value);
        match wanted.iter_mut().find(|(id, _)| id == name) {
            Some(item) => item.1 = line,
            None => wanted.push((name.clone(), line)),
        }
    }
    // Свой alias run игрока (вне блока run) главнее нашего.
    if cfg_document::get(&cfg_document::remove_block(&doc, "run"), "alias:run").is_some() {
        wanted.retain(|(id, _)| id != "run");
    }
    // Что лаунчер писал по прежним переназначениям: шаблон строки, где значения игрока
    // (чувствительность, m_pitch) могли быть другими, чем сейчас в config.cfg.
    let mut wrote: Vec<(String, regex::Regex)> = Vec::new();
    if !before.is_empty() {
        let mut raw = bind_presets::aliases_for(ctx.game_id, ctx.platform, &before);
        if bind_presets::needs_auto_alias(ctx.game_id, ctx.platform, &before) {
            raw.insert(0, (hl1_guide::AUTO_ALIAS.to_owned(), String::new()));
        }
        for (name, value) in raw {
            let matcher = line_matcher(&alias_line(&name, &value));
            match wrote.iter_mut().find(|(id, _)| *id == name) {
                Some(item) => item.1 = matcher,
                None => wrote.push((name, matcher)),
            }
        }
    }
    let wrote_of = |id: &str| wrote.iter().find(|(name, _)| name == id).map(|(_, m)| m);
    let is_written = |block: Option<&cfg_document::Block>, id: &str| {
        block.is_some_and(|block| {
            block.body.len() == 1
                && wrote_of(id).is_some_and(|matcher| matcher.is_match(js::trim(&block.body[0])))
        })
    };
    let mut kept_edited = Vec::new();
    for id in cfg_document::block_ids(&doc) {
        // Блоки, о которых лаунчер не знает (переход, прошлые версии, руки), не трогаем.
        if wanted.iter().any(|(name, _)| *name == id) || wrote_of(&id).is_none() {
            continue;
        }
        if options.remove_edited || is_written(cfg_document::find_block(&doc, &id).as_ref(), &id) {
            doc = cfg_document::remove_block(&doc, &id);
        } else {
            kept_edited.push(id);
        }
    }
    // Нужный блок правленый руками не откатываем: обновляем, только если его нет или он ещё как
    // написал лаунчер.
    for (id, line) in &wanted {
        let block = cfg_document::find_block(&doc, id);
        if block.is_none() || is_written(block.as_ref(), id) {
            doc = cfg_document::set_block(&doc, id, std::slice::from_ref(line), "scripts");
        }
    }

    let mut changed = Vec::new();
    if let (Some(previous), Some(settings)) = (options.previous_settings, options.settings) {
        let (next, lines) = player_config::apply_personalization(
            &doc,
            ctx.game_id,
            previous,
            settings,
            &current.text,
            options.explicit_personalization,
        );
        doc = next;
        changed = lines;
    }

    let out = cfg_document::serialize(&doc);
    let (mut file, mut sha256) = (current.file.clone(), current.sha256.clone());
    if out != current.text {
        let expected = options.expected.unwrap_or(current.sha256.as_deref());
        let written = player_config::write(ctx, &out, Some(expected))?;
        file = written.file;
        sha256 = Some(written.sha256);
    }

    let files = ctx.search.files_of(
        ctx.install,
        ctx.game_directory,
        engine,
        LanguageOption::Detect,
    )?;
    // Скрипт гайда зовёт снимок (alias deffov), а при запуске он не выполняется.
    let wants_snapshot = wanted
        .iter()
        .any(|(_, line)| player_config::mentions_snapshot(line));
    if let Some(settings) = options.settings
        && (wants_snapshot || options.previous_settings.is_some())
    {
        player_config::sync_snapshot(ctx, &out, settings, wants_snapshot)?;
    }
    // game.cfg движок выполняет при каждой загрузке карты. Строку лаунчер только добавляет:
    // без назначенных скриптов пустой alias ничего не делает.
    if auto_alias {
        node_fs::mkdir_recursive(&files.mod_dir)?;
        ensure_exec_line(
            &files.path_of(hl1_guide::GAME_CFG),
            hl1_guide::AUTO_ALIAS,
            profile.encoding,
        )?;
    }
    Ok(PlayerBinds::Applied {
        file,
        kept_edited,
        changed,
        sha256,
    })
}

/// Копия файлов импорта: `.hlsr/backup/<ГГГГММДД-ЧЧММСС>/` и её `import.json`.
const IMPORT_MANIFEST: &str = "import.json";

/// `BACKUP_STAMP`: `/^\d{8}-\d{6}$/`.
fn is_backup_stamp(stamp: &str) -> bool {
    let bytes = stamp.as_bytes();
    bytes.len() == 15
        && bytes[8] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(at, byte)| at == 8 || byte.is_ascii_digit())
}

/// Копия файлов перед заменой.
struct Backup {
    dir: PathBuf,
    sha256: String,
}

/// Куда и когда пишутся копии импорта.
#[derive(Debug, Clone, Copy, Default)]
pub struct BackupOptions<'a> {
    /// Последняя копия импорта по играм (`player-config-backups.json` в папке лаунчера, не игры:
    /// `.hlsr/backup` туда может подложить чужой `.hlsrpack`). `None` — копии не запоминаются.
    pub backups_file: Option<&'a Path>,
    /// Часы (мс) для имени копии; `None` — системные.
    pub now: Option<i64>,
}

fn player_backup_root(install: &Path) -> PathBuf {
    join(&[
        &text_of(install),
        crate::config_migration::BACKUP_DIR[0],
        crate::config_migration::BACKUP_DIR[1],
    ])
}

/// Папка копии `.hlsr/backup/<время>/`. Занятое имя (два импорта за секунду) сдвигается.
fn new_backup_dir(install: &Path, options: &BackupOptions) -> PathBuf {
    let base = options.now.unwrap_or_else(player_config::now_ms);
    let root = text_of(&player_backup_root(install));
    let mut shift = 0;
    loop {
        let dir = join(&[&root, &crate::config_migration::stamp(base + shift)]);
        if !dir.exists() {
            return dir;
        }
        shift += 1000;
    }
}

/// Копия файлов в новой папке и список `import.json` `{kept, created}` (пути от папки игры):
/// файла нет — он в `created`, «Вернуть прежний» его удалит. Список пишется раньше замены файлов.
fn backup_files(
    install: &Path,
    files: &[PathBuf],
    options: &BackupOptions,
) -> Result<Backup, Failure> {
    let dir = new_backup_dir(install, options);
    let (mut kept, mut created) = (Vec::new(), Vec::new());
    for file in files {
        let relative = crate::config_migration::relative_to(install, file);
        let Some(bytes) = node_fs::read_bytes_optional(file)? else {
            created.push(relative);
            continue;
        };
        let copy = join(&[&text_of(&dir), &relative]);
        if let Some(parent) = copy.parent() {
            node_fs::mkdir_recursive(parent)?;
        }
        node_fs::write_bytes(&copy, &bytes)?;
        kept.push(relative);
    }
    node_fs::mkdir_recursive(&dir)?;
    let manifest = stringify(&json!({"kept": kept, "created": created}), "  ");
    node_fs::write_bytes(&dir.join(IMPORT_MANIFEST), manifest.as_bytes())?;
    Ok(Backup {
        dir,
        sha256: player_config::sha256_hex(manifest.as_bytes()),
    })
}

fn remember_backup(options: &BackupOptions, game_id: &str, backup: &Backup) -> Result<(), Failure> {
    let Some(file) = options.backups_file else {
        return Ok(());
    };
    let mut all = match hlsr_core::json_store::read_json_safe(file) {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    let stamp = NodePath::native().basename(&text_of(&backup.dir));
    all.insert(
        game_id.to_owned(),
        json!({"stamp": stamp, "sha256": backup.sha256}),
    );
    crate::atomic::write_json_atomic(file, &Value::Object(all))?;
    Ok(())
}

/// Путь из `import.json` от `base`: без «..», абсолютных путей, «:» и ссылок по дороге.
fn plain_path(base: &Path, relative: &Value) -> Result<PathBuf, Failure> {
    let path = native();
    let invalid =
        |message: String| Failure::Delivery(DeliveryError::new(Code::PathInvalid, message));
    let text = relative.as_str().unwrap_or_default();
    let shown = match relative {
        Value::String(text) => text.clone(),
        other => crate::js_value::template(Some(other)),
    };
    let parts: Vec<&str> = if relative.is_string() {
        text.split(['\\', '/']).collect()
    } else {
        Vec::new()
    };
    if parts.is_empty()
        || path.is_absolute(text)
        || parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == ".." || part.contains(':'))
    {
        return Err(invalid(format!("Недопустимый путь в копии: {shown}")));
    }
    let base_text = text_of(base);
    for count in 1..=parts.len() {
        let mut pieces = vec![base_text.as_str()];
        pieces.extend_from_slice(&parts[..count]);
        let step = path.join(&pieces);
        if std::fs::symlink_metadata(&step).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(invalid(format!("Ссылка в пути копии: {shown}")));
        }
    }
    let mut pieces = vec![base_text.as_str()];
    pieces.extend_from_slice(&parts);
    let full = path.join(&pieces);
    Ok(PathBuf::from(assert_inside_with(
        &path,
        &base_text,
        &full,
        Some("Файл копии"),
    )?))
}

/// Импорт одним файлом (`flattenImport` 0.4.9): текст файла игрока и файлы, которые он зовёт из
/// alias и bind (ложатся в папку мода). Тексты — байты выбранных файлов как latin1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flattened {
    pub text: String,
    /// `(file, text)`: путь от папки мода и текст.
    pub runtime_files: Vec<(String, String)>,
}

/// Итог [`import_player_config`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportPlayer {
    Unsupported,
    Invalid,
    /// Файл изменён с тех пор, как его видело окно.
    Changed,
    Ok {
        sha256: String,
        file: PathBuf,
        /// Папка копии от папки игры.
        backup: String,
        /// Файлы скриптов (пути от папки мода).
        files: Vec<String>,
    },
}

/// `importPlayerConfig`: прежний файл игрока и заменяемые файлы скриптов уходят в
/// `.hlsr/backup/<время>/`, потом файлы скриптов ложатся в папку мода, и последним — новый файл
/// игрока. `expected` — отпечаток файла, как его видело окно.
pub fn import_player_config(
    ctx: &player_config::Ctx,
    built: &Flattened,
    expected: Option<Option<&str>>,
    options: &BackupOptions,
) -> Result<ImportPlayer, player_config::Error> {
    let Some(found) = player_config::locate(ctx)? else {
        return Ok(ImportPlayer::Unsupported);
    };
    let names: Vec<String> = built
        .runtime_files
        .iter()
        .map(|(file, _)| file.to_lowercase())
        .collect();
    let distinct: std::collections::HashSet<&String> = names.iter().collect();
    let size = built.text.encode_utf16().count()
        + built
            .runtime_files
            .iter()
            .map(|(_, text)| text.encode_utf16().count())
            .sum::<usize>();
    if js::trim(&built.text).is_empty()
        || distinct.len() != names.len()
        || !built.runtime_files.iter().all(|(file, _)| {
            cfg_import::is_safe_import_name(file)
                && !cfg_import::is_engine_run(file.rsplit('/').next().unwrap_or(file))
        })
        || size > game_binds::MAX_IMPORT_BYTES
    {
        return Ok(ImportPlayer::Invalid);
    }
    let files = ctx.search.files_of(
        ctx.install,
        ctx.game_directory,
        found.engine,
        LanguageOption::Detect,
    )?;
    let current = player_config::read(ctx)?;
    if let Some(expected) = expected
        && expected != current.sha256.as_deref()
    {
        return Ok(ImportPlayer::Changed);
    }
    // HL1 (rulings R12, R16): перед импортом — разделы шаблона «правила» (exec WON.cfg, если
    // импорт его не выполняет сам) и «настройки»; что импорт задаёт сам, ниже перекроет.
    // Заголовка «Your own» нет: разделы лаунчера при следующих записях дописываются в конец.
    let won = cfg_import::execs_of(&built.text, found.engine)
        .iter()
        .any(|exec| !exec.runtime && exec.target.key == "won.cfg");
    let head = if ctx.game_id == "hl1-won" {
        let names: &[&str] = if won {
            &["settings"]
        } else {
            &["rules", "settings"]
        };
        player_config::template_lines(ctx.game_id, names, ctx.platform)
    } else {
        Vec::new()
    };
    let rules = if head.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", head.join("\n"))
    };
    let crlf = found.encoding == Encoding::Latin1 || current.text.contains("\r\n");
    let with_eol = |text: &str| {
        let lf = text.replace("\r\n", "\n");
        if crlf { lf.replace('\n', "\r\n") } else { lf }
    };

    let mut targets = Vec::new();
    for (name, text) in &built.runtime_files {
        targets.push((
            name.clone(),
            text,
            import_target(&files, &name.replace('\\', "/"))?,
        ));
    }
    let mut to_copy = vec![current.file.clone()];
    to_copy.extend(targets.iter().map(|(_, _, file)| file.clone()));
    let backup = backup_files(ctx.install, &to_copy, options)?;
    remember_backup(options, ctx.game_id, &backup)?;

    // Тексты импорта — байты выбранных файлов как latin1: пишутся теми же байтами (R17).
    for (_, text, file) in &targets {
        if let Some(dir) = file.parent() {
            node_fs::mkdir_recursive(dir)?;
        }
        node_fs::write_bytes(file, &js::latin1_bytes(&with_eol(text)))?;
    }
    let written = player_config::write(
        ctx,
        &with_eol(&format!("{rules}{}", built.text)),
        Some(current.sha256.as_deref()),
    )?;
    Ok(ImportPlayer::Ok {
        sha256: written.sha256,
        file: written.file,
        backup: crate::config_migration::relative_to(ctx.install, &backup.dir),
        files: targets.into_iter().map(|(name, _, _)| name).collect(),
    })
}

/// Итог [`undo_import`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoImport {
    /// Своей копии нет.
    Nothing,
    Ok {
        sha256: Option<String>,
        backup: String,
    },
}

/// `undoImport` («Вернуть прежний»): последняя копия возвращается байт в байт, созданные импортом
/// файлы удаляются. Сначала текущие файлы уходят в новую копию (R18): правки после импорта не
/// теряются, и возврат можно вернуть. Использованная копия переименовывается в `<время>.undone`.
/// Берётся только копия, которую записал сам лаунчер (A-7: совпадает SHA-256 её `import.json`).
pub fn undo_import(
    ctx: &player_config::Ctx,
    options: &BackupOptions,
) -> Result<UndoImport, player_config::Error> {
    let known = options
        .backups_file
        .map(hlsr_core::json_store::read_json_safe)
        .and_then(|all| all.get(ctx.game_id).cloned())
        .filter(js::truthy);
    let Some(known) = known else {
        return Ok(UndoImport::Nothing);
    };
    let stamp = crate::js_value::template(known.get("stamp"));
    if !is_backup_stamp(&stamp) {
        return Ok(UndoImport::Nothing);
    }
    let dir = join(&[&text_of(&player_backup_root(ctx.install)), &stamp]);
    let Some(bytes) = node_fs::read_bytes_optional(&dir.join(IMPORT_MANIFEST))? else {
        return Ok(UndoImport::Nothing);
    };
    if known.get("sha256").and_then(Value::as_str) != Some(&player_config::sha256_hex(&bytes)) {
        return Ok(UndoImport::Nothing);
    }
    let broken = || {
        Failure::Delivery(DeliveryError::new(
            Code::PathInvalid,
            "Список копии повреждён",
        ))
    };
    let manifest = parse_js_json(&js::utf8_string(&bytes))
        .map_err(|_| Failure::Raw(RawError::new("Unexpected token in JSON")))?;
    let (Some(Value::Array(kept)), Some(Value::Array(created))) =
        (manifest.get("kept"), manifest.get("created"))
    else {
        return Err(broken().into());
    };
    let mut pairs = Vec::new();
    for relative in kept {
        pairs.push((
            plain_path(&dir, relative)?,
            plain_path(ctx.install, relative)?,
        ));
    }
    let mut removed = Vec::new();
    for relative in created {
        removed.push(plain_path(ctx.install, relative)?);
    }
    let mut current: Vec<PathBuf> = pairs.iter().map(|(_, target)| target.clone()).collect();
    current.extend(removed.iter().cloned());
    let next = backup_files(ctx.install, &current, options)?;
    for (source, target) in &pairs {
        if let Some(parent) = target.parent() {
            node_fs::mkdir_recursive(parent)?;
        }
        node_fs::copy_file(source, target)?;
    }
    for target in &removed {
        rm_sync(target, false, 0, Duration::from_millis(100))?;
    }
    let undone = PathBuf::from(format!("{}.undone", text_of(&dir)));
    std::fs::rename(&dir, &undone)
        .map_err(|error| RawError::fs_with_dest(&error, "rename", &dir, Some(&undone)))?;
    remember_backup(options, ctx.game_id, &next)?;
    let read = player_config::read(ctx)?;
    Ok(UndoImport::Ok {
        sha256: read.sha256,
        backup: crate::config_migration::relative_to(ctx.install, &dir),
    })
}

/// Файлы для проверки по правилам speedrun.com (только HL1): файл игрока и все cfg папки мода,
/// которые он может выполнить (exec на верхнем уровне и внутри alias и bind), как latin1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckFiles {
    Unsupported,
    /// Файла игрока нет.
    Nothing,
    Ok {
        /// Ключ корня (имя файла игрока в нижнем регистре).
        root: String,
        /// Ключ → текст.
        texts: JsObject<String>,
        /// Ключ → путь от папки игры с прямыми слэшами.
        paths: JsObject<String>,
    },
}

/// Сбор файлов `checkPlayerConfig`; саму проверку делает вызывающий (`check_config`).
pub fn check_files(ctx: &player_config::Ctx) -> Result<CheckFiles, player_config::Error> {
    if ctx.game_id != "hl1-won" {
        return Ok(CheckFiles::Unsupported);
    }
    let Some(found) = player_config::locate(ctx)? else {
        return Ok(CheckFiles::Unsupported);
    };
    let current = player_config::read(ctx)?;
    if !current.exists {
        return Ok(CheckFiles::Nothing);
    }
    let files = ctx.search.files_of(
        ctx.install,
        ctx.game_directory,
        found.engine,
        LanguageOption::Detect,
    )?;
    let relative =
        |file: &Path| crate::config_migration::relative_to(ctx.install, file).replace('\\', "/");
    let root = NodePath::native()
        .basename(&text_of(&current.file))
        .to_lowercase();
    let mut texts = JsObject::new();
    let mut paths = JsObject::new();
    texts.insert(root.clone(), current.text.clone());
    paths.insert(root.clone(), relative(&current.file));
    let mut queue = std::collections::VecDeque::from([current.text.clone()]);
    while texts.len() < cfg_import::MAX_FILES {
        let Some(next) = queue.pop_front() else {
            break;
        };
        for exec in cfg_import::execs_of(&next, found.engine) {
            let target = exec.target;
            if texts.contains_key(&target.key) || !cfg_import::is_safe_import_name(&target.name) {
                continue;
            }
            // Файла нет — проверка покажет «файл не найден».
            let Ok(file) = import_target(&files, &target.name) else {
                continue;
            };
            let Ok(meta) = std::fs::metadata(&file) else {
                continue;
            };
            if !meta.is_file() || meta.len() > 1024 * 1024 {
                continue;
            }
            let Ok(bytes) = std::fs::read(&file) else {
                continue;
            };
            let text = js::latin1_string(&bytes);
            texts.insert(target.key.clone(), text.clone());
            paths.insert(target.key.clone(), relative(&file));
            queue.push_back(text);
        }
    }
    Ok(CheckFiles::Ok { root, texts, paths })
}

/// Что экспортировать (`exportPlayerConfig` до окна выбора).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportPlan {
    Unsupported,
    Nothing,
    /// Один .cfg: файл игрока и имя по умолчанию для окна сохранения.
    Single {
        file: PathBuf,
        name: String,
    },
    /// Папка: файл игрока и файлы, которые он зовёт из alias и bind (`(имя, байты)`).
    Chain {
        file: PathBuf,
        chain: Vec<(String, Vec<u8>)>,
    },
}

/// Экспорт: файл игрока, который зовёт из alias и bind файлы папки мода, уходит в выбранную
/// папку вместе с ними (подпапки сохраняются), иначе — один .cfg. Байты не меняются.
pub fn export_plan(ctx: &player_config::Ctx) -> Result<ExportPlan, player_config::Error> {
    let Some(found) = player_config::locate(ctx)? else {
        return Ok(ExportPlan::Unsupported);
    };
    let current = player_config::read(ctx)?;
    if !current.exists {
        return Ok(ExportPlan::Nothing);
    }
    let files = ctx.search.files_of(
        ctx.install,
        ctx.game_directory,
        found.engine,
        LanguageOption::Detect,
    )?;
    let mut chain: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from([current.text.clone()]);
    while chain.len() < cfg_import::MAX_FILES {
        let Some(next) = queue.pop_front() else {
            break;
        };
        for exec in cfg_import::execs_of(&next, found.engine) {
            let target = exec.target;
            let base = target.key.rsplit('/').next().unwrap_or(&target.key);
            if !exec.runtime
                || seen.contains(&target.key)
                || cfg_import::is_engine_run(base)
                || !cfg_import::is_safe_import_name(&target.name)
            {
                continue;
            }
            seen.insert(target.key.clone());
            let file = import_target(&files, &target.name)?;
            // Файла нет — ничего не копируем.
            let bytes = std::fs::metadata(&file)
                .ok()
                .filter(std::fs::Metadata::is_file)
                .and_then(|_| std::fs::read(&file).ok());
            let Some(bytes) = bytes else {
                continue;
            };
            queue.push_back(node_fs::decode(&bytes, found.encoding));
            chain.push((target.name.clone(), bytes));
        }
    }
    if chain.is_empty() {
        let name = format!(
            "{}-{}",
            ctx.game_id,
            NodePath::native().basename(&text_of(&current.file))
        );
        return Ok(ExportPlan::Single {
            file: current.file,
            name,
        });
    }
    Ok(ExportPlan::Chain {
        file: current.file,
        chain,
    })
}

/// Экспорт одним файлом в выбранное место.
pub fn export_single(file: &Path, target: &Path) -> Result<(), Failure> {
    let bytes = node_fs::read_bytes(file)?;
    node_fs::write_bytes(target, &bytes)?;
    Ok(())
}

/// Экспорт папкой: ничего существующего не перезаписываем — всё ложится в новую подпапку
/// `<игра>-config-<время>[-N]` выбранной папки. Возвращает её и число файлов.
pub fn export_chain(
    game_id: &str,
    file: &Path,
    chain: &[(String, Vec<u8>)],
    chosen: &Path,
    now: i64,
) -> Result<(PathBuf, usize), Failure> {
    let path = native();
    let chosen_text = text_of(chosen);
    let name = format!("{game_id}-config-{}", crate::config_migration::stamp(now));
    let mut dir = join(&[&chosen_text, &name]);
    let mut n = 2;
    while dir.exists() {
        dir = join(&[&chosen_text, &format!("{name}-{n}")]);
        n += 1;
    }
    node_fs::mkdir_recursive(&dir)?;
    let dir_text = text_of(&dir);
    let put = |name: &str, bytes: &[u8]| -> Result<(), Failure> {
        let full = path.join(&[
            &dir_text,
            &name.split('/').collect::<Vec<_>>().join(path.sep()),
        ]);
        let target = PathBuf::from(assert_inside_with(
            &path,
            &dir_text,
            &full,
            Some("Файл экспорта"),
        )?);
        if let Some(parent) = target.parent() {
            node_fs::mkdir_recursive(parent)?;
        }
        node_fs::write_bytes(&target, bytes)?;
        Ok(())
    };
    put(&path.basename(&text_of(file)), &node_fs::read_bytes(file)?)?;
    for (name, bytes) in chain {
        put(name, bytes)?;
    }
    Ok((dir, chain.len() + 1))
}
