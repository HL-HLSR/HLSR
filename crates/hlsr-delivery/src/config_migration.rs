//! Переход с 0.4.8 (`config-migration.js` 0.4.9): файл игрока, hlsr-*.cfg лаунчера и импорт один
//! раз сливаются в файл игрока. Итог в игре тот же, что дала бы 0.4.8 при следующем запуске:
//! каждая строка exec заменяется текстом файла на том же месте, порядок выполнения не меняется.
//! Прежние файлы переносятся в `.hlsr/backup/migration-<дата>/`, последней ставится отметка
//! `.hlsr/config-setup.json`.

use crate::bind_presets;
use crate::cfg_document::{self, Line};
use crate::cfg_import::{self, ExecTarget};
use crate::cfg_search::{Files, LanguageOption};
use crate::errors::RawError;
use crate::game_binds::{self, Encoding};
use crate::game_binds_install::{self, IMPORT_TRAILER_MARK, is_launcher_exec};
use crate::game_config::line_commands;
use crate::ghosting_default_autoexec::default_autoexec;
use crate::ghosting_personalization;
use crate::goldsrc_personalization;
use crate::hl1_default_userconfig;
use crate::node_fs;
use crate::player_config::{self, Ctx, Error, Located, PrepareOptions, Read, read_setup_mark};
use chrono::{Datelike, Local, TimeZone, Timelike};
use hlsr_core::js::fs::read_dir_names;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform, errno};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Папка копий от папки игры.
pub const BACKUP_DIR: [&str; 2] = [".hlsr", "backup"];
const MAX_DEPTH: usize = 8;
/// Сколько байт файлов раскрывает один переход: k строк exec на следующий файл дали бы k^8 копий.
/// Сверх него exec остаётся строкой — игра выполнит файл на том же месте.
const MAX_EXPANDED_BYTES: usize = 4 * game_binds::MAX_IMPORT_BYTES;
/// Под Linux (Wine) «save :» сейв не удаляет: бинд GHOSTING_SAVE_DELETION под Linux.
const LINUX_SAVE_DELETION_BIND: &str = "bind h \"save /; reload\"";

/// Раздел файла игрока для текста файла 0.4.8 (`SECTION_OF`).
fn section_of(key: &str) -> Option<&'static str> {
    match key {
        goldsrc_personalization::PERSONALIZATION_FILE => Some("personalization"),
        game_binds::BINDS_FILE => Some("binds"),
        "hlsr-config.cfg" => Some("settings"),
        _ => None,
    }
}

/// `LAUNCHER_FILE`: `/^hlsr-[^/]*\.cfg$/i`.
fn is_launcher_file(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 9
        && bytes[..5].eq_ignore_ascii_case(b"hlsr-")
        && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".cfg")
        && !name[5..name.len() - 4].contains('/')
}

/// `BACKUP_NAME`: `/^migration-\d{8}-\d{6}$/`.
fn is_backup_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("migration-") else {
        return false;
    };
    let bytes = rest.as_bytes();
    bytes.len() == 15
        && bytes[8] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(at, byte)| at == 8 || byte.is_ascii_digit())
}

fn run_template_line() -> String {
    format!("alias run \"{}\"", bind_presets::HL1_RUN_ALIAS)
}

fn lines_of(text: &str) -> Vec<String> {
    cfg_document::parse(text)
        .lines
        .into_iter()
        .map(|line| line.text)
        .collect()
}

fn join(parts: &[&str]) -> PathBuf {
    PathBuf::from(NodePath::native().join(parts))
}

fn text_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `path.relative(installPath, file)` для файла внутри папки игры.
pub(crate) fn relative_to(install: &Path, file: &Path) -> String {
    file.strip_prefix(install)
        .map(text_of)
        .unwrap_or_else(|_| text_of(file))
}

