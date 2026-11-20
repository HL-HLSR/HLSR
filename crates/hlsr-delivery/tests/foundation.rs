//! Основа доставки против 0.4.7: фикстура `foundation.json` из
//! `tools/reference/delivery/foundation.js` (модули 0.4.7 в Node 24 на тех же входах). Что
//! изменилось в 0.4.9 (коды ошибок, списки изменяемых путей), сверяется с `base-049.json`
//! (`tools/reference/delivery/base-049.js`).

mod support;

use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use hlsr_delivery::archive_entries::{
    ArchiveEntry, InspectOptions, Listing, classify_path, inspect_entries, parse_listing,
};
use hlsr_delivery::errors::{
    Code, DeliveryError, Failure, RawError, antivirus_error, is_antivirus_block,
    to_delivery_error_on,
};
use hlsr_delivery::file_ops::{self, LOCK_CODES};
use hlsr_delivery::library_marker::{self, MARKER_ID, MARKER_SCHEMA};
use hlsr_delivery::path_match::{Matcher, normalize};
use hlsr_delivery::paths::{self, Layout, LibraryPaths};
use hlsr_delivery::{
    free_space, library_path, mutable_paths, progress_states, semver, user_configs,
};
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};
use support::tree::{replacing, snapshot};
use support::{Mismatches, assert_json_eq, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("foundation.json"));
static BASE: LazyLock<Value> = LazyLock::new(|| fixture("base-049.json"));

fn platform_of(name: &str) -> Platform {
    match name {
        "win32" => Platform::Win32,
        "linux" => Platform::Linux,
        other => panic!("платформа {other}"),
    }
}

/// `path` платформы с тем же текущим каталогом, что у скрипта сверки.
fn node_path(platform: &str) -> NodePath {
    let cwd = match platform {
        "win32" => "C:\\Users\\hlsr\\work",
        _ => "/home/hlsr/work",
    };
    NodePath::new(platform_of(platform), cwd)
}

const PLATFORMS: [&str; 2] = ["linux", "win32"];

fn public(error: &DeliveryError) -> Value {
    error.to_public()
}

/// Итог вызова в виде фикстуры: `{ok}`, `{error: toPublic()}` или `{raw: {code, message}}`.
fn outcome<T: Into<Value>>(result: Result<T, Failure>) -> Value {
    match result {
        Ok(value) => json!({"ok": value.into()}),
        Err(Failure::Delivery(error)) => json!({"error": error.to_public()}),
        Err(Failure::Raw(raw)) => json!({"raw": {"code": raw.code, "message": raw.message}}),
    }
}

fn delivery<T>(result: Result<T, DeliveryError>) -> Result<T, Failure> {
    result.map_err(Failure::Delivery)
}

fn raw_error(fields: &Value) -> RawError {
    RawError {
        code: opt_str(&fields["code"]).map(str::to_owned),
        syscall: opt_str(&fields["syscall"]).map(str::to_owned),
        path: opt_str(&fields["path"]).map(str::to_owned),
        message: opt_str(&fields["message"]).unwrap_or_default().to_owned(),
        stderr: opt_str(&fields["stderr"]).map(str::to_owned),
    }
}

// ---------------------------------------------------------------- errors.js

#[test]
fn error_codes_are_those_of_0_4_9() {
    let expected: Vec<&str> = items(&BASE["errors"]["codes"]).iter().map(str_of).collect();
    let actual: Vec<&str> = Code::ALL.iter().map(|code| code.as_str()).collect();
    assert_eq!(actual, expected);
    for name in expected {
        assert_eq!(Code::from_name(name).map(Code::as_str), Some(name));
    }
    assert_eq!(Code::from_name("BOGUS_CODE"), None);
}

#[test]
fn delivery_error_construction_and_public_form() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["errors"]["construct"]) {
        let mut error = DeliveryError::new(
            Code::coerce(str_of(&case["code"])),
            opt_str(&case["message"]).unwrap_or_default(),
        );
        if !case["details"].is_null() {
            error = error.with_details(case["details"].clone());
        }
        mismatches.check(
            &public(&error),
            &case["public"],
            format!("new DeliveryError({}, {})", case["code"], case["message"]),
        );
    }
    mismatches.finish("DeliveryError");
}

