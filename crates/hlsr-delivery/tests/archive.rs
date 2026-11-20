//! ArchiveService (7-Zip): сверка с 0.4.7. Сценарии записаны скриптом
//! `tools/reference/delivery/archive.js` из настоящего `ArchiveService.js` 0.4.7:
//! листинг, проверка записей, распаковка, упаковка и чтение файла — на настоящем 7za
//! (p7zip 16.02 из `resources/7zip`), аргументы, коды выхода, отмена — на поддельном
//! 7za (скрипт sh). Прогресс распаковки node-7z сверяется на потоках кусков вывода.

mod support;

use hlsr_core::js;
use hlsr_core::json_text::js_number;
use hlsr_delivery::archive_service::{
    ArchiveService, CreateOptions, ExtractOptions, ExtractProgressParser, InspectArgs,
};
use hlsr_delivery::errors::{Code, DeliveryError, Failure};
use hlsr_delivery::ports::MemoryLogger;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{Mismatches, fixture, items, str_of, tree};

/// Тесты порождают процессы и пишут исполняемые скрипты: по одному, иначе только что
/// записанный скрипт может попасть в чужой `fork` и не запуститься («Text file busy»).
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    SERIAL.lock().await
}

fn archives_dir() -> PathBuf {
    support::fixture_path("archives")
}

fn real_7za() -> PathBuf {
    hlsr_core::paths::Resources::detect().sevenzip()
}

/// Сообщения 7za зависят от локали (strerror), время записей — от пояса.
fn pinned_env() -> [(&'static str, &'static str); 3] {
    [("LC_ALL", "C.UTF-8"), ("LANG", "C.UTF-8"), ("TZ", "UTC")]
}

fn service_with(bin: &Path) -> (ArchiveService, Arc<MemoryLogger>) {
    let logger = Arc::new(MemoryLogger::default());
    let service = ArchiveService::new(bin, logger.clone()).with_env(pinned_env());
    (service, logger)
}

// ---------------------------------------------------------------- содержимое и подмены

fn content_bytes(spec: &Value) -> Vec<u8> {
    if let Some(text) = spec.get("utf8").and_then(Value::as_str) {
        return text.as_bytes().to_vec();
    }
    if let Some(name) = spec.get("copyOf").and_then(Value::as_str) {
        return std::fs::read(archives_dir().join(name)).unwrap();
    }
    if let Some(byte) = spec.get("fill").and_then(Value::as_u64) {
        return vec![byte as u8; spec["count"].as_u64().unwrap() as usize];
    }
    js::buffer_from_base64(str_of(&spec["base64"]))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

/// 2024-01-02T03:04:05Z: время файлов дерева в сценариях (его 7za кладёт в архив; от него
/// зависит размер сжатого заголовка).
const FIXED_MTIME_SECS: u64 = 1_704_164_645;

fn set_mtime(path: &Path, time: std::time::SystemTime) {
    let is_dir = path.is_dir();
    // Каталог в Windows так не открыть; его времени в списках `create` нет.
    if cfg!(windows) && is_dir {
        return;
    }
    let file = if is_dir {
        std::fs::File::open(path)
    } else {
        std::fs::OpenOptions::new().write(true).open(path)
    };
    if let Ok(file) = file {
        let _ = file.set_modified(time);
    }
}

fn touch_all(dir: &Path, time: std::time::SystemTime) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            touch_all(&path, time);
        }
        set_mtime(&path, time);
    }
    set_mtime(dir, time);
}

/// Дерево из `setup.trees`: постоянные права и время, как у `writeTree` скрипта.
fn write_tree(dir: &Path, files: &Value) {
    std::fs::create_dir_all(dir).unwrap();
    for (rel, spec) in files.as_object().unwrap() {
        let full = dir.join(rel);
        if spec.get("dir").is_some() {
            std::fs::create_dir_all(&full).unwrap();
            set_mode(&full, 0o755);
            continue;
        }
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, content_bytes(spec)).unwrap();
        set_mode(&full, 0o644);
    }
    set_mode(dir, 0o755);
    touch_all(
        dir,
        std::time::UNIX_EPOCH + Duration::from_secs(FIXED_MTIME_SECS),
    );
}

