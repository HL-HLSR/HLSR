//! Чанки против 0.4.7: фикстура `chunks.json` из `tools/reference/update/chunks.js` (модуль
//! `launcher-update/chunks.js` в Node 24 на тех же входах; серверы 127.0.0.1, `https` подменён на
//! `http`, allowlist — loopback без путей `/blocked…`, как в самотестах 0.4.7 подменяют
//! `isAllowedUrl`). Данные файлов в фикстуре — описания: Node и Rust строят одни и те же байты
//! детерминированным генератором (xorshift32), поэтому сверяются границы кусков, хеши, манифест,
//! запросы и итоговые деревья; сжатые байты — у образцов Node (`samples`): свой brotli (качество 9)
//! даёт те же байты (`own_compression_matches_node_bytes`). Размеры (`csize`) в манифестах сценариев
//! заменяют значения Node, а в сообщении «размер N != M» — метки `{csize-1}` и `{csize}`.
//!
//! Самотесты `launcher-chunks-selftest.js`: «нарезка: куски покрывают файл…» (`cut_properties_…`),
//! «сборка версии: совпавшее берётся с диска…» (случай «update: whole files…» и
//! `eight_requests_at_most…`), «подменённый кусок, чужой манифест и адрес вне allowlist…»
//! (случаи `chunk is not brotli`, `chunk with other content…`, `expected version differs`,
//! `path traversal`, `redirected out of the allowlist`); тесты 4 (Linux) и 5 (Windows-апдейтер) —
//! задачи 3 и 6.
//!
//! Тексты сетевых ошибок Node, V8 (`JSON.parse`) и zlib в Rust другие: в сверке их заменяют метки
//! `network`, `json`, `decode` (`kind`), остальные тексты сравниваются дословно.

mod support;

use hlsr_core::js::Platform;
use hlsr_update::cancel::CancelToken;
use hlsr_update::chunks::{
    self, AllowUrl, AssembleOptions, ChunkError, ChunkFetch, ChunkInfo, Delta, DescribeOptions,
    Expected, FetchManifestOptions, FetchOptions, HttpsFetch, Manifest, ManifestFile, Progress,
};
use hlsr_update::ports::{BoxFuture, FakeClock, Logger, MemoryLogger};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use support::http::{self, RequestLog, Server};
use support::{Mismatches, fixture, items, sha256_hex, str_of, tree};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("chunks.json"));

// ------------------------------------------------------------ данные файлов

/// xorshift32, старший байт состояния (`stream` скрипта сверки).
fn stream(seed: u32, size: usize) -> Vec<u8> {
    let mut state = if seed == 0 { 1 } else { seed };
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 24) as u8
        })
        .collect()
}

const WORDS: [&str; 24] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
    "kilo", "lima", "mike", "november", "oscar", "papa", "quebec", "romeo", "sierra", "tango",
    "uniform", "victor", "whiskey", "xray",
];

fn text(seed: u32, size: usize) -> Vec<u8> {
    let picks = stream(seed, size);
    let mut out = String::new();
    let mut i = 0;
    while out.len() < size {
        out.push_str(WORDS[usize::from(picks[i]) % WORDS.len()]);
        out.push(' ');
        i += 1;
    }
    out.truncate(size);
    out.into_bytes()
}

fn seed_size(pair: &Value) -> (u32, usize) {
    (
        pair[0].as_u64().expect("зерно") as u32,
        pair[1].as_u64().expect("размер") as usize,
    )
}

/// Описание данных фикстуры → байты.
fn data(spec: &Value) -> Vec<u8> {
    if let Some(pair) = spec.get("stream") {
        let (seed, size) = seed_size(pair);
        return stream(seed, size);
    }
    if let Some(pair) = spec.get("text") {
        let (seed, size) = seed_size(pair);
        return text(seed, size);
    }
    if let Some(size) = spec.get("zeros") {
        return vec![0; size.as_u64().expect("размер") as usize];
    }
    if let Some(pair) = spec.get("pattern") {
        let unit = from_hex(str_of(&pair[0]));
        let size = pair[1].as_u64().expect("размер") as usize;
        return (0..size).map(|i| unit[i % unit.len()]).collect();
    }
    if let Some(literal) = spec.get("literal") {
        return str_of(literal).as_bytes().to_vec();
    }
    if let Some(parts) = spec.get("concat") {
        return items(parts).iter().flat_map(data).collect();
    }
    if let Some(patch) = spec.get("patch") {
        let mut out = data(&patch["of"]);
        let at = patch["at"].as_u64().expect("смещение") as usize;
        let with = data(&patch["with"]);
        out[at..at + with.len()].copy_from_slice(&with);
        return out;
    }
    if let Some(slice) = spec.get("slice") {
        let bytes = data(&slice[0]);
        let (from, to) = (
            slice[1].as_u64().unwrap() as usize,
            slice[2].as_u64().unwrap() as usize,
        );
        return bytes[from..to].to_vec();
    }
    panic!("описание данных {spec}");
}

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("шестнадцатеричная запись"))
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn platform(name: &str) -> Platform {
    match name {
        "win32" => Platform::Win32,
        "linux" => Platform::Linux,
        other => panic!("платформа {other}"),
    }
}

/// Разрешён `http` на 127.0.0.1, кроме путей `/blocked…`.
fn allow_loopback(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "http"
        && parsed.host_str() == Some("127.0.0.1")
        && !parsed.path().starts_with("/blocked")
}

fn allow() -> AllowUrl {
    Arc::new(allow_loopback)
}

// ------------------------------------------------------------ gear и границы

#[test]
fn gear_table_matches_node() {
    let expected: Vec<u32> = items(&FIXTURE["gear"])
        .iter()
        .map(|value| value.as_u64().unwrap() as u32)
        .collect();
    assert_eq!(chunks::GEAR.to_vec(), expected);
}

#[test]
fn cut_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["cut"]) {
        let pieces: Vec<Value> = chunks::cut(&data(&case["data"]))
            .iter()
            .map(|range| json!([range.start, range.end]))
            .collect();
        mismatches.check(
            &Value::Array(pieces),
            &case["pieces"],
            format!("cut: {}", str_of(&case["id"])),
        );
    }
    mismatches.finish("cut");
}

