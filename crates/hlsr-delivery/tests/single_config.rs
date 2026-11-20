//! Единый файл игрока против 0.4.9 (задача A): фикстура `single-config-049.json` из
//! `tools/reference/delivery/single-config-049.js`.
//!
//! - шаблоны файла игрока байт в байт;
//! - правки `cfg_document` (BOM, последняя строка без перевода, одиночный `\r`, unbindall, блоки);
//! - сценарии на временных папках: бинды и скрипты, персонализация (кириллица в `gh_name`,
//!   autoexec в cp1251), «Конфиги» со сверкой sha256, переход с 0.4.8 (файл владельца Ghosting,
//!   HL1 с импортом прошлых версий), импорт файлов владельца HL1 и «Вернуть прежний» (только своя
//!   копия, A-7), опрос. Сравниваются ответ шага, дерево (sha256 каждого файла) и текст файла
//!   игрока. Имена копий — местное время: с обеих сторон вместо них метка `<stamp>`.
//!
//! Ниже — самотесты 0.4.9 (`cfg-document`, `player-config`, `player-binds`,
//! `player-personalization`, `config-migration`), которых нет в сценариях.

mod support;

use hlsr_core::js::{self, Platform};
use hlsr_delivery::cfg_document as doc;
use hlsr_delivery::cfg_search::{CfgSearch, SteamLanguage};
use hlsr_delivery::config_migration;
use hlsr_delivery::game_binds_install::{
    self, ApplyOptions, BackupOptions, Flattened, ImportPlayer, PlayerBinds, UndoImport,
};
use hlsr_delivery::game_config::line_commands;
use hlsr_delivery::game_config_install::{self, WriteConfigError};
use hlsr_delivery::player_config::{self, Ctx, PrepareOptions};
use hlsr_delivery::ports::FakeRegistry;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use support::scenario::{path_in, temp_root};
use support::{Mismatches, fixture, items, sha256_hex, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("single-config-049.json"));

fn search() -> CfgSearch {
    CfgSearch::new(SteamLanguage::new(Arc::new(FakeRegistry::default())))
}

fn base64(text: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .unwrap()
}

fn ctx<'a>(search: &'a CfgSearch, install: &'a Path, game_id: &'a str, dir: &'a str) -> Ctx<'a> {
    Ctx {
        search,
        install,
        game_id,
        game_directory: dir,
        platform: Platform::Win32,
    }
}

// ---------------------------------------------------------------- шаблоны и документ

#[test]
fn templates_are_those_of_0_4_9_byte_for_byte() {
    for (game_id, bytes) in FIXTURE["templates"].as_object().unwrap() {
        let text = player_config::to_file_text(
            game_id,
            &player_config::template(game_id, Platform::Win32),
        );
        assert_eq!(js::latin1_bytes(&text), base64(str_of(bytes)), "{game_id}");
    }
}

#[test]
fn document_edits_are_those_of_0_4_9() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["documents"]) {
        let text = str_of(&case["text"]);
        let mut current = doc::parse(text);
        assert_eq!(doc::serialize(&current), text, "serialize(parse) {text:?}");
        for (index, op) in items(&case["ops"]).iter().enumerate() {
            let args = items(op);
            let arg = |n: usize| str_of(&args[n]);
            let lines = |n: usize| -> Vec<String> {
                items(&args[n])
                    .iter()
                    .map(|line| str_of(line).to_owned())
                    .collect()
            };
            current = match arg(0) {
                "set" => doc::set(&current, arg(1), arg(2), arg(3)),
                "removeKey" => doc::remove_key(&current, arg(1)),
                "insertLines" => doc::insert_lines(&current, arg(1), &lines(2)),
                "setBlock" => doc::set_block(&current, arg(1), &lines(2), arg(3)),
                "removeBlock" => doc::remove_block(&current, arg(1)),
                other => panic!("неизвестная правка {other}"),
            };
            mismatches.check(
                &json!(doc::serialize(&current)),
                &case["after"][index],
                format!("{text:?}: {op}"),
            );
        }
        mismatches.check(
            &json!(doc::block_ids(&current)),
            &case["blockIds"],
            format!("{text:?}: blockIds"),
        );
    }
    mismatches.finish("cfg-document");
}

// ---------------------------------------------------------------- сценарии

struct Run {
    _temp: tempfile::TempDir,
    temp: PathBuf,
    install: PathBuf,
    backups: PathBuf,
    search: CfgSearch,
    game_id: String,
    game_directory: String,
    stamps: Vec<String>,
}

fn files_under(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().unwrap().is_dir() {
            files_under(&entry.path(), &format!("{prefix}{name}/"), out);
        } else {
            out.push((format!("{prefix}{name}"), entry.path()));
        }
    }
}

