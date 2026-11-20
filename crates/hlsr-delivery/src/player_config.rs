//! Единый файл игрока (`player-config.js` 0.4.9): HL1 — userconfig.cfg, Ghosting и HL2 —
//! autoexec.cfg. Лаунчер правит его на месте по разделам ([`cfg_document`]), а не держит рядом
//! слои hlsr-*.cfg.
//!
//! Файл читается и пишется байт в байт у всех движков (ruling R17): его текст — байты как latin1,
//! UTF-8 и cp1251 переживают любую правку. Свои строки (шаблон, персонализация, бинды) лаунчер
//! кладёт так, как их писала 0.4.8: GoldSrc — latin1, Source — UTF-8 ([`to_file_text`]).

use crate::bind_presets;
use crate::cfg_document::{self, Doc};
use crate::cfg_search::{CfgSearch, LanguageOption};
use crate::errors::{DeliveryError, Failure, RawError};
use crate::game_binds::{self, Encoding};
use crate::ghosting_default_autoexec::default_autoexec;
use crate::ghosting_personalization;
use crate::goldsrc_import_adopt::{adopt_imported, cvars_of};
use crate::goldsrc_personalization;
use crate::hl1_default_userconfig;
use crate::launch_args;
use crate::node_fs;
use hlsr_core::js::object::JsObject;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::json_text::{parse_js_json, stringify};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Разделы файла игрока по порядку.
pub const ORDER: [&str; 6] = [
    "rules",
    "settings",
    "personalization",
    "binds",
    "scripts",
    "own",
];

/// Отметка опроса «с нуля или свой конфиг» и перехода с 0.4.8 (от папки игры).
pub const SETUP_FILE: [&str; 2] = [".hlsr", "config-setup.json"];
const SETUP_CHOICES: [&str; 3] = ["scratch", "own", "skip"];
/// Блок extras Ghosting в разделе personalization.
pub const EXTRAS_BLOCK: &str = "ghosting-extras";
const HL2_SETTINGS: [&str; 4] = [
    "sv_max_queries_sec 9",
    "save_screenshot 0",
    "cl_showpos 1",
    "cl_showfps 1",
];
/// Команды extras Ghosting, у которых нет значения cvar (файлы спрея и чата).
const ASSET_EXTRAS: [&str; 2] = ["gh_spray_file", "gh_chat_connect"];
const BLOCK_VERBS: [&str; 4] = [
    "gh_spray_set",
    "gh_spray_clear",
    "gh_chat_connect",
    "gh_chat_disconnect",
];

/// Ошибка файла игрока: свой код (`CONFIG_UNSUPPORTED`, `CONFIG_CHANGED`, `CONFIG_SETUP_CHOICE`)
/// или ошибка файловой системы.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{message}")]
    Player { code: &'static str, message: String },
    /// В коробке: ошибка файловой системы крупная, а `Result` с ней ходит по всему модулю.
    #[error(transparent)]
    Failure(Box<Failure>),
}

impl Error {
    fn player(code: &'static str, message: impl Into<String>) -> Error {
        Error::Player {
            code,
            message: message.into(),
        }
    }

    /// `error.code`: свой код, код ошибки доставки или errno.
    pub fn code(&self) -> Option<String> {
        match self {
            Error::Player { code, .. } => Some((*code).to_owned()),
            Error::Failure(failure) => match failure.as_ref() {
                Failure::Delivery(error) => Some(error.code.as_str().to_owned()),
                Failure::Raw(raw) => raw.code.clone(),
            },
        }
    }

    /// `error.code || error.message` для журнала.
    pub fn code_or_message(&self) -> String {
        self.code().unwrap_or_else(|| self.to_string())
    }

    pub fn is(&self, code: &str) -> bool {
        matches!(self, Error::Player { code: own, .. } if *own == code)
    }
}

impl From<Failure> for Error {
    fn from(error: Failure) -> Error {
        Error::Failure(Box::new(error))
    }
}

impl From<RawError> for Error {
    fn from(error: RawError) -> Error {
        Failure::from(error).into()
    }
}