#[test]
fn antivirus_block_detection() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["errors"]["isAntivirusBlock"]) {
        let platform = platform_of(str_of(&case["platform"]));
        let actual = is_antivirus_block(&raw_error(&case["error"]), platform);
        mismatches.check_eq(
            actual,
            case["result"].as_bool().unwrap(),
            format!("isAntivirusBlock({}, {platform:?})", case["error"]),
        );
    }
    mismatches.finish("isAntivirusBlock");
}

#[test]
fn antivirus_error_text() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["errors"]["antivirusError"]) {
        let error = antivirus_error(
            opt_str(&case["path"]),
            opt_str(&case["message"]).unwrap_or_default(),
        );
        mismatches.check(
            &public(&error),
            &case["public"],
            format!("antivirusError({}, {})", case["path"], case["message"]),
        );
    }
    mismatches.finish("antivirusError");
}

#[test]
fn to_delivery_error_maps_like_0_4_7() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["errors"]["toDeliveryError"]) {
        let platform = platform_of(str_of(&case["platform"]));
        let failure = if let Some(raw) = case["error"].get("raw") {
            Failure::Raw(raw_error(raw))
        } else {
            let own = &case["error"]["delivery"];
            Failure::Delivery(
                DeliveryError::new(Code::coerce(str_of(&own["code"])), str_of(&own["message"]))
                    .with_details(own["details"].clone()),
            )
        };
        let fallback = opt_str(&case["fallback"]).map(Code::coerce);
        let actual = to_delivery_error_on(platform, failure, fallback);
        mismatches.check(
            &public(&actual),
            &case["public"],
            format!(
                "toDeliveryError({}, {}) на {platform:?}",
                case["error"], case["fallback"]
            ),
        );
    }
    mismatches.finish("toDeliveryError");
}

// ------------------------------------------- path-match.js, mutable-paths.js

#[test]
fn mutable_path_lists_are_those_of_0_4_9() {
    let groups = &BASE["patterns"]["groups"];
    let ours: [(&str, Value); 8] = [
        ("USER_DATA", json!(mutable_paths::USER_DATA)),
        ("RUNTIME_STATE", json!(mutable_paths::RUNTIME_STATE)),
        ("ENGINE_CACHES", json!(mutable_paths::ENGINE_CACHES)),
        ("LIBRARY_INTERNALS", json!(mutable_paths::LIBRARY_INTERNALS)),
        (
            "UPDATED_DEPENDENCIES",
            json!(mutable_paths::UPDATED_DEPENDENCIES),
        ),
        ("CUSTOMIZATION", json!(*mutable_paths::CUSTOMIZATION)),
        ("EPISODE_DIRS", json!(mutable_paths::EPISODE_DIRS)),
        ("MUTABLE_PATTERNS", json!(*mutable_paths::MUTABLE_PATTERNS)),
    ];
    for (name, list) in ours {
        assert_json_eq(&list, &groups[name], name);
    }
}

#[test]
fn matches_mutable_like_0_4_9() {
    let corpus: Vec<&str> = items(&FIXTURE["patterns"]["corpus"])
        .iter()
        .chain(items(&BASE["patterns"]["extra"]))
        .map(str_of)
        .collect();
    let matching: Vec<usize> = (0..corpus.len())
        .filter(|&i| mutable_paths::matches_mutable(corpus[i]))
        .collect();
    let expected: Vec<usize> = items(&BASE["patterns"]["matching"])
        .iter()
        .map(|i| i.as_u64().unwrap() as usize)
        .collect();
    assert_eq!(matching, expected);
}

#[test]
fn normalize_matches_path_match_js() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["patterns"]["normalize"]) {
        mismatches.check_eq(
            normalize(str_of(&case[0])),
            str_of(&case[1]).to_owned(),
            format!("normalize({})", case[0]),
        );
    }
    mismatches.finish("normalize");
}

