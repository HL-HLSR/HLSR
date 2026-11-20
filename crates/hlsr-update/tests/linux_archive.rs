//! Linux-установка против 0.4.7 без ссылок и `/proc`: фикстура `archive.json`
//! (`tools/reference/update/archive.js`).
//!
//! - `extract` — `archive.js` на архивах-ловушках: итог или текст отказа, события прогресса, дерево
//!   каталога (у ошибок gzip дерево не сверяется: Node отбрасывает распакованное в той же порции, где
//!   zlib нашёл ошибку, Rust успевает его разобрать; каталог `.staging-*` всё равно удаляется);
//! - `payload` — `embedded-payload.js`: `describe`, `problemsOf`, `read`;
//! - `stage` — `ArchiveInstall.prepare/discard`: блокировка, уборка `.staging-*`, отказы проверки и
//!   `verifyStage`, отмена (в Windows `identity` блокировки — `null`, как у Node без `/proc`);
//! - `download` — `ArchiveDownload.download` против сервера на 127.0.0.1.
//!
//! Самотесты `launcher-update-selftest.js`: «tar traversal, links, truncated streams and expanded size
//! limits are rejected», «corrupted archive cannot alter an installed version» (часть подготовки),
//! «parallel installation is locked; cancelled preparation releases its lock», «launcher extraction uses
//! unpatched fs» (уборка брошенных `.staging-*`), «real HTTP installer download falls back, verifies
//! bytes…» (часть загрузки).

mod support;

use hlsr_update::cancel::CancelToken;
use hlsr_update::linux::archive::{self, ArchiveError, ExtractOptions, Sink, TarStream};
use hlsr_update::linux::archive_download::{ArchiveDownload, DownloadOptions, DownloadProgress};
use hlsr_update::linux::archive_install::ArchiveInstall;
use hlsr_update::linux::embedded_payload::{self, Describe};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use support::http::{RequestLog, start_server, without_default_accept};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("archive.json"));

fn base64(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .unwrap()
}

/// Путь в выводе: временный каталог → метка, затем разделитель «/»; хвост `.staging-*` → `{rand}`.
fn mask(text: &str, roots: &[(&str, &Path)]) -> String {
    let mut out = text.to_owned();
    let mut touched = false;
    for (label, dir) in roots {
        let dir = dir.to_string_lossy();
        if out.contains(dir.as_ref()) {
            out = out.replace(dir.as_ref(), label);
            touched = true;
        }
    }
    if touched {
        out = out.replace('\\', "/");
    }
    staging(&out)
}

fn staging(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(index) = rest.find(".staging-") {
        let after = &rest[index + ".staging-".len()..];
        let tail: String = after.chars().take(6).collect();
        out.push_str(&rest[..index]);
        if tail.len() == 6 && tail.chars().all(|c| c.is_ascii_alphanumeric()) {
            out.push_str(".staging-{rand}");
            rest = &after[6..];
        } else {
            out.push_str(".staging-");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Текст ошибки разбора JSON у Rust свой — метка `<json>`, как в фикстуре.
fn json_label(text: &str) -> String {
    if text.contains(" at line ") && text.contains(" column ") {
        return "<json>".to_owned();
    }
    text.to_owned()
}

/// Дерево каталога как `tree()` скрипта: файлы, каталоги, `.update-lock` корня — отметкой.
fn tree(root: &Path) -> Value {
    let mut entries = Vec::new();
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<Value>) {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect();
        names.sort_by_key(|entry| entry.file_name());
        for entry in names {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                out.push(json!({"path": rel, "kind": "dir"}));
                walk(&entry.path(), &rel, out);
            } else if name == ".update-lock" && prefix.is_empty() {
                out.push(json!({"path": rel, "kind": "lock"}));
            } else {
                let bytes = std::fs::read(entry.path()).unwrap();
                let sha: String = Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                out.push(json!({"path": rel, "kind": "file", "size": bytes.len(), "sha256": sha}));
            }
        }
    }
    if root.exists() {
        walk(root, "", &mut entries);
    }
    for entry in &mut entries {
        let path = staging(entry["path"].as_str().unwrap());
        entry["path"] = json!(path);
    }
    entries.sort_by(|a, b| {
        hlsr_core::js::cmp_utf16(a["path"].as_str().unwrap(), b["path"].as_str().unwrap())
    });
    Value::Array(entries)
}

// ---------------------------------------------------------------- extract

#[test]
fn extract_matches_0_4_7() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["extract"]) {
        let name = str_of(&case["name"]);
        let dir = tempfile::tempdir().unwrap();
        let archive_file = dir.path().join("archive.tar.gz");
        std::fs::write(&archive_file, base64(str_of(&case["gz"]))).unwrap();
        let dest = dir.path().join("out");
        std::fs::create_dir(&dest).unwrap();
        for pre in case["pre"].as_array().into_iter().flatten() {
            std::fs::write(dest.join(str_of(pre)), "старый файл").unwrap();
        }
        let options = &case["options"];
        let cancel = CancelToken::new();
        if options["abort"].as_bool() == Some(true) {
            cancel.cancel();
        }
        let mut progress = Vec::new();
        let mut on_progress =
            |transferred: u64, total: u64| progress.push(json!([transferred, total]));
        let outcome = archive::extract(
            &archive_file,
            &dest,
            ExtractOptions {
                max_bytes: options["maxBytes"].as_u64(),
                expected_bytes: options["expectedBytes"].as_u64(),
                cancel: Some(cancel),
                on_progress: Some(&mut on_progress),
            },
        );
        let result = match &outcome {
            Ok(value) => json!({"ok": {"bytes": value.bytes, "entries": value.entries}}),
            Err(error) if error.is_gzip() => json!({"error": "<gzip>"}),
            Err(error) => json!({"error": mask(&error.to_string(), &[("{dir}", &dest)])}),
        };
        mismatches.check(&result, &case["result"], format!("extract «{name}»: итог"));
        if outcome.as_ref().is_err_and(ArchiveError::is_gzip) {
            continue;
        }
        mismatches.check(
            &json!(progress),
            &case["progress"],
            format!("extract «{name}»: прогресс"),
        );
        mismatches.check(
            &tree(&dest),
            &case["tree"],
            format!("extract «{name}»: дерево"),
        );
    }
    mismatches.finish("archive.extract");
}

