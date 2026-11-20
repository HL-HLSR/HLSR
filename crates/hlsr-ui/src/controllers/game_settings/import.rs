//! Импорт cfg (`GameSettingsModal.vue`: `importConfig`, `withCompanions`, `pickImportChain`,
//! `importReport`; логика цепочки `exec` — `hlsr_delivery::cfg_import`).
//!
//! Окно выбора файла открывает главный процесс (`player-config-import-pick`): сначала игрок
//! выбирает конфиг (один или несколько файлов), затем для каждого `exec` цепочки открывается
//! ещё одно окно с его именем; отмена окна — файл не подключается, его `exec` отключается.
//! До «Сохранить» ничего не записано: настройки из файлов становятся правками вкладки.
//!
//! Цепочка и отчёт общие с импортом биндов («Клавиатура», задача 29).

use super::super::jsv;
use super::config::{ConfigEditor, Edits, from_json, to_json};
use super::draft::Tr;
use crate::services::Services;
use hlsr_core::js;
use hlsr_delivery::cfg_import::{self, ImportFile, ImportFiles, Picked, Report, Root};
use hlsr_delivery::game_config;
use serde_json::{Value, json};

const PICK: &str = "player-config-import-pick";

/// Итог импорта для подвала окна (`configImport`/`bindsImport`: `{error, text, details}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub error: bool,
    pub text: String,
    /// Подробности во всплывающей подсказке (`title`).
    pub details: Option<String>,
}

impl Note {
    pub fn error(text: String) -> Note {
        Note {
            error: true,
            text,
            details: None,
        }
    }
}

/// Строки отчёта о цепочке (`importReport`): краткие — в подвал, подробности — в подсказку.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportText {
    pub short: Vec<String>,
    pub details: Option<String>,
}

/// `list(items)`: первые четыре и «+N».
fn list(items: &[String]) -> String {
    let shown = items.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > 4 {
        format!("{shown} +{}", items.len() - 4)
    } else {
        shown
    }
}

/// `importReport(report)`.
pub fn import_report(report: &Report, tr: Tr<'_>) -> ReportText {
    let mut short = Vec::new();
    let mut details = Vec::new();
    if !report.files.is_empty() {
        short.push(tr("#L_KB_IMPORT_CHAIN", &[&report.files.len().to_string()]));
    }
    if !report.missing.is_empty() {
        short.push(tr("#L_KB_IMPORT_CHAIN_MISSING", &[&list(&report.missing)]));
    }
    if !report.cycles.is_empty() {
        let cycles: Vec<String> = report
            .cycles
            .iter()
            .map(|(from, to)| format!("{from} → {to}"))
            .collect();
        short.push(tr("#L_KB_IMPORT_LOOP", &[&list(&cycles)]));
    }
    if !report.duplicate_files.is_empty() {
        let pairs: Vec<String> = report
            .duplicate_files
            .iter()
            .map(|(a, b)| format!("{a} = {b}"))
            .collect();
        short.push(tr("#L_KB_IMPORT_DUP_FILES", &[&list(&pairs)]));
    }
    if !report.duplicates.is_empty() {
        let label = |item: &cfg_import::Duplicate| {
            if item.kind == "cvar" {
                item.name.clone()
            } else {
                format!("{} {}", item.kind, item.name)
            }
        };
        let labels: Vec<String> = report.duplicates.iter().map(label).collect();
        short.push(tr("#L_KB_IMPORT_DUPLICATES", &[&list(&labels)]));
        for item in &report.duplicates {
            let mut files: Vec<&str> = Vec::new();
            for file in &item.files {
                if !files.contains(&file.as_str()) {
                    files.push(file);
                }
            }
            let value = if item.value.is_empty() {
                "—"
            } else {
                &item.value
            };
            details.push(format!("{}: {} → {value}", label(item), files.join(", ")));
        }
    }
    ReportText {
        short,
        details: (!details.is_empty()).then(|| details.join("\n")),
    }
}

/// Ответ `player-config-import-pick` для выбранного файла: `{status:"ok", name, dir, text}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedFile {
    pub name: String,
    pub text: String,
    pub dir: Option<String>,
}

fn string_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => js::to_string(other),
    }
}