#[test]
fn matchers_agree_on_corpus() {
    let corpus: Vec<&str> = items(&FIXTURE["patterns"]["corpus"])
        .iter()
        .map(str_of)
        .collect();
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["patterns"]["matchers"]) {
        let name = str_of(&case["name"]);
        let patterns: Vec<&str> = items(&case["patterns"]).iter().map(str_of).collect();
        let matcher = Matcher::new(&patterns);
        mismatches.check(
            &json!(matcher.patterns()),
            &case["normalized"],
            format!("{name}: match.patterns"),
        );
        let matching: Vec<usize> = corpus
            .iter()
            .enumerate()
            .filter(|(_, p)| matcher.matches(p))
            .map(|(index, _)| index)
            .collect();
        let expected: Vec<usize> = items(&case["matching"])
            .iter()
            .map(|i| i.as_u64().unwrap() as usize)
            .collect();
        if matching != expected {
            let show = |list: &[usize]| {
                list.iter()
                    .map(|&i| format!("{:?}", corpus[i]))
                    .collect::<Vec<_>>()
            };
            let extra: Vec<usize> = matching
                .iter()
                .copied()
                .filter(|i| !expected.contains(i))
                .collect();
            let missing: Vec<usize> = expected
                .iter()
                .copied()
                .filter(|i| !matching.contains(i))
                .collect();
            mismatches.check_eq(
                show(&extra),
                show(&missing),
                format!("{name}: лишние (+) и пропущенные (-) совпадения"),
            );
        }
    }
    mismatches.finish("createMatcher");
}

// --------------------------------------------------------- archive-entries.js

fn listing_json(listing: &Listing) -> Value {
    listing.to_json()
}

fn entry_from(value: &Value) -> ArchiveEntry {
    ArchiveEntry {
        path: str_of(&value["path"]).to_owned(),
        size: value["size"].as_f64().unwrap_or(0.0),
        packed_size: value["packedSize"].as_f64().unwrap_or(0.0),
        attributes: opt_str(&value["attributes"]).unwrap_or_default().to_owned(),
        modified: opt_str(&value["modified"]).map(str::to_owned),
        crc: opt_str(&value["crc"]).map(str::to_owned),
        encrypted: value["encrypted"].as_bool().unwrap_or(false),
        is_directory: value["isDirectory"].as_bool().unwrap_or(false),
        is_symlink: value["isSymlink"].as_bool().unwrap_or(false),
    }
}

fn options_from(value: &Value) -> InspectOptions {
    InspectOptions {
        max_entries: value["maxEntries"].as_f64(),
        max_uncompressed_size: value["maxUncompressedSize"].as_f64(),
    }
}

#[test]
fn parse_listing_on_7za_output() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["archive"]["listings"]) {
        let actual = listing_json(&parse_listing(str_of(&case["stdout"])));
        mismatches.check(
            &actual,
            &case["result"],
            format!("parseListing: {}", case["name"]),
        );
    }
    mismatches.finish("parseListing");
}

#[test]
fn classify_path_problems() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["archive"]["classify"]) {
        mismatches.check(
            &json!(classify_path(str_of(&case[0]))),
            &case[1],
            format!("classifyPath({})", case[0]),
        );
    }
    mismatches.finish("classifyPath");
}

#[test]
fn inspect_entries_reports() {
    let listings = items(&FIXTURE["archive"]["listings"]);
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["archive"]["inspect"]) {
        let entries: Vec<ArchiveEntry> = match case.get("listing") {
            Some(name) => {
                let stdout =
                    listings.iter().find(|l| l["name"] == *name).unwrap()["stdout"].clone();
                parse_listing(str_of(&stdout)).entries
            }
            None => items(&case["entries"]).iter().map(entry_from).collect(),
        };
        let report = inspect_entries(&entries, &options_from(&case["options"]));
        let what = case
            .get("listing")
            .map_or_else(|| "синтетические записи".to_owned(), ToString::to_string);
        mismatches.check(
            &report.to_json(),
            &case["report"],
            format!("inspectEntries({what}, {})", case["options"]),
        );
    }
    mismatches.finish("inspectEntries");
}

#[test]
fn archive_entry_json_has_js_field_order() {
    let entry = &parse_listing("----------\nPath = a\nSize = 1\n").entries[0];
    let keys: Vec<String> = entry
        .to_json()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        keys,
        [
            "path",
            "size",
            "packedSize",
            "attributes",
            "modified",
            "crc",
            "encrypted",
            "isDirectory",
            "isSymlink"
        ]
    );
}

// ------------------------------------------------------------------ semver.js