/// Самотест «нарезка…»: куски покрывают файл, укладываются в пределы и не зависят от сдвига.
#[test]
fn cut_properties_hold() {
    let bytes = stream(5, 3 * 1024 * 1024);
    let pieces = chunks::cut(&bytes);
    assert_eq!(pieces[0].start, 0);
    assert_eq!(pieces.last().unwrap().end, bytes.len());
    for (i, piece) in pieces.iter().enumerate() {
        if i > 0 {
            assert_eq!(piece.start, pieces[i - 1].end);
        }
        assert!(piece.len() <= chunks::MAX_CHUNK);
        if i + 1 < pieces.len() {
            assert!(piece.len() >= chunks::MIN_CHUNK);
        }
    }
    assert_eq!(chunks::cut(&bytes), pieces);
    // Вставка в начало меняет только первые куски: остальные находятся снова.
    let shifted: Vec<u8> = "вставка".bytes().chain(bytes.iter().copied()).collect();
    let hashes: HashSet<String> = pieces
        .iter()
        .map(|range| chunks::sha256(&bytes[range.clone()]))
        .collect();
    let again = chunks::cut(&shifted)
        .iter()
        .filter(|range| hashes.contains(&chunks::sha256(&shifted[(*range).clone()])))
        .count();
    assert!(
        again >= pieces.len() - 2,
        "найдено заново {again} из {}",
        pieces.len()
    );
}

#[test]
fn hashes_and_addresses() {
    assert_eq!(
        chunks::sha256(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    let hash = "ab".repeat(32);
    assert_eq!(chunks::chunk_path(&hash), format!("ab/{hash}"));
    for base in [
        "https://hlsr.pro/downloads/launcher/chunks",
        "https://hlsr.pro/downloads/launcher/chunks/",
        "https://hlsr.pro/downloads/launcher/chunks///",
    ] {
        assert_eq!(
            chunks::chunk_url(base, &hash),
            format!("https://hlsr.pro/downloads/launcher/chunks/ab/{hash}")
        );
    }
}

// ------------------------------------------------------------ сжатие

#[test]
fn node_samples_decode_to_their_data() {
    for sample in items(&FIXTURE["samples"]) {
        let id = str_of(&sample["id"]);
        let packed = from_hex(str_of(&sample["packedHex"]));
        let size = sample["size"].as_u64().unwrap();
        let raw = chunks::decompress(&packed, size).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(raw, data(&sample["data"]), "{id}");
        assert_eq!(sha256_hex(&raw), str_of(&sample["sha256"]), "{id}");
    }
}

#[test]
fn decompress_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["decompress"]) {
        let packed = from_hex(str_of(&case["packedHex"]));
        let actual = match chunks::decompress(&packed, case["expectedSize"].as_u64().unwrap()) {
            Ok(raw) => json!({"ok": {"length": raw.len(), "sha256": sha256_hex(&raw)}}),
            Err(error) => json!({"error": error.kind()}),
        };
        mismatches.check(
            &actual,
            &case["result"],
            format!("decompress: {}", str_of(&case["id"])),
        );
    }
    mismatches.finish("decompress");
}

/// Своё сжатие даёт те же байты, что Node (brotli, качество 9): имена кусков — sha256 данных, но
/// сервер хранит сжатые байты, а `csize` манифеста выпуска сверяется по ним.
#[test]
fn own_compression_matches_node_bytes() {
    let samples = items(&FIXTURE["samples"]);
    assert!(samples.len() >= 9, "образцы фикстуры");
    let mut differing = Vec::new();
    for sample in samples {
        let id = str_of(&sample["id"]);
        let node = from_hex(str_of(&sample["packedHex"]));
        let own = chunks::compress(&data(&sample["data"]));
        if own != node {
            differing.push(format!(
                "{id}: Node {} байт, Rust {}",
                node.len(),
                own.len()
            ));
        }
    }
    assert!(
        differing.is_empty(),
        "байты brotli разошлись: {differing:#?}"
    );
}

#[test]
fn own_compression_round_trips_within_the_chunk_limit() {
    for raw in [
        text(1, 262_144),
        stream(2, 262_144),
        vec![0; 262_144],
        vec![7],
    ] {
        let packed = chunks::compress(&raw);
        assert!(packed.len() as u64 <= chunks::MAX_COMPRESSED_CHUNK);
        assert_eq!(chunks::decompress(&packed, raw.len() as u64).unwrap(), raw);
    }
}

// ------------------------------------------------------------ problemsOf

/// `{"$gen": n}` вместо списка файлов: n правильных пустых файлов `f0…` (как в скрипте сверки).
fn expand_gen(mut manifest: Value) -> Value {
    let count = manifest
        .get("files")
        .and_then(|files| files.get("$gen"))
        .and_then(Value::as_u64);
    if let Some(count) = count {
        let empty = chunks::sha256(b"");
        manifest["files"] = Value::Array(
            (0..count)
                .map(|i| json!({"path": format!("f{i}"), "size": 0, "sha256": empty, "chunks": []}))
                .collect(),
        );
    }
    manifest
}

fn expected_of(value: &Value) -> Expected<'_> {
    Expected {
        version: value.get("version").and_then(Value::as_str),
        platform: value.get("platform").and_then(Value::as_str),
    }
}

#[test]
fn problems_of_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["problems"]) {
        let manifest = expand_gen(case["manifest"].clone());
        let problems = chunks::problems_of(&manifest, &expected_of(&case["expected"]));
        mismatches.check(
            &json!(problems),
            &case["problems"],
            format!("problemsOf: {}", str_of(&case["id"])),
        );
    }
    mismatches.finish("problemsOf");
}

#[test]
fn good_manifest_round_trips_through_the_typed_form() {
    let case = items(&FIXTURE["problems"])
        .iter()
        .find(|case| case["id"] == "good")
        .expect("правильный манифест в фикстуре");
    let manifest = Manifest::from_value(&case["manifest"]).expect("разбор правильного манифеста");
    assert_eq!(manifest.version, "0.5.0");
    assert_eq!(manifest.platform, Platform::Linux);
    assert!(manifest.files[0].exec && !manifest.files[1].exec);
    assert_eq!(manifest.to_value(), case["manifest"]);
}

#[test]
fn delta_block_becomes_typed() {
    let delta = json!({
        "schema": 1,
        "manifest": {
            "filename": "HLSR-0.5.0-win32.chunks.json",
            "size": 123456,
            "sha256": "d".repeat(64),
            "urls": ["https://hlsr.pro/downloads/launcher/stable/HLSR-0.5.0-win32.chunks.json"],
        },
        "chunkBaseUrls": ["https://hlsr.pro/downloads/launcher/chunks"],
    });
    let typed = Delta::from_value(&delta, hlsr_update::channel::is_allowed_url).unwrap();
    assert_eq!(typed.filename, "HLSR-0.5.0-win32.chunks.json");
    assert_eq!(typed.size, 123456);
    assert_eq!(typed.sha256, "d".repeat(64));
    assert_eq!(
        typed.chunk_base_urls,
        ["https://hlsr.pro/downloads/launcher/chunks"]
    );
    // Тот же блок с чужим адресом отвергается теми же причинами, что у deltaProblems.
    let refused = Delta::from_value(&delta, |_| false).unwrap_err();
    assert_eq!(refused.len(), 2);
    assert!(refused[0].starts_with("delta.manifest.urls вне allowlist"));
    assert!(Delta::from_value(&json!(null), |_| true).is_err());
}