/// Счётчик записей без записи на диск.
#[derive(Default)]
struct CountingSink {
    directories: u64,
}

impl Sink for CountingSink {
    fn directory(&mut self, _relative: &str) -> Result<(), ArchiveError> {
        self.directories += 1;
        Ok(())
    }
    fn parent(&mut self, _relative: &str) -> Result<(), ArchiveError> {
        Ok(())
    }
    fn open(&mut self, _relative: &str, _executable: bool) -> Result<(), ArchiveError> {
        Ok(())
    }
    fn write(&mut self, _data: &[u8]) -> Result<(), ArchiveError> {
        Ok(())
    }
    fn close(&mut self) {}
}

#[test]
fn more_than_50000_entries_are_rejected() {
    let entries: Vec<Value> = (0..=archive::MAX_ENTRIES)
        .map(|n| json!({"name": format!("d{n}/"), "type": "5"}))
        .collect();
    let tar = support::ustar::tar_of(&json!({ "entries": entries }));
    let mut stream = TarStream::new(archive::DEFAULT_MAX_BYTES);
    let mut sink = CountingSink::default();
    let result = stream.feed(&tar, &mut sink, &mut None);
    assert!(matches!(result, Err(ArchiveError::TooBig)), "{result:?}");
    assert_eq!(sink.directories, archive::MAX_ENTRIES);

    let entries: Vec<Value> = (0..archive::MAX_ENTRIES)
        .map(|n| json!({"name": format!("d{n}/"), "type": "5"}))
        .collect();
    let tar = support::ustar::tar_of(&json!({ "entries": entries }));
    let mut stream = TarStream::new(archive::DEFAULT_MAX_BYTES);
    stream
        .feed(&tar, &mut CountingSink::default(), &mut None)
        .unwrap();
    assert_eq!(stream.finish(None).unwrap().entries, archive::MAX_ENTRIES);
}

#[test]
fn builders_agree_on_tar_bytes() {
    // Построитель Rust (tests/support/ustar.rs) даёт те же байты tar, что lib/ustar.js.
    use std::io::Read as _;
    for case in items(&FIXTURE["extract"]) {
        let spec = &case["archive"];
        if spec.get("gzip").is_some() || spec.get("cut").is_some() || spec.get("trailing").is_some()
        {
            continue;
        }
        let mut node = Vec::new();
        flate2::read::GzDecoder::new(&base64(str_of(&case["gz"]))[..])
            .read_to_end(&mut node)
            .unwrap();
        assert_eq!(
            support::ustar::tar_of(spec),
            node,
            "tar «{}»",
            str_of(&case["name"])
        );
    }
}