#[test]
fn semver_parse_and_compare() {
    let section = &FIXTURE["semver"];
    let versions: Vec<&str> = items(&section["parse"])
        .iter()
        .map(|case| str_of(&case[0]))
        .collect();
    let mut mismatches = Mismatches::default();
    for case in items(&section["parse"]) {
        let actual = match semver::parse(str_of(&case[0])) {
            Some(v) => json!({
                "major": hlsr_core::json_text::js_number(v.major),
                "minor": hlsr_core::json_text::js_number(v.minor),
                "patch": hlsr_core::json_text::js_number(v.patch),
                "prerelease": v.prerelease,
            }),
            None => Value::Null,
        };
        mismatches.check(&actual, &case[1], format!("parse({})", case[0]));
    }
    for (i, a) in versions.iter().enumerate() {
        for (j, b) in versions.iter().enumerate() {
            let expected = section["compare"][i][j].as_i64().unwrap();
            let actual = match semver::compare(a, b) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            };
            mismatches.check_eq(actual, expected, format!("compare({a:?}, {b:?})"));
            mismatches.check_eq(
                semver::lt(a, b),
                section["lt"][i][j].as_bool().unwrap(),
                format!("lt({a:?}, {b:?})"),
            );
            mismatches.check_eq(
                semver::gte(a, b),
                section["gte"][i][j].as_bool().unwrap(),
                format!("gte({a:?}, {b:?})"),
            );
        }
    }
    mismatches.finish("semver");
}

// ------------------------------------------------------------------- paths.js

fn library_json(paths: &LibraryPaths) -> Value {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    json!({
        "root": s(&paths.root), "games": s(&paths.games), "tools": s(&paths.tools),
        "downloads": s(&paths.downloads), "backups": s(&paths.backups), "internal": s(&paths.internal),
        "staging": s(&paths.staging), "state": s(&paths.state), "rollback": s(&paths.rollback),
    })
}

#[test]
fn library_layout_and_resolution() {
    let section = &FIXTURE["paths"];
    let mut mismatches = Mismatches::default();
    for platform in PLATFORMS {
        let p = node_path(platform);
        let layout = Layout::of(&p);
        let actual = json!({
            "games": layout.games, "tools": layout.tools, "downloads": layout.downloads, "backups": layout.backups,
            "internal": layout.internal, "staging": layout.staging, "state": layout.state, "rollback": layout.rollback,
        });
        mismatches.check(
            &actual,
            &section["layout"][platform],
            format!("LAYOUT {platform}"),
        );
        for case in items(&section["resolveLibrary"][platform]) {
            let result = delivery(
                paths::resolve_library_with(&p, str_of(&case[0])).map(|lib| library_json(&lib)),
            );
            mismatches.check(
                &outcome(result),
                &case[1],
                format!("resolveLibrary({}) {platform}", case[0]),
            );
        }
    }
    mismatches.finish("resolveLibrary");
}

#[test]
fn inside_checks_posix_and_win32() {
    let section = &FIXTURE["paths"];
    let mut mismatches = Mismatches::default();
    for platform in PLATFORMS {
        let p = node_path(platform);
        for case in items(&section["isInside"][platform]) {
            let actual = paths::is_inside_with(&p, str_of(&case[0]), str_of(&case[1]));
            mismatches.check_eq(
                actual,
                case[2].as_bool().unwrap(),
                format!("isInside({}, {}) {platform}", case[0], case[1]),
            );
        }
        for case in items(&section["assertInside"][platform]) {
            let result = paths::assert_inside_with(
                &p,
                str_of(&case[0]),
                str_of(&case[1]),
                opt_str(&case[2]),
            );
            mismatches.check(
                &outcome(delivery(result)),
                &case[3],
                format!(
                    "assertInside({}, {}, {}) {platform}",
                    case[0], case[1], case[2]
                ),
            );
        }
        for case in items(&section["removeWithinZone"][platform]) {
            let result = paths::removal_target_with(&p, str_of(&case[0]), str_of(&case[1]))
                .map(|_| Value::Null);
            mismatches.check(
                &outcome(delivery(result)),
                &case[2],
                format!("removeWithinZone({}, {}) {platform}", case[0], case[1]),
            );
        }
        let root = if platform == "win32" {
            "C:\\HLSR"
        } else {
            "/home/u/HLSR"
        };
        let library = paths::resolve_library_with(&p, root).unwrap();
        for case in items(&section["gameInstallPath"][platform]) {
            let result = paths::game_install_path_with(&p, &library, str_of(&case[0]));
            mismatches.check(
                &outcome(delivery(result)),
                &case[1],
                format!("gameInstallPath({}) {platform}", case[0]),
            );
        }
        let games = library.games.to_string_lossy();
        for case in items(&section["resolveInside"][platform]) {
            let result = file_ops::resolve_inside_with(&p, &games, str_of(&case[0]), None);
            mismatches.check(
                &outcome(delivery(result)),
                &case[1],
                format!("resolveInside({}) {platform}", case[0]),
            );
        }
    }
    mismatches.finish("paths.js");
}