impl Run {
    fn ctx(&self) -> Ctx<'_> {
        ctx(
            &self.search,
            &self.install,
            &self.game_id,
            &self.game_directory,
        )
    }

    fn unstamp(&self, text: &str) -> String {
        self.stamps.iter().fold(text.to_owned(), |text, stamp| {
            text.replace(stamp, "<stamp>")
        })
    }

    fn norm(&self, value: &Value) -> Value {
        let text = serde_json::to_string(value).unwrap();
        let root = serde_json::to_string(&self.temp.to_string_lossy()).unwrap();
        let text = text
            .replace(&root[1..root.len() - 1], "<root>")
            .replace("\\\\", "/");
        serde_json::from_str(&self.unstamp(&text)).unwrap()
    }

    fn tree(&self) -> Value {
        let mut files = Vec::new();
        files_under(&self.temp, "", &mut files);
        let mut out = Map::new();
        for (relative, path) in files {
            let mut bytes = std::fs::read(&path).unwrap();
            if relative.ends_with("player-config-backups.json") {
                bytes = self
                    .unstamp(&String::from_utf8(bytes).unwrap())
                    .into_bytes();
            }
            out.insert(self.unstamp(&relative), json!(sha256_hex(&bytes)));
        }
        Value::Object(out)
    }

    fn player(&self) -> Value {
        let found = player_config::locate(&self.ctx()).unwrap();
        match found.and_then(|found| std::fs::read(found.file).ok()) {
            Some(bytes) => json!(js::latin1_string(&bytes)),
            None => Value::Null,
        }
    }

    fn step(&mut self, step: &Value) -> Value {
        if let Some(now) = step["now"].as_i64() {
            for at in [now, now + 1000] {
                let stamp = config_migration::stamp(at);
                if !self.stamps.contains(&stamp) {
                    self.stamps.push(stamp);
                }
            }
        }
        let value = self.run(step).unwrap_or_else(|code| json!({"error": code}));
        json!({"value": self.norm(&value), "tree": self.tree(), "player": self.player()})
    }

    fn run(&self, step: &Value) -> Result<Value, String> {
        let ctx = self.ctx();
        let fail = |error: player_config::Error| error.code_or_message();
        let opt = |key: &str| step.get(key).filter(|value| !value.is_null());
        let now = step["now"].as_i64().or(FIXTURE["now"].as_i64());
        let backups = BackupOptions {
            backups_file: Some(&self.backups),
            now: step["now"].as_i64(),
        };
        let settings = opt("settings");
        Ok(match str_of(&step["op"]) {
            "ensure" => {
                let result = player_config::ensure(&ctx).map_err(fail)?;
                json!({"file": result.file, "created": result.created})
            }
            "prepare" => {
                let options = PrepareOptions {
                    settings,
                    launcher_version: Some("0.4.9"),
                    now,
                };
                let result = player_config::prepare(&ctx, &options).map_err(fail)?;
                json!({"file": result.file, "migrated": result.migrated, "created": result.created})
            }
            "applyBinds" => {
                let options = ApplyOptions {
                    previous: opt("previous"),
                    remove_edited: step["removeEdited"].as_bool().unwrap_or(false),
                    settings,
                    previous_settings: opt("previousSettings"),
                    expected: None,
                    explicit_personalization: step["explicit"].as_bool().unwrap_or(false),
                };
                match game_binds_install::apply_player_binds(&ctx, &step["binds"], &options)
                    .map_err(fail)?
                {
                    PlayerBinds::Skipped => json!({"status": "skipped", "keptEdited": []}),
                    PlayerBinds::Applied {
                        file,
                        kept_edited,
                        changed,
                        sha256,
                    } => json!({"status": "applied", "file": file, "keptEdited": kept_edited,
                                "changed": changed, "sha256": sha256}),
                }
            }
            "writePersonalization" => {
                let result = player_config::write_personalization(
                    &ctx,
                    &step["previous"],
                    &step["next"],
                    None,
                )
                .map_err(fail)?;
                json!({"file": result.file, "changed": result.changed, "sha256": result.sha256})
            }
            "inspect" => {
                let result =
                    player_config::inspect_personalization(&ctx, &step["stored"]).map_err(fail)?;
                let mut out = Map::new();
                out.insert("settings".into(), result.settings);
                if let Some(sha256) = result.sha256 {
                    out.insert("sha256".into(), json!(sha256));
                }
                Value::Object(out)
            }
            "needsMigration" => json!(config_migration::needs_migration(&ctx).map_err(fail)?),
            "migrate" => {
                let options = PrepareOptions {
                    settings,
                    launcher_version: Some("0.4.9"),
                    now,
                };
                let result = config_migration::migrate(&ctx, &options).map_err(fail)?;
                json!({"status": if result.migrated { "migrated" } else { "skipped" },
                       "file": result.file, "backupDir": result.backup_dir})
            }
            "import" => {
                let built = Flattened {
                    text: str_of(&step["built"]["text"]).to_owned(),
                    runtime_files: items(&step["built"]["runtimeFiles"])
                        .iter()
                        .map(|item| {
                            (
                                str_of(&item["file"]).to_owned(),
                                str_of(&item["text"]).to_owned(),
                            )
                        })
                        .collect(),
                };
                match game_binds_install::import_player_config(&ctx, &built, None, &backups)
                    .map_err(fail)?
                {
                    ImportPlayer::Unsupported => json!({"status": "unsupported"}),
                    ImportPlayer::Invalid => json!({"status": "invalid"}),
                    ImportPlayer::Changed => json!({"status": "changed"}),
                    ImportPlayer::Ok {
                        sha256,
                        file,
                        backup,
                        files,
                    } => json!({"status": "ok", "sha256": sha256, "file": file, "backup": backup,
                                "files": files}),
                }
            }
            "undo" => match game_binds_install::undo_import(&ctx, &backups).map_err(fail)? {
                UndoImport::Nothing => json!({"status": "nothing"}),
                UndoImport::Ok { sha256, backup } => {
                    json!({"status": "ok", "sha256": sha256, "backup": backup})
                }
            },
            "readConfig" => {
                let engine = hlsr_delivery::bind_presets::engine_for(&self.game_id).unwrap();
                match game_config_install::read_config(
                    &self.search,
                    &self.install,
                    &self.game_directory,
                    &self.game_id,
                    engine,
                )
                .map_err(fail)?
                {
                    None => Value::Null,
                    Some(result) => json!({
                        "values": result.values.iter().map(|(k, v)| (k.to_owned(), json!(v))).collect::<Map<_, _>>(),
                        "sources": result.sources.iter().map(|(k, v)| (k.to_owned(), json!(v))).collect::<Map<_, _>>(),
                        "files": result.files,
                        "sha256": result.sha256,
                        "playerFile": result.player_file,
                        "shadowed": result.shadowed,
                    }),
                }
            }
            "writeConfig" => {
                let engine = hlsr_delivery::bind_presets::engine_for(&self.game_id).unwrap();
                match game_config_install::write_config(
                    &self.search,
                    &self.install,
                    &self.game_directory,
                    &self.game_id,
                    engine,
                    &step["changes"],
                    None,
                ) {
                    Ok(result) => json!({"written": result.written, "sha256": result.sha256}),
                    Err(WriteConfigError::Config(error)) => {
                        return Err(error.code.as_str().to_owned());
                    }
                    Err(WriteConfigError::Player(error)) => return Err(fail(error)),
                    Err(WriteConfigError::Other(failure)) => return Err(failure.to_string()),
                }
            }
            "setupState" => json!({"needed": player_config::setup_state(&ctx).map_err(fail)?}),
            "setupDone" => {
                let choice = str_of(&step["choice"]);
                let options = PrepareOptions {
                    settings,
                    launcher_version: None,
                    now,
                };
                player_config::setup_done(&ctx, choice, &options).map_err(fail)?;
                json!({"choice": choice})
            }
            other => panic!("неизвестный шаг {other}"),
        })
    }
}