// ------------------------------------------------------------ деревья сборки

fn write_tree(root: &Path, files: &Value) {
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(root).unwrap();
    for file in items(files) {
        let target = root.join(str_of(&file["path"]));
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, data(&file["data"])).unwrap();
    }
}

fn exec_paths(files: &Value) -> HashSet<String> {
    items(files)
        .iter()
        .filter(|file| file["exec"] == true)
        .map(|file| str_of(&file["path"]).to_owned())
        .collect()
}

/// Манифест Rust с размерами после сжатия Node: остальное сравнивается как есть.
fn with_node_csize(manifest: &Manifest, node: &Value) -> Manifest {
    let mut manifest = manifest.clone();
    for (file, node_file) in manifest.files.iter_mut().zip(items(&node["files"])) {
        for (chunk, node_chunk) in file.chunks.iter_mut().zip(items(&node_file["chunks"])) {
            assert!(
                chunk
                    .csize
                    .is_some_and(|c| c > 0 && c <= chunks::MAX_COMPRESSED_CHUNK),
                "{}: размер после сжатия {:?}",
                file.path,
                chunk.csize
            );
            chunk.csize = node_chunk.get(2).and_then(Value::as_u64);
        }
    }
    manifest
}

fn store_listing(store: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(dirs) = std::fs::read_dir(store) else {
        return names;
    };
    for dir in dirs {
        let dir = dir.unwrap();
        for file in std::fs::read_dir(dir.path()).unwrap() {
            names.push(format!(
                "{}/{}",
                dir.file_name().to_string_lossy(),
                file.unwrap().file_name().to_string_lossy()
            ));
        }
    }
    names.sort();
    names
}

#[test]
fn describe_tree_and_build_store_match_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["trees"]) {
        let id = str_of(&case["id"]);
        let scratch = tempfile::tempdir().unwrap();
        let (root, store) = (scratch.path().join("tree"), scratch.path().join("store"));
        write_tree(&root, &case["files"]);
        for dir in case["dirs"].as_array().into_iter().flatten() {
            std::fs::create_dir_all(root.join(str_of(dir))).unwrap();
        }
        let execs = exec_paths(&case["files"]);
        let executable = |relative: &str| execs.contains(relative);
        let exclude: Vec<&str> = case["exclude"]
            .as_array()
            .into_iter()
            .flatten()
            .map(str_of)
            .collect();
        let version = str_of(&case["version"]);
        let platform = platform(str_of(&case["platform"]));
        let options = DescribeOptions {
            version,
            platform,
            exclude: &exclude,
            executable: Some(&executable),
        };
        let result = &case["result"];

        let mut seen = Vec::new();
        let described = chunks::describe_tree(&root, &options, &mut |hash, piece| {
            seen.push(json!([hash, piece.len()]));
            Ok(())
        })
        .unwrap();
        mismatches.check(
            &described.to_value(),
            &result["described"],
            format!("{id}: describeTree"),
        );
        mismatches.check(
            &Value::Array(seen),
            &result["seen"],
            format!("{id}: onChunk"),
        );

        let built = chunks::build_store(&root, &store, &options).unwrap();
        let aligned = with_node_csize(&built.manifest, &result["manifest"]);
        mismatches.check(
            &aligned.to_value(),
            &result["manifest"],
            format!("{id}: buildStore"),
        );
        mismatches.check_eq(
            aligned.to_json_line(),
            str_of(&result["jsonLine"]).to_owned(),
            format!("{id}: jsonLine"),
        );
        mismatches.check(
            &json!({"written": built.written, "reused": built.reused, "chunks": built.chunks}),
            &result["counts"],
            format!("{id}: счётчики"),
        );
        mismatches.check(
            &json!(store_listing(&store)),
            &result["store"],
            format!("{id}: хранилище"),
        );
        let second = chunks::build_store(&root, &store, &options).unwrap();
        mismatches.check(
            &json!({"written": second.written, "reused": second.reused, "chunks": second.chunks}),
            &result["second"],
            format!("{id}: повторная нарезка"),
        );
        let problems = chunks::problems_of(
            &built.manifest.to_value(),
            &Expected {
                version: Some(version),
                platform: Some(platform.as_str()),
            },
        );
        mismatches.check(
            &json!(problems),
            &result["problems"],
            format!("{id}: problemsOf"),
        );
    }
    mismatches.finish("describeTree/buildStore");
}

// ------------------------------------------------------------ серверы сценария

/// Серверы сценария и подстановка их адресов; `dead` — адрес без слушателя.
struct Servers {
    servers: Vec<Server>,
    map: BTreeMap<String, String>,
    log: RequestLog,
}

async fn start(names: &[String], dead: bool) -> Servers {
    let log = RequestLog::default();
    let mut servers = Vec::new();
    let mut map = BTreeMap::new();
    for name in names {
        let server = http::start_server(name, log.clone()).await;
        map.insert(name.clone(), server.authority.clone());
        servers.push(server);
    }
    if dead {
        map.insert(
            "dead".to_owned(),
            format!("127.0.0.1:{}", http::dead_port().await),
        );
    }
    Servers { servers, map, log }
}

impl Servers {
    fn server(&self, name: &str) -> &Server {
        self.servers
            .iter()
            .find(|s| s.name == name)
            .expect("сервер")
    }

    /// Маршруты сервера: объект `{путь: [ответы]}` с подстановкой адресов.
    fn set_routes(&self, name: &str, routes: &Value) {
        let filled = http::fill(routes, &self.map);
        *self.server(name).routes.lock().unwrap() =
            filled.as_object().expect("маршруты сервера").clone();
    }

    /// `[server, method, url, user-agent]` по алфавиту (потоки загрузки порядок не держат).
    fn request_lines(&self) -> Value {
        let mut lines: Vec<Value> = self
            .log
            .lock()
            .unwrap()
            .iter()
            .map(|entry| {
                json!([
                    entry["server"],
                    entry["method"],
                    entry["url"],
                    entry["headers"]["user-agent"]
                        .as_str()
                        .map_or(Value::Null, |ua| json!(ua)),
                ])
            })
            .collect();
        let mut lines = match http::unfill(&Value::Array(std::mem::take(&mut lines)), &self.map) {
            Value::Array(lines) => lines,
            _ => unreachable!(),
        };
        lines.sort_by_key(ToString::to_string);
        Value::Array(lines)
    }
}