// ------------------------------------------------------------ маркер библиотеки

#[test]
fn library_marker_reading() {
    let section = &FIXTURE["marker"];
    assert_eq!(MARKER_SCHEMA, section["schema"].as_i64().unwrap());
    assert_eq!(MARKER_ID, str_of(&section["id"]));
    if Platform::CURRENT == Platform::Linux {
        assert_eq!(
            library_marker::marker_relative().to_string_lossy(),
            str_of(&section["relative"])
        );
    }
    let mut mismatches = Mismatches::default();
    for (name, case) in section["read"].as_object().unwrap() {
        let root = tempfile::tempdir().unwrap();
        if let Some(text) = opt_str(&case["text"]) {
            std::fs::create_dir_all(root.path().join(".hlsr")).unwrap();
            std::fs::write(root.path().join(".hlsr").join("library.json"), text).unwrap();
        }
        let actual = library_marker::read_marker(root.path()).unwrap_or(Value::Null);
        mismatches.check(&actual, &case["data"], format!("readMarker: {name}"));
    }
    mismatches.finish("readMarker");
}

#[test]
fn ensure_layout_creates_folders_and_marker_once() {
    let section = &FIXTURE["marker"]["ensureLayout"];
    let now = SystemTime::UNIX_EPOCH + Duration::from_millis(section["now"].as_u64().unwrap());
    let temp = tempfile::tempdir().unwrap();
    // Без символических ссылок в пути, как fs.realpathSync у скрипта; в Windows
    // canonicalize дал бы `\\?\C:\…`, который resolveLibrary справедливо отвергает.
    let base = if cfg!(windows) {
        temp.path().to_path_buf()
    } else {
        std::fs::canonicalize(temp.path()).unwrap()
    };
    let root = base.join("Игры HLSR");
    let root_text = root.to_string_lossy().into_owned();
    let library = paths::resolve_library(&root_text).unwrap();
    let transform = replacing(&root_text, "<root>");
    let state = || {
        let marker = std::fs::read_to_string(root.join(".hlsr").join("library.json")).unwrap();
        let escaped = serde_json::to_string(&root_text).unwrap();
        let marker = marker
            .replace(&escaped[1..escaped.len() - 1], "<root>")
            .replace(&root_text, "<root>");
        json!({"tree": snapshot(&root, Some(&transform)), "marker": marker})
    };

    // 0.4.9: в папку, которой не было, маркер пишется с `createdBy` (base_049.rs); здесь папка
    // есть, и маркер — как у 0.4.7.
    std::fs::create_dir_all(&root).unwrap();
    paths::ensure_layout(&library, now).unwrap();
    assert_json_eq(&state(), &section["first"], "ensureLayout: первый вызов");
    paths::ensure_layout(&library, now + Duration::from_secs(1)).unwrap();
    assert_json_eq(
        &state(),
        &section["second"],
        "ensureLayout: повторный вызов",
    );
    std::fs::write(
        root.join(".hlsr").join("library.json"),
        r#"{"schema":1,"id":"other"}"#,
    )
    .unwrap();
    paths::ensure_layout(&library, now + Duration::from_secs(2)).unwrap();
    assert_json_eq(
        &state(),
        &section["rewritten"],
        "ensureLayout: чужой маркер",
    );
}

// ---------------------------------------------------------------- file-ops.js

