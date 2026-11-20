//! Построение файлового манифеста по дереву (`files-manifest.js`: `build`, `collectFiles`,
//! `hashFileSync`) против 0.4.7. Два сценария в `manifests.json` (`build`): `portable` — дерево
//! из имён, которые есть на любой ОС (кириллица, пробелы, изменяемые игроком пути), и `unix` —
//! то же дерево плюс имена с `<`, `?` и обратной косой чертой, `CON.txt`, символьные ссылки
//! и U+FFFF; второй только на Unix.

mod support;

use hlsr_core::js;
use hlsr_core::json_text::{parse_js_json, stringify};
use hlsr_delivery::errors::{Failure, RawError};
use hlsr_delivery::files_manifest::{
    self, BuildOptions, BuildProgress, collect_files, hash_file_sync,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, UNIX_EPOCH};
use support::{assert_json_eq, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("manifests.json"));

/// Значение так, как его видит JSON после записи: число в записи JS, порядок ключей прежний.
fn norm(value: &Value) -> Value {
    parse_js_json(&stringify(value, "")).expect("JSON")
}

fn names(value: &Value) -> Vec<String> {
    items(value).iter().map(|v| str_of(v).to_owned()).collect()
}

fn raw_outcome(result: Result<String, RawError>) -> Value {
    match result {
        Ok(hash) => json!({"ok": hash.len()}),
        Err(error) => json!({"raw": {"name": "Error", "message": error.message}}),
    }
}

fn make_tree(root: &Path, tree: &Value) {
    for entry in items(tree) {
        let full = str_of(&entry["path"])
            .split('/')
            .fold(root.to_path_buf(), |p, part| p.join(part));
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        if entry["dir"].as_bool() == Some(true) {
            std::fs::create_dir_all(&full).unwrap();
        } else if let Some(target) = entry["symlink"].as_str() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &full).unwrap();
            #[cfg(not(unix))]
            panic!("ссылка {target} только на Unix");
        } else {
            std::fs::write(&full, str_of(&entry["content"])).unwrap();
        }
    }
}

/// Списки пропущенного в порядке обхода. Фикстура снята в Linux (libuv сортирует имена `strcmp`);
/// в Windows порядок `readdir` — NTFS, поэтому там сравниваются отсортированные списки.
fn skipped_json(skipped: &files_manifest::Skipped) -> Value {
    json!({
        "mutable": skipped.mutable,
        "links": skipped.links,
        "unsafe": skipped.unsafe_entries,
    })
}

#[cfg(unix)]
fn assert_skipped(actual: &files_manifest::Skipped, expected: &Value, what: &str) {
    assert_json_eq(&skipped_json(actual), expected, what);
}

#[cfg(not(unix))]
fn assert_skipped(actual: &files_manifest::Skipped, expected: &Value, what: &str) {
    let sort = |value: &Value| {
        let mut out = value.clone();
        for list in out.as_object_mut().unwrap().values_mut() {
            list.as_array_mut()
                .unwrap()
                .sort_by(|a, b| js::cmp_utf16(a.as_str().unwrap(), b.as_str().unwrap()));
        }
        out
    };
    assert_json_eq(&sort(&skipped_json(actual)), &sort(expected), what);
}