/// `stamp(now)`: местное время `ГГГГММДД-ЧЧММСС`.
pub fn stamp(now: i64) -> String {
    match Local.timestamp_millis_opt(now).earliest() {
        Some(date) => format!(
            "{}{:02}{:02}-{:02}{:02}{:02}",
            date.year(),
            date.month(),
            date.day(),
            date.hour(),
            date.minute(),
            date.second()
        ),
        None => "NaNNaNNaN-NaNNaNNaN".to_owned(),
    }
}

struct Context<'a> {
    ctx: &'a Ctx<'a>,
    options: &'a PrepareOptions<'a>,
    found: Located,
    engine: &'static str,
    files: Files,
    /// exec импорта в файле игрока (нижний регистр).
    execs: HashSet<String>,
    /// Файлы записи импорта, которые раскрываются внутри (кроме тех, что движок выполняет сам).
    record_files: HashSet<String>,
    /// Байты, уже раскрытые [`Context::expand`] (предел — `MAX_EXPANDED_BYTES`).
    expanded: Cell<usize>,
}

fn context_of<'a>(
    ctx: &'a Ctx<'a>,
    options: &'a PrepareOptions<'a>,
) -> Result<Option<Context<'a>>, Error> {
    let Some(found) = player_config::locate(ctx)? else {
        return Ok(None);
    };
    let record = game_binds_install::read_import_record(ctx.install);
    let engine = found.engine;
    let files = ctx.search.files_of(
        ctx.install,
        ctx.game_directory,
        engine,
        LanguageOption::Detect,
    )?;
    let execs = record
        .iter()
        .flat_map(|record| record.execs.iter())
        .map(|name| name.to_lowercase())
        .collect();
    let record_files = record
        .iter()
        .flat_map(|record| record.files.iter())
        .map(|item| item.path.as_str())
        .filter(|name| !cfg_import::is_engine_run(name.rsplit('/').next().unwrap_or(name)))
        .map(str::to_lowercase)
        .collect();
    Ok(Some(Context {
        ctx,
        options,
        found,
        engine,
        files,
        execs,
        record_files,
        expanded: Cell::new(0),
    }))
}