#[test]
fn lock_errors_wrap_into_file_locked() {
    let section = &FIXTURE["fileOps"];
    assert_json_eq(&json!(LOCK_CODES), &section["lockCodes"], "LOCK_CODES");
    let mut mismatches = Mismatches::default();
    for case in items(&section["wrapLockError"]) {
        let raw = RawError {
            code: opt_str(&case["code"]).map(str::to_owned),
            message: "x".to_owned(),
            ..RawError::default()
        };
        let actual = file_ops::wrap_lock_error(&raw, str_of(&case["relative"]))
            .map_or(Value::Null, |e| e.to_public());
        mismatches.check(
            &actual,
            &case["public"],
            format!("wrapLockError({}, {})", case["code"], case["relative"]),
        );
    }
    mismatches.finish("wrapLockError");
}

#[tokio::test]
async fn hash_file_reads_in_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let content: Vec<u8> = (0..3 * 1024 * 1024 + 7)
        .map(|i: usize| ((i * 31) % 251) as u8)
        .collect();
    std::fs::write(dir.path().join("big.bin"), &content).unwrap();
    std::fs::write(dir.path().join("empty.bin"), "").unwrap();
    let hashes = &FIXTURE["fileOps"]["hashes"];
    assert_eq!(
        file_ops::hash_file(&dir.path().join("big.bin"))
            .await
            .unwrap(),
        str_of(&hashes["big"])
    );
    assert_eq!(
        file_ops::hash_file(&dir.path().join("empty.bin"))
            .await
            .unwrap(),
        str_of(&hashes["empty"])
    );
    assert!(file_ops::exists(&dir.path().join("big.bin")).await);
    assert!(!file_ops::exists(&dir.path().join("none")).await);
    let missing = file_ops::hash_file(&dir.path().join("none"))
        .await
        .unwrap_err();
    assert_eq!(missing.code.as_deref(), Some("ENOENT"));
}

#[test]
fn prune_empty_dirs_stops_at_root_and_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for sub in ["a/b/c", "x/y", "x/z", "keep"] {
        std::fs::create_dir_all(root.join(sub)).unwrap();
    }
    std::fs::write(root.join("x/z/file.txt"), "f").unwrap();
    file_ops::prune_empty_dirs(root, &root.join("a").join("b").join("c"));
    file_ops::prune_empty_dirs(root, &root.join("x").join("y"));
    file_ops::prune_empty_dirs(root, &root.join("missing").join("dir"));
    file_ops::prune_empty_dirs(&root.join("keep"), root);
    assert_json_eq(
        &snapshot(root, None),
        &FIXTURE["fileOps"]["pruned"],
        "pruneEmptyDirs",
    );
}