fn picked_file(value: &Value) -> PickedFile {
    PickedFile {
        name: string_of(jsv::prop(value, "name")),
        text: string_of(jsv::prop(value, "text")),
        dir: jsv::str_of(jsv::prop(value, "dir"))
            .filter(|dir| !dir.is_empty())
            .map(str::to_owned),
    }
}

async fn invoke(services: &Services, args: Vec<Value>) -> Result<Value, String> {
    services.invoke(PICK, args).await
}

/// Первое окно выбора: игрок указывает конфиг или несколько файлов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstPick {
    /// Окно закрыто (`canceled`) или нет ответа — импорта нет.
    Canceled,
    /// `status` не `ok`: слишком большой файл или не прочитан.
    Failed { too_large: bool },
    Files {
        files: Vec<PickedFile>,
        dir: Option<String>,
    },
}

/// `ipc.invoke("player-config-import-pick", gameId, { multiple: true })`.
pub async fn pick_first(services: &Services, game_id: &str) -> Result<FirstPick, String> {
    let result = invoke(services, vec![json!(game_id), json!({"multiple": true})]).await?;
    if !js::truthy(&result) || jsv::is_str(jsv::prop(&result, "status"), "canceled") {
        return Ok(FirstPick::Canceled);
    }
    if !jsv::is_str(jsv::prop(&result, "status"), "ok") {
        return Ok(FirstPick::Failed {
            too_large: jsv::is_str(jsv::prop(&result, "code"), "BINDS_FILE_TOO_LARGE"),
        });
    }
    // `result.files || [result]`
    let files = match jsv::prop(&result, "files") {
        Some(Value::Array(files)) => files.iter().map(picked_file).collect(),
        _ => vec![picked_file(&result)],
    };
    Ok(FirstPick::Files {
        files,
        dir: jsv::str_of(jsv::prop(&result, "dir")).map(str::to_owned),
    })
}

/// `withCompanions`: файлы игрока, которые движок выполнил бы сам рядом с выбранными (GoldSrc):
/// окно выбора открывается сразу на них; отмена — импорт без этого файла.
pub async fn with_companions(
    services: &Services,
    game_id: &str,
    engine: &str,
    mut picked: Vec<Picked>,
    dir: Option<&str>,
) -> Result<Vec<Picked>, String> {
    if engine != "goldsrc" {
        return Ok(picked);
    }
    let names: Vec<String> = picked.iter().map(|file| file.name.clone()).collect();
    for want in cfg_import::companions_of(&names) {
        let mut options = json!({"want": {"name": want.name, "from": "valve.rc"}});
        options["dir"] = dir.map_or(Value::Null, |dir| json!(dir));
        let chosen = invoke(services, vec![json!(game_id), options]).await?;
        if !jsv::is_str(jsv::prop(&chosen, "status"), "ok") {
            continue;
        }
        let file = Picked {
            name: string_of(jsv::prop(&chosen, "name")),
            text: string_of(jsv::prop(&chosen, "text")),
        };
        if want.before {
            picked.insert(0, file);
        } else {
            picked.push(file);
        }
    }
    Ok(picked)
}

/// Что собрала цепочка `exec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    pub files: ImportFiles,
    pub missing: Vec<String>,
}

/// `pickImportChain`: по окну выбора на каждый файл, который зовёт `exec`. Отмена окна — файл
/// не подключается (`missing`). `top_level_only` — только `exec`, которые выполняются сразу, не
/// по клавише (настройки вкладки «Конфиги» и бинды Ghosting).
pub async fn pick_chain(
    services: &Services,
    game_id: &str,
    root: &Root,
    mut dir: Option<String>,
    engine: &str,
    top_level_only: bool,
    picked: ImportFiles,
) -> Result<Chain, String> {
    let mut files = picked;
    let mut missing: Vec<String> = Vec::new();
    for _ in 0..cfg_import::MAX_FILES {
        let Some(want) = cfg_import::next_wanted(engine, root, &files, &missing, top_level_only)
        else {
            break;
        };
        let mut options = json!({"want": {"name": want.name, "from": want.from}});
        options["dir"] = dir.as_deref().map_or(Value::Null, |dir| json!(dir));
        let chosen = invoke(services, vec![json!(game_id), options]).await?;
        if jsv::is_str(jsv::prop(&chosen, "status"), "ok") {
            // target — имя из exec: под ним файл ляжет в папку игры.
            files.insert(
                want.key.clone(),
                ImportFile {
                    name: string_of(jsv::prop(&chosen, "name")),
                    text: string_of(jsv::prop(&chosen, "text")),
                    target: Some(want.name.clone()),
                },
            );
            if let Some(next) = jsv::str_of(jsv::prop(&chosen, "dir")).filter(|d| !d.is_empty()) {
                dir = Some(next.to_owned());
            }
        } else {
            missing.push(want.key);
        }
    }
    Ok(Chain { files, missing })
}