#[test]
fn scenarios_match_0_4_9_byte_for_byte() {
    let mut mismatches = Mismatches::default();
    for scenario in items(&FIXTURE["scenarios"]) {
        let name = str_of(&scenario["name"]);
        let (temp_dir, temp) = temp_root();
        let install = temp.join("game");
        let backups = temp.join("user").join("player-config-backups.json");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(backups.parent().unwrap()).unwrap();
        for (relative, bytes) in scenario["files"].as_object().unwrap() {
            let file = path_in(&install, relative);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, base64(str_of(bytes))).unwrap();
        }
        let mut run = Run {
            _temp: temp_dir,
            temp,
            install,
            backups,
            search: search(),
            game_id: str_of(&scenario["gameId"]).to_owned(),
            game_directory: str_of(&scenario["gameDirectory"]).to_owned(),
            stamps: Vec::new(),
        };
        for (index, step) in items(&scenario["steps"]).iter().enumerate() {
            let actual = run.step(step);
            let expected = &scenario["results"][index];
            let label = format!("{name}, шаг {index} ({})", str_of(&step["op"]));
            mismatches.check(
                &actual["value"],
                &expected["value"],
                format!("{label}: ответ"),
            );
            mismatches.check(
                &actual["player"],
                &expected["player"],
                format!("{label}: файл игрока"),
            );
            mismatches.check(
                &actual["tree"],
                &expected["tree"],
                format!("{label}: дерево"),
            );
        }
    }
    mismatches.finish("единый конфиг: сценарии");
}

// ---------------------------------------------------------------- самотесты 0.4.9

/// `scripts/lib/cfg-state.js`: что остаётся в игре после цепочки cfg.
#[derive(Debug, Default, PartialEq)]
struct State {
    cvars: HashMap<String, String>,
    binds: HashMap<String, String>,
    aliases: HashMap<String, String>,
}