/// Ошибка → класс и текст (текст только у своих сообщений), как `classify` скрипта сверки.
fn classify(error: &ChunkError, map: &BTreeMap<String, String>) -> Value {
    let message = http::unfill(&json!(error.to_string()), map);
    let own = |kind: String| json!({"kind": kind, "message": message});
    match error {
        ChunkError::Aborted
        | ChunkError::NotAllowed(_)
        | ChunkError::TooManyRedirects
        | ChunkError::Status { .. }
        | ChunkError::TooLarge { .. }
        | ChunkError::Timeout(_)
        | ChunkError::ManifestMismatch
        | ChunkError::ManifestInvalid(_)
        | ChunkError::ManifestUnavailable
        | ChunkError::ChunkCorrupt(_)
        | ChunkError::ChunkChanged(_)
        | ChunkError::FileMismatch(_) => own(error.kind()),
        ChunkError::ChunkSize {
            hash,
            got,
            expected,
        } => {
            assert_eq!(
                *got as u64 + 1,
                *expected,
                "сценарий укорачивает кусок на байт"
            );
            json!({
                "kind": "chunk-size",
                "message": format!("Кусок {hash}: размер {{csize-1}} != {{csize}}"),
            })
        }
        other => json!({"kind": other.kind(), "message": null}),
    }
}

/// Отмена через `ms` мс от начала (0 — до начала).
fn cancel_after(ms: Option<u64>) -> Option<CancelToken> {
    let ms = ms?;
    let token = CancelToken::new();
    if ms == 0 {
        token.cancel();
    } else {
        let later = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            later.cancel();
        });
    }
    Some(token)
}

/// Отмена «через N мс» в сценариях 0.4.7 попадает на запросы, которые сервер ещё держит (или уже
/// отдал ответ). По часам машины это не повторить: под нагрузкой срок наступает раньше, чем
/// запросы дошли, или позже, чем сервер ответил. Поэтому: отмена, которая обязана прервать
/// ожидание, идёт по условию (число запросов дошло до записанного), а сервер молчит почти
/// бесконечно; отмена, которая приходит после ответа, не происходит до конца вызова.
enum Abort {
    Off,
    Now,
    OnRequests(usize),
    AfterAnswer,
}

impl Abort {
    /// `aborted` — запись 0.4.7 говорит, что вызов оборвала отмена; `requests` — запросы записи.
    fn plan(case: &Value, aborted: bool, requests: &Value) -> Abort {
        match case["abortAfterMs"].as_u64() {
            None => Abort::Off,
            Some(0) => Abort::Now,
            Some(_) if aborted => Abort::OnRequests(items(requests).len()),
            Some(_) => Abort::AfterAnswer,
        }
    }

    /// Маршруты сценария: при отмене по условию задержки ответов растянуты.
    fn routes(&self, routes: &Value) -> Value {
        match self {
            Abort::OnRequests(_) => stretch_delays(routes),
            _ => routes.clone(),
        }
    }

    fn token(&self, journal: &RequestLog) -> Option<CancelToken> {
        match self {
            Abort::Off => None,
            Abort::Now => cancel_after(Some(0)),
            Abort::AfterAnswer => Some(CancelToken::new()),
            Abort::OnRequests(expected) => {
                let token = CancelToken::new();
                let (journal, later, expected) = (journal.clone(), token.clone(), *expected);
                tokio::spawn(async move {
                    // Предел (30 с) нужен только затем, чтобы сорванный тест не завис.
                    for _ in 0..3000 {
                        if journal.lock().unwrap().len() >= expected {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    later.cancel();
                });
                Some(token)
            }
        }
    }
}

/// Все `delayMs` маршрутов — 10 минут: ответ не приходит, пока его не оборвут.
fn stretch_delays(routes: &Value) -> Value {
    match routes {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let stretched = if key == "delayMs" && value.is_number() {
                        json!(600_000)
                    } else {
                        stretch_delays(value)
                    };
                    (key.clone(), stretched)
                })
                .collect(),
        ),
        Value::Array(list) => Value::Array(list.iter().map(stretch_delays).collect()),
        other => other.clone(),
    }
}

fn server_names(routes: &Value) -> Vec<String> {
    routes
        .as_object()
        .expect("серверы")
        .keys()
        .cloned()
        .collect()
}

// ------------------------------------------------------------ fetchBuffer

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_buffer_matches_node() {
    let mut mismatches = Mismatches::default();
    let fetch = HttpsFetch::new();
    for case in items(&FIXTURE["fetch"]) {
        let id = str_of(&case["id"]);
        let url_template = str_of(&case["url"]);
        let servers = start(
            &server_names(&case["routes"]),
            url_template.contains("{dead}"),
        )
        .await;
        let abort = Abort::plan(
            case,
            case["result"]["results"][0]["error"]["kind"] == "aborted",
            &case["result"]["requests"],
        );
        for name in server_names(&case["routes"]) {
            servers.set_routes(&name, &abort.routes(&case["routes"][&name]));
        }
        let url = http::fill_text(url_template, &servers.map);
        let mut results = Vec::new();
        for _ in 0..case["repeat"].as_u64().unwrap_or(1) {
            let options = FetchOptions {
                is_allowed: allow(),
                max_bytes: case["maxBytes"].as_u64().unwrap(),
                cancel: abort.token(&servers.log),
            };
            results.push(match fetch.fetch(&url, &options).await {
                Ok(body) => json!({"ok": {"length": body.len(), "sha256": sha256_hex(&body)}}),
                Err(error) => json!({"error": classify(&error, &servers.map)}),
            });
        }
        let actual = json!({"results": results, "requests": servers.request_lines()});
        mismatches.check(&actual, &case["result"], format!("fetchBuffer: {id}"));
    }
    mismatches.finish("fetchBuffer");
}