/// Итог импорта настроек во вкладку «Конфиги».
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigImport {
    /// Правки окна после импорта; `None` — окно не менялось (ошибка, пустой файл, отмена).
    pub edits: Option<Edits>,
    /// Что показать в подвале; `None` — окно выбора закрыли.
    pub note: Option<Note>,
}

/// Применить настройки из файла к правкам окна (`importConfig` после сборки цепочки):
/// значение, равное записанному в cfg, правкой не считается. `name` — имя корневого файла.
pub fn apply_values(
    editor: &ConfigEditor,
    edits: &Edits,
    flat_text: &str,
    name: &str,
    report: &ReportText,
    tr: Tr<'_>,
) -> ConfigImport {
    let Some(catalog) = editor.catalog() else {
        return ConfigImport {
            edits: None,
            note: Some(Note::error(tr("#L_GS_CFG_IMPORT_EMPTY", &[name]))),
        };
    };
    let imported = game_config::import_values(catalog, flat_text);
    if imported.values.is_empty() {
        return ConfigImport {
            edits: None,
            note: Some(Note::error(tr("#L_GS_CFG_IMPORT_EMPTY", &[name]))),
        };
    }
    let mut next = edits.clone();
    let mut changed = 0;
    for (cvar, value) in &imported.values {
        let Some(field) = catalog.by_cvar(&cvar.to_lowercase()) else {
            continue;
        };
        let stored = editor.stored(field);
        let default = game_config::default_value(field);
        let shown = match next.get(field.cvar).and_then(from_json) {
            Some(edit) => edit,
            None => stored.unwrap_or(default),
        };
        if *value != shown {
            changed += 1;
        }
        if Some(*value) == stored || (stored.is_none() && *value == default) {
            next.shift_remove(field.cvar);
        } else {
            next.insert(field.cvar.to_owned(), to_json(*value));
        }
    }
    let first = if changed > 0 {
        tr("#L_GS_CFG_IMPORT_DONE", &[name, &changed.to_string()])
    } else {
        tr("#L_GS_CFG_IMPORT_SAME", &[name])
    };
    let mut parts = vec![first];
    if !imported.skipped.is_empty() {
        parts.push(tr(
            "#L_GS_CFG_IMPORT_SKIPPED",
            &[&imported.skipped.join(", ")],
        ));
    }
    parts.extend(report.short.iter().cloned());
    ConfigImport {
        edits: Some(next),
        note: Some(Note {
            error: false,
            text: parts.join(" "),
            details: report.details.clone(),
        }),
    }
}

/// `importConfig()` целиком: окна выбора, цепочка `exec`, правки. Ошибка канала — «Не удалось
/// прочитать файл». Правки окна `edits()` читаются после окон выбора (`Object.assign({},
/// this.configChanges)` 0.4.7 — после `await`): окна выбора не модальны, поля могли поменять.
pub async fn import_config(
    services: &Services,
    game_id: &str,
    editor: &ConfigEditor,
    edits: &dyn Fn() -> Edits,
    tr: Tr<'_>,
) -> ConfigImport {
    match run_import(services, game_id, editor, edits, tr).await {
        Ok(result) => result,
        Err(_) => ConfigImport {
            edits: None,
            note: Some(Note::error(tr("#L_KB_IMPORT_FAILED", &[]))),
        },
    }
}