fn run_chain(read: &dyn Fn(&str) -> Option<String>, roots: &[&str]) -> State {
    fn run(
        read: &dyn Fn(&str) -> Option<String>,
        raw: &str,
        depth: usize,
        visited: &mut Vec<String>,
        state: &mut State,
    ) {
        let name = raw.replace('\\', "/").to_lowercase();
        if depth > 8 || visited.contains(&name) {
            return;
        }
        let Some(text) = read(&name) else {
            return;
        };
        visited.push(name);
        for line in text.split('\n') {
            for command in line_commands(line.strip_suffix('\r').unwrap_or(line)) {
                let args: Vec<&str> = command.args.iter().map(|arg| arg.value.as_str()).collect();
                match args[0].to_lowercase().as_str() {
                    "exec" if args.len() >= 2 => run(read, args[1], depth + 1, visited, state),
                    "exec" => {}
                    "unbindall" => state.binds.clear(),
                    "bind" if args.len() >= 3 => {
                        state
                            .binds
                            .insert(args[1].to_lowercase(), args[2..].join(" "));
                    }
                    "unbind" if args.len() >= 2 => {
                        state.binds.remove(&args[1].to_lowercase());
                    }
                    "alias" if args.len() >= 2 => {
                        state
                            .aliases
                            .insert(args[1].to_owned(), args[2..].join(" "));
                    }
                    verb if args.len() >= 2 && !matches!(verb, "bind" | "unbind" | "alias") => {
                        state.cvars.insert(verb.to_owned(), args[1].to_owned());
                    }
                    _ => {}
                }
            }
        }
    }
    let mut state = State::default();
    let mut visited = Vec::new();
    for root in roots {
        run(read, root, 0, &mut visited, &mut state);
    }
    state
}

fn edit(text: &str, f: impl FnOnce(&doc::Doc) -> doc::Doc) -> String {
    doc::serialize(&f(&doc::parse(text)))
}

#[test]
fn cfg_document_get_set_and_keys() {
    assert_eq!(
        doc::get(
            &doc::parse("alias x \"default_fov 120\"\n"),
            "cvar:default_fov"
        ),
        None,
        "внутри alias ключ не ищется"
    );
    let found = doc::get(&doc::parse("fov 1\nfov 2\n"), "cvar:fov").unwrap();
    assert_eq!((found.value.as_str(), found.line, found.index), ("2", 1, 0));
    let token = |value: &str| hlsr_delivery::game_config::Token {
        value: value.to_owned(),
        quoted: false,
    };
    let key = |words: &[&str]| doc::key_of(&words.iter().map(|w| token(w)).collect::<Vec<_>>());
    assert_eq!(key(&["unbind", "F"]).as_deref(), Some("bind:f"));
    assert_eq!(key(&["exec", "a.cfg"]), None);
    assert_eq!(key(&["echo", "hi"]), None);
    assert_eq!(key(&["+attack"]), None);
    assert_eq!(key(&["FOV", "90"]).as_deref(), Some("cvar:fov"));
    assert_eq!(key(&["alias", "Run", "x"]).as_deref(), Some("alias:run"));
    assert_eq!(
        doc::get(&doc::parse("bind f x\nunbind f\n"), "bind:f"),
        None
    );
    let found = doc::get(&doc::parse("unbind f\nbind f z\n"), "bind:f").unwrap();
    assert_eq!((found.value.as_str(), found.line), ("z", 1));
    let block = doc::find_block(
        &doc::set_block(
            &doc::parse("a 1\n"),
            "run",
            &["x".to_owned(), "y".to_owned()],
            "scripts",
        ),
        "run",
    )
    .unwrap();
    assert_eq!(block.body, ["x", "y"]);
    assert_eq!(doc::find_block(&doc::parse("a 1\n"), "run"), None);
}

#[test]
fn cfg_document_three_binds_above_unbindall_leave_one_line_after_it() {
    let mut current = doc::parse(&format!(
        "{}\nbind \"f\" \"+use\"\n\n{}\nunbindall\nbind w \"+forward\"\n",
        doc::header("binds"),
        doc::header("own")
    ));
    for value in ["+jump", "+duck", "+attack"] {
        current = doc::set(&current, "bind:f", value, "binds");
    }
    let text = doc::serialize(&current);
    let after = &text[text.find("unbindall").unwrap()..];
    assert_eq!(after.matches("bind \"f\"").count(), 1, "{text}");
    let state = run_chain(
        &|name| (name == "config.cfg").then(|| text.clone()),
        &["config.cfg"],
    );
    assert_eq!(state.binds.get("f").map(String::as_str), Some("+attack"));
}

#[test]
fn cfg_document_latin1_bytes_outside_the_edited_line_do_not_change() {
    let high: Vec<u8> = (0x80..=0xFFu8).collect();
    let mut source = b"// ".to_vec();
    source.extend(&high);
    source.extend(b"\r\nfov 90\r\nb 2\r\n");
    let out = edit(&js::latin1_string(&source), |d| {
        doc::set(d, "cvar:fov", "110", "settings")
    });
    let mut expected = b"// ".to_vec();
    expected.extend(&high);
    expected.extend(b"\r\nfov 110\r\nb 2\r\n");
    assert_eq!(js::latin1_bytes(&out), expected);
}

#[test]
fn cfg_document_set_survives_crooked_quotes() {
    // Слова строки для правки на месте и аргументы команды считаются одинаково: индекс значения
    // не выходит за границы.
    for line in [
        "bind \"a b\"c \"+jump",
        "bind a\"b c\"d e",
        "fov \"90",
        "fov 9\"0\" 1",
        "alias x\"y \"z\" w",
        "unbind \"f",
        "bind \"\" \"\"",
        "bind f\"\"\"g; fov\"1",
    ] {
        let parsed = doc::parse(&format!("{line}\n"));
        for item in doc::commands_of(&parsed) {
            if let Some(key) = item.key {
                let out = doc::serialize(&doc::set(&parsed, &key, "zz", "settings"));
                assert!(out.contains("zz"), "{line} → {out}");
            }
        }
    }
}