// ------------------------------------------------------------ fetchManifest

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_manifest_matches_node() {
    let mut mismatches = Mismatches::default();
    let fetch = HttpsFetch::new();
    for case in items(&FIXTURE["manifest"]) {
        let id = str_of(&case["id"]);
        let servers = start(&server_names(&case["routes"]), false).await;
        let abort = Abort::plan(
            case,
            case["result"]["result"]["error"]["kind"] == "aborted",
            &case["result"]["requests"],
        );
        for name in server_names(&case["routes"]) {
            servers.set_routes(&name, &abort.routes(&case["routes"][&name]));
        }
        let authority = &servers.map["p"];
        let delta = Delta {
            filename: "HLSR-0.5.0-linux.chunks.json".to_owned(),
            size: case["declared"]["size"].as_u64().unwrap(),
            sha256: str_of(&case["declared"]["sha256"]).to_owned(),
            manifest_urls: items(&case["urls"])
                .iter()
                .map(|url| format!("http://{authority}{}", str_of(url)))
                .collect(),
            chunk_base_urls: vec![format!("http://{authority}/chunks")],
        };
        let options = FetchManifestOptions {
            fetch: &fetch,
            is_allowed: allow(),
            expected: expected_of(&case["expected"]),
            cancel: abort.token(&servers.log),
        };
        let result = match chunks::fetch_manifest(&delta, &options).await {
            Ok(manifest) => json!({"ok": {
                "version": manifest.version,
                "platform": manifest.platform.as_str(),
                "files": manifest.files.len(),
            }}),
            Err(error) => json!({"error": classify(&error, &servers.map)}),
        };
        let actual = json!({"result": result, "requests": servers.request_lines()});
        mismatches.check(&actual, &case["result"], format!("fetchManifest: {id}"));
    }
    mismatches.finish("fetchManifest");
}

// ------------------------------------------------------------ assemble

/// Куски, которые `assemble` скачает: не в целых файлах и не среди кусков старых файлов, в
/// порядке манифеста, без повторов. Для каждого — `(путь файла, номер куска)`.
fn needed_chunks(manifest: &Manifest, reuse: &chunks::ReusePlan) -> Vec<(String, usize)> {
    let mut needed: Vec<(String, usize)> = Vec::new();
    let mut seen = HashSet::new();
    for file in &manifest.files {
        if reuse.whole.contains_key(&file.path) {
            continue;
        }
        for (index, chunk) in file.chunks.iter().enumerate() {
            if !reuse.local.contains_key(&chunk.hash) && seen.insert(chunk.hash.clone()) {
                needed.push((file.path.clone(), index));
            }
        }
    }
    needed
}

fn chunk_of<'a>(manifest: &'a Manifest, path: &str, index: usize) -> &'a ChunkInfo {
    &manifest
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap()
        .chunks[index]
}

/// Кусок сценария: `{needed: n}` — n-й из нужных, `{file, index}` — место в файле.
fn chunk_ref<'a>(
    manifest: &'a Manifest,
    needed: &[(String, usize)],
    reference: &Value,
) -> &'a ChunkInfo {
    match reference.get("needed").and_then(Value::as_u64) {
        Some(n) => {
            let (path, index) = &needed[n as usize];
            chunk_of(manifest, path, *index)
        }
        None => chunk_of(
            manifest,
            str_of(&reference["file"]),
            reference["index"].as_u64().unwrap() as usize,
        ),
    }
}

fn packed_of(store: &Path, hash: &str) -> Vec<u8> {
    std::fs::read(store.join(&hash[..2]).join(hash)).unwrap()
}

/// Ответ сервера для шага правки (`step` скрипта сверки).
fn route_step(change: &Value, hash: &str, size: u64, store: &Path, authority: &str) -> Value {
    let good = json!({"bodyHex": to_hex(&packed_of(store, hash))});
    if change.get("passthrough").is_some() {
        let mut step = good;
        for (key, value) in change.as_object().unwrap() {
            if key != "passthrough" {
                step[key] = value.clone();
            }
        }
        return step;
    }
    if let Some(redirect) = change.get("redirect").and_then(Value::as_str) {
        let location = format!("http://{authority}{}", redirect.replace("{hash}", hash));
        return json!({"status": 302, "headers": [["Location", location]], "body": ""});
    }
    if let Some(kind) = change.get("tamper").and_then(Value::as_str) {
        let bytes = packed_of(store, hash);
        return match kind {
            "garbage" => json!({"bodyHex": to_hex(&stream(77, bytes.len()))}),
            "short" => json!({"bodyHex": to_hex(&bytes[..bytes.len() - 1])}),
            "long" => json!({
                "bodyHex": to_hex(&[bytes.as_slice(), &[0]].concat()),
                "contentLength": bytes.len() + 5000,
                "cutAfter": bytes.len() + 1,
            }),
            "otherContent" => {
                json!({"bodyHex": to_hex(&chunks::compress(&stream(78, size as usize)))})
            }
            other => panic!("порча {other}"),
        };
    }
    change.clone()
}

/// «Ответ больше N байт: …/<sha256>» → N заменено на `{csize}` (у куска это его размер после
/// сжатия, он зависит от сжатия).
fn csize_label(message: &str, manifest: &Manifest) -> String {
    let rest = message
        .strip_prefix("Ответ больше ")
        .expect("сообщение о пределе");
    let (limit, tail) = rest.split_once(" байт: ").expect("предел и адрес");
    let hash = tail.rsplit('/').next().unwrap();
    let csize = manifest
        .files
        .iter()
        .flat_map(|file| &file.chunks)
        .find(|chunk| chunk.hash == hash)
        .and_then(|chunk| chunk.csize)
        .expect("кусок из манифеста");
    assert_eq!(
        limit,
        csize.to_string(),
        "предел — размер куска после сжатия"
    );
    format!("Ответ больше {{csize}} байт: {tail}")
}

