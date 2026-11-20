//! Поиск cfg, записи и установка биндов/импорта против 0.4.7: фикстура `cfg-install.json` из
//! `tools/reference/delivery/cfg-install.js`. Сценарий — данные (файлы и шаги), его выполняют
//! модули 0.4.7 в Node 24 и эти модули на тех же деревьях во временных папках; сравниваются
//! ответ каждого шага и дерево файлов (путь, размер, sha256, текст latin1).
//!
//! Сценарии, помеченные `unix`, зависят от ОС (регистр имён, обратные косые, ссылки, права
//! доступа, порядок `readdir`): на Windows они пропускаются.

mod support;

use hlsr_core::js::object::JsObject;
use hlsr_core::js::{self, Platform};
use hlsr_core::json_text::stringify;
use hlsr_delivery::cfg_import::Output;
use hlsr_delivery::cfg_search::{self, CfgSearch, LanguageOption, SteamLanguage};
use hlsr_delivery::errors::Failure;
use hlsr_delivery::game_binds::{BindRead, Encoding};
use hlsr_delivery::game_binds_install::{
    self, ApplyBinds, ImportSource, InstallImport, RemoveImport,
};
use hlsr_delivery::ports::{FakeRegistry, RegistryHive, RegistryValue};
use hlsr_delivery::runs_scan::{self, DeleteResult, ScanResult};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("cfg-install.json"));

/// Время записи файлов без своего (секунды), как в сценарии.
const DEFAULT_MTIME: f64 = 1_600_000_000.0;
/// Текст файла попадает в снимок, если файл не больше.
const TEXT_LIMIT: u64 = 6000;

// ------------------------------------------------------------- файлы сценария

fn bytes_of(item: &Value) -> Vec<u8> {
    if let Some(text) = opt_str(&item["latin1"]) {
        return js::latin1_bytes(text);
    }
    if let Some(text) = opt_str(&item["utf8"]) {
        return text.as_bytes().to_vec();
    }
    if let Some(hex) = opt_str(&item["hex"]) {
        return (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
            .collect();
    }
    if let Some(repeat) = item["repeat"].as_array() {
        return js::latin1_bytes(&str_of(&repeat[0]).repeat(repeat[1].as_u64().unwrap() as usize));
    }
    Vec::new()
}

/// `fs.utimesSync(file, seconds, seconds)` Node в Linux: libuv переводит секунды в
/// `timespec` — целая часть, дробная в наносекундах с точностью до микросекунды.
fn set_mtime(file: &Path, seconds: f64) {
    let whole = seconds.trunc();
    let mut nanos = ((seconds - whole) * 1e9) as i64;
    nanos -= nanos % 1000;
    let time = UNIX_EPOCH + Duration::new(whole as u64, nanos as u32);
    let handle = std::fs::OpenOptions::new()
        .write(true)
        .open(file)
        .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    handle
        .set_times(std::fs::FileTimes::new().set_modified(time))
        .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
}

fn path_in(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part))
}

fn put_file(root: &Path, relative: &str, item: &Value) {
    let file = path_in(root, relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, bytes_of(item)).unwrap();
    set_mtime(&file, item["mtime"].as_f64().unwrap_or(DEFAULT_MTIME));
}

fn setup(root: &Path, list: &Value) {
    for item in list.as_array().into_iter().flatten() {
        if let Some(dir) = opt_str(&item["dir"]) {
            std::fs::create_dir_all(path_in(root, dir)).unwrap();
        } else if let Some(link) = opt_str(&item["symlink"]) {
            #[cfg(unix)]
            {
                let link = path_in(root, link);
                std::fs::create_dir_all(link.parent().unwrap()).unwrap();
                let target = str_of(&item["target"]).replace("<root>", root.to_str().unwrap());
                std::os::unix::fs::symlink(target, link).unwrap();
            }
            #[cfg(not(unix))]
            let _ = link;
        } else if let Some(generate) = item.get("generate") {
            for n in 0..generate["count"].as_u64().unwrap() {
                let pad = generate["pad"].as_u64().unwrap_or(1) as usize;
                let name = str_of(&generate["name"]).replace("{n}", &format!("{n:0pad$}"));
                let dir = str_of(&generate["dir"]);
                let step = generate["step"].as_f64().unwrap_or(0.0);
                let mut file = json!({"latin1": opt_str(&generate["latin1"]).unwrap_or("x")});
                if let Some(base) = generate["mtime"].as_f64() {
                    file["mtime"] = json!(base + n as f64 * step);
                }
                put_file(root, &format!("{dir}/{name}"), &file);
            }
        } else {
            put_file(root, str_of(&item["file"]), item);
        }
    }
}