#[cfg(unix)]
#[test]
fn extracted_files_get_0755_or_0644() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.tar.gz");
    let spec = json!({"entries": [
        {"name": "exec", "mode": 0o010, "text": "x"},
        {"name": "plain", "mode": 0o666, "text": "x"},
        {"name": "d/", "type": "5", "mode": 0o700},
    ]});
    std::fs::write(&file, support::ustar::gzip(&support::ustar::tar_of(&spec))).unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    archive::extract(&file, &out, ExtractOptions::default()).unwrap();
    let mode = |name: &str| {
        std::fs::metadata(out.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    let umask = 0o022;
    assert_eq!(mode("exec"), 0o755 & !umask);
    assert_eq!(mode("plain"), 0o644 & !umask);
    assert_eq!(mode("d"), 0o755 & !umask);
}

// ---------------------------------------------------------------- payload

#[test]
fn payload_matches_0_4_7() {
    let payload = &FIXTURE["payload"];
    let mut mismatches = Mismatches::default();
    for case in items(&payload["describe"]) {
        let input = &case["input"];
        let output = embedded_payload::describe(&Describe {
            version: str_of(&input["version"]),
            size: input["size"].as_u64().unwrap(),
            sha256: str_of(&input["sha256"]),
            sha512: str_of(&input["sha512"]),
            unpacked_size: input["unpackedSize"].as_u64().unwrap(),
        });
        mismatches.check(&output, &case["output"], "describe");
    }
    for case in items(&payload["archiveName"]) {
        mismatches.check(
            &json!(embedded_payload::archive_name(str_of(&case["version"]))),
            &case["name"],
            "archiveName",
        );
    }
    for case in items(&payload["problems"]) {
        mismatches.check(
            &json!(embedded_payload::problems_of(&case["data"])),
            &case["problems"],
            format!("problemsOf «{}»", str_of(&case["name"])),
        );
    }
    for case in items(&payload["read"]) {
        let dir = tempfile::tempdir().unwrap();
        if let Some(text) = case["text"].as_str() {
            std::fs::write(dir.path().join(embedded_payload::DESCRIPTOR), text).unwrap();
        }
        let result = match embedded_payload::read(dir.path()) {
            Ok(value) => json!({"ok": {
                "manifest": value.manifest,
                "archive": mask(&value.archive.to_string_lossy(), &[("{dir}", dir.path())]),
            }}),
            Err(error) => {
                let prefix = "В установщике нет встроенного HLSR: ";
                let text = match error.strip_prefix(prefix) {
                    Some(rest) if !rest.starts_with("ENOENT") => {
                        format!("{prefix}{}", json_label(rest))
                    }
                    _ => error,
                };
                json!({"error": mask(&text, &[("{dir}", dir.path())])})
            }
        };
        mismatches.check(
            &result,
            &case["result"],
            format!("read «{}»", str_of(&case["name"])),
        );
    }
    mismatches.finish("embedded-payload");
}

// ---------------------------------------------------------------- stage

#[test]
fn stage_matches_0_4_7() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["stage"]) {
        let name = str_of(&case["name"]);
        let work = tempfile::tempdir().unwrap();
        let work_path = std::fs::canonicalize(work.path()).unwrap();
        let root = work_path.join("install root");
        let manifest = &case["manifest"];
        let archive_file = work_path.join(str_of(&manifest["installer"]["filename"]));
        if let Some(gz) = case["gz"].as_str() {
            std::fs::write(&archive_file, base64(gz)).unwrap();
        }
        let setup = &case["setup"];
        if let Some(lock) = setup["lock"].as_str() {
            std::fs::create_dir_all(&root).unwrap();
            let file = root.join(".update-lock");
            std::fs::write(&file, lock).unwrap();
            let age = setup["lockAgeMs"].as_u64().unwrap_or(0);
            let when = std::time::SystemTime::now() - std::time::Duration::from_millis(age);
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(when)
                .unwrap();
        }
        if setup["orphan"].as_bool() == Some(true) {
            let orphan = root.join("versions").join(".staging-orphan");
            std::fs::create_dir_all(&orphan).unwrap();
            std::fs::write(orphan.join("resources-app.asar-partial"), "garbage").unwrap();
        }
        let mut instances: HashMap<String, ArchiveInstall> = HashMap::new();
        let mut steps = Vec::new();
        for step in items(&case["steps"]) {
            let key = step["instance"].as_str().unwrap_or("a").to_owned();
            let install = instances
                .entry(key)
                .or_insert_with(|| ArchiveInstall::new(&root));
            let result = if step["op"] == "prepare" {
                let cancel = CancelToken::new();
                if step["abort"].as_bool() == Some(true) {
                    cancel.cancel();
                }
                runtime.block_on(install.prepare(&archive_file, manifest, Some(&cancel)))
            } else {
                install.discard();
                Ok(())
            };
            let result = match result {
                Ok(()) => json!({"ok": true}),
                Err(error) => {
                    json!({"error": json_label(&mask(&error, &[("{root}", &root), ("{work}", &work_path)]))})
                }
            };
            let mut record = step.clone();
            let object = record.as_object_mut().unwrap();
            object.insert("result".into(), result);
            object.insert(
                "stage".into(),
                install.stage().map_or(Value::Null, |stage| {
                    json!(staging(
                        &stage
                            .strip_prefix(&root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/")
                    ))
                }),
            );
            object.insert("locked".into(), json!(install.locked()));
            object.insert("tree".into(), tree(&root));
            steps.push(record);
        }
        for install in instances.values_mut() {
            install.discard();
        }
        mismatches.check(
            &Value::Array(steps),
            &case["steps"],
            format!("stage «{name}»"),
        );
    }
    mismatches.finish("ArchiveInstall.prepare");
}