async fn run_import(
    services: &Services,
    game_id: &str,
    editor: &ConfigEditor,
    edits: &dyn Fn() -> Edits,
    tr: Tr<'_>,
) -> Result<ConfigImport, String> {
    let nothing = ConfigImport {
        edits: None,
        note: None,
    };
    let (files, dir) = match pick_first(services, game_id).await? {
        FirstPick::Canceled => return Ok(nothing),
        FirstPick::Failed { too_large } => {
            let key = if too_large {
                "#L_KB_IMPORT_TOO_LARGE"
            } else {
                "#L_KB_IMPORT_FAILED"
            };
            return Ok(ConfigImport {
                edits: None,
                note: Some(Note::error(tr(key, &[]))),
            });
        }
        FirstPick::Files { files, dir } => (files, dir),
    };
    let picked: Vec<Picked> = files
        .iter()
        .map(|file| Picked {
            name: file.name.clone(),
            text: file.text.clone(),
        })
        .collect();
    let picked = with_companions(services, game_id, &editor.engine, picked, dir.as_deref()).await?;
    let cfg_import::RootOfPicked { root, files } = cfg_import::root_of_picked(&picked);
    let chain = pick_chain(services, game_id, &root, dir, &editor.engine, true, files).await?;
    let built = cfg_import::build_import(&editor.engine, &root, &chain.files, &chain.missing);
    let report = import_report(&built.report, tr);
    Ok(apply_values(
        editor,
        &edits(),
        &built.flat_text,
        &root.name,
        &report,
        tr,
    ))
}

// ---- импорт страницы «Персонализация» (`GameConfigsPanel.importConfig` 0.4.9) -----------------

/// Движок игры файла игрока (`bindPresets.engineFor`).
pub fn engine_of(game_id: &str) -> Option<&'static str> {
    hlsr_delivery::bind_presets::engine_for(game_id)
}

/// Окно-подсказка перед импортом HL1 (`confirmImportIntro`): окон выбора будет несколько.
/// `None` — не GoldSrc, подсказки нет.
pub fn intro(engine: Option<&str>, tr: Tr<'_>) -> Option<crate::controllers::shell::Confirm> {
    (engine == Some("goldsrc")).then(|| crate::controllers::shell::Confirm {
        title: tr("#L_CFG_IMPORT_INTRO_TITLE", &[]),
        text: tr("#L_CFG_IMPORT_INTRO_TEXT", &[]),
        items: [
            "#L_CFG_IMPORT_INTRO_STEP1",
            "#L_CFG_IMPORT_INTRO_STEP2",
            "#L_CFG_IMPORT_INTRO_STEP3",
        ]
        .iter()
        .map(|key| tr(key, &[]))
        .collect(),
        confirm_label: Some(tr("#L_CFG_IMPORT_INTRO_OK", &[])),
        ..Default::default()
    })
}

/// Итог действия страницы (`notice`: `{ error, text, details, undo, rules }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageNotice {
    pub error: bool,
    pub text: String,
    pub details: Option<String>,
    /// «Вернуть прежний конфиг».
    pub undo: bool,
    /// «Показать» — к вкладке «Проверка».
    pub rules: bool,
}

impl PageNotice {
    pub fn error(text: String) -> PageNotice {
        PageNotice {
            error: true,
            text,
            details: None,
            undo: false,
            rules: false,
        }
    }
}

/// Окна выбора страницы: файл, файлы рядом, цепочка `exec` (все уровни).
#[derive(Debug, Clone, PartialEq)]
pub enum PagePick {
    Canceled,
    /// Ошибка выбора (`#L_KB_IMPORT_TOO_LARGE` / `_FAILED`).
    Failed(String),
    /// Тело `player-config-import-apply`: `{ root, files, missing }`.
    Ready(Value),
}

fn root_value(root: &Root) -> Value {
    let mut value = json!({"name": root.name, "text": root.text});
    // `rootOfPicked` нескольких файлов: корня на диске нет, его exec идут в файл игрока.
    if root.virtual_root {
        value["virtual"] = json!(true);
    }
    value
}

fn files_value(files: &ImportFiles) -> Value {
    let mut map = serde_json::Map::new();
    for (key, file) in files.iter() {
        let mut item = json!({"name": file.name, "text": file.text});
        if let Some(target) = &file.target {
            item["target"] = json!(target);
        }
        map.insert(key.to_owned(), item);
    }
    Value::Object(map)
}