async fn run_assemble(case: &Value, scratch: &Path) -> Value {
    let (next_root, old_root) = (scratch.join("next"), scratch.join("old"));
    let (store, target, cache) = (
        scratch.join("store"),
        scratch.join("target"),
        scratch.join("cache"),
    );
    write_tree(&next_root, &case["next"]);
    let execs = exec_paths(&case["next"]);
    let executable = |relative: &str| execs.contains(relative);
    let options = DescribeOptions {
        version: "0.5.0",
        platform: platform(str_of(&case["platform"])),
        exclude: &[],
        executable: Some(&executable),
    };
    let built = chunks::build_store(&next_root, &store, &options).unwrap();
    let original = built.manifest.clone();
    let mut manifest = built.manifest;
    let source = if case["old"].is_null() {
        None
    } else {
        write_tree(&old_root, &case["old"]);
        Some(old_root)
    };
    std::fs::create_dir_all(&target).unwrap();
    if case["cacheMissing"] != true {
        std::fs::create_dir_all(&cache).unwrap();
    }

    let reuse = chunks::plan_reuse(&manifest, source.as_deref(), None).unwrap();
    let needed = needed_chunks(&manifest, &reuse);

    // Куски, уже лежащие в кеше.
    let mut cached: Vec<(String, u64, &str)> = Vec::new();
    if case["cache"] == "all" {
        for file in &manifest.files {
            for chunk in &file.chunks {
                cached.push((chunk.hash.clone(), chunk.size, "good"));
            }
        }
    } else {
        for entry in case["cache"].as_array().into_iter().flatten() {
            let chunk = chunk_ref(&manifest, &needed, entry);
            let mode = entry["mode"].as_str().unwrap_or("good");
            cached.push((chunk.hash.clone(), chunk.size, mode));
        }
    }
    for (hash, size, mode) in cached {
        let raw = chunks::decompress(&packed_of(&store, &hash), size).unwrap();
        let place = cache.join(&hash);
        match mode {
            "good" => std::fs::write(&place, &raw).unwrap(),
            "wrongContent" => std::fs::write(&place, vec![0x55; raw.len()]).unwrap(),
            "wrongSize" => std::fs::write(&place, &raw[..raw.len() - 1]).unwrap(),
            "partial" => std::fs::write(cache.join(format!("{hash}.partial")), b"stale").unwrap(),
            other => panic!("режим кеша {other}"),
        }
    }

    // Правки манифеста после нарезки: ломают сборку.
    if let Some(mutate) = case.get("manifestMutate") {
        if let Some(sha) = mutate.get("sha256").and_then(Value::as_str) {
            let path = str_of(&mutate["file"]);
            manifest
                .files
                .iter_mut()
                .find(|f| f.path == path)
                .unwrap()
                .sha256 = sha.to_owned();
        }
        if let Some(n) = mutate.get("neededChunk").and_then(Value::as_u64) {
            let (path, index) = &needed[n as usize];
            let file: &mut ManifestFile =
                manifest.files.iter_mut().find(|f| &f.path == path).unwrap();
            if let Some(hash) = mutate.get("chunkHash").and_then(Value::as_str) {
                file.chunks[*index].hash = hash.to_owned();
            }
            if let Some(delta) = mutate.get("chunkSize").and_then(Value::as_u64) {
                file.chunks[*index].size += delta;
            }
        }
    }

    // Серверы: куски из хранилища по /chunks/<aa>/<sha256>, плюс правки сценария.
    let names: Vec<String> = items(&case["servers"])
        .iter()
        .map(|n| str_of(n).to_owned())
        .collect();
    let abort = Abort::plan(
        case,
        case["result"]["outcome"]["error"]["kind"] == "aborted",
        &case["result"]["requests"],
    );
    let servers = start(&names, false).await;
    let hashes: HashSet<String> = original
        .files
        .iter()
        .flat_map(|f| f.chunks.iter().map(|c| c.hash.clone()))
        .collect();
    let size_of = |hash: &str| {
        original
            .files
            .iter()
            .flat_map(|f| &f.chunks)
            .find(|c| c.hash == hash)
            .unwrap()
            .size
    };
    for name in &names {
        let authority = &servers.map[name];
        let mut routes = Map::new();
        for hash in &hashes {
            let good = json!([{"bodyHex": to_hex(&packed_of(&store, hash))}]);
            routes.insert(format!("/chunks/{}/{hash}", &hash[..2]), good.clone());
            routes.insert(format!("/elsewhere/{hash}"), good.clone());
            routes.insert(format!("/blocked/{hash}"), good);
        }
        let over = &case["routes"][name];
        let steps = |hash: &str, list: &Value| -> Value {
            Value::Array(
                items(list)
                    .iter()
                    .map(|change| route_step(change, hash, size_of(hash), &store, authority))
                    .collect(),
            )
        };
        if let Some(all) = over.get("all") {
            for hash in &hashes {
                routes.insert(format!("/chunks/{}/{hash}", &hash[..2]), steps(hash, all));
            }
        }
        if let Some(one) = over.get("chunk") {
            let hash = chunk_ref(&original, &needed, one).hash.clone();
            routes.insert(
                format!("/chunks/{}/{hash}", &hash[..2]),
                steps(&hash, &one["steps"]),
            );
        }
        servers.set_routes(name, &abort.routes(&Value::Object(routes)));
    }

    let bases: Vec<String> = items(&case["bases"])
        .iter()
        .map(|name| match str_of(name) {
            "blocked" => format!("http://{}/blocked/chunks", servers.map["p"]),
            name => format!(
                "http://{}/chunks{}",
                servers.map[name],
                case["baseSuffix"].as_str().unwrap_or("")
            ),
        })
        .collect();

    let log = Arc::new(MemoryLogger::default());
    let last: Arc<Mutex<Option<Progress>>> = Arc::default();
    let mutated = manifest.clone();
    let mut run = AssembleOptions::new(
        manifest,
        target.clone(),
        cache.clone(),
        bases,
        Arc::new(HttpsFetch::new()),
        allow(),
    );
    run.source_root = source;
    run.cancel = abort.token(&servers.log);
    run.log = Some(log.clone() as Arc<dyn Logger>);
    let recorded = last.clone();
    run.on_progress = Some(Arc::new(move |progress| {
        *recorded.lock().unwrap() = Some(progress);
    }));
    let result = chunks::assemble(run).await;

    let outcome = match &result {
        Ok(done) => json!({"ok": {
            "downloadedChunks": done.downloaded_chunks,
            "reusedBytes": done.reused_bytes,
        }}),
        Err(error) => {
            let mut classified = classify(error, &servers.map);
            if case["kindOnly"] == true {
                classified["message"] = Value::Null;
            }
            if classified["kind"] == "too-large" {
                classified["message"] =
                    json!(csize_label(str_of(&classified["message"]), &original));
            }
            json!({"error": classified})
        }
    };
    let final_progress = match (&result, *last.lock().unwrap()) {
        (Ok(done), Some(progress)) => {
            assert_eq!(progress.done, done.downloaded_bytes, "итог загрузки");
            json!(progress.done == progress.total)
        }
        (Ok(_), None) => json!(false),
        (Err(_), _) => Value::Null,
    };
    let log_lines: Vec<Value> = log
        .take()
        .into_iter()
        .map(|(_, line)| http::unfill(&json!(line), &servers.map))
        .collect();
    json!({
        "manifest": {"files": mutated.files.iter().map(|file| json!({
            "path": file.path,
            "chunks": file.chunks.iter().map(|c| json!([c.hash, c.size])).collect::<Vec<_>>(),
        })).collect::<Vec<_>>()},
        "outcome": outcome,
        "log": log_lines,
        "finalProgressIsTotal": final_progress,
        "requests": servers.request_lines(),
        "target": tree::snapshot(&target, None),
        "cache": if cache.exists() { tree::snapshot(&cache, None) } else { json!([]) },
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn assemble_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["assemble"]) {
        let scratch = tempfile::tempdir().unwrap();
        let actual = run_assemble(case, scratch.path()).await;
        mismatches.check(
            &actual,
            &case["result"],
            format!("assemble: {}", str_of(&case["id"])),
        );
    }
    mismatches.finish("assemble");
}

// ------------------------------------------------------------ свои проверки Rust

type Answer = BoxFuture<'static, Result<Vec<u8>, ChunkError>>;

/// Подделка `fetch`: ответ даёт функция по адресу; число одновременных запросов считается.
struct FnFetch {
    respond: Box<dyn Fn(String) -> Answer + Send + Sync>,
    current: AtomicUsize,
    peak: AtomicUsize,
    calls: AtomicUsize,
}

impl FnFetch {
    fn new(respond: impl Fn(String) -> Answer + Send + Sync + 'static) -> Arc<FnFetch> {
        Arc::new(FnFetch {
            respond: Box::new(respond),
            current: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        })
    }
}

impl ChunkFetch for FnFetch {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        options: &'a FetchOptions,
    ) -> BoxFuture<'a, Result<Vec<u8>, ChunkError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let future = (self.respond)(url.to_owned());
        let cancel = options.cancel.clone();
        Box::pin(async move {
            let result = match cancel {
                Some(cancel) => tokio::select! {
                    biased;
                    () = cancel.cancelled() => Err(ChunkError::Aborted),
                    result = future => result,
                },
                None => future.await,
            };
            self.current.fetch_sub(1, Ordering::SeqCst);
            result
        })
    }
}

