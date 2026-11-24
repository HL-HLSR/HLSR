//! Сплиты LiveSplit и исправления под Wine против 0.4.7: фикстура `livesplit-splits.json` из
//! `tools/reference/delivery/livesplit-splits.js`, и самотесты `livesplit-splits-selftest.js`,
//! `linux-selftest.js` (исправления LiveSplit).
//!
//! Ч — чистые функции: наборы по играм, пути файлов, тексты `.lss` и `.lsl` (большие — длина
//! UTF-8 и sha256). С — сценарии на папке LiveSplit во временном каталоге (`ls/` — сама папка,
//! `res/` — ресурсы лаунчера): `prepare`, `enableServer`, `installBunnySplit`,
//! `applyLiveSplitFixes`; итог шага, строки журнала, дерево с текстом и байты `settings.cfg`.

mod support;

use hlsr_delivery::errors::RawError;
use hlsr_delivery::live_split_fixes::{self, Fix};
use hlsr_delivery::livesplit_splits::{self as splits, LiveSplitSplits};
use hlsr_delivery::ports::{Logger, MemoryLogger};
use regex::Regex;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, LazyLock};
use support::library::{Timeline, push, take};
use support::scenario::{self, common_step, path_in, temp_root};
use support::{Mismatches, fixture, items, opt_str, sha256_hex, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("livesplit-splits.json"));

/// `summary(text)` скрипта: до 3000 знаков UTF-16 — текст, длиннее — длина UTF-8 и sha256.
fn summary(text: Option<&str>) -> Value {
    match text {
        None => Value::Null,
        Some(text) if text.encode_utf16().count() <= 3000 => json!({"text": text}),
        Some(text) => json!({"length": text.len(), "sha256": sha256_hex(text.as_bytes())}),
    }
}

// ---------------------------------------------------------------- Ч

#[test]
fn constants_match_0_4_7() {
    let c = &FIXTURE["constants"];
    assert_eq!(c["BUNNYSPLIT"], splits::BUNNYSPLIT);
    assert_eq!(c["COMPONENTS_DIRECTORY"], splits::COMPONENTS_DIRECTORY);
    assert_eq!(c["FIXES_DIRECTORY"], live_split_fixes::FIXES_DIRECTORY);
    let table: Vec<Value> = live_split_fixes::LIVESPLIT_FIXES
        .iter()
        .map(|fix| {
            json!({"target": fix.target, "source": fix.source, "original": fix.original, "fixed": fix.fixed})
        })
        .collect();
    assert_eq!(c["LIVESPLIT_FIXES"], json!(table));
}

#[test]
fn presets_match_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["describe"]) {
        let game = str_of(&case[0]);
        m.check(
            &json!(splits::describe(game)),
            &case[1],
            format!("describe {game}"),
        );
    }
    for case in items(&FIXTURE["presetOf"]) {
        let (game, preset) = (str_of(&case[0]), opt_str(&case[1]));
        let actual = splits::preset_of(game, preset).map(|preset| preset.id);
        m.check(
            &json!(actual),
            &case[2],
            format!("presetOf {game} {preset:?}"),
        );
    }
    m.finish("наборы");
}

#[test]
fn files_and_xml_match_0_4_7() {
    let mut m = Mismatches::default();
    let mut files = items(&FIXTURE["files"]).iter();
    let mut xml = items(&FIXTURE["splitsXml"]).iter();
    for case in items(&FIXTURE["describe"]) {
        let game = str_of(&case[0]);
        for preset in splits::presets_of(game) {
            let expected = files.next().expect("лишний набор");
            let actual = splits::files_of(game, preset);
            m.check(
                &json!([game, preset.id, {"splits": actual.splits, "layout": actual.layout}]),
                expected,
                format!("filesOf {game} {}", preset.id),
            );
            if preset.bundled.is_none() {
                let expected = xml.next().expect("лишние сплиты");
                m.check(
                    &json!([game, preset.id, summary(Some(&splits::splits_xml(preset)))]),
                    expected,
                    format!("splitsXml {game} {}", preset.id),
                );
            }
        }
    }
    assert!(
        files.next().is_none() && xml.next().is_none(),
        "наборов меньше, чем в 0.4.7"
    );
    let bases = &FIXTURE["layoutBases"];
    for case in items(&FIXTURE["layoutXml"]) {
        let (base, game, preset) = (str_of(&case[0]), str_of(&case[1]), str_of(&case[2]));
        let preset = splits::preset_of(game, Some(preset)).unwrap();
        let actual = splits::layout_xml(str_of(&bases[base]), preset);
        m.check(
            &summary(actual.as_deref()),
            &case[3],
            format!("layoutXml {base} {game} {}", preset.id),
        );
    }
    m.finish("файлы и XML");
}

// ---------------------------------------------------------------- С