// --------------------------------------------------------------- снимок дерева

fn tree_with_text(root: &Path) -> Value {
    let mut tree = support::tree::snapshot(root, None);
    for entry in tree.as_array_mut().unwrap() {
        let relative = str_of(&entry["path"]).to_owned();
        match str_of(&entry["kind"]) {
            "symlink" => {
                let target = str_of(&entry["target"]).replace(root.to_str().unwrap(), "<root>");
                entry["target"] = json!(target);
            }
            "file" if entry["size"].as_u64().unwrap() <= TEXT_LIMIT => {
                let bytes = std::fs::read(path_in(root, &relative)).unwrap();
                entry["latin1"] = json!(js::latin1_string(&bytes));
            }
            _ => {}
        }
    }
    tree
}

// ---------------------------------------------------------------- ответы шагов

fn rel(root: &Path, path: &Path) -> String {
    if path == root {
        return ".".to_owned();
    }
    match path.strip_prefix(root) {
        Ok(rest) => rest
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => normalize_text(root, &path.to_string_lossy()),
    }
}

/// Текст ошибки: `<root>` вместо временной папки, `<tmp>` вместо её родителя (выход за пределы
/// установки называет родителя), прямые косые.
fn normalize_text(root: &Path, text: &str) -> String {
    let parent = root.parent().unwrap().to_str().unwrap();
    text.replace(root.to_str().unwrap(), "<root>")
        .replace(parent, "<tmp>")
        .replace('\\', "/")
}

/// Исключение → то, что видно снаружи: код; у `DeliveryError` — ещё и текст (в нём путь).
fn outcome<T>(root: &Path, result: Result<T, Failure>, ok: impl FnOnce(T) -> Value) -> Value {
    match result {
        Ok(value) => json!({"ok": ok(value)}),
        Err(Failure::Delivery(error)) => json!({"error": {
            "code": error.code.as_str(),
            "message": normalize_text(root, &error.message),
        }}),
        Err(Failure::Raw(error)) => json!({"error": {"code": error.code}}),
    }
}

fn entries_of<V>(object: &JsObject<V>, one: impl Fn(&V) -> Value) -> Value {
    Value::Array(
        object
            .iter()
            .map(|(key, value)| json!([key, one(value)]))
            .collect(),
    )
}

fn dump_read(read: &BindRead) -> Value {
    json!({
        "binds": entries_of(&read.binds, |bind| json!({"command": bind.command, "source": bind.source})),
        "aliases": entries_of(&read.aliases, |text| json!(text)),
        "cvars": entries_of(&read.cvars, |text| json!(text)),
        "files": read.files,
    })
}

fn item_value(item: &runs_scan::RunItem) -> Value {
    json!({"id": item.id, "name": item.name, "size": item.size, "modifiedAt": item.modified_at})
}

/// Больше 40 записей — сводка: число, три первые, три последние и sha256 JSON всего списка.
fn summarize(items: &[runs_scan::RunItem]) -> Value {
    let values: Vec<Value> = items.iter().map(item_value).collect();
    if values.len() <= 40 {
        return Value::Array(values);
    }
    let text = stringify(&Value::Array(values.clone()), "");
    let sha: String = Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    json!({
        "count": values.len(),
        "head": values[..3],
        "tail": values[values.len() - 3..],
        "sha256": sha,
    })
}

fn scan_value(root: &Path, result: &ScanResult) -> Value {
    match result {
        ScanResult::Error { code } => json!({"status": "error", "code": code}),
        ScanResult::NoDirectory { path } => match path {
            Some(path) => json!({"status": "no-directory", "path": rel(root, path), "items": []}),
            None => json!({"status": "no-directory", "items": []}),
        },
        ScanResult::Ok {
            path,
            items,
            truncated,
        } => json!({
            "status": "ok",
            "path": rel(root, path),
            "items": summarize(items),
            "truncated": truncated,
        }),
    }
}

fn language_of(value: &Value) -> LanguageOption<'_> {
    match value {
        Value::String(text) if text == "detect" => LanguageOption::Detect,
        Value::String(text) => LanguageOption::Given(Some(text)),
        _ => LanguageOption::Given(None),
    }
}

fn encoding_of(name: &str) -> Encoding {
    match name {
        "latin1" => Encoding::Latin1,
        _ => Encoding::Utf8,
    }
}