/// Манифест из `count` файлов по одному куску: данные куска получаются из номера.
fn small_manifest(count: usize, size: usize) -> (Manifest, Vec<Vec<u8>>) {
    let mut files = Vec::new();
    let mut raws = Vec::new();
    for i in 0..count {
        let raw = stream(1000 + i as u32, size);
        let packed = chunks::compress(&raw);
        files.push(ManifestFile {
            path: format!("f{i}"),
            size: size as u64,
            sha256: chunks::sha256(&raw),
            exec: false,
            chunks: vec![ChunkInfo {
                hash: chunks::sha256(&raw),
                size: size as u64,
                csize: Some(packed.len() as u64),
            }],
        });
        raws.push(raw);
    }
    (
        Manifest {
            version: "0.5.0".to_owned(),
            platform: Platform::Linux,
            files,
            attest: None,
        },
        raws,
    )
}

fn serve_from(raws: Vec<Vec<u8>>) -> impl Fn(String) -> Answer + Send + Sync {
    let by_hash: BTreeMap<String, Vec<u8>> = raws
        .into_iter()
        .map(|raw| (chunks::sha256(&raw), chunks::compress(&raw)))
        .collect();
    move |url| {
        let hash = url.rsplit('/').next().unwrap().to_owned();
        let found = by_hash.get(&hash).cloned();
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            found.ok_or(ChunkError::Status { status: 404, url })
        })
    }
}