struct Scrub {
    replacements: Vec<(String, String)>,
    list_name: regex::Regex,
}

impl Scrub {
    fn new(replacements: Vec<(String, &str)>) -> Scrub {
        Scrub {
            replacements: replacements
                .into_iter()
                .map(|(from, to)| (from, to.to_owned()))
                .collect(),
            list_name: regex::Regex::new(r"\.hlsr-(extract|pack)-[0-9]+-[0-9a-z]+\.txt").unwrap(),
        }
    }

    fn string(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (from, to) in &self.replacements {
            out = out.replace(from.as_str(), to);
        }
        self.list_name
            .replace_all(&out, ".hlsr-${1}-<pid>-<time>.txt")
            .into_owned()
    }

    fn value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.string(text)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.value(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| {
                        let item = if key == "modified" && item.is_string() {
                            json!("<time>")
                        } else {
                            self.value(item)
                        };
                        (key.clone(), item)
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// Сравнение «по сути» для 7-Zip 21.07 под Windows: пути через `\`, атрибуты без `_`,
/// текст ошибок 7za другой — остаются коды.
fn portable(value: &Value) -> Value {
    match value {
        Value::String(text) => json!(text.replace('\\', "/")),
        Value::Array(items) => Value::Array(items.iter().map(portable).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| {
                    let item = match (key.as_str(), item) {
                        ("path", Value::String(path)) => json!(path.replace('\\', "/")),
                        ("attributes", Value::String(attrs)) => json!(
                            attrs
                                .split(' ')
                                .map(|word| {
                                    if word.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                                        word.trim_end_matches('_').to_owned()
                                    } else {
                                        word.to_owned()
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        ),
                        ("Path", _) | ("message", _) => json!("<text>"),
                        ("archive", Value::Object(_)) => json!("<archive>"),
                        _ => portable(item),
                    };
                    (key.clone(), item)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Записи листинга как отсортированные тройки «путь, размер, CRC».
fn entry_set(outcome: &Value) -> Value {
    let mut rows: Vec<(String, String, String)> = outcome["ok"]["entries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["path"]
                            .as_str()
                            .unwrap_or_default()
                            .replace('\\', "/"),
                        entry["size"].to_string(),
                        entry["crc"].to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort();
    json!(rows)
}

// ---------------------------------------------------------------- сценарии

/// Пути в `call`: «archives:имя» — архив-фикстура, «root:путь» — внутри папки сценария.
fn resolve(spec: &str, root: &Path) -> PathBuf {
    if let Some(name) = spec.strip_prefix("archives:") {
        archives_dir().join(name)
    } else if let Some(rel) = spec.strip_prefix("root:") {
        if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(rel)
        }
    } else {
        PathBuf::from(spec)
    }
}

fn fake_script(log_file: &Path, body: &str) -> String {
    let log = serde_json::to_string(&log_file.to_string_lossy()).unwrap();
    format!(
        "#!/bin/sh\n{{ printf 'ARGV\\n'; for a in \"$@\"; do printf '%s\\n' \"$a\"; done\n  \
         for a in \"$@\"; do case \"$a\" in @*) printf 'LIST\\n'; cat \"${{a#@}}\" ;; esac; done\n  \
         printf 'CWD %s\\n' \"$PWD\"; }} >> {log}\n{body}\n"
    )
}

fn failure_value(failure: Failure) -> Value {
    match failure {
        Failure::Delivery(error) => json!({"error": error.to_public()}),
        Failure::Raw(error) => json!({"raw": {"code": error.code, "message": error.message}}),
    }
}

fn delivery_failure(error: DeliveryError) -> Value {
    failure_value(Failure::Delivery(error))
}

fn ok_value(value: Value) -> Value {
    json!({ "ok": value })
}

fn opt_f64(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn strings(value: &Value) -> Vec<String> {
    items(value).iter().map(|v| str_of(v).to_owned()).collect()
}

fn sorted_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by(|a, b| js::cmp_utf16(a, b));
    names
}

/// Что сценарий отдал: исход, прогресс (подряд идущие повторы убраны), предупреждения журнала.
struct Run {
    outcome: Value,
    progress: Vec<f64>,
    listing: Option<Value>,
}

fn dedupe(values: Vec<f64>) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    for value in values {
        if out.last() != Some(&value) {
            out.push(value);
        }
    }
    out
}

async fn run_call(service: &ArchiveService, scenario: &Value, root: &Path, scrub: &Scrub) -> Run {
    let call = &scenario["call"];
    let method = str_of(&call["method"]);
    let archive = resolve(str_of(&call["archive"]), root);
    let summary = scenario.get("summary").is_some();
    let progress = Arc::new(Mutex::new(Vec::<f64>::new()));
    let cancel_after = call.get("cancelAfterMs").and_then(Value::as_u64);
    let mut listing = None;

    let outcome = match method {
        "list" => match service.list(&archive).await {
            Ok(result) if summary => {
                let entries = &result.entries;
                ok_value(json!({
                    "count": entries.len(),
                    "first": entries.first().map(|e| e.path.clone()),
                    "last": entries.last().map(|e| e.path.clone()),
                    "sizes": js_number(entries.iter().map(|e| e.size).sum()),
                }))
            }
            Ok(result) => ok_value(result.to_json()),
            Err(error) => delivery_failure(error),
        },
        "inspect" => {
            let options = call.get("options").cloned().unwrap_or(json!({}));
            let args = InspectArgs {
                max_entries: opt_f64(&options, "maxEntries"),
                max_uncompressed_size: opt_f64(&options, "maxUncompressedSize"),
                root_prefix: options
                    .get("rootPrefix")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
            match service.inspect(&archive, &args).await {
                Ok(result) => ok_value(json!({
                    "listing": result.listing.to_json(),
                    "report": result.report.to_json(),
                })),
                Err(error) => delivery_failure(error),
            }
        }
        "extract" => {
            let sink = progress.clone();
            let job = service.extract(
                &archive,
                &resolve(str_of(&call["dest"]), root),
                ExtractOptions {
                    on_progress: Some(Arc::new(move |fraction| {
                        sink.lock().unwrap().push(fraction);
                    })),
                },
            );
            if let Some(ms) = cancel_after {
                let handle = job.cancel_handle();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    handle.cancel();
                });
            }
            match job.await {
                Ok(done) => ok_value(json!({"destination": done.destination.to_string_lossy()})),
                Err(error) => delivery_failure(error),
            }
        }
        "create" => {
            let sink = progress.clone();
            let options = CreateOptions {
                cwd: call
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(|c| resolve(c, root)),
                files: strings(&call["files"]),
                head_files: call
                    .get("headFiles")
                    .map(strings)
                    .unwrap_or_default()
                    .iter()
                    .map(|file| resolve(file, root).to_string_lossy().into_owned())
                    .collect(),
                list_dir: call
                    .get("listDir")
                    .and_then(Value::as_str)
                    .map(|c| resolve(c, root)),
                threads: opt_f64(call, "threads"),
                on_progress: Some(Arc::new(move |percent| {
                    sink.lock().unwrap().push(f64::from(percent));
                })),
            };
            let job = service.create(&archive, options);
            if let Some(ms) = cancel_after {
                let handle = job.cancel_handle();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    handle.cancel();
                });
            }
            match job.await {
                Ok(done) => {
                    if scenario["bin"] == "real" {
                        listing = Some(match service.list(&archive).await {
                            Ok(result) => ok_value(result.to_json()),
                            Err(error) => delivery_failure(error),
                        });
                    }
                    ok_value(json!({"archivePath": done.archive_path.to_string_lossy()}))
                }
                Err(error) => delivery_failure(error),
            }
        }
        "readText" => {
            let max_bytes = call.get("maxBytes").and_then(Value::as_u64);
            match service
                .read_text(&archive, str_of(&call["entry"]), max_bytes)
                .await
            {
                Ok(text) if summary => ok_value(json!({
                    "length": text.encode_utf16().count(),
                    "sha256": hex(&Sha256::digest(text.as_bytes())),
                })),
                Ok(text) => ok_value(json!(text)),
                Err(error) => delivery_failure(error),
            }
        }
        other => panic!("неизвестный метод {other}"),
    };
    let progress = dedupe(progress.lock().unwrap().clone());
    Run {
        outcome: scrub.value(&outcome),
        progress,
        listing: listing.map(|l| scrub.value(&l)),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Этот сценарий на этой ОС сверяется целиком (Linux) или «по сути» (Windows, 7-Zip 21.07).
fn comparable_on_this_os(scenario: &Value) -> bool {
    if !cfg!(windows) {
        return true;
    }
    // Поддельный 7za — скрипт sh. Символьные ссылки и «..» 7-Zip 21.07 для Windows
    // раскладывает иначе (листинг тех же архивов — в 7za-listings.json).
    let id = str_of(&scenario["id"]);
    // Код ошибки `mkdir` на файле в пути в Windows — свой (его проверяет `node_fs`).
    scenario["bin"] == "real" && !matches!(id, "extract/symlink" | "extract/traversal")
}

fn expected_scrub(root: &Path, aux: &Path) -> Scrub {
    let cwd = std::env::current_dir().unwrap();
    Scrub::new(vec![
        (root.to_string_lossy().into_owned(), "<root>"),
        (archives_dir().to_string_lossy().into_owned(), "<archives>"),
        (aux.to_string_lossy().into_owned(), "<aux>"),
        (cwd.to_string_lossy().into_owned(), "<inherited>"),
    ])
}

async fn run_scenario(scenario: &Value, mismatches: &mut Mismatches) {
    let id = str_of(&scenario["id"]);
    let root_guard = tempfile::tempdir().unwrap();
    let aux_guard = tempfile::tempdir().unwrap();
    // Подмены по настоящим путям: на Linux временные папки без ссылок в пути.
    let root = std::fs::canonicalize(root_guard.path()).unwrap();
    let aux = std::fs::canonicalize(aux_guard.path()).unwrap();
    if let Some(trees) = scenario.get("setup").and_then(|s| s.get("trees")) {
        for (dir, files) in trees.as_object().unwrap() {
            write_tree(&root.join(dir), files);
        }
    }

    let log_file = aux.join("fake.log");
    let bin = if scenario["bin"] == "real" {
        real_7za()
    } else {
        let bin = aux.join("7za-fake");
        write_script(
            &bin,
            &fake_script(&log_file, str_of(&scenario["bin"]["fake"])),
        );
        if scenario["bin"].get("missing").is_some() {
            aux.join("нет-7za")
        } else {
            bin
        }
    };
    let (service, logger) = service_with(&bin);
    let scrub = expected_scrub(&root, &aux);
    let expect = &scenario["expect"];
    let run = run_call(&service, scenario, &root, &scrub).await;

    let mut run = run;
    // Текст ошибки node-7z у настоящего 7za зависит от разбиения stderr на куски (в 0.4.7 —
    // от запуска к запуску): сверяются код и начало текста.
    if scenario.get("loose").is_some()
        && let Some(message) = run.outcome["error"]["message"].as_str()
    {
        let head = format!("{}: ", message.split(": ").next().unwrap_or_default());
        run.outcome["error"]["message"] = json!(head);
    }
    let strict = !cfg!(windows);
    let view = |value: &Value| {
        if strict {
            value.clone()
        } else {
            portable(value)
        }
    };
    let outcome = if strict {
        run.outcome.clone()
    } else {
        // Текст ошибок 7za — другой версии; сравниваются коды.
        portable(&run.outcome)
    };
    mismatches.check(&outcome, &view(&expect["outcome"]), format!("{id}: исход"));

    if expect.get("progress").is_some() && !strict {
        // 7-Zip 21.07 печатает и конечные проценты; сверяется только порядок.
        let sorted = run.progress.windows(2).all(|pair| pair[0] <= pair[1]);
        mismatches.check_eq(
            (sorted, run.progress.iter().all(|p| (0.0..=1.0).contains(p))),
            (true, true),
            format!("{id}: прогресс {:?}", run.progress),
        );
    } else if expect.get("progress").is_some() {
        mismatches.check_eq(
            run.progress.clone(),
            expect["progress"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect::<Vec<_>>(),
            format!("{id}: прогресс"),
        );
    }
    let warnings: Vec<Value> = logger
        .take()
        .into_iter()
        .map(|(level, message)| {
            assert_eq!(level, "warn", "{id}");
            json!(scrub.string(&message))
        })
        .collect();
    mismatches.check(
        &Value::Array(warnings),
        &expect.get("warnings").cloned().unwrap_or(json!([])),
        format!("{id}: предупреждения журнала"),
    );
    if scenario["bin"] != "real" {
        let record = std::fs::read_to_string(&log_file).unwrap_or_default();
        mismatches.check(
            &json!(scrub.string(&record)),
            &expect["record"],
            format!("{id}: аргументы и списки"),
        );
    }
    if let Some(expected) = expect.get("listing") {
        let actual = run.listing.as_ref().unwrap_or(&Value::Null);
        if strict {
            mismatches.check(actual, expected, format!("{id}: листинг созданного архива"));
        } else {
            // Порядок блоков, сжатие и атрибуты у 7-Zip 21.07 другие: набор записей и их содержимое.
            mismatches.check(
                &entry_set(actual),
                &entry_set(expected),
                format!("{id}: записи созданного архива"),
            );
        }
    }
    if let Some(after) = expect.get("after") {
        let names_only = scenario.get("namesOnly").is_some();
        for (name, expected) in after.as_object().unwrap() {
            let dir = resolve(name, &root);
            let actual = if !dir.exists() {
                Value::Null
            } else if names_only {
                json!(sorted_names(&dir))
            } else {
                scrub.value(&tree::snapshot(&dir, None))
            };
            mismatches.check(&actual, expected, format!("{id}: {name} после операции"));
        }
    }
}

#[cfg(unix)]
fn write_script(path: &Path, text: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(path)
        .unwrap();
    file.write_all(text.as_bytes()).unwrap();
    file.sync_all().unwrap();
}

#[cfg(not(unix))]
fn write_script(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

#[tokio::test]
async fn scenarios_match_0_4_7() {
    let _serial = serial().await;
    let fixture = fixture("archive.json");
    let mut mismatches = Mismatches::default();
    let mut ran = 0;
    for scenario in items(&fixture["scenarios"]) {
        // `extractFiles` в 0.4.9 удалён: ремонт идёт только с зеркала.
        if !comparable_on_this_os(scenario) || scenario["call"]["method"] == "extractFiles" {
            continue;
        }
        run_scenario(scenario, &mut mismatches).await;
        ran += 1;
    }
    assert!(ran > 50, "сценариев прогнано: {ran}");
    mismatches.finish("ArchiveService");
}

#[test]
fn committed_archives_are_the_ones_the_script_made() {
    let fixture = fixture("archive.json");
    for archive in items(&fixture["archives"]) {
        let name = str_of(&archive["name"]);
        let bytes = std::fs::read(archives_dir().join(name)).unwrap();
        assert_eq!(
            bytes.len() as u64,
            archive["size"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            hex(&Sha256::digest(&bytes)),
            str_of(&archive["sha256"]),
            "{name}"
        );
    }
}

// ---------------------------------------------------------------- разбор вывода node-7z

#[test]
fn progress_parser_matches_node_7z_on_every_chunking() {
    let fixture = fixture("archive.json");
    let parser = &fixture["parser"];
    let order = strings(&parser["sampleOrder"]);
    let samples: Vec<Vec<u8>> = order
        .iter()
        .map(|name| js::buffer_from_base64(str_of(&parser["samples"][name.as_str()])))
        .collect();
    let outcomes = items(&parser["outcomes"]);
    let mut mismatches = Mismatches::default();
    for case in items(&parser["cases"]) {
        let case = items(case);
        let sample = case[0].as_u64().unwrap() as usize;
        let bytes = &samples[sample];
        // "each" — по одному байту; иначе — места разрезов.
        let cuts: Vec<usize> = match case[1].as_str() {
            Some("each") => (1..bytes.len()).collect(),
            _ => items(&case[1])
                .iter()
                .map(|c| c.as_u64().unwrap() as usize)
                .collect(),
        };
        let mut parser = ExtractProgressParser::new();
        let mut percents: Vec<f64> = Vec::new();
        let mut from = 0;
        for cut in cuts.iter().copied().chain([bytes.len()]) {
            percents.extend(parser.feed(&bytes[from..cut]));
            from = cut;
        }
        let expected: Vec<f64> = items(&outcomes[case[2].as_u64().unwrap() as usize])
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        mismatches.check_eq(
            percents,
            expected,
            format!(
                "{} / куски {:?}",
                order[sample],
                cuts.iter().take(12).collect::<Vec<_>>()
            ),
        );
    }
    mismatches.finish("разбор вывода node-7z");
}

// ---------------------------------------------------------------- walk

#[test]
fn walk_matches_0_4_7() {
    let fixture = fixture("archive.json");
    let walk = &fixture["walk"];
    let root = tempfile::tempdir().unwrap();
    for (rel, content) in walk["tree"].as_object().unwrap() {
        let full = root.path().join(rel);
        match content.as_str() {
            Some(text) => {
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(&full, text).unwrap();
            }
            None => std::fs::create_dir_all(&full).unwrap(),
        }
    }
    for case in items(&walk["cases"]) {
        let limit = case["limit"].as_u64().map(|l| l as usize);
        assert_eq!(
            json!(ArchiveService::walk(root.path(), limit)),
            case["result"],
            "limit {limit:?}"
        );
    }
    assert_eq!(
        json!(ArchiveService::walk(&root.path().join("нет"), None)),
        walk["missing"]
    );
}

// ---------------------------------------------------------------- то, чего у Node не снять

/// Скрипт-«7za» для тестов без сценария. Без `sh` (Windows) они не идут.
#[cfg(unix)]
fn fake_bin(dir: &Path, body: &str) -> PathBuf {
    let bin = dir.join("7za-fake");
    write_script(&bin, &format!("#!/bin/sh\n{body}\n"));
    bin
}

/// Зашифрованные архивы. `extract` (без `-p`) в 0.4.7 ждёт пароль вечно, пока
/// stdin открыт (спецификация, решение 10); здесь stdin пуст — отказ сразу. `list` и `readText`
/// передают `-p` и не ждут ни там, ни здесь.
#[tokio::test]
async fn encrypted_archives_do_not_wait_for_a_password() {
    if cfg!(windows) {
        return;
    }
    let _serial = serial().await;
    let (service, _logger) = service_with(&real_7za());
    let encrypted = archives_dir().join("encrypted-headers.7z");
    let listed = tokio::time::timeout(Duration::from_secs(60), service.list(&encrypted))
        .await
        .expect("list не должен ждать пароль");
    assert_eq!(listed.unwrap_err().code, Code::ArchiveListFailed);

    let dir = tempfile::tempdir().unwrap();
    let extracted = tokio::time::timeout(
        Duration::from_secs(60),
        service.extract(
            &encrypted,
            &dir.path().join("out"),
            ExtractOptions::default(),
        ),
    )
    .await
    .expect("extract не должен ждать пароль");
    assert_eq!(extracted.unwrap_err().code, Code::ExtractFailed);

    let inner = archives_dir().join("encrypted.7z");
    let read = tokio::time::timeout(
        Duration::from_secs(60),
        service.read_text(&inner, "pack/ghostingmod/file.txt", Some(100)),
    )
    .await
    .expect("read_text не должен ждать пароль");
    // Пароль пуст: данные расшифрованы неверно, 7za отвечает ошибкой CRC и кодом 2.
    let error = read.unwrap_err();
    assert_eq!(error.code, Code::ArchiveListFailed);
    assert!(
        error.message.contains("CRC Failed in encrypted file"),
        "{}",
        error.message
    );
}

#[cfg(unix)]
#[tokio::test]
async fn list_gives_up_after_the_timeout_and_kills_7za() {
    let _serial = serial().await;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("alive");
    let bin = fake_bin(
        dir.path(),
        &format!("echo $$ > '{}'\nexec sleep 30", marker.display()),
    );
    let (service, _logger) = service_with(&bin);
    let service = service.with_list_timeout(Duration::from_millis(400));
    let started = std::time::Instant::now();
    let error = service.list(&dir.path().join("a.7z")).await.unwrap_err();
    assert_eq!(error.code, Code::ArchiveListFailed);
    assert_eq!(error.message, "7-Zip не ответил за отведённое время");
    assert!(started.elapsed() < Duration::from_secs(10));
    // Процесс убит: `sleep 30` не остаётся.
    let pid: i32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists() || is_zombie(pid),
        "7za pid {pid} жив после тайм-аута"
    );
}

#[cfg(unix)]
fn is_zombie(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| {
            stat.rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        })
        .unwrap_or(true)
}

/// Отмена до первого опроса: процесс не запускается (в 0.4.7 `cancel()` возвращается
/// вместе с уже идущим процессом, раньше него вызвать нельзя).
#[cfg(unix)]
#[tokio::test]
async fn cancel_before_start_never_spawns() {
    let _serial = serial().await;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let bin = fake_bin(dir.path(), &format!("touch '{}'", marker.display()));
    let (service, _logger) = service_with(&bin);
    let job = service.extract(
        &dir.path().join("a.7z"),
        &dir.path().join("out"),
        ExtractOptions::default(),
    );
    job.cancel_handle().cancel();
    let error = job.await.unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (Code::Cancelled, "Распаковка отменена")
    );
    let job = service.create(
        &dir.path().join("p.hlsrpack"),
        CreateOptions {
            cwd: Some(dir.path().to_path_buf()),
            files: vec!["a".into()],
            ..CreateOptions::default()
        },
    );
    job.cancel_handle().cancel();
    let error = job.await.unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (Code::Cancelled, "Экспорт отменён")
    );
    assert!(!marker.exists());
}

/// Выброшенная задача убивает 7za (в 0.4.7 при `detached: true` он бы дорабатывал сам).
#[cfg(unix)]
#[tokio::test]
async fn dropping_a_job_kills_7za() {
    let _serial = serial().await;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("pid");
    let bin = fake_bin(
        dir.path(),
        &format!("echo $$ > '{}'\nexec sleep 30", marker.display()),
    );
    let (service, _logger) = service_with(&bin);
    let job = service.extract(
        &dir.path().join("a.7z"),
        &dir.path().join("out"),
        ExtractOptions::default(),
    );
    let _ = tokio::time::timeout(Duration::from_millis(500), job).await;
    let pid: i32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!Path::new(&format!("/proc/{pid}")).exists() || is_zombie(pid));
}

/// Windows: блокировку антивирусом 7-Zip сообщает текстом вируса в stderr; на Linux
/// проверка выключена (`isAntivirusBlock` смотрит на `process.platform`).
#[cfg(unix)]
#[tokio::test]
async fn antivirus_text_in_stderr_maps_to_antivirus_blocked_on_windows_only() {
    use hlsr_core::js::Platform;
    use hlsr_delivery::errors::antivirus_error;
    let _serial = serial().await;
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("Игра.7z");
    let stderr = "ERROR: Can not open file as archive\nOperation did not complete successfully because the file contains a virus";
    let bin = fake_bin(
        dir.path(),
        &format!("printf '%s\\n' '{stderr}' >&2\nexit 2"),
    );
    let trimmed = stderr.to_owned();
    for platform in [Platform::Win32, Platform::Linux] {
        let (service, _logger) = service_with(&bin);
        let service = service.with_platform(platform);
        let create: DeliveryError = service
            .create(
                &archive,
                CreateOptions {
                    cwd: Some(dir.path().to_path_buf()),
                    files: vec!["a".into()],
                    ..CreateOptions::default()
                },
            )
            .await
            .unwrap_err();
        let read_text: DeliveryError = service
            .read_text(&archive, "x", Some(10))
            .await
            .unwrap_err();
        let extract: DeliveryError = service
            .extract(&archive, &dir.path().join("o2"), ExtractOptions::default())
            .await
            .unwrap_err();
        if platform == Platform::Win32 {
            let archive = archive.to_string_lossy();
            let expected = antivirus_error(Some(&archive), &trimmed);
            for error in [create, read_text] {
                assert_eq!(error, expected);
            }
            // extract: причина — сообщение ошибки node-7z (текст после «ERROR: »).
            assert_eq!(
                extract,
                antivirus_error(Some(&archive), "Can not open file as archive")
            );
        } else {
            assert_eq!(create.code, Code::PackFailed);
            assert_eq!(read_text.code, Code::ArchiveListFailed);
            assert_eq!(extract.code, Code::ExtractFailed);
        }
    }
}