/// `normalizer(root)` скрипта: временная папка, временный файл атомарной записи и копирования.
fn norm(root: &Path, text: &str) -> String {
    static ATOMIC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(/\.[^/'\s]+)\.\d+\.[0-9a-z]+\.tmp").unwrap());
    let text = text.replace(&root.to_string_lossy().into_owned(), "<root>");
    let text = if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text
    };
    ATOMIC
        .replace_all(&text, "$1.<pid>.<time>.tmp")
        .replace(&format!(".{}.tmp", std::process::id()), ".<pid>.tmp")
}

struct StepLogger {
    timeline: Timeline,
    root: std::path::PathBuf,
}

impl StepLogger {
    fn log(&self, level: &str, message: &str) {
        push(
            &self.timeline,
            json!({"log": level, "message": norm(&self.root, message)}),
        );
    }
}

impl Logger for StepLogger {
    fn info(&self, message: &str) {
        self.log("info", message);
    }
    fn warn(&self, message: &str) {
        self.log("warn", message);
    }
    fn error(&self, message: &str) {
        self.log("error", message);
    }
}

fn outcome<T: serde::Serialize>(root: &Path, result: Result<T, RawError>) -> Value {
    match result {
        Ok(value) => json!({"ok": value}),
        Err(error) => json!({"raw": {"code": error.code, "message": norm(root, &error.message)}}),
    }
}

fn run_scenario(spec: &Value, m: &mut Mismatches) {
    let (_temp, root) = temp_root();
    scenario::setup(&root, &spec["setup"]);
    let ls = root.join("ls");
    let resources = root.join("res");
    let name = str_of(&spec["name"]);
    assert_eq!(
        items(&spec["steps"]).len(),
        items(&spec["results"]).len(),
        "шагов и результатов поровну"
    );
    for (index, (step, expected)) in items(&spec["steps"])
        .iter()
        .zip(items(&spec["results"]))
        .enumerate()
    {
        let timeline: Timeline = Arc::default();
        let logger: Arc<dyn Logger> = Arc::new(StepLogger {
            timeline: timeline.clone(),
            root: root.clone(),
        });
        let service = LiveSplitSplits::new(&resources, logger);
        let actual = match str_of(&step["op"]) {
            "prepare" => {
                let had_components = ls.join("Components").exists();
                let result = service
                    .prepare(&ls, str_of(&step["gameId"]), opt_str(&step["presetId"]))
                    .map(|prepared| {
                        json!({
                            "args": prepared.args,
                            "preset": prepared.preset.map(|preset| json!({
                                "id": preset.id,
                                "kind": preset.kind.as_str(),
                                "category": preset.category,
                            })),
                        })
                    });
                let mut actual =
                    json!({"outcome": outcome(&root, result), "timeline": take(&timeline)});
                without_ghosting_asl(&ls, had_components, &mut actual);
                actual
            }
            "enableServer" => {
                let result = service.enable_server(&ls, step["port"].as_i64().unwrap());
                json!({"outcome": outcome(&root, result), "timeline": take(&timeline)})
            }
            "installBunnySplit" => {
                let result = service.install_bunny_split(&ls);
                json!({"outcome": outcome(&root, result), "timeline": take(&timeline)})
            }
            "fixes" => {
                let table: Vec<(String, String, String, String)> = items(&step["table"])
                    .iter()
                    .map(|item| {
                        (
                            str_of(&item["target"]).to_owned(),
                            str_of(&item["source"]).to_owned(),
                            sha256_hex(str_of(&item["originalText"]).as_bytes()),
                            sha256_hex(str_of(&item["fixedText"]).as_bytes()),
                        )
                    })
                    .collect();
                let fixes: Vec<Fix<'_>> = table
                    .iter()
                    .map(|(target, source, original, fixed)| Fix {
                        target,
                        source,
                        original,
                        fixed,
                    })
                    .collect();
                let result = live_split_fixes::apply_live_split_fixes(&ls, &resources, &fixes);
                json!({"outcome": outcome(&root, result), "timeline": take(&timeline)})
            }
            "text" => {
                let bytes = std::fs::read(path_in(&root, str_of(&step["file"]))).unwrap();
                json!({"text": hlsr_core::js::latin1_string(&bytes)})
            }
            _ => common_step(&root, step)
                .unwrap_or_else(|| panic!("{name}: неизвестный шаг {}", step["op"])),
        };
        m.check(
            &actual,
            expected,
            format!("{name}, шаг {index} ({})", step["op"]),
        );
    }
}

/// Отступление от 0.4.7 (24.11): для Ghosting `prepare` ещё ставит свой автосплиттер — убрать
/// его файл и строку журнала, остальное сверяется как было (проверка — в
/// `ghosting_gets_asl_and_millisecond_timer`).
fn without_ghosting_asl(ls: &Path, had_components: bool, actual: &mut Value) {
    let components = ls.join("Components");
    if std::fs::remove_file(components.join(splits::GHOSTING_ASL)).is_ok() && !had_components {
        let _ = std::fs::remove_dir(&components);
    }
    if let Some(lines) = actual["timeline"].as_array_mut() {
        lines.retain(|line| !line.to_string().contains(splits::GHOSTING_ASL));
    }
}

#[test]
fn scenarios_match_0_4_7() {
    let mut m = Mismatches::default();
    for spec in items(&FIXTURE["scenarios"]) {
        run_scenario(spec, &mut m);
    }
    m.finish("сценарии сплитов");
}

// ---------------------------------------------------------------- самотесты 0.4.7

/// `linux-selftest.js`: «исправленный SourceSplit в ресурсах совпадает с таблицей».
#[test]
fn bundled_sourcesplit_fix_matches_table() {
    let resources = hlsr_core::paths::Resources::detect();
    for fix in live_split_fixes::LIVESPLIT_FIXES {
        let file = resources
            .dir()
            .join(live_split_fixes::FIXES_DIRECTORY)
            .join(fix.source);
        let bytes = std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        assert_eq!(sha256_hex(&bytes), fix.fixed, "{}", fix.source);
        assert_ne!(fix.original, fix.fixed);
    }
}

/// BunnySplit из ресурсов лаунчера на месте (`resources/livesplit-components`).
#[test]
fn bundled_bunnysplit_is_present() {
    let resources = hlsr_core::paths::Resources::detect();
    let file = resources
        .dir()
        .join(splits::COMPONENTS_DIRECTORY)
        .join(splits::BUNNYSPLIT);
    assert!(file.is_file(), "{}", file.display());
}

// ---------------------------------------------------------------- BunnySplit 0.4.9

/// Папка LiveSplit с `Components` и ресурсы, где BunnySplit — текст «dll».
fn bunnysplit_dirs(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let (ls, res) = (root.join("ls"), root.join("res"));
    std::fs::create_dir_all(ls.join("Components")).unwrap();
    std::fs::create_dir_all(res.join(splits::COMPONENTS_DIRECTORY)).unwrap();
    std::fs::write(
        res.join(splits::COMPONENTS_DIRECTORY)
            .join(splits::BUNNYSPLIT),
        "dll",
    )
    .unwrap();
    (ls, res)
}

fn components(ls: &Path) -> Vec<String> {
    std::fs::read_dir(ls.join("Components"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// «BunnySplit: поставляемый файл закреплён».
#[test]
fn shipped_bunnysplit_is_pinned() {
    let file = hlsr_core::paths::Resources::detect()
        .dir()
        .join(splits::COMPONENTS_DIRECTORY)
        .join(splits::BUNNYSPLIT);
    assert_eq!(
        sha256_hex(&std::fs::read(file).unwrap()),
        splits::BUNNYSPLIT_SHA256
    );
    assert!(!splits::BUNNYSPLIT_BROKEN_SHA256.contains(&splits::BUNNYSPLIT_SHA256));
    assert_eq!(
        splits::BUNNYSPLIT_BROKEN_SHA256,
        ["0c6a8d199f8ef6c5e3bc32239569f017568220b03314a5424a56acd714279794"]
    );
}

/// «BunnySplit: сломанная копия заменяется», «своя версия пользователя не трогается».
#[test]
fn broken_bunnysplit_is_replaced_and_users_own_kept() {
    let root = tempfile::tempdir().unwrap();
    let (ls, res) = bunnysplit_dirs(root.path());
    let target = ls.join("Components").join(splits::BUNNYSPLIT);
    let logger = Arc::new(MemoryLogger::default());
    let service =
        LiveSplitSplits::new(&res, logger.clone()).with_broken_sha256(vec![sha256_hex(b"broken")]);

    std::fs::write(&target, "mine").unwrap();
    assert!(!service.install_bunny_split(&ls).unwrap());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "mine");

    std::fs::write(&target, "broken").unwrap();
    assert!(service.install_bunny_split(&ls).unwrap());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "dll");
    assert!(
        logger
            .take()
            .iter()
            .any(|(_, line)| line.contains("заменён сломанный BunnySplit"))
    );
    assert_eq!(components(&ls), [splits::BUNNYSPLIT]);
}

/// «BunnySplit: при ошибке переименования .tmp не остаётся»: сломанную копию держит открытой
/// другая программа (без общего удаления), замена не удаётся.
#[cfg(windows)]
#[test]
fn failed_rename_leaves_no_temporary_file() {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    let root = tempfile::tempdir().unwrap();
    let (ls, res) = bunnysplit_dirs(root.path());
    let target = ls.join("Components").join(splits::BUNNYSPLIT);
    std::fs::write(&target, "broken").unwrap();
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&target)
        .unwrap();
    let service = LiveSplitSplits::new(&res, Arc::new(MemoryLogger::default()))
        .with_broken_sha256(vec![sha256_hex(b"broken")]);
    assert!(service.install_bunny_split(&ls).is_err());
    assert_eq!(components(&ls), [splits::BUNNYSPLIT]);
}

/// `linux-selftest.js`: «исправленный компонент заменяет только ту сборку, из которой сделан»
/// — временный файл копирования не остаётся.
#[test]
fn fix_leaves_no_temporary_file() {
    let root = tempfile::tempdir().unwrap();
    let tool = root.path().join("livesplit");
    let resources = root.path().join("resources");
    std::fs::create_dir_all(tool.join("Components")).unwrap();
    std::fs::create_dir_all(resources.join(live_split_fixes::FIXES_DIRECTORY)).unwrap();
    std::fs::write(
        resources
            .join(live_split_fixes::FIXES_DIRECTORY)
            .join("Split.dll"),
        "исправленный",
    )
    .unwrap();
    std::fs::write(tool.join("Components").join("Split.dll"), "исходный").unwrap();
    let (original, fixed) = (
        sha256_hex("исходный".as_bytes()),
        sha256_hex("исправленный".as_bytes()),
    );
    let fixes = [Fix {
        target: "Components/Split.dll",
        source: "Split.dll",
        original: &original,
        fixed: &fixed,
    }];
    let apply = || live_split_fixes::apply_live_split_fixes(&tool, &resources, &fixes).unwrap();
    assert_eq!(apply(), ["Components/Split.dll"]);
    assert!(
        apply().is_empty(),
        "исправленный файл повторно не копируется"
    );
    let names: Vec<String> = std::fs::read_dir(tool.join("Components"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["Split.dll"], "временный файл не остаётся");
}

/// `livesplit-splits-selftest.js`: «HL2: сплиты с SourceSplit пишутся один раз» — рекорды в
/// файле не затираются, запись в `settings.cfg` не повторяется.
#[test]
fn records_survive_second_prepare() {
    let root = tempfile::tempdir().unwrap();
    let ls = root.path();
    std::fs::write(
        ls.join("settings.cfg"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<Settings>\r\n  <RecentSplits />\r\n  <ActiveAutoSplitters />\r\n</Settings>\r\n",
    )
    .unwrap();
    let service = LiveSplitSplits::new(ls.join("res"), Arc::new(MemoryLogger::default()));
    let prepared = service
        .prepare(ls, "hl2-4260", Some("scriptless-chapters"))
        .unwrap();
    assert_eq!(
        prepared.args[..2],
        ["-s", "splits\\hlsr\\hl2-4260\\scriptless-chapters.lss"]
    );
    let file = path_in(ls, "splits/hlsr/hl2-4260/scriptless-chapters.lss");
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text.matches("<Segment>").count(), 14);
    std::fs::write(&file, "PB").unwrap();
    service
        .prepare(ls, "hl2-4260", Some("scriptless-chapters"))
        .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "PB");
    let settings = std::fs::read_to_string(ls.join("settings.cfg")).unwrap();
    assert_eq!(settings.matches("scriptless-chapters.lss").count(), 1);
}

/// Ghosting (24.11): готовые сплиты из архива получают автосплиттер в `Components` и таймер с
/// миллисекундами; свой формат игрока не трогается, повторный запуск ничего не переписывает.
#[test]
fn ghosting_gets_asl_and_millisecond_timer() {
    let root = tempfile::tempdir().unwrap();
    let ls = root.path();
    std::fs::create_dir_all(ls.join("splits/chapters")).unwrap();
    std::fs::write(ls.join("splits/chapters/chapters.lss"), "<Run />").unwrap();
    let layout = ls.join("splits/chapters/chapters.lsl");
    std::fs::write(
        &layout,
        "<Layout><TimerFormat>1.23</TimerFormat><TimerFormat>1.2</TimerFormat></Layout>",
    )
    .unwrap();
    let service = LiveSplitSplits::new(ls.join("res"), Arc::new(MemoryLogger::default()));
    service
        .prepare(ls, "ghosting-renewed", Some("chapters"))
        .unwrap();
    let asl = std::fs::read_to_string(ls.join("Components").join(splits::GHOSTING_ASL)).unwrap();
    assert!(asl.contains("47 48 4F 53 54 49 4E 47 2D 4C 53 2D 76 31 00 00"));
    assert_eq!(
        std::fs::read_to_string(&layout).unwrap(),
        "<Layout><TimerFormat>1.234</TimerFormat><TimerFormat>1.2</TimerFormat></Layout>"
    );
    assert!(
        !service.install_ghosting_asl(ls).unwrap(),
        "тот же файл не переписывается"
    );
    assert!(!service.timer_milliseconds(&layout).unwrap());
}