fn temp() -> (tempfile::TempDir, PathBuf) {
    temp_root()
}

const HL1: &str = "Half-Life/valve_WON";

#[test]
fn player_config_locate_finds_the_language_folder_and_source_cfg() {
    let (_dir, root) = temp();
    let search = search();
    let found = player_config::locate(&ctx(&search, &root, "hl1-won", HL1))
        .unwrap()
        .unwrap();
    assert_eq!(
        found.file,
        path_in(&root, "Half-Life/valve_WON/userconfig.cfg")
    );
    let ghosting = player_config::locate(&ctx(&search, &root, "ghosting-renewed", "ghostingmod"));
    assert_eq!(
        ghosting.unwrap().unwrap().file,
        path_in(&root, "ghostingmod/cfg/autoexec.cfg")
    );
    let hl2 = player_config::locate(&ctx(&search, &root, "hl2-4260", "hl2"))
        .unwrap()
        .unwrap();
    assert_eq!(hl2.file, path_in(&root, "hl2/cfg/autoexec.cfg"));
    assert_eq!(
        player_config::locate(&ctx(&search, &root, "unknown", "x")).unwrap(),
        None
    );

    // Язык Steam не прочитан: игра пишет config.cfg в языковую папку — файл игрока там же.
    std::fs::create_dir_all(path_in(&root, "Half-Life/valve_WON_russian")).unwrap();
    std::fs::write(path_in(&root, "Half-Life/valve_WON_russian/config.cfg"), "").unwrap();
    let found = player_config::locate(&ctx(&search, &root, "hl1-won", HL1));
    assert_eq!(
        found.unwrap().unwrap().file,
        path_in(&root, "Half-Life/valve_WON_russian/userconfig.cfg")
    );
}

#[test]
fn player_config_template_has_six_sections_in_order_and_ascii_hl1() {
    let text = player_config::template("hl1-won", Platform::Win32);
    let at: Vec<usize> = doc::SECTIONS
        .iter()
        .map(|(_, header)| text.find(header).unwrap())
        .collect();
    assert!(at.windows(2).all(|pair| pair[0] < pair[1]));
    let won = text.find("exec WON.cfg").unwrap();
    assert!(won > at[0] && won < at[1]);
    let run = text.find("alias run").unwrap();
    assert!(run > at[1] && run < at[2]);
    assert!(text.is_ascii());
    assert!(text.ends_with("\r\n") && !text.replace("\r\n", "").contains('\n'));
    assert!(text.find("// Write your own").unwrap() > at[5]);
    assert!(
        text.find("bxt_hud_incorrect_fps_indicator 0").unwrap() < at[2],
        "в settings только команды"
    );
    assert!(
        player_config::template("hl2-2707", Platform::Win32).contains(
            "sv_max_queries_sec 9\r\nsave_screenshot 0\r\ncl_showpos 1\r\ncl_showfps 1\r\n"
        )
    );
    let ghosting = player_config::template("ghosting-renewed", Platform::Win32);
    assert!(ghosting.contains("gh_name \"myrun\"") && !ghosting.contains("exec hlsr-"));
}

#[test]
fn player_config_write_checks_sha256_and_leaves_no_temp_files() {
    let (_dir, root) = temp();
    let search = search();
    let ctx = ctx(&search, &root, "hl1-won", HL1);
    player_config::ensure(&ctx).unwrap();
    let before = player_config::read(&ctx).unwrap();
    assert!(before.exists);
    let done = player_config::write(
        &ctx,
        &format!("{}default_fov 110\r\n", before.text),
        Some(before.sha256.as_deref()),
    )
    .unwrap();
    assert_eq!(
        player_config::read(&ctx).unwrap().sha256.as_deref(),
        Some(done.sha256.as_str())
    );
    let bytes = std::fs::read(&done.file).unwrap();
    let error = player_config::write(&ctx, "x\r\n", Some(before.sha256.as_deref())).unwrap_err();
    assert!(error.is("CONFIG_CHANGED"));
    assert_eq!(std::fs::read(&done.file).unwrap(), bytes);
    let names: Vec<String> = std::fs::read_dir(done.file.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["userconfig.cfg"]);
    // Файл появился, пока ждали «файла нет».
    let error = player_config::write(&ctx, "b\r\n", Some(None)).unwrap_err();
    assert!(error.is("CONFIG_CHANGED"));
    // Без отпечатка — без проверки.
    player_config::write(&ctx, "a\r\n", None).unwrap();
    assert_eq!(std::fs::read(&done.file).unwrap(), b"a\r\n");
}