/// `importConfig` страницы до записи: `player-config-import-pick`, `withCompanions`,
/// `pickImportChain(…, false, …)`.
pub async fn pick_player_config(
    services: &Services,
    game_id: &str,
    tr: Tr<'_>,
) -> Result<PagePick, String> {
    let (files, dir) = match pick_first(services, game_id).await? {
        FirstPick::Canceled => return Ok(PagePick::Canceled),
        FirstPick::Failed { too_large } => {
            let key = if too_large {
                "#L_KB_IMPORT_TOO_LARGE"
            } else {
                "#L_KB_IMPORT_FAILED"
            };
            return Ok(PagePick::Failed(tr(key, &[])));
        }
        FirstPick::Files { files, dir } => (files, dir),
    };
    let engine = hlsr_delivery::bind_presets::engine_for(game_id).unwrap_or_default();
    let picked: Vec<Picked> = files
        .iter()
        .map(|file| Picked {
            name: file.name.clone(),
            text: file.text.clone(),
        })
        .collect();
    let picked = with_companions(services, game_id, engine, picked, dir.as_deref()).await?;
    let cfg_import::RootOfPicked { root, files } = cfg_import::root_of_picked(&picked);
    let chain = pick_chain(services, game_id, &root, dir, engine, false, files).await?;
    Ok(PagePick::Ready(json!({
        "root": root_value(&root),
        "files": files_value(&chain.files),
        "missing": chain.missing,
    })))
}

/// `importErrorText(result)`.
pub fn import_error_text(result: Option<&Value>, tr: Tr<'_>) -> String {
    let key = match result.and_then(|r| jsv::str_of(jsv::prop(r, "code"))) {
        Some("GAME_RUNNING") => "#L_D_ERR_GAME_RUNNING",
        Some("BINDS_FILE_TOO_LARGE") => "#L_KB_IMPORT_TOO_LARGE",
        _ => "#L_KB_IMPORT_WRITE_FAILED",
    };
    tr(key, &[])
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| string_of(Some(item)))
        .collect()
}

fn pairs(value: &Value) -> Vec<(String, String)> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|pair| (string_of(pair.get(0)), string_of(pair.get(1))))
        .collect()
}

/// Отчёт цепочки из ответа `player-config-import-apply` (`files` не нужен — `files: []`).
fn report_of(report: &Value) -> Report {
    Report {
        files: Vec::new(),
        missing: strings(&report["missing"]),
        cycles: pairs(&report["cycles"]),
        duplicate_files: pairs(&report["duplicateFiles"]),
        duplicates: report["duplicates"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| cfg_import::Duplicate {
                kind: string_of(item.get("kind")),
                name: string_of(item.get("name")),
                files: strings(&item["files"]),
                value: string_of(item.get("value")),
            })
            .collect(),
    }
}