fn expand(text: &Value) -> String {
    match text {
        Value::String(text) => text.clone(),
        spec => {
            str_of(&spec["repeat"][0]).repeat(spec["repeat"][1].as_u64().unwrap() as usize)
                + opt_str(&spec["tail"]).unwrap_or_default()
        }
    }
}

fn outputs_of(value: &Value) -> Vec<Output> {
    items(value)
        .iter()
        .map(|item| Output {
            file: str_of(&item["file"]).to_owned(),
            text: expand(&item["text"]),
            root: item["root"].as_bool().unwrap_or(false),
        })
        .collect()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| list.iter().map(|s| str_of(s).to_owned()).collect())
        .unwrap_or_default()
}

struct Runner {
    root: PathBuf,
    _temp: tempfile::TempDir,
    search: CfgSearch,
}

impl Runner {
    fn new() -> Runner {
        let temp = tempfile::tempdir().unwrap();
        // В Windows canonicalize даёт путь с префиксом \\?\ — в текстах ошибок он лишний.
        let root = if cfg!(windows) {
            temp.path().to_path_buf()
        } else {
            std::fs::canonicalize(temp.path()).unwrap()
        };
        // Реестра нет: язык Steam не читается, как в Linux.
        let search = CfgSearch::new(SteamLanguage::new(Arc::new(FakeRegistry::default())));
        Runner {
            root,
            _temp: temp,
            search,
        }
    }

    fn step(&self, step: &Value) -> Value {
        let root = &self.root;
        let install: PathBuf = if step["install"] == "empty" {
            PathBuf::new()
        } else {
            root.clone()
        };
        let gd = opt_str(&step["gameDirectory"]).unwrap_or_default();
        let engine = opt_str(&step["engine"]).unwrap_or_default();
        let game_id = opt_str(&step["gameId"]).unwrap_or_default();
        match str_of(&step["op"]) {
            "tree" => json!({"tree": tree_with_text(root)}),
            "write" => {
                put_file(root, str_of(&step["file"]), step);
                Value::Null
            }
            "remove" => {
                let path = path_in(root, str_of(&step["file"]));
                let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
                Value::Null
            }
            "mkdir" => {
                std::fs::create_dir_all(path_in(root, str_of(&step["dir"]))).unwrap();
                Value::Null
            }
            "utime" => {
                set_mtime(
                    &path_in(root, str_of(&step["file"])),
                    step["mtime"].as_f64().unwrap(),
                );
                Value::Null
            }
            "chmod" => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = step["mode"].as_u64().unwrap() as u32;
                    std::fs::set_permissions(
                        path_in(root, str_of(&step["file"])),
                        std::fs::Permissions::from_mode(mode),
                    )
                    .unwrap();
                }
                Value::Null
            }
            "modDirOf" => outcome(
                root,
                cfg_search::mod_dir_of(&install, gd, engine).map_err(Failure::from),
                |dir| json!(rel(root, &dir)),
            ),
            "searchDirs" => outcome(
                root,
                self.search
                    .search_dirs(&install, gd, engine, language_of(&step["language"])),
                |dirs| Value::Array(dirs.iter().map(|d| json!(rel(root, d))).collect()),
            ),
            "files" => outcome(
                root,
                self.search
                    .files_of(&install, gd, engine, language_of(&step["language"])),
                |files| {
                    let names: Vec<Value> = items(&step["names"])
                        .iter()
                        .map(|name| {
                            let name = str_of(name);
                            json!([name, files.exists(name), rel(root, &files.path_of(name))])
                        })
                        .collect();
                    json!({
                        "dirs": files.dirs.iter().map(|d| rel(root, d)).collect::<Vec<_>>(),
                        "modDir": rel(root, &files.mod_dir),
                        "names": names,
                    })
                },
            ),
            "readBinds" => dump_read(&game_binds_install::read_install_binds(
                &self.search,
                &install,
                gd,
                engine,
            )),
            "applyBinds" => outcome(
                root,
                game_binds_install::apply_binds(
                    &self.search,
                    &install,
                    game_id,
                    gd,
                    &step["overrides"],
                ),
                |result| match result {
                    ApplyBinds::Skipped => json!({"status": "skipped"}),
                    ApplyBinds::Applied { file, appended } => json!({
                        "status": "applied", "file": rel(root, &file), "appended": appended,
                    }),
                },
            ),
            "ensureExecLine" => outcome(
                root,
                game_binds_install::ensure_exec_line(
                    &path_in(root, str_of(&step["file"])),
                    str_of(&step["line"]),
                    encoding_of(str_of(&step["encoding"])),
                )
                .map_err(Failure::from),
                |appended| json!(appended),
            ),
            "installImport" => {
                let text;
                let outputs;
                let source = if step.get("text").is_some() {
                    text = expand(&step["text"]);
                    ImportSource::Text(&text)
                } else {
                    outputs = outputs_of(&step["outputs"]);
                    ImportSource::Outputs(&outputs)
                };
                outcome(
                    root,
                    game_binds_install::install_import(
                        &self.search,
                        &install,
                        game_id,
                        gd,
                        source,
                        &strings(&step["execs"]),
                    ),
                    |result| match result {
                        InstallImport::Unsupported => json!({"status": "unsupported"}),
                        InstallImport::Invalid => json!({"status": "invalid"}),
                        InstallImport::Ok { files } => json!({"status": "ok", "files": files}),
                    },
                )
            }
            "removeImport" => outcome(
                root,
                game_binds_install::remove_import(&self.search, &install, game_id, gd),
                |result| match result {
                    RemoveImport::Unsupported => json!({"status": "unsupported"}),
                    RemoveImport::Ok {
                        restored_userconfig: None,
                    } => json!({"status": "ok"}),
                    RemoveImport::Ok {
                        restored_userconfig: Some(restored),
                    } => json!({
                        "status": "ok", "restoredUserconfig": restored,
                    }),
                },
            ),
            "syncTrailer" => outcome(
                root,
                game_binds_install::sync_import_trailer(&self.search, &install, game_id, gd),
                |changed| json!(changed),
            ),
            "readImportRecord" => match game_binds_install::read_import_record(&install) {
                None => Value::Null,
                Some(record) => json!({
                    "files": record.files.iter().map(|f| json!({"path": f.path, "created": f.created})).collect::<Vec<_>>(),
                    "execs": record.execs,
                }),
            },
            "scan" => scan_value(
                root,
                &runs_scan::scan(
                    &install,
                    str_of(&step["relativeDir"]),
                    opt_str(&step["extension"]),
                ),
            ),
            "scanMany" => scan_value(
                root,
                &runs_scan::scan_many(
                    &install,
                    &strings(&step["relativeDirs"]),
                    opt_str(&step["extension"]),
                ),
            ),
            "languageDirs" => json!(runs_scan::language_dirs(
                &install,
                str_of(&step["relativeDir"])
            )),
            "runDirs" => json!(runs_scan::run_dirs(&install, str_of(&step["relativeDir"]))),
            "resolveItem" => match runs_scan::resolve_item(
                &install,
                str_of(&step["relativeDir"]),
                str_of(&step["name"]),
                opt_str(&step["extension"]),
            ) {
                Some(path) => json!(rel(root, &path)),
                None => Value::Null,
            },
            "deleteItem" => match runs_scan::delete_item(
                &install,
                str_of(&step["relativeDir"]),
                str_of(&step["name"]),
                opt_str(&step["extension"]),
            ) {
                DeleteResult::Ok { name } => json!({"status": "ok", "name": name}),
                DeleteResult::Error { code } => json!({"status": "error", "code": code}),
            },
            other => panic!("неизвестный шаг {other}"),
        }
    }
}