impl From<DeliveryError> for Error {
    fn from(error: DeliveryError) -> Error {
        Failure::from(error).into()
    }
}

/// Игра и папки, с которыми работает файл игрока. `platform` — ОС игры: от неё зависит бинд
/// удаления сейва в шаблоне Ghosting.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub search: &'a CfgSearch,
    pub install: &'a Path,
    pub game_id: &'a str,
    pub game_directory: &'a str,
    pub platform: Platform,
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn encoding_of(game_id: &str) -> Encoding {
    game_binds::engine_of(bind_presets::engine_for(game_id).unwrap_or("")).encoding
}

/// `toFileText(gameId, text)`: свои строки — байтами, какими их писала 0.4.8 (Source — UTF-8),
/// в виде текста файла (байты как latin1).
pub fn to_file_text(game_id: &str, text: &str) -> String {
    js::latin1_string(&node_fs::encode(text, encoding_of(game_id)))
}

/// `fromFileText(gameId, text)`: текст файла обратно, для значений в окне.
pub fn from_file_text(game_id: &str, text: &str) -> String {
    node_fs::decode(&js::latin1_bytes(text), encoding_of(game_id))
}

fn trim_blank(lines: &[&str]) -> Vec<String> {
    let mut from = 0;
    let mut to = lines.len();
    while from < to && lines[from].is_empty() {
        from += 1;
    }
    while to > from && lines[to - 1].is_empty() {
        to -= 1;
    }
    lines[from..to]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

/// Стандартные строки шаблона по разделам.
fn bodies(game_id: &str, platform: Platform) -> Vec<(&'static str, Vec<String>)> {
    if game_id == "hl1-won" {
        let all: Vec<&str> = hl1_default_userconfig::text().split("\r\n").collect();
        let (first, rest) = all.split_first().expect("в тексте есть строки");
        // Хвост-подсказка игроку («Write your own…») — в раздел игрока, в settings только команды.
        let hint = rest
            .iter()
            .position(|line| line.starts_with("// Write your own"))
            .unwrap_or(rest.len());
        return vec![
            ("rules", vec![(*first).to_owned()]),
            ("settings", trim_blank(&rest[..hint])),
            ("own", trim_blank(&rest[hint..])),
        ];
    }
    if game_id == "ghosting-renewed" {
        let text = default_autoexec(platform);
        let lines: Vec<&str> = text.split("\r\n").collect();
        return vec![("settings", trim_blank(&lines))];
    }
    if bind_presets::engine_for(game_id).is_some() {
        return vec![(
            "settings",
            HL2_SETTINGS.iter().map(|line| (*line).to_owned()).collect(),
        )];
    }
    Vec::new()
}

/// `templateLines(gameId, names)`: разделы шаблона по порядку `names` — заголовок и стандартные
/// строки, между разделами пустая строка.
pub fn template_lines(game_id: &str, names: &[&str], platform: Platform) -> Vec<String> {
    let body = bodies(game_id, platform);
    let mut lines = Vec::new();
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            lines.push(String::new());
        }
        lines.push(cfg_document::header(name).to_owned());
        if let Some((_, own)) = body.iter().find(|(key, _)| key == name) {
            lines.extend(own.iter().cloned());
        }
    }
    lines
}

/// `template(gameId)`: свежий файл игрока — все шесть разделов по порядку, только заголовки и
/// стандартные строки.
pub fn template(game_id: &str, platform: Platform) -> String {
    template_lines(game_id, &ORDER, platform).join("\r\n") + "\r\n"
}

/// Где файл игрока.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub file: PathBuf,
    pub mod_dir: PathBuf,
    pub engine: &'static str,
    pub encoding: Encoding,
}