#[test]
fn player_config_is_stock_ensure_and_own_file() {
    let platform = Platform::Win32;
    assert!(player_config::is_stock("hl1-won", "", platform));
    assert!(player_config::is_stock(
        "hl1-won",
        &player_config::template("hl1-won", platform),
        platform
    ));
    assert!(!player_config::is_stock(
        "hl1-won",
        "default_fov 110\r\n",
        platform
    ));
    let stock = format!(
        "{}\r\nexec hlsr-binds.cfg\r\n",
        hlsr_delivery::hl1_default_userconfig::stock()
    );
    assert!(player_config::is_stock("hl1-won", &stock, platform));

    let (_dir, root) = temp();
    let search = search();
    let ctx = ctx(&search, &root, "hl1-won", HL1);
    let file = path_in(&root, "Half-Life/valve_WON/userconfig.cfg");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "default_fov 110\r\n").unwrap();
    assert!(!player_config::ensure(&ctx).unwrap().created);
    assert_eq!(std::fs::read(&file).unwrap(), b"default_fov 110\r\n");
    let previous = format!(
        "{}exec hlsr-personalization.cfg\r\n",
        hlsr_delivery::hl1_default_userconfig::text()
    );
    std::fs::write(&file, js::latin1_bytes(&previous)).unwrap();
    assert!(
        player_config::ensure(&ctx).unwrap().created,
        "стандартный файл 0.4.8 заменяется"
    );
    assert_eq!(
        js::latin1_string(&std::fs::read(&file).unwrap()),
        player_config::template("hl1-won", platform)
    );
}

#[test]
fn player_config_setup_state_and_done() {
    let (_dir, root) = temp();
    let search = search();
    let ctx = ctx(&search, &root, "hl1-won", HL1);
    let mark = path_in(&root, ".hlsr/config-setup.json");
    std::fs::create_dir_all(mark.parent().unwrap()).unwrap();
    std::fs::write(&mark, "{\"migrated\":\"0.4.9\"}").unwrap();
    player_config::setup_done(&ctx, "own", &PrepareOptions::default()).unwrap();
    assert!(!path_in(&root, "Half-Life/valve_WON/userconfig.cfg").exists());
    let saved: Value = serde_json::from_slice(&std::fs::read(&mark).unwrap()).unwrap();
    assert_eq!(saved["migrated"], "0.4.9");
    assert_eq!(saved["choice"], "own");
    let names: Vec<String> = std::fs::read_dir(mark.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["config-setup.json"], "временных файлов нет");
    let error = player_config::setup_done(&ctx, "other", &PrepareOptions::default()).unwrap_err();
    assert!(error.is("CONFIG_SETUP_CHOICE"));

    let (_dir, root) = temp();
    let ctx = self::ctx(&search, &root, "hl1-won", HL1);
    std::fs::create_dir_all(path_in(&root, HL1)).unwrap();
    std::fs::write(
        path_in(&root, "Half-Life/valve_WON/userconfig.cfg"),
        "exec hlsr-binds.cfg\r\n",
    )
    .unwrap();
    assert!(
        !player_config::setup_state(&ctx).unwrap(),
        "установке 0.4.8 нужен переход"
    );
    assert!(
        !path_in(&root, ".hlsr/config-setup.json").exists(),
        "состояние ничего не пишет"
    );
}

fn hl1_env(text: Option<&str>) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (dir, root) = temp();
    let file = path_in(&root, "Half-Life/valve_WON/userconfig.cfg");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    if let Some(text) = text {
        std::fs::write(&file, js::latin1_bytes(text)).unwrap();
    }
    (dir, root, file)
}

fn apply(root: &Path, binds: Value, options: ApplyOptions) -> PlayerBinds {
    let search = search();
    game_binds_install::apply_player_binds(&ctx(&search, root, "hl1-won", HL1), &binds, &options)
        .unwrap()
}

fn latin1(file: &Path) -> String {
    js::latin1_string(&std::fs::read(file).unwrap())
}