/// `readImportRecord` 0.4.7 отдаёт элементы записи как есть; читают из них только путь и
/// истинность `created`.
fn normalize_expected(step: &Value, expected: &Value) -> Value {
    if str_of(&step["op"]) != "readImportRecord" || expected.is_null() {
        return expected.clone();
    }
    let files: Vec<Value> = items(&expected["files"])
        .iter()
        .map(|f| json!({"path": f["path"], "created": f.get("created").is_some_and(js::truthy)}))
        .collect();
    json!({"files": files, "execs": expected["execs"]})
}

fn is_root_user() -> bool {
    #[cfg(unix)]
    {
        rustix::process::geteuid().is_root()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn run_group(group: &str) {
    let mut mismatches = Mismatches::default();
    for scenario in items(&FIXTURE[group]) {
        let name = str_of(&scenario["name"]);
        if scenario["unix"].as_bool().unwrap_or(false) && cfg!(windows) {
            continue;
        }
        // Права доступа не действуют на root.
        if name.contains("прав") && is_root_user() {
            continue;
        }
        let runner = Runner::new();
        setup(&runner.root, &scenario["setup"]);
        assert_eq!(
            items(&scenario["steps"]).len(),
            items(&scenario["results"]).len(),
            "шагов и результатов поровну"
        );
        for (index, step) in items(&scenario["steps"]).iter().enumerate() {
            let actual = runner.step(step);
            let expected = normalize_expected(step, &scenario["results"][index]);
            mismatches.check(&actual, &expected, format!("{name}: шаг {index} {step}"));
        }
        #[cfg(unix)]
        {
            // Папки без прав мешают удалению временного каталога.
            fn open_up(dir: &Path) {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
                if let Ok(read) = std::fs::read_dir(dir) {
                    for entry in read.flatten() {
                        if entry.file_type().is_ok_and(|t| t.is_dir()) {
                            open_up(&entry.path());
                        }
                    }
                }
            }
            open_up(&runner.root);
        }
    }
    mismatches.finish(&format!(
        "cfg-search / runs-scan / game-binds-install: {group}"
    ));
}

#[test]
fn constants_are_those_of_0_4_7() {
    let constants = &FIXTURE["constants"];
    assert_eq!(
        runs_scan::MAX_RESULTS as u64,
        constants["MAX_RESULTS"].as_u64().unwrap()
    );
    assert_eq!(
        runs_scan::MAX_ENTRIES_SCANNED as u64,
        constants["MAX_ENTRIES_SCANNED"].as_u64().unwrap()
    );
    assert_eq!(runs_scan::EXTENSION, str_of(&constants["EXTENSION"]));
    assert_eq!(
        game_binds_install::IMPORT_TRAILER_MARK,
        str_of(&constants["IMPORT_TRAILER_MARK"])
    );
}

#[test]
fn cfg_search_on_scenarios() {
    run_group("search");
}

#[test]
fn runs_scan_on_scenarios() {
    run_group("runs");
}

#[test]
fn game_binds_install_on_scenarios() {
    run_group("binds");
}

/// Язык Steam из реестра: значения `reg query` разных видов и кеш на 30 секунд (часы стоят).
#[test]
fn steam_language_from_registry_and_its_cache() {
    let section = &FIXTURE["registry"];
    let runner = Runner::new();
    setup(&runner.root, &section["setup"]);
    let registry = Arc::new(FakeRegistry::default());
    let base = Instant::now();
    let now = Arc::new(Mutex::new(base));
    let clock = {
        let now = Arc::clone(&now);
        Box::new(move || *now.lock().unwrap())
    };
    let search = CfgSearch::new(SteamLanguage::with(
        registry.clone(),
        Platform::Win32,
        clock,
    ));
    let gd = str_of(&section["gameDirectory"]);
    let mut mismatches = Mismatches::default();
    for ask in items(&section["asks"]) {
        *now.lock().unwrap() = base + Duration::from_millis(ask["at"].as_u64().unwrap());
        let value = &ask["value"];
        let key = "Software\\Valve\\Steam";
        match value {
            Value::Null => registry.remove(RegistryHive::CurrentUser, key, "Language"),
            _ => {
                let data = &value["data"];
                let registry_value = match str_of(&value["type"]) {
                    "REG_SZ" => RegistryValue::Sz(str_of(data).to_owned()),
                    "REG_EXPAND_SZ" => RegistryValue::ExpandSz(str_of(data).to_owned()),
                    _ => RegistryValue::Dword(data.as_u64().unwrap() as u32),
                };
                registry.set(RegistryHive::CurrentUser, key, "Language", registry_value);
            }
        }
        let before = registry.calls();
        let dirs = search
            .search_dirs(&runner.root, gd, "goldsrc", LanguageOption::Detect)
            .unwrap();
        let dirs: Vec<String> = dirs.iter().map(|d| rel(&runner.root, d)).collect();
        let context = format!("{} {}", ask["note"], value);
        mismatches.check(&json!(dirs), &ask["dirs"], format!("папки: {context}"));
        mismatches.check_eq(
            registry.calls() - before,
            ask["queried"].as_u64().unwrap() as usize,
            format!("вопросов к реестру: {context}"),
        );
    }
    mismatches.finish("cfg-search.js: язык Steam");
}

#[test]
fn steam_language_is_not_read_outside_windows() {
    let registry = Arc::new(FakeRegistry::default());
    registry.set(
        RegistryHive::CurrentUser,
        "Software\\Valve\\Steam",
        "Language",
        RegistryValue::Sz("russian".into()),
    );
    let language = SteamLanguage::with(registry.clone(), Platform::Linux, Box::new(Instant::now));
    assert_eq!(language.read(), None);
    assert_eq!(registry.calls(), 0, "реестр не спрашивают");
    let language = SteamLanguage::with(registry.clone(), Platform::Win32, Box::new(Instant::now));
    assert_eq!(language.read().as_deref(), Some("russian"));
}