/// `locate(installPath, gameId, gameDirectory)`: `None` — у игры нет файла игрока.
pub fn locate(ctx: &Ctx) -> Result<Option<Located>, Failure> {
    let Some(engine) = bind_presets::engine_for(ctx.game_id) else {
        return Ok(None);
    };
    if ctx.game_directory.is_empty() {
        return Ok(None);
    }
    let files = ctx.search.files_of(
        ctx.install,
        ctx.game_directory,
        engine,
        LanguageOption::Detect,
    )?;
    let profile = game_binds::engine_of(engine);
    // Нового файла нет — он создаётся там, где игра держит config.cfg (языковая папка GoldSrc).
    let file = if files.exists(profile.exec_file) {
        files.path_of(profile.exec_file)
    } else {
        let first = files.dirs.first().cloned().unwrap_or_default();
        PathBuf::from(NodePath::native().join(&[&first.to_string_lossy(), profile.exec_file]))
    };
    let mod_dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok(Some(Located {
        file,
        mod_dir,
        engine,
        encoding: profile.encoding,
    }))
}

/// Прочитанный файл игрока: текст — байты как latin1; `sha256` — `None`, если файла нет.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub file: PathBuf,
    pub exists: bool,
    pub text: String,
    pub sha256: Option<String>,
}

/// `read(...)`: нет файла игрока у игры — `CONFIG_UNSUPPORTED`.
pub fn read(ctx: &Ctx) -> Result<Read, Error> {
    let Some(found) = locate(ctx)? else {
        return Err(Error::player(
            "CONFIG_UNSUPPORTED",
            format!("Нет файла игрока для {}", ctx.game_id),
        ));
    };
    Ok(match node_fs::read_bytes_optional(&found.file)? {
        None => Read {
            file: found.file,
            exists: false,
            text: String::new(),
            sha256: None,
        },
        Some(bytes) => Read {
            file: found.file,
            exists: true,
            text: js::latin1_string(&bytes),
            sha256: Some(sha256_hex(&bytes)),
        },
    })
}

/// Записанный файл игрока.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub file: PathBuf,
    pub sha256: String,
}

fn random_hex() -> String {
    let mut bytes = [0u8; 4];
    // Сбой источника случайности — только менее уникальное имя временного файла.
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Запись через временный файл рядом и rename.
pub(crate) fn write_via_temp(target: &Path, temp: &Path, bytes: &[u8]) -> Result<(), RawError> {
    let result = node_fs::write_bytes(temp, bytes).and_then(|()| {
        std::fs::rename(temp, target)
            .map_err(|error| RawError::fs_with_dest(&error, "rename", temp, Some(target)))
    });
    if result.is_err() {
        // Временный файл уже убран или его не было.
        let _ = std::fs::remove_file(temp);
    }
    result
}

/// `write(...)`: через временный файл рядом и rename. `expected` — отпечаток, который вызывающий
/// видел при чтении: `None` — без проверки, `Some(None)` — файла не было; не совпал —
/// `CONFIG_CHANGED`, файл не трогаем.
pub fn write(ctx: &Ctx, text: &str, expected: Option<Option<&str>>) -> Result<Written, Error> {
    let current = read(ctx)?;
    if let Some(expected) = expected
        && expected != current.sha256.as_deref()
    {
        return Err(Error::player(
            "CONFIG_CHANGED",
            "Файл изменён с момента чтения",
        ));
    }
    let bytes = js::latin1_bytes(text);
    if let Some(dir) = current.file.parent() {
        node_fs::mkdir_recursive(dir)?;
    }
    let temp = PathBuf::from(format!(
        "{}.hlsr-{}-{}.tmp",
        current.file.to_string_lossy(),
        std::process::id(),
        random_hex()
    ));
    write_via_temp(&current.file, &temp, &bytes)?;
    Ok(Written {
        file: current.file,
        sha256: sha256_hex(&bytes),
    })
}

/// Строка exec файла лаунчера: `/^exec\s+hlsr-[a-z-]+\.cfg$/i`.
fn is_launcher_exec(line: &str) -> bool {
    crate::game_binds_install::is_launcher_exec(line)
}

/// `isStock(gameId, text)`: стоковый или шаблонный файл лаунчер вправе заменить; файл игрока —
/// нет. `text` — как его даёт [`read`] (байты как latin1).
pub fn is_stock(game_id: &str, text: &str, platform: Platform) -> bool {
    if js::trim(text).is_empty() || text == to_file_text(game_id, &template(game_id, platform)) {
        return true;
    }
    let normalized = text.replace("\r\n", "\n");
    let own: Vec<&str> = normalized
        .split('\n')
        .filter(|line| !is_launcher_exec(js::trim(line)))
        .collect();
    let own = own.join("\n");
    let own = own.trim_end_matches(js::is_whitespace);
    let hash = sha256_hex(&js::latin1_bytes(own));
    // Свой стандартный файл 0.4.8 тоже заменяем: в нём ещё нет разделов.
    let previous = match game_id {
        "hl1-won" => Some(hl1_default_userconfig::text().to_owned()),
        "ghosting-renewed" => Some(default_autoexec(platform)),
        _ => None,
    };
    if let Some(previous) = previous {
        let previous = to_file_text(game_id, &previous).replace("\r\n", "\n");
        if own == previous.trim_end_matches(js::is_whitespace) {
            return true;
        }
    }
    match game_id {
        "hl1-won" => hl1_default_userconfig::REPLACEABLE_SHA256.contains(&hash.as_str()),
        "ghosting-renewed" => {
            ghosting_personalization::STOCK_AUTOEXEC_SHA256.contains(&hash.as_str())
        }
        _ => false,
    }
}

/// Итог [`ensure`]: `created` — шаблон записан.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ensured {
    pub file: PathBuf,
    pub created: bool,
}