#[test]
fn player_binds_r15_only_the_difference_is_written() {
    // Бинд, правленный руками после переназначения, следующее «Сохранить» не откатывает.
    let (_dir, root, file) = hl1_env(Some("// ===== HLSR: binds =====\r\n"));
    std::fs::write(
        path_in(&root, "Half-Life/valve_WON/config.cfg"),
        "bind f \"+use\"\r\nexec userconfig.cfg\r\n",
    )
    .unwrap();
    let over = json!({"f": {"command": "+jump", "base": "+use"}});
    apply(
        &root,
        over.clone(),
        ApplyOptions {
            previous: Some(&json!({})),
            ..ApplyOptions::default()
        },
    );
    std::fs::write(
        &file,
        js::latin1_bytes(&latin1(&file).replace("\"+jump\"", "\"+duck\" // my edit")),
    )
    .unwrap();
    let edited = latin1(&file);
    let mut more = over.clone();
    more["g"] = json!({"command": "+use", "base": ""});
    apply(
        &root,
        more,
        ApplyOptions {
            previous: Some(&over),
            ..ApplyOptions::default()
        },
    );
    assert_eq!(
        latin1(&file),
        edited.replace(
            "bind \"f\" \"+duck\" // my edit\r\n",
            "bind \"f\" \"+duck\" // my edit\r\nbind \"g\" \"+use\"\r\n"
        )
    );
    // Правленый блок скрипта остаётся и попадает в keptEdited; с removeEdited — удаляется.
    let (_dir, root, file) = hl1_env(Some(""));
    let first = json!({"f": {"command": "+fps_20_hold", "base": ""}});
    apply(&root, first.clone(), ApplyOptions::default());
    std::fs::write(
        &file,
        js::latin1_bytes(&latin1(&file).replace("fps_max 20", "fps_max 25")),
    )
    .unwrap();
    let kept = apply(
        &root,
        json!({}),
        ApplyOptions {
            previous: Some(&first),
            ..ApplyOptions::default()
        },
    );
    let PlayerBinds::Applied { kept_edited, .. } = kept else {
        panic!()
    };
    assert_eq!(kept_edited, ["+fps_20_hold"]);
    assert!(latin1(&file).contains("fps_max 25"));
    let removed = apply(
        &root,
        json!({}),
        ApplyOptions {
            previous: Some(&first),
            remove_edited: true,
            ..ApplyOptions::default()
        },
    );
    let PlayerBinds::Applied { kept_edited, .. } = removed else {
        panic!()
    };
    assert!(kept_edited.is_empty());
    assert!(!latin1(&file).contains("HLSR script"));
}

#[test]
fn player_binds_config_changed_between_read_and_write_keeps_the_file() {
    let (_dir, root, file) = hl1_env(Some("// первая\r\n"));
    let search = search();
    let ctx = ctx(&search, &root, "hl1-won", HL1);
    let seen = player_config::read(&ctx).unwrap().sha256;
    std::fs::write(&file, "// чужая правка\r\n").unwrap();
    let error = game_binds_install::apply_player_binds(
        &ctx,
        &json!({"f": {"command": "+duck", "base": ""}}),
        &ApplyOptions {
            expected: Some(seen.as_deref()),
            ..ApplyOptions::default()
        },
    )
    .unwrap_err();
    assert!(error.is("CONFIG_CHANGED"));
    assert_eq!(
        std::fs::read(&file).unwrap(),
        "// чужая правка\r\n".as_bytes()
    );
}

#[test]
fn player_binds_import_with_unbindall_then_binds_and_run_work_after_it() {
    // C2/I5: импорт config.cfg с unbindall — правила и настройки шаблона выше, новые бинды и
    // «Начать ран» после импорта работают.
    let (_dir, root, file) = hl1_env(None);
    let mod_dir = path_in(&root, HL1);
    std::fs::write(
        mod_dir.join("config.cfg"),
        "unbindall\r\nbind w \"+forward\"\r\nexec userconfig.cfg\r\n",
    )
    .unwrap();
    let text = "unbindall\nbind w \"+forward\"\nbind space \"+jump\"\nsensitivity \"2\"\ncl_showfps \"0\"\n";
    let built = Flattened {
        text: text.to_owned(),
        runtime_files: Vec::new(),
    };
    let search = search();
    let ctx = ctx(&search, &root, "hl1-won", HL1);
    let result =
        game_binds_install::import_player_config(&ctx, &built, None, &BackupOptions::default())
            .unwrap();
    assert!(matches!(result, ImportPlayer::Ok { .. }));
    let head = player_config::template_lines("hl1-won", &["rules", "settings"], Platform::Win32)
        .join("\r\n")
        + "\r\n\r\n";
    let body = text.replace('\n', "\r\n");
    assert_eq!(latin1(&file), format!("{head}{body}"));

    apply(
        &root,
        json!({"k": {"command": "+duck", "base": ""}, "p": {"command": "run", "base": ""}}),
        ApplyOptions {
            previous: Some(&json!({})),
            ..ApplyOptions::default()
        },
    );
    let out = latin1(&file);
    assert!(
        out.starts_with(&format!("{head}{body}")),
        "импорт не тронут"
    );
    let state = run_chain(
        &|name| {
            std::fs::read(mod_dir.join(name))
                .ok()
                .map(|bytes| js::latin1_string(&bytes))
        },
        &["config.cfg"],
    );
    assert_eq!(state.binds.get("k").map(String::as_str), Some("+duck"));
    assert_eq!(state.binds.get("p").map(String::as_str), Some("run"));
    assert_eq!(
        state.aliases.get("run").map(String::as_str),
        Some(hlsr_delivery::bind_presets::HL1_RUN_ALIAS)
    );
    assert_eq!(state.cvars.get("cl_showfps").map(String::as_str), Some("0"));
}