/// Выполнить сценарий и вернуть временную папку (для сверки «хеш файла»).
fn run_scenario(scenario: &Value) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("build");
    std::fs::create_dir(&root).unwrap();
    make_tree(&root, &scenario["tree"]);
    let now = UNIX_EPOCH + Duration::from_millis(1_792_067_696_789);
    assert_eq!(js::iso_string(now), "2026-10-15T12:34:56.789Z");

    let mut progress: Vec<BuildProgress> = Vec::new();
    let mut on_progress = |event: &BuildProgress| progress.push(event.clone());
    let built = files_manifest::build(BuildOptions {
        game_id: Some("hl1-won"),
        version: Some("1.0.0"),
        now: Some(now),
        on_progress: Some(&mut on_progress),
        ..BuildOptions::new(&root)
    })
    .unwrap();
    assert_json_eq(
        &norm(&built.manifest),
        &scenario["manifest"],
        "build: манифест",
    );
    assert_skipped(&built.skipped, &scenario["skipped"], "build: пропущено");
    assert_eq!(
        files_manifest::serialize(&built.manifest),
        str_of(&scenario["serialized"]),
        "serialize(build)"
    );
    let progress: Vec<Value> = progress
        .iter()
        .map(|p| json!({"processed": p.processed, "total": p.total, "path": p.path}))
        .collect();
    assert_json_eq(&json!(progress), &scenario["progress"], "build: ход");

    let none = |_: &str| false;
    let plain = files_manifest::build(BuildOptions {
        now: Some(now),
        is_mutable: Some(&none),
        ..BuildOptions::new(&root)
    })
    .unwrap();
    assert_json_eq(
        &norm(&plain.manifest),
        &scenario["plain"]["manifest"],
        "build без имени и версии",
    );
    assert_skipped(
        &plain.skipped,
        &scenario["plain"]["skipped"],
        "build без изменяемых: пропущено",
    );

    let zone = |relative: &str| relative.starts_with("z/");
    let collected = collect_files(&root, Some(&zone)).unwrap();
    assert_eq!(
        collected
            .files
            .iter()
            .map(|f| f.relative.clone())
            .collect::<Vec<_>>(),
        names(&scenario["collected"]["files"])
    );
    assert!(collected.files.iter().all(|f| f.full.starts_with(&root)));
    assert_skipped(
        &collected.skipped,
        &scenario["collected"]["skipped"],
        "collectFiles: пропущено",
    );

    // Каталога нет.
    let missing = dir.path().join("нет такого");
    let error = files_manifest::build(BuildOptions::new(&missing)).unwrap_err();
    let mut actual = match error {
        Failure::Delivery(error) => json!({"error": error.to_public()}),
        Failure::Raw(error) => panic!("{error}"),
    };
    let text = actual["error"]["message"]
        .as_str()
        .unwrap()
        .replace(dir.path().to_str().unwrap(), "<root>")
        .replace('\\', "/");
    actual["error"]["message"] = json!(text);
    assert_json_eq(&actual, &scenario["missing"], "build без каталога");
    (dir, root)
}

/// Нет такого файла: сообщение `fs` Node, путь временной папки — `<root>`.
fn scrub(value: Value, dir: &Path) -> Value {
    let escaped = serde_json::to_string(dir.to_str().unwrap()).unwrap();
    let text = value
        .to_string()
        .replace(&escaped[1..escaped.len() - 1], "<root>")
        .replace("\\\\", "/");
    parse_js_json(&text).unwrap()
}

#[test]
fn files_manifest_build_matches_node() {
    let scenario = &FIXTURE["build"]["portable"];
    let (dir, root) = run_scenario(scenario);

    assert_eq!(
        hash_file_sync(&root.join("b.txt")).unwrap(),
        str_of(&scenario["hashFile"])
    );
    let large = dir.path().join("large.bin");
    let size = scenario["hashLarge"]["size"].as_u64().unwrap() as usize;
    std::fs::write(&large, vec![7u8; size]).unwrap();
    assert_eq!(
        hash_file_sync(&large).unwrap(),
        str_of(&scenario["hashLarge"]["sha256"])
    );
    let missing = raw_outcome(hash_file_sync(&dir.path().join("нет")));
    assert_json_eq(
        &scrub(missing, dir.path()),
        &scenario["hashMissing"],
        "hashFileSync без файла",
    );
}

/// Дерево с именами, которых нет в Windows, и ссылками; `hashFileSync` каталога.
#[cfg(unix)]
#[test]
fn files_manifest_build_matches_node_on_unix_names() {
    let scenario = &FIXTURE["build"]["unix"];
    let (dir, root) = run_scenario(scenario);
    let directory = raw_outcome(hash_file_sync(&root));
    assert_json_eq(
        &scrub(directory, dir.path()),
        &scenario["hashDir"],
        "hashFileSync каталога",
    );
}