/// `ensure(...)`: шаблон вместо отсутствующего или стокового файла.
pub fn ensure(ctx: &Ctx) -> Result<Ensured, Error> {
    let current = read(ctx)?;
    if current.exists && !is_stock(ctx.game_id, &current.text, ctx.platform) {
        return Ok(Ensured {
            file: current.file,
            created: false,
        });
    }
    let text = to_file_text(ctx.game_id, &template(ctx.game_id, ctx.platform));
    // Шаблон уже на месте: перезапись сдвинула бы mtime, и открытый в «Файлах cfg» файл не
    // сохранился бы («изменился на диске»). Отступление от 0.4.9: там шаблон переписывался.
    if current.exists && current.text == text {
        return Ok(Ensured {
            file: current.file,
            created: false,
        });
    }
    match write(ctx, &text, Some(current.sha256.as_deref())) {
        Ok(_) => Ok(Ensured {
            file: current.file,
            created: true,
        }),
        // Файл успел создать или изменить кто-то другой — это нормально, запуск не ломаем.
        Err(error) if error.is("CONFIG_CHANGED") => Ok(Ensured {
            file: current.file,
            created: false,
        }),
        Err(error) => Err(error),
    }
}

/// Что известно о лаунчере при подготовке файла (`options` 0.4.9).
#[derive(Debug, Clone, Copy, Default)]
pub struct PrepareOptions<'a> {
    /// Настройки игры из хранилища (`settings`): по ним переход пишет тексты файлов 0.4.8.
    pub settings: Option<&'a Value>,
    /// Версия лаунчера: в отметку `migrated`.
    pub launcher_version: Option<&'a str>,
    /// Часы (мс) для имени копии; `None` — системные.
    pub now: Option<i64>,
}

/// Итог [`prepare`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub file: PathBuf,
    pub migrated: bool,
    pub created: bool,
}

/// `prepare(...)`: перед первыми записями — один раз переход с 0.4.8, иначе шаблон вместо
/// отсутствующего.
pub fn prepare(ctx: &Ctx, options: &PrepareOptions) -> Result<Prepared, Error> {
    if crate::config_migration::needs_migration(ctx)? {
        let result = crate::config_migration::migrate(ctx, options)?;
        return Ok(Prepared {
            file: result.file.unwrap_or_default(),
            migrated: result.migrated,
            created: false,
        });
    }
    let result = ensure(ctx)?;
    Ok(Prepared {
        file: result.file,
        migrated: false,
        created: result.created,
    })
}

fn setup_path(install: &Path) -> PathBuf {
    PathBuf::from(NodePath::native().join(&[
        &install.to_string_lossy(),
        SETUP_FILE[0],
        SETUP_FILE[1],
    ]))
}