#[test]
fn undo_takes_only_the_launchers_own_backup_a7() {
    let (_dir, root, file) = hl1_env(Some("bind f \"+use\"\r\n"));
    let search = search();
    let ctx = ctx(&search, &root, "hl1-won", HL1);
    let (_user, user) = temp();
    let backups_file = user.join("player-config-backups.json");
    let options = BackupOptions {
        backups_file: Some(&backups_file),
        now: Some(1_762_000_000_000),
    };
    let built = Flattened {
        text: "bind g \"+duck\"\n".to_owned(),
        runtime_files: Vec::new(),
    };
    let ImportPlayer::Ok { backup, .. } =
        game_binds_install::import_player_config(&ctx, &built, None, &options).unwrap()
    else {
        panic!("импорт не записан");
    };
    // Чужая копия (подложена в .hlsr/backup) с подменённым import.json не берётся.
    let manifest = path_in(&root, &backup.replace('\\', "/")).join("import.json");
    let original = std::fs::read(&manifest).unwrap();
    std::fs::write(
        &manifest,
        b"{\"kept\": [], \"created\": [\"Half-Life/valve_WON/userconfig.cfg\"]}",
    )
    .unwrap();
    assert_eq!(
        game_binds_install::undo_import(&ctx, &options).unwrap(),
        UndoImport::Nothing
    );
    // Без записи лаунчера — тоже нет.
    let none = BackupOptions {
        backups_file: None,
        now: options.now,
    };
    std::fs::write(&manifest, &original).unwrap();
    assert_eq!(
        game_binds_install::undo_import(&ctx, &none).unwrap(),
        UndoImport::Nothing
    );
    // Своя — возвращается байт в байт.
    let undo = BackupOptions {
        now: Some(1_762_000_060_000),
        ..options
    };
    assert!(matches!(
        game_binds_install::undo_import(&ctx, &undo).unwrap(),
        UndoImport::Ok { .. }
    ));
    assert_eq!(std::fs::read(&file).unwrap(), b"bind f \"+use\"\r\n");
}

#[test]
fn import_rejects_unsafe_and_engine_run_names() {
    let (_dir, root, _file) = hl1_env(Some(""));
    let search = search();
    let ctx = ctx(&search, &root, "hl1-won", HL1);
    for (file, text) in [
        ("../x.cfg", "a"),
        ("config.cfg", "a"),
        ("scripts/autoexec.cfg", "a"),
    ] {
        let built = Flattened {
            text: "bind f x\n".to_owned(),
            runtime_files: vec![(file.to_owned(), text.to_owned())],
        };
        let result =
            game_binds_install::import_player_config(&ctx, &built, None, &BackupOptions::default());
        assert_eq!(result.unwrap(), ImportPlayer::Invalid, "{file}");
    }
    let empty = Flattened {
        text: "  \n".to_owned(),
        runtime_files: Vec::new(),
    };
    let result =
        game_binds_install::import_player_config(&ctx, &empty, None, &BackupOptions::default());
    assert_eq!(result.unwrap(), ImportPlayer::Invalid);
}

#[test]
fn ghosting_migration_on_linux_fixes_the_save_deletion_bind() {
    let (_dir, root) = temp();
    let search = search();
    let autoexec = path_in(&root, "ghostingmod/cfg/autoexec.cfg");
    std::fs::create_dir_all(autoexec.parent().unwrap()).unwrap();
    let own = "gh_name \"me\"\r\n  bind h \"save :; reload\"\r\nexec hlsr-binds.cfg\r\n";
    for (platform, bind) in [
        (Platform::Linux, "  bind h \"save /; reload\"\r\n"),
        (Platform::Win32, "  bind h \"save :; reload\"\r\n"),
    ] {
        std::fs::write(&autoexec, own).unwrap();
        let _ = std::fs::remove_file(path_in(&root, ".hlsr/config-setup.json"));
        let ctx = Ctx {
            platform,
            ..ctx(&search, &root, "ghosting-renewed", "ghostingmod")
        };
        let result = config_migration::migrate(&ctx, &PrepareOptions::default()).unwrap();
        assert!(result.migrated);
        let text = String::from_utf8(std::fs::read(&autoexec).unwrap()).unwrap();
        assert!(text.contains(bind), "{platform:?}: {text}");
        assert!(!text.contains("exec hlsr-"), "{text}");
    }
}

#[test]
fn migration_of_an_exec_fan_out_stays_bounded() {
    // 40 строк exec на следующий файл, 8 уровней: без предела раскрытия — 40^7 копий текста.
    let (_dir, root, file) = hl1_env(Some("exec hlsr-a.cfg\r\n"));
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    for (at, name) in names.iter().enumerate() {
        let body = match names.get(at + 1) {
            Some(next) => format!("exec hlsr-{next}.cfg\r\n").repeat(40),
            None => "fov 90\r\n".to_owned(),
        };
        std::fs::write(file.with_file_name(format!("hlsr-{name}.cfg")), body).unwrap();
    }
    let search = search();
    let ctx = ctx(&search, &root, "hl1-won", HL1);
    let result = config_migration::migrate(&ctx, &PrepareOptions::default()).unwrap();
    assert!(result.migrated);
    let text = latin1(&file);
    assert!(text.len() < 4 * 1024 * 1024, "{}", text.len());
    assert!(
        text.contains("exec hlsr-"),
        "сверх предела exec остаётся строкой"
    );
    let mark = std::fs::read_to_string(path_in(&root, ".hlsr/config-setup.json")).unwrap();
    assert!(mark.contains("\"migrated\""), "{mark}");
}