#[cfg(unix)]
#[test]
fn copy_and_remove_report_like_node() {
    use std::os::unix::fs::PermissionsExt;
    let section = &FIXTURE["fileOps"];
    let temp = tempfile::tempdir().unwrap();
    let ro = std::fs::canonicalize(temp.path()).unwrap();
    let root_text = ro.to_string_lossy().into_owned();
    std::fs::write(ro.join("source.txt"), "s").unwrap();
    std::fs::create_dir(ro.join("locked")).unwrap();
    std::fs::create_dir(ro.join("folder")).unwrap();
    std::fs::set_permissions(ro.join("locked"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let normalized = |value: Value| -> Value {
        serde_json::from_str(&value.to_string().replace(&root_text, "<root>")).unwrap()
    };

    let copy_locked = file_ops::copy_file_sync(
        &ro.join("source.txt"),
        &ro.join("locked/t.txt"),
        Some("game/t.txt"),
    );
    let copy_creates =
        file_ops::copy_file_sync(&ro.join("source.txt"), &ro.join("new/deep/t.txt"), None);
    let remove_folder = file_ops::remove_file_sync(&ro.join("folder"), Some("game/folder"));
    let remove_missing = file_ops::remove_file_sync(&ro.join("missing.txt"), None);
    std::fs::set_permissions(ro.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_json_eq(
        &normalized(outcome(copy_locked.map(|()| Value::Null))),
        &section["copyLocked"],
        "copyFileSync в каталог без записи",
    );
    assert_json_eq(
        &normalized(outcome(copy_creates.map(|()| Value::Null))),
        &section["copyCreatesDirs"],
        "copyFileSync в новый каталог",
    );
    assert_json_eq(
        &snapshot(&ro, None),
        &section["copiedTree"],
        "дерево после копирования",
    );
    assert_json_eq(
        &normalized(outcome(remove_folder.map(|()| Value::Null))),
        &section["removeFolder"],
        "removeFileSync каталога",
    );
    assert_json_eq(
        &normalized(outcome(remove_missing.map(|()| Value::Null))),
        &section["removeMissing"],
        "removeFileSync без файла",
    );

    // removeWithinZone: libc++ remove_all упирается в каталог без записи — ENOTEMPTY
    // после повторов (паузы 0, 0, 0, 0, 1, 1 с), путь в тексте — UTF-8 как Latin-1.
    let zone_temp = tempfile::tempdir().unwrap();
    let zone_root = std::fs::canonicalize(zone_temp.path()).unwrap();
    let zone = zone_root.join("Загрузки");
    std::fs::create_dir_all(zone.join("Игра")).unwrap();
    std::fs::write(zone.join("Игра").join("архив.7z"), "x").unwrap();
    std::fs::set_permissions(zone.join("Игра"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let started = std::time::Instant::now();
    let denied = paths::remove_within_zone(&zone, &zone.join("Игра"));
    let elapsed = started.elapsed();
    std::fs::set_permissions(zone.join("Игра"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let zone_text = zone_root.to_string_lossy().into_owned();
    let denied: Value = serde_json::from_str(
        &outcome(denied.map(|()| Value::Null))
            .to_string()
            .replace(&zone_text, "<root>"),
    )
    .unwrap();
    assert_json_eq(
        &denied,
        &section["removeDenied"],
        "removeWithinZone без права записи",
    );
    assert!(
        (Duration::from_secs(2)..Duration::from_secs(4)).contains(&elapsed),
        "паузы повторов как у Node 24: {elapsed:?}"
    );

    // Соседи неудаляемого каталога удаляются все, сам он остаётся; итог — ENOTEMPTY.
    let siblings_temp = tempfile::tempdir().unwrap();
    let siblings_root = std::fs::canonicalize(siblings_temp.path()).unwrap();
    let siblings_zone = siblings_root.join("Загрузки");
    let game = siblings_zone.join("игра");
    for rel in [
        "a.txt",
        "b/locked.txt",
        "c/d.txt",
        "e.txt",
        "f/g/h.txt",
        "i/j/k.txt",
        "z.txt",
    ] {
        let file = game.join(rel);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, rel).unwrap();
    }
    std::fs::set_permissions(game.join("b"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let removed = paths::remove_within_zone(&siblings_zone, &game);
    let tree = snapshot(&siblings_zone, None);
    std::fs::set_permissions(game.join("b"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let siblings_text = siblings_root.to_string_lossy().into_owned();
    let removed: Value = serde_json::from_str(
        &outcome(removed.map(|()| Value::Null))
            .to_string()
            .replace(&siblings_text, "<root>"),
    )
    .unwrap();
    assert_json_eq(
        &removed,
        &section["removeSiblings"],
        "removeWithinZone с соседями",
    );
    assert_json_eq(&tree, &section["siblingsTree"], "дерево после повторов");
}

// ------------------------------------------------------------ user-configs.js

#[test]
fn user_edited_configs_like_0_4_7() {
    let section = &FIXTURE["userConfigs"];
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // Фикстура снята в Linux: в Windows `\\` — разделитель, а регистр имён не важен,
    // поэтому такие пути там дают другой (тоже верный для 0.4.7) ответ и пропускаются.
    let known: Vec<String> = section["files"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .chain(
            items(&section["directories"])
                .iter()
                .map(|d| str_of(d).to_owned()),
        )
        .collect();
    let usable = |relative: &str| {
        Platform::CURRENT == Platform::Linux
            || !(relative.contains('\\')
                || known
                    .iter()
                    .any(|k| k != relative && k.eq_ignore_ascii_case(relative)))
    };
    for (relative, text) in section["files"].as_object().unwrap() {
        if !usable(relative) {
            continue;
        }
        let full = root.join(relative);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, str_of(text)).unwrap();
    }
    for relative in items(&section["directories"]) {
        std::fs::create_dir_all(root.join(str_of(relative))).unwrap();
    }
    let previous = |name: &str| -> Option<Value> {
        let value = &section["previousVariants"][name];
        (!value.is_null()).then(|| value.clone())
    };

    let mut mismatches = Mismatches::default();
    for case in items(&section["isUserEdited"]) {
        let relative = str_of(&case["relative"]);
        if !usable(relative) {
            continue;
        }
        let prev = previous(str_of(&case["previous"]));
        let actual = user_configs::is_user_edited(
            root,
            relative,
            prev.as_ref(),
            opt_str(&case["newSha256"]),
        );
        mismatches.check_eq(
            actual,
            case["result"].as_bool().unwrap(),
            format!(
                "isUserEdited({relative:?}, {}, {})",
                case["previous"], case["newSha256"]
            ),
        );
    }
    for (name, expected) in section["keptByPatch"].as_object().unwrap() {
        let kept = user_configs::kept_by_patch(root, &section["patch"], previous(name).as_ref());
        let kept: Vec<String> = kept.into_iter().filter(|p| usable(p)).collect();
        let expected: Vec<&str> = items(expected)
            .iter()
            .map(str_of)
            .filter(|p| usable(p))
            .collect();
        mismatches.check_eq(
            kept,
            expected.iter().map(|s| (*s).to_owned()).collect(),
            format!("keptByPatch: previous {name}"),
        );
    }
    for case in items(&section["isConfigPath"]) {
        mismatches.check_eq(
            user_configs::is_config_path(str_of(&case[0])),
            case[1].as_bool().unwrap(),
            format!("isConfigPath({})", case[0]),
        );
    }
    mismatches.finish("user-configs.js");
}

// -------------------------------------------- progress-states, library-path, free-space

#[test]
fn progress_states_like_shared_module() {
    let section = &FIXTURE["progressStates"];
    assert_json_eq(
        &json!(progress_states::BUSY_STATES),
        &section["busy"],
        "BUSY_STATES",
    );
    assert_json_eq(
        &json!(progress_states::PROGRESS_STATES),
        &section["progress"],
        "PROGRESS_STATES",
    );
    assert_json_eq(
        &json!(progress_states::SUCCESS_STATES),
        &section["success"],
        "SUCCESS_STATES",
    );
    let mut mismatches = Mismatches::default();
    for case in items(&section["checks"]) {
        let state = str_of(&case[0]);
        let actual = json!([
            state,
            progress_states::is_busy(state),
            progress_states::is_progress(state),
            progress_states::is_success(state)
        ]);
        mismatches.check(
            &actual,
            case,
            format!("isBusy/isProgress/isSuccess({state:?})"),
        );
    }
    fn status(value: &Value) -> Option<(&str, &str)> {
        (!value.is_null()).then(|| (str_of(&value["gameId"]), str_of(&value["state"])))
    }
    for case in items(&section["shouldHoldFinish"]) {
        let actual = progress_states::should_hold_finish(status(&case[0]), status(&case[1]));
        mismatches.check_eq(
            actual,
            case[2].as_bool().unwrap(),
            format!("shouldHoldFinish({}, {})", case[0], case[1]),
        );
    }
    mismatches.finish("progress-states.js");
}

#[test]
fn default_library_path_like_shared_module() {
    let section = &FIXTURE["libraryPath"];
    assert_eq!(
        library_path::default_library_path_for(Platform::Linux, str_of(&section["home"])),
        str_of(&section["linux"])
    );
    assert_eq!(
        library_path::default_library_path_for(Platform::Win32, "C:\\Users\\u"),
        str_of(&section["win32"])
    );
}

#[test]
fn free_space_constants() {
    let section = &FIXTURE["freeSpace"];
    assert_eq!(free_space::CACHE_MS, section["cacheMs"].as_u64().unwrap());
    if Platform::CURRENT == Platform::Linux {
        use hlsr_delivery::ports::{FreeSpace, SystemFreeSpace};
        let missing = SystemFreeSpace.free_bytes(Path::new("/nonexistent-hlsr/dir"));
        assert_eq!(
            json!(missing),
            section["missingDir"],
            "viaStatfs без каталога"
        );
    }
}