// ---------------------------------------------------------------- download

/// Порт, на котором никто не слушает: отказ в соединении, как у `deadPort` скрипта (в Windows он
/// приходит после повторов SYN, около 2 с; порт 0 из `support::http::dead_port` дал бы другой код).
fn closed_port() -> u16 {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("порт");
    probe.local_addr().expect("адрес").port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_matches_0_4_7() {
    let download = &FIXTURE["download"];
    let body = &download["body"];
    let mut mismatches = Mismatches::default();
    for case in items(&download["scenarios"]) {
        let name = str_of(&case["name"]);
        let log = RequestLog::default();
        let server = start_server("a", log.clone()).await;
        let routes = case["routes"].as_object().unwrap().clone();
        server.routes.lock().unwrap().extend(routes);
        let base = format!("http://{}", server.authority);
        let mut map = BTreeMap::from([("a".to_owned(), server.authority.clone())]);
        if items(&case["urls"])
            .iter()
            .any(|url| str_of(url).contains("{dead}"))
        {
            map.insert("dead".to_owned(), format!("127.0.0.1:{}", closed_port()));
        }
        let urls: Vec<String> = items(&case["urls"])
            .iter()
            .map(|url| {
                let url = str_of(url);
                if url.starts_with('/') {
                    format!("{base}{url}")
                } else {
                    support::http::fill_text(url, &map)
                }
            })
            .collect();
        let manifest = json!({"version": "0.4.8", "installer": {
            "filename": body["filename"], "size": body["size"], "sha256": body["sha256"],
            "sha512": body["sha512"], "urls": urls,
        }});
        let dir = tempfile::tempdir().unwrap();
        let directory = dir.path().join("update");
        if case["pre"].as_bool() == Some(true) {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(str_of(&body["filename"])), "занято").unwrap();
        }
        let cancel = CancelToken::new();
        if case["abort"].as_bool() == Some(true) {
            cancel.cancel();
        }
        let progress: Arc<Mutex<Vec<DownloadProgress>>> = Arc::default();
        let sink = progress.clone();
        let options = DownloadOptions {
            is_allowed: Arc::new(|url: &str| url.starts_with("http://127.0.0.1:")),
            cancel: Some(cancel),
            on_progress: Some(Arc::new(move |event| sink.lock().unwrap().push(event))),
            ..DownloadOptions::default()
        };
        let result = ArchiveDownload::new()
            .download(&manifest, &directory, &options)
            .await;
        let result = match &result {
            Ok(target) => json!({
                "ok": target.strip_prefix(dir.path()).unwrap().to_string_lossy().replace('\\', "/"),
                "sha256": support::sha256_hex(&std::fs::read(target).unwrap()),
            }),
            Err(error) => json!({"error": mask(error, &[("{dir}", dir.path())])}),
        };
        let last = progress.lock().unwrap().last().copied();
        let progress = match (&result["ok"], last) {
            (Value::String(_), Some(last)) => {
                json!({"transferred": last.transferred, "total": last.total, "percent": hlsr_core::json_text::js_number(last.percent)})
            }
            _ => Value::Null,
        };
        let requests: Vec<Value> = without_default_accept(&log.lock().unwrap())
            .into_iter()
            .map(|entry| json!({"method": entry["method"], "url": entry["url"], "headers": entry["headers"]}))
            .collect();
        let actual = json!({
            "result": support::http::unfill(&result, &map),
            "requests": requests,
            "progress": progress,
            "tree": tree(&directory),
        });
        let expected = json!({
            "result": case["result"], "requests": case["requests"],
            "progress": case["progress"], "tree": case["tree"],
        });
        mismatches.check(&actual, &expected, format!("download «{name}»"));
    }
    mismatches.finish("ArchiveDownload.download");
}