fn options_for(manifest: Manifest, scratch: &Path, fetch: Arc<dyn ChunkFetch>) -> AssembleOptions {
    AssembleOptions::new(
        manifest,
        scratch.join("target"),
        scratch.join("cache"),
        vec!["https://hlsr.pro/downloads/launcher/chunks".to_owned()],
        fetch,
        Arc::new(|_: &str| true),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eight_requests_at_most_run_at_once() {
    let (manifest, raws) = small_manifest(30, 20_000);
    // Первые восемь запросов ждут друг друга: пока их не восемь в полёте, никто не отвечает.
    // Так «восемь сразу» достигается всегда, а не только когда машина успела запустить их за
    // 20 мс ответа. Девятый до ответа на первые не дойдёт — это и проверяет предел.
    let arrived = Arc::new(AtomicUsize::new(0));
    let serve = serve_from(raws);
    let fetch = FnFetch::new(move |url| {
        let answer = serve(url);
        let number = arrived.fetch_add(1, Ordering::SeqCst) + 1;
        let arrived = arrived.clone();
        Box::pin(async move {
            if number <= chunks::DOWNLOAD_CONCURRENCY {
                // Предел (5 с) нужен только затем, чтобы при меньшем пределе тест не завис.
                for _ in 0..5000 {
                    if arrived.load(Ordering::SeqCst) >= chunks::DOWNLOAD_CONCURRENCY {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
            answer.await
        })
    });
    let scratch = tempfile::tempdir().unwrap();
    std::fs::create_dir(scratch.path().join("target")).unwrap();
    let done = chunks::assemble(options_for(manifest, scratch.path(), fetch.clone()))
        .await
        .unwrap();
    assert_eq!(done.downloaded_chunks, 30);
    assert_eq!(fetch.calls.load(Ordering::SeqCst), 30);
    assert_eq!(
        fetch.peak.load(Ordering::SeqCst),
        chunks::DOWNLOAD_CONCURRENCY
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn progress_is_throttled_and_the_last_event_always_comes() {
    let (manifest, raws) = small_manifest(3, 20_000);
    let total: u64 = manifest
        .files
        .iter()
        .map(|f| f.chunks[0].csize.unwrap())
        .sum();
    let fetch = FnFetch::new(serve_from(raws));
    let scratch = tempfile::tempdir().unwrap();
    std::fs::create_dir(scratch.path().join("target")).unwrap();
    let events: Arc<Mutex<Vec<Progress>>> = Arc::default();
    let mut options = options_for(manifest, scratch.path(), fetch);
    options.clock = Arc::new(FakeClock::new(5_000));
    let sink = events.clone();
    options.on_progress = Some(Arc::new(move |progress| {
        sink.lock().unwrap().push(progress)
    }));
    chunks::assemble(options).await.unwrap();
    // Часы стоят: первое событие (0 из total) и последнее; в середине сто двадцать миллисекунд
    // ничего не прошло, поэтому остальные подавлены. Скорость за 0 с — по нижней границе 1 мс.
    assert_eq!(
        *events.lock().unwrap(),
        [
            Progress {
                done: 0,
                total,
                bytes_per_second: 0
            },
            Progress {
                done: total,
                total,
                bytes_per_second: total * 1000
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_stops_the_download_and_writes_nothing() {
    let (manifest, _raws) = small_manifest(10, 20_000);
    let fetch = FnFetch::new(|_| {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Err(ChunkError::Network("не должно дойти".to_owned()))
        })
    });
    let scratch = tempfile::tempdir().unwrap();
    std::fs::create_dir(scratch.path().join("target")).unwrap();
    let mut options = options_for(manifest, scratch.path(), fetch);
    options.cancel = cancel_after(Some(100));
    let started = std::time::Instant::now();
    let error = chunks::assemble(options).await.unwrap_err();
    assert!(error.is_abort(), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        std::fs::read_dir(scratch.path().join("target"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_target_file_is_not_overwritten() {
    let (manifest, raws) = small_manifest(2, 20_000);
    let fetch = FnFetch::new(serve_from(raws));
    let scratch = tempfile::tempdir().unwrap();
    let target = scratch.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("f0"), b"mine").unwrap();
    let error = chunks::assemble(options_for(manifest, scratch.path(), fetch))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ChunkError::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists),
        "{error}"
    );
    assert_eq!(std::fs::read(target.join("f0")).unwrap(), b"mine");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_carry_the_updater_headers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let head = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            let read = stream.read(&mut chunk).await.unwrap();
            buffer.extend_from_slice(&chunk[..read]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
        String::from_utf8(buffer).unwrap().to_lowercase()
    });
    let options = FetchOptions {
        is_allowed: allow(),
        max_bytes: 10,
        cancel: None,
    };
    let body = HttpsFetch::new()
        .fetch(
            &format!("http://127.0.0.1:{port}/chunks/ab/cd?x=1#frag"),
            &options,
        )
        .await
        .unwrap();
    assert_eq!(body, b"ok");
    let head = head.await.unwrap();
    assert!(
        head.starts_with("get /chunks/ab/cd?x=1 http/1.1\r\n"),
        "{head}"
    );
    assert!(head.contains("\r\nuser-agent: hlsr-updater\r\n"), "{head}");
    assert!(head.contains("\r\naccept-encoding: identity\r\n"), "{head}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_timeout_names_the_url() {
    let servers = start(&["p".to_owned()], false).await;
    // Сервер молчит намного дольше простоя: тест не зависит от скорости машины.
    servers.set_routes(
        "p",
        &json!({"/slow": [{"body": "late", "delayMs": 30_000}]}),
    );
    let url = format!("http://{}/slow", servers.map["p"]);
    let options = FetchOptions {
        is_allowed: allow(),
        max_bytes: 100,
        cancel: None,
    };
    let error = HttpsFetch::new()
        .with_idle_timeout(Duration::from_millis(100))
        .fetch(&url, &options)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ChunkError::Timeout(at) if at == &url),
        "{error}"
    );
    assert_eq!(error.to_string(), format!("Таймаут: {url}"));
}

/// Кусок старого файла читается при записи, уже после плана: если файл успел стать короче,
/// сборка падает с `Файл … короче ожидаемого`, а не пишет мусор.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn source_that_shrinks_after_the_plan_is_reported() {
    let scratch = tempfile::tempdir().unwrap();
    let source = scratch.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let old_file = source.join("a");
    let (new_part, old_part) = (stream(1, 20_000), stream(2, 10_000));
    std::fs::write(&old_file, &old_part).unwrap();
    let whole: Vec<u8> = [new_part.as_slice(), old_part.as_slice()].concat();
    let packed = chunks::compress(&new_part);
    let manifest = Manifest {
        version: "0.5.0".to_owned(),
        platform: Platform::Linux,
        files: vec![ManifestFile {
            path: "a".to_owned(),
            size: whole.len() as u64,
            sha256: chunks::sha256(&whole),
            exec: false,
            chunks: vec![
                ChunkInfo {
                    hash: chunks::sha256(&new_part),
                    size: 20_000,
                    csize: Some(packed.len() as u64),
                },
                ChunkInfo {
                    hash: chunks::sha256(&old_part),
                    size: 10_000,
                    csize: Some(10_010),
                },
            ],
        }],
        attest: None,
    };
    let shrink = old_file.clone();
    let fetch = FnFetch::new(move |_| {
        let (packed, shrink) = (packed.clone(), shrink.clone());
        Box::pin(async move {
            std::fs::write(&shrink, b"short").unwrap();
            Ok(packed)
        })
    });
    let mut options = options_for(manifest, scratch.path(), fetch);
    options.source_root = Some(source);
    std::fs::create_dir(scratch.path().join("target")).unwrap();
    let error = chunks::assemble(options).await.unwrap_err();
    assert!(
        matches!(&error, ChunkError::ShortFile(file) if file.ends_with('a')),
        "{error}"
    );
}

#[test]
fn plan_reuse_stops_on_cancel() {
    let (manifest, _) = small_manifest(2, 1000);
    let scratch = tempfile::tempdir().unwrap();
    let token = CancelToken::new();
    token.cancel();
    let error = chunks::plan_reuse(&manifest, Some(scratch.path()), Some(&token)).unwrap_err();
    assert!(error.is_abort());
    // Без старой версии отмену никто не проверяет, и плана нет.
    let plan = chunks::plan_reuse(&manifest, None, Some(&token)).unwrap();
    assert!(plan.whole.is_empty() && plan.local.is_empty());
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn describe_tree_reads_the_exec_bit_for_linux_only() {
        let scratch = tempfile::tempdir().unwrap();
        for (name, mode) in [("run", 0o755), ("data", 0o644), ("odd", 0o741)] {
            let file = scratch.path().join(name);
            std::fs::write(&file, name).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let flags = |platform| {
            let options = DescribeOptions::new("0.5.0", platform);
            let manifest =
                chunks::describe_tree(scratch.path(), &options, &mut |_, _| Ok(())).unwrap();
            manifest
                .files
                .iter()
                .map(|f| (f.path.clone(), f.exec))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            flags(Platform::Linux),
            [
                ("data".to_owned(), false),
                ("odd".to_owned(), true),
                ("run".to_owned(), true)
            ]
        );
        assert!(flags(Platform::Win32).iter().all(|(_, exec)| !exec));
    }

    #[test]
    fn describe_tree_refuses_links() {
        let scratch = tempfile::tempdir().unwrap();
        std::fs::write(scratch.path().join("real"), "x").unwrap();
        std::os::unix::fs::symlink("real", scratch.path().join("link")).unwrap();
        let options = DescribeOptions::new("0.5.0", Platform::Linux);
        let error =
            chunks::describe_tree(scratch.path(), &options, &mut |_, _| Ok(())).unwrap_err();
        assert_eq!(error.to_string(), "Ссылка в сборке: link");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn assemble_sets_modes_from_the_manifest() {
        let (mut manifest, raws) = small_manifest(2, 20_000);
        manifest.files[0].exec = true;
        let fetch = FnFetch::new(serve_from(raws));
        let scratch = tempfile::tempdir().unwrap();
        let target = scratch.path().join("target");
        std::fs::create_dir(&target).unwrap();
        chunks::assemble(options_for(manifest, scratch.path(), fetch))
            .await
            .unwrap();
        let mode = |name: &str| {
            std::fs::metadata(target.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("f0"), 0o755);
        assert_eq!(
            mode("f1") & 0o111,
            0,
            "обычный файл не исполняемый: {:o}",
            mode("f1")
        );
    }
}