/// `readSetupMark(installPath)`: отметка-объект или `None`.
pub(crate) fn read_setup_mark(install: &Path) -> Option<Map<String, Value>> {
    let bytes = std::fs::read(setup_path(install)).ok()?;
    let text = js::utf8_string(&bytes);
    // JSON.parse не принимает метку порядка байтов.
    if text.starts_with('\u{FEFF}') {
        return None;
    }
    match parse_js_json(&text).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// `setupState(...)`: опрос «с нуля или свой конфиг» нужен, пока нет отметки, файла игрока нет
/// или он стоковый, и установке 0.4.8 не нужен переход (его сделает запуск). Ничего не пишет.
pub fn setup_state(ctx: &Ctx) -> Result<bool, Error> {
    if read_setup_mark(ctx.install).is_some() || setup_path(ctx.install).exists() {
        return Ok(false);
    }
    if crate::config_migration::needs_migration(ctx)? {
        return Ok(false);
    }
    let current = read(ctx)?;
    Ok(!current.exists || is_stock(ctx.game_id, &current.text, ctx.platform))
}

/// `setupDone(..., choice)`: «с нуля» и «пропустить» готовят файл игрока, «свой» его не трогает
/// (импорт запишет сам). Отметка дописывается к уже стоящей (там может быть `migrated`).
pub fn setup_done(ctx: &Ctx, choice: &str, options: &PrepareOptions) -> Result<(), Error> {
    if !SETUP_CHOICES.contains(&choice) {
        return Err(Error::player(
            "CONFIG_SETUP_CHOICE",
            format!("Неизвестный ответ: {choice}"),
        ));
    }
    if choice != "own" {
        prepare(ctx, options)?;
    }
    let mut mark = read_setup_mark(ctx.install).unwrap_or_default();
    let now = options.now.unwrap_or_else(now_ms);
    mark.insert("choice".into(), json!(choice));
    mark.insert("at".into(), json!(crate::timing::iso_from_ms(now)));
    write_setup_mark(ctx.install, mark)
}

/// Отметка `.hlsr/config-setup.json` через временный файл: сбой посередине не оставляет битую
/// (битую читают как отсутствующую, и `migrated` терялся бы).
pub(crate) fn write_setup_mark(install: &Path, mark: Map<String, Value>) -> Result<(), Error> {
    let target = setup_path(install);
    let temp = PathBuf::from(format!(
        "{}.{}-{}.tmp",
        target.to_string_lossy(),
        std::process::id(),
        random_hex()
    ));
    if let Some(dir) = target.parent() {
        node_fs::mkdir_recursive(dir)?;
    }
    write_via_temp(
        &target,
        &temp,
        stringify(&Value::Object(mark), "  ").as_bytes(),
    )?;
    Ok(())
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

// ------------------------------------------------------------------- персонализация

/// Персонализация живёт в файле игрока: лаунчер читает оттуда значения и пишет только те, что
/// игрок изменил в окне. Строки берутся из генераторов `to_cfg_lines` (их вывод не меняется).
fn is_personalized(game_id: &str) -> bool {
    matches!(game_id, "hl1-won" | "ghosting-renewed")
}

/// `SNAPSHOT_EXEC`: `/exec\s+hlsr-personalization\.cfg/i` где угодно в тексте.
pub fn mentions_snapshot(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find("exec") {
        let start = from + at + 4;
        let rest = &lower[start..];
        let blanks = rest.len() - rest.trim_start_matches(js::is_whitespace).len();
        if blanks > 0 && rest[blanks..].starts_with("hlsr-personalization.cfg") {
            return true;
        }
        from = start;
    }
    false
}

fn starts_with_ascii_ci(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.is_char_boundary(prefix.len())
        && text[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn is_youtube_line(line: &str) -> bool {
    line.trim_start_matches(js::is_whitespace)
        .starts_with("// HLSR_YOUTUBE_URL ")
}

/// `BLOCK_LINE`: строка команды блока extras (где бы она ни стояла).
fn is_block_line(line: &str) -> bool {
    let rest = line.trim_start_matches(js::is_whitespace);
    BLOCK_VERBS.iter().any(|verb| {
        starts_with_ascii_ci(rest, verb)
            && rest[verb.len()..]
                .chars()
                .next()
                .is_none_or(js::is_whitespace)
    }) || starts_with_ascii_ci(rest, "// HLSR_YOUTUBE_URL ")
}

/// `/^\s*([^\s"]+)(?:\s+(\S.*))?$/`: глагол и значение строки.
fn verb_of(line: &str) -> Option<(&str, Option<&str>)> {
    let rest = line.trim_start_matches(js::is_whitespace);
    let length: usize = rest
        .chars()
        .take_while(|c| !js::is_whitespace(*c) && *c != '"')
        .map(char::len_utf8)
        .sum();
    if length == 0 {
        return None;
    }
    let (verb, after) = rest.split_at(length);
    if after.is_empty() {
        return Some((verb, None));
    }
    let value = after.trim_start_matches(js::is_whitespace);
    let terminator = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    if value.len() == after.len() || value.is_empty() || value.contains(terminator) {
        return None;
    }
    Some((verb, Some(value)))
}

/// Строки генератора в виде текста файла: cvar-строки и строки блока extras.
fn generated_lines(game_id: &str, settings: &Value) -> (Vec<String>, Vec<String>) {
    if game_id == "hl1-won" {
        let text = to_file_text(game_id, &goldsrc_personalization::to_cfg_lines(settings));
        // split(/\r?\n/): `\r` снимается только перед `\n`.
        let pieces: Vec<&str> = text.split('\n').collect();
        let last = pieces.len() - 1;
        let lines = pieces
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let line = if index < last {
                    line.strip_suffix('\r').unwrap_or(line)
                } else {
                    line
                };
                line.to_owned()
            })
            .collect();
        return (lines, Vec::new());
    }
    if game_id != "ghosting-renewed" {
        return (Vec::new(), Vec::new());
    }
    let personalization = if js::truthy(settings) {
        settings.get("personalization").unwrap_or(&Value::Null)
    } else {
        settings
    };
    let text = to_file_text(
        game_id,
        &ghosting_personalization::to_cfg_lines(personalization),
    );
    let (mut cvars, mut block) = (Vec::new(), Vec::new());
    for line in text.split('\n') {
        let to_block = is_youtube_line(line)
            || verb_of(line).is_some_and(|(verb, value)| {
                BLOCK_VERBS
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(verb))
                    || (value.is_none() && !verb.starts_with("//"))
            });
        if to_block {
            block.push(line.to_owned());
        } else {
            cvars.push(line.to_owned());
        }
    }
    (cvars, block)
}

/// `personalizationPairs(gameId, settings)`: cvar → значение.
pub fn personalization_pairs(game_id: &str, settings: &Value) -> JsObject<String> {
    cvars_of(&generated_lines(game_id, settings).0.join("\n"))
}

/// `extrasBlock(gameId, settings)`: строки блока extras Ghosting.
pub fn extras_block(game_id: &str, settings: &Value) -> Vec<String> {
    generated_lines(game_id, settings).1
}

fn object_of(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// `overlayPersonalization(gameId, stored, text)`: настройки `stored`, поверх которых лежит файл
/// игрока. Поля, которые задаёт cvar, берутся из файла, а если cvar в файле нет — это значение игры
/// по умолчанию (не из settings.json). Поля без cvar (паки, файлы спрея и YouTube) остаются.
pub fn overlay_personalization(game_id: &str, stored: &Value, text: &str) -> Value {
    if !is_personalized(game_id) || !js::truthy(stored) {
        return stored.clone();
    }
    let defaults = launch_args::default_settings().to_value();
    let from_file = cvars_of(text);
    let current = personalization_pairs(game_id, stored);
    let base = personalization_pairs(game_id, &defaults);
    if game_id == "hl1-won" {
        let lines: Vec<String> = current
            .iter()
            .filter_map(|(name, value)| {
                let wanted = from_file.get(name).or_else(|| base.get(name))?;
                (wanted != value).then(|| format!("{name} \"{wanted}\""))
            })
            .collect();
        if lines.is_empty() {
            return stored.clone();
        }
        let adopted = adopt_imported(
            &lines.join("\n"),
            &object_of(stored.get("personalization")),
            &object_of(stored.get("bxt")),
        );
        let mut out = stored.as_object().cloned().unwrap_or_default();
        out.insert(
            "personalization".into(),
            Value::Object(adopted.personalization),
        );
        out.insert("bxt".into(), Value::Object(adopted.bxt));
        return Value::Object(out);
    }
    let core: Vec<String> = base
        .iter()
        .map(|(name, value)| format!("{name} \"{}\"", from_file.get(name).unwrap_or(value)))
        .collect();
    let source = from_file_text(game_id, &format!("{}\n{text}", core.join("\n")));
    let Some(imported) = ghosting_personalization::import_from_text(&source) else {
        return stored.clone();
    };
    let mut imported = imported.to_value();
    let own = ghosting_personalization::sanitize_personalization(
        stored.get("personalization").unwrap_or(&Value::Null),
    )
    .to_value();
    if let (Some(Value::Object(extra)), Some(Value::Object(own_extra))) =
        (imported.get_mut("extra"), own.get("extra"))
    {
        for key in ASSET_EXTRAS {
            if !extra.contains_key(key)
                && let Some(value) = own_extra.get(key)
            {
                extra.insert(key.to_owned(), value.clone());
            }
        }
    }
    let cross_of = |look: &Value| -> Vec<(String, String)> {
        personalization_pairs(game_id, &json!({"personalization": look}))
            .iter()
            .filter(|(name, _)| name.starts_with("gh_cross"))
            .map(|(name, value)| (name.to_owned(), value.clone()))
            .collect()
    };
    let initial = ghosting_personalization::default_personalization().to_value();
    let preset = if cross_of(&imported) == cross_of(&own) {
        own["crossPreset"].clone()
    } else if cross_of(&imported) == cross_of(&initial) {
        initial["crossPreset"].clone()
    } else {
        json!("custom")
    };
    imported["crossPreset"] = preset;
    let result = ghosting_personalization::sanitize_personalization(&imported).to_value();
    if stringify(&result, "") == stringify(&own, "") {
        return stored.clone();
    }
    let mut out = stored.as_object().cloned().unwrap_or_default();
    out.insert("personalization".into(), result);
    Value::Object(out)
}

/// Настройки с поправкой на файл игрока и его отпечаток: `sha256` — `None` (`undefined`), если у
/// игры нет файла игрока; `Some(None)` — файла нет.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspected {
    pub settings: Value,
    pub sha256: Option<Option<String>>,
}

/// `inspectPersonalization(...)`: отпечаток есть у любой игры с файлом игрока (HL2 тоже:
/// «Клавиатура» пишет со сверкой).
pub fn inspect_personalization(ctx: &Ctx, stored: &Value) -> Result<Inspected, Error> {
    if bind_presets::engine_for(ctx.game_id).is_none() || ctx.game_directory.is_empty() {
        return Ok(Inspected {
            settings: stored.clone(),
            sha256: None,
        });
    }
    let current = read(ctx)?;
    Ok(Inspected {
        settings: overlay_personalization(ctx.game_id, stored, &current.text),
        sha256: Some(current.sha256),
    })
}

/// `readPersonalization(...)`.
pub fn read_personalization(ctx: &Ctx, stored: &Value) -> Result<Value, Error> {
    Ok(inspect_personalization(ctx, stored)?.settings)
}

/// Блок extras заменяется целиком; такие же команды вне блока (перенос с 0.4.8) убираются, иначе
/// старая строка gh_spray_set осталась бы и выполнялась при каждом запуске.
fn replace_extras_block(doc: &Doc, lines: &[String]) -> Doc {
    let out = if lines.is_empty() {
        cfg_document::remove_block(doc, EXTRAS_BLOCK)
    } else {
        cfg_document::set_block(doc, EXTRAS_BLOCK, lines, "personalization")
    };
    let block = cfg_document::find_block(&out, EXTRAS_BLOCK);
    let kept = out
        .lines
        .iter()
        .enumerate()
        .filter(|(index, line)| {
            !is_block_line(&line.text)
                || block
                    .as_ref()
                    .is_some_and(|block| *index >= block.begin && *index <= block.end)
        })
        .map(|(_, line)| line.clone())
        .collect();
    Doc { lines: kept, ..out }
}

/// `applyPersonalization(doc, gameId, previous, next, text, { explicit })`: в раздел
/// personalization идут только значения, которые отличаются у `previous` (каким его видело окно,
/// то есть с учётом файла) и `next`. `explicit` («Сбросить»): все значения `next` пишутся явными
/// строками, даже равные показанному умолчанию. Возвращает документ и изменённые cvar.
pub fn apply_personalization(
    doc: &Doc,
    game_id: &str,
    previous: &Value,
    next: &Value,
    text: &str,
    explicit: bool,
) -> (Doc, Vec<String>) {
    if !is_personalized(game_id) {
        return (doc.clone(), Vec::new());
    }
    let seen = overlay_personalization(game_id, previous, text);
    let before = personalization_pairs(game_id, &seen);
    let after = personalization_pairs(game_id, next);
    let mut changed = Vec::new();
    let mut out = doc.clone();
    for (name, value) in after.iter() {
        if !explicit && before.get(name) == Some(value) {
            continue;
        }
        let key = format!("cvar:{name}");
        if cfg_document::get(&out, &key).is_some_and(|present| &present.value == value) {
            continue;
        }
        out = cfg_document::set(&out, &key, value, "personalization");
        changed.push(name.to_owned());
    }
    let wanted = extras_block(game_id, next);
    if wanted.join("\n") != extras_block(game_id, &seen).join("\n") {
        out = replace_extras_block(&out, &wanted);
        changed.push(EXTRAS_BLOCK.to_owned());
    }
    (out, changed)
}

/// `syncSnapshot(...)`: hlsr-personalization.cfg остаётся снимком для скриптов гайда HL1 (exec в
/// игре). Переписывается, если лежит в папке мода или на него ссылается файл игрока (`force`).
pub fn sync_snapshot(ctx: &Ctx, text: &str, settings: &Value, force: bool) -> Result<bool, Error> {
    if bind_presets::engine_for(ctx.game_id) != Some("goldsrc") || !js::truthy(settings) {
        return Ok(false);
    }
    let files = ctx.search.files_of(
        ctx.install,
        ctx.game_directory,
        "goldsrc",
        LanguageOption::Detect,
    )?;
    let name = goldsrc_personalization::PERSONALIZATION_FILE;
    if !force && !files.exists(name) && !mentions_snapshot(text) {
        return Ok(false);
    }
    node_fs::mkdir_recursive(&files.mod_dir)?;
    node_fs::write_text(
        &files.path_of(name),
        &goldsrc_personalization::to_cfg_lines(settings),
        Encoding::Latin1,
    )?;
    Ok(true)
}

/// Итог [`write_personalization`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalizationWritten {
    pub file: PathBuf,
    pub changed: Vec<String>,
    pub sha256: Option<String>,
}

/// `writePersonalization(..., previous, next, { expectedSha256 })`: `previous` — настройки до
/// сохранения, `next` — после.
pub fn write_personalization(
    ctx: &Ctx,
    previous: &Value,
    next: &Value,
    expected: Option<Option<&str>>,
) -> Result<PersonalizationWritten, Error> {
    let current = read(ctx)?;
    let (doc, changed) = apply_personalization(
        &cfg_document::parse(&current.text),
        ctx.game_id,
        previous,
        next,
        &current.text,
        false,
    );
    let out = cfg_document::serialize(&doc);
    let (mut file, mut sha256) = (current.file.clone(), current.sha256.clone());
    if out != current.text {
        let expected = expected.unwrap_or(current.sha256.as_deref());
        let written = write(ctx, &out, Some(expected))?;
        file = written.file;
        sha256 = Some(written.sha256);
    }
    sync_snapshot(ctx, &out, next, false)?;
    Ok(PersonalizationWritten {
        file,
        changed,
        sha256,
    })
}