/// `importNotice(result, rules)`: `rules` — итог проверки `(ошибок, предупреждений)` или `None`.
pub fn import_notice(result: &Value, rules: Option<(u64, u64)>, tr: Tr<'_>) -> PageNotice {
    let summary = &result["summary"];
    let report = &result["report"];
    let unsafe_files = strings(&report["unsafe"]);
    let number = |value: &Value| {
        js::value::number(value)
            .filter(|n| *n != 0.0 && !n.is_nan())
            .map_or_else(|| "0".to_owned(), js::number::number_to_string)
    };
    let chain_issues = ["missing", "cycles", "duplicateFiles", "duplicates"]
        .iter()
        .any(|name| {
            report[*name]
                .as_array()
                .is_some_and(|list| !list.is_empty())
        });
    let counts = tr(
        "#L_CFG_IMPORT_DONE",
        &[
            &number(&summary["binds"]),
            &number(&summary["cvars"]),
            &number(&summary["aliases"]),
            &number(&summary["files"]),
        ],
    );
    let unknown = js::truthy(&summary["unknown"]);
    let broken = rules.is_some_and(|(errors, warnings)| errors > 0 || warnings > 0);
    if !unknown && unsafe_files.is_empty() && !chain_issues && !broken {
        return PageNotice {
            error: false,
            text: tr("#L_CFG_IMPORT_SUCCESS", &[]),
            details: Some(counts),
            undo: true,
            rules: false,
        };
    }
    let mut parts = vec![counts];
    if chain_issues {
        parts.extend(import_report(&report_of(report), tr).short);
    }
    if unknown {
        parts.push(tr("#L_CFG_IMPORT_UNKNOWN", &[&number(&summary["unknown"])]));
    }
    if !unsafe_files.is_empty() {
        parts.push(tr("#L_CFG_IMPORT_UNSAFE", &[&unsafe_files.join(", ")]));
    }
    if let Some((errors, warnings)) = rules {
        parts.push(if errors > 0 || warnings > 0 {
            tr(
                "#L_HC_IMPORT_LINE",
                &[&errors.to_string(), &warnings.to_string()],
            )
        } else {
            tr("#L_HC_IMPORT_CLEAN", &[])
        });
    }
    PageNotice {
        error: false,
        text: parts.join(" "),
        details: (!unsafe_files.is_empty()).then(|| unsafe_files.join("\n")),
        undo: true,
        rules: broken,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(key: &str, args: &[&str]) -> String {
        if args.is_empty() {
            key.to_owned()
        } else {
            format!("{key}({})", args.join("|"))
        }
    }

    // GameConfigsPanel.importNotice: без пропусков и нарушений — «всё успешно», числа в
    // подсказке; иначе числа, цепочка, нераспознанное, небезопасное и итог правил.
    #[test]
    fn the_page_notice_says_success_or_lists_what_went_wrong() {
        let clean = json!({"status": "ok", "summary": {"binds": 12, "cvars": 3, "aliases": 2, "files": 1, "unknown": 0},
            "report": {"missing": [], "cycles": [], "duplicateFiles": [], "duplicates": [], "unsafe": []}});
        let notice = import_notice(&clean, Some((0, 0)), &tr);
        assert_eq!(notice.text, "#L_CFG_IMPORT_SUCCESS");
        assert_eq!(
            notice.details.as_deref(),
            Some("#L_CFG_IMPORT_DONE(12|3|2|1)")
        );
        assert!(notice.undo && !notice.rules && !notice.error);
        let messy = json!({"status": "ok", "summary": {"binds": 1, "unknown": 4},
            "report": {"missing": ["jump.cfg"], "cycles": [["a.cfg", "b.cfg"]], "unsafe": ["evil.cfg"]}});
        let notice = import_notice(&messy, Some((2, 1)), &tr);
        assert_eq!(
            notice.text,
            "#L_CFG_IMPORT_DONE(1|0|0|0) #L_KB_IMPORT_CHAIN_MISSING(jump.cfg) #L_KB_IMPORT_LOOP(a.cfg → b.cfg) \
             #L_CFG_IMPORT_UNKNOWN(4) #L_CFG_IMPORT_UNSAFE(evil.cfg) #L_HC_IMPORT_LINE(2|1)"
        );
        assert_eq!(notice.details.as_deref(), Some("evil.cfg"));
        assert!(notice.rules && notice.undo);
        // Нарушений нет, но не всё разобрано: строка «нарушений нет», «Показать» не нужна.
        let notice = import_notice(&messy, Some((0, 0)), &tr);
        assert!(notice.text.ends_with("#L_HC_IMPORT_CLEAN") && !notice.rules);
        assert_eq!(
            import_error_text(Some(&json!({"code": "GAME_RUNNING"})), &tr),
            "#L_D_ERR_GAME_RUNNING"
        );
        assert_eq!(import_error_text(None, &tr), "#L_KB_IMPORT_WRITE_FAILED");
    }

    #[test]
    fn the_intro_is_only_for_goldsrc() {
        let confirm = intro(Some("goldsrc"), &tr).unwrap();
        assert_eq!(confirm.items.len(), 3);
        assert_eq!(
            confirm.confirm_label.as_deref(),
            Some("#L_CFG_IMPORT_INTRO_OK")
        );
        assert!(intro(Some("source"), &tr).is_none());
        assert!(intro(None, &tr).is_none());
    }
}