impl Context<'_> {
    /// Текст файла — байты как latin1, как у файла игрока (R17): переход не меняет ни одного
    /// байта игрока. `None` — файла нет.
    fn read_text(&self, name: &str) -> Result<Option<String>, Error> {
        let file = game_binds_install::import_target(&self.files, name)?;
        match node_fs::read_bytes(&file) {
            Ok(bytes) => Ok(Some(js::latin1_string(&bytes))),
            Err(error) if matches!(error.code.as_deref(), Some("ENOENT" | "EISDIR")) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Текст файла 0.4.8 таким, каким она записала бы его перед запуском (её генераторы);
    /// `None` — генератора нет или нет настроек.
    fn generated_text(&self, key: &str) -> Option<String> {
        let settings = self
            .options
            .settings
            .filter(|settings| js::truthy(settings))?;
        if key == game_binds::BINDS_FILE {
            let clean =
                game_binds::sanitize_overrides(settings.get("binds").unwrap_or(&Value::Null));
            let (aliases, _) = game_binds_install::script_aliases(
                self.ctx.search,
                self.ctx.install,
                self.ctx.game_id,
                self.ctx.game_directory,
                self.engine,
                &clean,
                self.ctx.platform,
            );
            return Some(game_binds::to_cfg_lines(
                &clean,
                Some(self.engine),
                &aliases,
            ));
        }
        if key == goldsrc_personalization::PERSONALIZATION_FILE {
            if self.ctx.game_id == "hl1-won" {
                return Some(goldsrc_personalization::to_cfg_lines(settings));
            }
            if self.ctx.game_id == "ghosting-renewed"
                && let Some(look) = settings
                    .get("personalization")
                    .filter(|look| js::truthy(look))
            {
                return Some(ghosting_personalization::to_cfg_lines(look));
            }
        }
        None
    }

    /// Текст генератора — байтами, какими 0.4.8 записала бы файл (Source — UTF-8).
    fn launcher_text(&self, key: &str) -> Result<Option<String>, Error> {
        match self.generated_text(key) {
            Some(generated) => Ok(Some(player_config::to_file_text(
                self.ctx.game_id,
                &generated,
            ))),
            None => self.read_text(key),
        }
    }

    /// Строки вместо exec: заголовок и текст файла, exec в нём раскрыты так же. `Some([])` —
    /// строку убрать (файла нет, игра его и не выполнит), `None` — оставить.
    fn expand(
        &self,
        target: &ExecTarget,
        stack: &[String],
        top_level: bool,
    ) -> Result<Option<Vec<String>>, Error> {
        if let Some(section) = section_of(&target.key) {
            return Ok(Some(match self.launcher_text(&target.key)? {
                None => Vec::new(),
                Some(text) => {
                    let mut lines = vec![cfg_document::header(section).to_owned()];
                    lines.extend(without_header(&lines_of(&text)));
                    lines
                }
            }));
        }
        let launcher = is_launcher_file(&target.key);
        let known = if top_level {
            &self.execs
        } else {
            &self.record_files
        };
        if !launcher && !known.contains(&target.key) {
            return Ok(None);
        }
        if stack.contains(&target.key)
            || stack.len() >= MAX_DEPTH
            || self.expanded.get() >= MAX_EXPANDED_BYTES
        {
            return Ok(None);
        }
        let Some(text) = self.read_text(&target.name)? else {
            return Ok(launcher.then(Vec::new));
        };
        self.expanded.set(self.expanded.get() + text.len());
        let mut inner_stack = stack.to_vec();
        inner_stack.push(target.key.clone());
        let mut lines = vec![format!("// ===== from {} =====", target.name)];
        for line in without_trailer(&lines_of(&text)) {
            let expanded = match exec_target_of(&line, self.engine) {
                Some(next) => self.expand(&next, &inner_stack, false)?,
                None => None,
            };
            match expanded {
                Some(expanded) => lines.extend(expanded),
                None => lines.push(line),
            }
        }
        Ok(Some(lines))
    }

    fn calls_launcher(&self, text: &str) -> bool {
        cfg_document::parse(text).lines.iter().any(|line| {
            exec_target_of(&line.text, self.engine)
                .is_some_and(|target| is_launcher_file(&target.key))
        })
    }

    /// Строки игрока остаются как есть (те же байты и переводы строк), меняются только строки exec.
    fn merged_text(&self, text: &str) -> Result<String, Error> {
        let doc = cfg_document::parse(text);
        let mut lines: Vec<Line> = Vec::new();
        for line in &doc.lines {
            let expanded = match exec_target_of(&line.text, self.engine) {
                Some(target) => self.expand(&target, &[], true)?,
                None => None,
            };
            let Some(expanded) = expanded else {
                lines.push(line.clone());
                continue;
            };
            if !expanded.is_empty()
                && lines
                    .last()
                    .is_some_and(|last| !js::trim(&last.text).is_empty())
            {
                lines.push(Line {
                    text: String::new(),
                    eol: doc.eol.clone(),
                });
            }
            let last = expanded.len().saturating_sub(1);
            for (index, item) in expanded.into_iter().enumerate() {
                lines.push(Line {
                    text: item,
                    eol: if index == last {
                        line.eol.clone()
                    } else {
                        doc.eol.clone()
                    },
                });
            }
        }
        let lines = without_shadowed_run(lines);
        Ok(cfg_document::serialize(&cfg_document::Doc { lines, ..doc }))
    }

    /// Что 0.4.8 сделала бы с файлом игрока перед запуском: стоковый — на свой стандартный со
    /// строками exec, под Linux у Ghosting — рабочий бинд удаления сейва, недостающие exec — в
    /// конец. Без настроек exec дописываются только для файлов, что лежат на диске.
    fn launched_text(&self, current: &Read) -> String {
        let game_id = self.ctx.game_id;
        let settings = self
            .options
            .settings
            .filter(|settings| js::truthy(settings));
        let mut text = current.text.clone();
        let normalized = text.replace("\r\n", "\n");
        let lines: Vec<&str> = normalized.split('\n').collect();
        let defaults = match game_id {
            "hl1-won" => Some((
                hl1_default_userconfig::text().to_owned(),
                &hl1_default_userconfig::REPLACEABLE_SHA256[..],
            )),
            "ghosting-renewed" => Some((
                default_autoexec(self.ctx.platform),
                ghosting_personalization::STOCK_AUTOEXEC_SHA256,
            )),
            _ => None,
        };
        if let Some((default_text, stock)) = defaults {
            let own: Vec<&str> = lines
                .iter()
                .copied()
                .filter(|line| !is_launcher_exec(js::trim(line)))
                .collect();
            let own = own.join("\n");
            let own = own.trim_end_matches(js::is_whitespace);
            let hash = player_config::sha256_hex(&js::latin1_bytes(own));
            if !current.exists || stock.contains(&hash.as_str()) {
                let exec_lines: String = lines
                    .iter()
                    .map(|line| js::trim(line))
                    .filter(|line| is_launcher_exec(line))
                    .map(|line| format!("{line}\r\n"))
                    .collect();
                text = player_config::to_file_text(game_id, &default_text) + &exec_lines;
            }
        }
        if game_id == "ghosting-renewed" && self.ctx.platform == Platform::Linux {
            text = text
                .split('\n')
                .map(|line| {
                    if js::trim(line) != ghosting_personalization::WINDOWS_SAVE_DELETION_BIND {
                        return line.to_owned();
                    }
                    let indent = &line[..line.find("bind").unwrap_or(0)];
                    let cr = if line.ends_with('\r') { "\r" } else { "" };
                    format!("{indent}{LINUX_SAVE_DELETION_BIND}{cr}")
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
        // Порядок, в каком 0.4.8 дописывает exec: HL1 — персонализация, потом бинды; Source —
        // бинды, потом персонализация Ghosting.
        let mut execs = vec![game_binds::BINDS_EXEC_LINE];
        if game_id == "hl1-won" {
            execs.insert(0, goldsrc_personalization::EXEC_LINE);
        }
        if game_id == "ghosting-renewed"
            && settings
                .is_none_or(|settings| settings.get("personalization").is_some_and(js::truthy))
        {
            execs.push(goldsrc_personalization::EXEC_LINE);
        }
        for exec_line in execs {
            if settings.is_none() && !self.files.exists(&exec_line["exec ".len()..]) {
                continue;
            }
            let present = text.split('\n').any(|line| {
                js::trim(line.strip_suffix('\r').unwrap_or(line)).to_lowercase() == exec_line
            });
            if present {
                continue;
            }
            let eol = if text.contains("\r\n") || self.found.encoding == Encoding::Latin1 {
                "\r\n"
            } else {
                "\n"
            };
            let suffix = if text.is_empty() || text.ends_with('\n') {
                ""
            } else {
                eol
            };
            text = format!("{text}{suffix}{exec_line}{eol}");
        }
        text
    }

    /// Переход прервали после записи нового файла: в нём уже разделы лаунчера и нет exec его
    /// файлов. Шаблон не в счёт: его мог поставить ensure вместо стандартного файла 0.4.8.
    fn is_merged(&self, text: &str) -> bool {
        !player_config::is_stock(self.ctx.game_id, text, self.ctx.platform)
            && cfg_document::parse(text).lines.iter().any(|line| {
                cfg_document::SECTIONS
                    .iter()
                    .any(|(_, header)| *header == js::trim(&line.text))
            })
            && !self.calls_launcher(text)
    }

    fn launcher_files(&self) -> Result<Vec<(String, PathBuf)>, Error> {
        let mut result = Vec::new();
        for dir in &self.files.dirs {
            let names = match read_dir_names(dir) {
                Ok(names) => names,
                Err(error) => {
                    if !matches!(errno::code(&error).as_deref(), Some("ENOENT" | "ENOTDIR")) {
                        return Err(RawError::fs(&error, "scandir", dir).into());
                    }
                    Vec::new()
                }
            };
            for name in names {
                if is_launcher_file(&name) {
                    let file = join(&[&text_of(dir), &name]);
                    result.push((name, file));
                }
            }
        }
        Ok(result)
    }

    /// hlsr-*.cfg, которые файл игрока ещё зовёт (alias "exec hlsr-import-x.cfg" импорта
    /// 0.4.3), остаются на месте: без них скрипт игрока не сработает.
    fn still_used(
        &self,
        text: &str,
        candidates: &[(String, PathBuf)],
    ) -> Result<HashSet<String>, Error> {
        let mut used = HashSet::new();
        let mut queue = vec![text.to_owned()];
        while let Some(next) = queue.pop() {
            for exec in cfg_import::execs_of(&next, self.engine) {
                let key = exec.target.key;
                if used.contains(&key)
                    || !candidates
                        .iter()
                        .any(|(name, _)| name.to_lowercase() == key)
                {
                    continue;
                }
                let inner = self.read_text(&key)?.unwrap_or_default();
                used.insert(key);
                queue.push(without_trailer(&lines_of(&inner)).join("\n"));
            }
        }
        Ok(used)
    }

    /// Доводя прерванный переход, файлы кладём к копии прежнего файла игрока.
    fn backup_dir(&self, recovering: bool) -> Result<PathBuf, Error> {
        let install = text_of(self.ctx.install);
        let root = join(&[&install, BACKUP_DIR[0], BACKUP_DIR[1]]);
        if recovering {
            let mut names = match read_dir_names(&root) {
                Ok(names) => names,
                Err(error) => {
                    if errno::code(&error).as_deref() != Some("ENOENT") {
                        return Err(RawError::fs(&error, "scandir", &root).into());
                    }
                    Vec::new()
                }
            };
            names.retain(|name| is_backup_name(name));
            names.sort();
            if let Some(last) = names.last() {
                return Ok(join(&[&text_of(&root), last]));
            }
        }
        let now = self.options.now.unwrap_or_else(player_config::now_ms);
        Ok(join(&[
            &text_of(&root),
            &format!("migration-{}", stamp(now)),
        ]))
    }
}

/// Строка, в которой только exec одного файла: куда он ведёт, как его найдёт движок.
fn exec_target_of(text: &str, engine: &str) -> Option<ExecTarget> {
    let commands = line_commands(text);
    if commands.len() != 1 || commands[0].args.len() != 2 {
        return None;
    }
    let args = &commands[0].args;
    if args[0].value.to_lowercase() != "exec" {
        return None;
    }
    cfg_import::exec_target(&args[1].value, engine)
}

/// Пояснения 0.4.8 в начале её файлов («Generated by HLSR Launcher…») в файле игрока не к месту.
fn without_header(lines: &[String]) -> Vec<String> {
    match lines
        .iter()
        .position(|line| !js::trim(line).starts_with("//"))
    {
        Some(first) => lines[first..].to_vec(),
        None => Vec::new(),
    }
}

/// Хвост syncImportTrailer 0.4.8: повтор exec лаунчера после импорта.
fn without_trailer(lines: &[String]) -> Vec<String> {
    let Some(mark) = lines
        .iter()
        .position(|line| js::trim(line) == IMPORT_TRAILER_MARK)
    else {
        return lines.to_vec();
    };
    let mut end = mark;
    while end > 0 && js::trim(&lines[end - 1]).is_empty() {
        end -= 1;
    }
    lines[..end].to_vec()
}

/// alias run шаблона 0.4.8 перекрыт alias run ниже (из hlsr-binds.cfg) — остаётся последний.
fn without_shadowed_run(lines: Vec<Line>) -> Vec<Line> {
    let template = run_template_line();
    let is_run = |line: &Line| {
        line_commands(&line.text)
            .iter()
            .any(|command| cfg_document::key_of(&command.args).as_deref() == Some("alias:run"))
    };
    let runs: Vec<bool> = lines.iter().map(is_run).collect();
    lines
        .into_iter()
        .enumerate()
        .filter(|(index, line)| {
            js::trim(&line.text) != template || !runs[index + 1..].iter().any(|run| *run)
        })
        .map(|(_, line)| line)
        .collect()
}

/// `needsMigration(...)`: установке 0.4.8 нужен переход, пока нет отметки `migrated`.
pub fn needs_migration(ctx: &Ctx) -> Result<bool, Error> {
    let options = PrepareOptions::default();
    let Some(context) = context_of(ctx, &options)? else {
        return Ok(false);
    };
    if read_setup_mark(ctx.install).is_some_and(|mark| mark.contains_key("migrated")) {
        return Ok(false);
    }
    let record = join(&[&text_of(ctx.install), ".hlsr", "cfg-import.json"]);
    if !context.launcher_files()?.is_empty() || record.exists() {
        return Ok(true);
    }
    Ok(context.calls_launcher(&player_config::read(ctx)?.text))
}

/// Итог [`migrate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migrated {
    /// `migrated` или `skipped`.
    pub migrated: bool,
    /// Файл игрока; `None` — у игры его нет.
    pub file: Option<PathBuf>,
    pub backup_dir: Option<PathBuf>,
}

fn copy_into_backup(file: &Path, backup: &Path) -> Result<(), Error> {
    if let Some(dir) = backup.parent() {
        node_fs::mkdir_recursive(dir)?;
    }
    node_fs::copy_file(file, backup)?;
    Ok(())
}

/// `migrate(...)`. Порядок: копия прежнего файла игрока → новый файл → перенос → отметка. Копия
/// раньше нового файла: сбой между ними не теряет прежний файл.
pub fn migrate(ctx: &Ctx, options: &PrepareOptions) -> Result<Migrated, Error> {
    let context = match context_of(ctx, options)? {
        Some(context) if needs_migration(ctx)? => Some(context),
        _ => None,
    };
    let Some(context) = context else {
        let file = player_config::locate(ctx)?.map(|found| found.file);
        return Ok(Migrated {
            migrated: false,
            file,
            backup_dir: None,
        });
    };
    let current = player_config::read(ctx)?;
    let recovering = context.is_merged(&current.text);
    let backup_dir = context.backup_dir(recovering)?;
    let backup_of = |file: &Path| join(&[&text_of(&backup_dir), &relative_to(ctx.install, file)]);
    let mut text = current.text.clone();
    if !recovering {
        text = context.merged_text(&context.launched_text(&current))?;
        if current.exists {
            copy_into_backup(&current.file, &backup_of(&current.file))?;
        }
        if text != current.text {
            player_config::write(ctx, &text, Some(current.sha256.as_deref()))?;
        }
    }
    let candidates = context.launcher_files()?;
    let used = context.still_used(&text, &candidates)?;
    for (name, file) in &candidates {
        let key = name.to_lowercase();
        let backup = backup_of(file);
        if !used.contains(&key) {
            if let Some(dir) = backup.parent() {
                node_fs::mkdir_recursive(dir)?;
            }
            std::fs::rename(file, &backup)
                .map_err(|error| RawError::fs_with_dest(&error, "rename", file, Some(&backup)))?;
            continue;
        }
        // Оставленный файл лаунчера — снимок для скриптов, которые зовут его во время игры
        // (alias deffov "exec hlsr-personalization.cfg" гайда): пишем его, как 0.4.8 перед
        // запуском, туда, откуда его выполнит игра. Прежний текст — в копию (один раз).
        let Some(generated) = context.generated_text(&key) else {
            continue;
        };
        if text_of(file) != text_of(&context.files.path_of(&key)) {
            continue;
        }
        if !backup.exists() {
            copy_into_backup(file, &backup)?;
        }
        node_fs::write_text(file, &generated, context.found.encoding)?;
    }
    let mut setup = read_setup_mark(ctx.install).unwrap_or_default();
    setup.insert(
        "migrated".into(),
        options
            .launcher_version
            .filter(|version| !version.is_empty())
            .map_or(Value::Null, |version| json!(version)),
    );
    player_config::write_setup_mark(ctx.install, setup)?;
    Ok(Migrated {
        migrated: true,
        file: Some(current.file),
        backup_dir: Some(backup_dir),
    })
}
