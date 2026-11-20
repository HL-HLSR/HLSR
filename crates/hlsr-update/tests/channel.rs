//! Канал обновления лаунчера против 0.4.7: фикстура `channel.json` из
//! `tools/reference/update/channel.js` (модули 0.4.7 в Node 24 на тех же входах; подписи —
//! тестовыми ключами из фиксированных зёрен). Самотесты `launcher-update-selftest.js` («gate falls
//! back and rejects unsigned, Windows installer, old archive schema and downgrade» — часть про
//! `check`) и `diagnostics-selftest.js` («empty, truncated or redirected metadata…» — часть про
//! канал) разобраны по случаям той же фикстуры и по тестам ниже.

mod support;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use hlsr_core::js::Platform;
use hlsr_core::json_text::{js_number, parse_js_json};
use hlsr_update::cancel::CancelToken;
use hlsr_update::channel::{
    self, ALLOWED_HOSTS, ChannelCheck, CheckOptions, LAUNCHER_KEY_ID, LAUNCHER_PUBLIC_KEYS,
    MAX_INSTALLER_BYTES, SUPPORTED_SCHEMA_VERSION, VerifyOptions, VerifyOutcome, check,
    compare_semver, delta_of, is_allowed_url, launcher_for, parse_semver, validate_shape,
    verify_installer_reader, verify_installer_stream,
};
use hlsr_update::chunks::delta_problems;
use hlsr_update::errors::LuCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::path::Path;
use std::pin::Pin;
use std::sync::LazyLock;
use std::task::{Context, Poll};
use support::{Mismatches, assert_json_eq, fixture, items, str_of};
use tokio::io::{AsyncRead, ReadBuf};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("channel.json"));

fn platform(name: &str) -> Platform {
    match name {
        "win32" => Platform::Win32,
        "linux" => Platform::Linux,
        other => panic!("платформа {other}"),
    }
}

fn parse(text: &str) -> Value {
    parse_js_json(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn key_pems(names: &Value) -> Vec<String> {
    items(names)
        .iter()
        .map(|name| str_of(&FIXTURE["keys"][str_of(name)]).to_owned())
        .collect()
}

// ------------------------------------------------------------ константы и адреса

#[test]
fn constants_match_node() {
    let constants = &FIXTURE["constants"];
    assert_eq!(
        constants["SUPPORTED_SCHEMA_VERSION"],
        json!(SUPPORTED_SCHEMA_VERSION)
    );
    assert_eq!(constants["MAX_INSTALLER_BYTES"], json!(MAX_INSTALLER_BYTES));
    assert_eq!(constants["ALLOWED_HOSTS"], json!(ALLOWED_HOSTS));
    assert_eq!(
        constants["LAUNCHER_PUBLIC_KEYS"],
        json!(LAUNCHER_PUBLIC_KEYS)
    );

    // `CODES`: те же имена и строки, ничего лишнего.
    let codes = constants["CODES"].as_object().expect("CODES");
    assert_eq!(codes.len(), LuCode::ALL.len());
    for (name, value) in codes {
        let code = LuCode::ALL
            .iter()
            .find(|code| code.name() == name)
            .unwrap_or_else(|| panic!("нет кода {name}"));
        assert_eq!(code.as_str(), str_of(value), "{name}");
    }
}

#[test]
fn production_key_id_is_the_hash_of_the_spki() {
    // keyId — первые 16 знаков sha256 от DER SubjectPublicKeyInfo (`common.keyIdOf`).
    let body: String = LAUNCHER_PUBLIC_KEYS[0]
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let der = BASE64_STANDARD.decode(body).expect("PEM");
    assert_eq!(der.len(), 44);
    let digest = Sha256::digest(&der);
    let id: String = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(id, LAUNCHER_KEY_ID);
}

#[test]
fn endpoints_match_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["endpoints"]) {
        let name = str_of(&case["platform"]);
        // `launcherFor` — любая ОС, кроме Linux, Windows.
        let target = if name == "linux" {
            Platform::Linux
        } else {
            Platform::Win32
        };
        let launcher = launcher_for(target);
        let context = format!("launcherFor({name})");
        mismatches.check_eq(
            launcher.primary_base.to_owned(),
            str_of(&case["primaryBase"]).to_owned(),
            format!("{context}.primaryBase"),
        );
        mismatches.check_eq(
            launcher.fallback_base.to_owned(),
            str_of(&case["fallbackBase"]).to_owned(),
            format!("{context}.fallbackBase"),
        );
        mismatches.check(
            &json!(launcher.channel_urls),
            &case["channelUrls"],
            format!("{context}.channelUrls"),
        );
        mismatches.check(
            &json!(launcher.latest_yml_urls),
            &case["latestYmlUrls"],
            format!("{context}.latestYmlUrls"),
        );
        mismatches.check(
            &json!(launcher.feed_urls),
            &case["feedUrls"],
            format!("{context}.feedUrls"),
        );
        for entry in items(&case["installerUrls"]) {
            mismatches.check(
                &json!(launcher.installer_urls(str_of(&entry["version"]), str_of(&entry["name"]))),
                &entry["urls"],
                format!(
                    "{context}.installerUrls({}, {})",
                    entry["version"], entry["name"]
                ),
            );
        }
        mismatches.check_eq(
            launcher.release_tag("0.5.0"),
            str_of(&case["releaseTag"]).to_owned(),
            format!("{context}.releaseTag"),
        );
        mismatches.check(
            &json!(launcher.chunk_manifest_urls("HLSR-0.5.0-win32.chunks.json")),
            &case["chunkManifestUrls"],
            format!("{context}.chunkManifestUrls"),
        );
        mismatches.check(
            &json!(launcher.chunk_base_urls()),
            &case["chunkBaseUrls"],
            format!("{context}.chunkBaseUrls"),
        );
    }
    mismatches.finish("адреса лаунчера");
}

// ------------------------------------------------------------ SemVer

#[test]
fn parse_semver_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["semver"]["versions"]) {
        let version = str_of(&case["version"]);
        let actual = parse_semver(version).map_or(Value::Null, |semver| {
            json!({
                "major": js_number(semver.major),
                "minor": js_number(semver.minor),
                "patch": js_number(semver.patch),
                "prerelease": semver.prerelease,
            })
        });
        mismatches.check(
            &actual,
            &case["parsed"],
            format!("parseSemver({version:?})"),
        );
    }
    mismatches.finish("parseSemver");
}

#[test]
fn compare_semver_matches_node_on_every_pair() {
    let versions: Vec<&str> = items(&FIXTURE["semver"]["versions"])
        .iter()
        .map(|case| str_of(&case["version"]))
        .collect();
    let matrix = items(&FIXTURE["semver"]["compare"]);
    assert_eq!(matrix.len(), versions.len());
    let mut mismatches = Mismatches::default();
    for (a, row) in versions.iter().zip(matrix) {
        for (b, expected) in versions.iter().zip(items(row)) {
            let actual = match compare_semver(a, b) {
                Ok(Ordering::Less) => json!(-1),
                Ok(Ordering::Equal) => json!(0),
                Ok(Ordering::Greater) => json!(1),
                Err(error) => json!(error.to_string()),
            };
            mismatches.check(&actual, expected, format!("compareSemver({a:?}, {b:?})"));
        }
    }
    mismatches.finish("compareSemver");
}

/// Пре-релиз сравнивается строкой, а не по правилам SemVer.
#[test]
fn prerelease_is_compared_as_a_string() {
    assert_eq!(
        compare_semver("0.5.0-beta.10", "0.5.0-beta.2"),
        Ok(Ordering::Less)
    );
    assert_eq!(compare_semver("0.5.0-beta.1", "0.5.0"), Ok(Ordering::Less));
    assert_eq!(compare_semver("0.5.0", "0.5.0-rc.1"), Ok(Ordering::Greater));
    assert_eq!(
        compare_semver("0.5.0-Beta", "0.5.0-beta"),
        Ok(Ordering::Less)
    );
    assert!(compare_semver("0.5", "0.5.0").is_err());
}

// ------------------------------------------------------------ allowlist

#[test]
fn is_allowed_url_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["allowedUrls"]) {
        let url = str_of(&case["url"]);
        mismatches.check_eq(
            is_allowed_url(url),
            case["allowed"].as_bool().expect("bool"),
            format!("isAllowedUrl({url:?})"),
        );
    }
    mismatches.finish("isAllowedUrl");
}

// ------------------------------------------------------------ форма и проверка

fn check_json(result: &Result<ChannelCheck, channel::NotSemver>) -> Value {
    match result {
        Ok(check) => json!({"ok": {
            "ok": check.ok,
            "problems": check.problems,
            "updateAvailable": check.update_available,
            "downgrade": check.downgrade,
            "unsupported": check.unsupported,
            "code": check.code.map(LuCode::as_str),
        }}),
        Err(error) => json!({"throws": error.to_string()}),
    }
}

#[test]
fn channels_match_node() {
    let cases = items(&FIXTURE["shapes"]);
    assert!(cases.len() > 250, "в фикстуре {} случаев", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let id = str_of(&case["id"]);
        let target = platform(str_of(&case["platform"]));
        let doc = parse(str_of(&case["doc"]));
        mismatches.check(
            &json!(validate_shape(&doc, target)),
            &case["shape"],
            format!("{id}: validateShape"),
        );

        let keys = key_pems(&case["keys"]);
        let current = case["currentVersion"].as_str();
        let result = check(
            &doc,
            &CheckOptions {
                current_version: current,
                public_keys: Some(&keys),
                platform: Some(target),
            },
        );
        mismatches.check(&check_json(&result), &case["check"], format!("{id}: check"));
    }
    mismatches.finish("каналы");
}

#[test]
fn test_signatures_are_not_accepted_by_the_production_key() {
    // Без `publicKeys` проверка идёт по `LAUNCHER_PUBLIC_KEYS`: тестовая подпись не проходит.
    for case in items(&FIXTURE["productionKey"]) {
        let target = platform(str_of(&case["platform"]));
        let doc = parse(str_of(&case["doc"]));
        let result = check(
            &doc,
            &CheckOptions {
                platform: Some(target),
                ..CheckOptions::default()
            },
        );
        assert_json_eq(
            &check_json(&result),
            &case["check"],
            str_of(&case["platform"]),
        );
    }
}

/// Незнакомые поля (блок `build`, будущие) сохраняются в проверенном документе и покрыты подписью.
#[test]
fn unknown_fields_are_covered_by_the_signature() {
    let signed = |name: &str| {
        let case = items(&FIXTURE["shapes"])
            .iter()
            .find(|case| str_of(&case["id"]) == name)
            .unwrap_or_else(|| panic!("нет случая {name}"));
        (parse(str_of(&case["doc"])), key_pems(&case["keys"]))
    };
    let (doc, keys) = signed("win32: build block signed");
    assert!(doc["build"]["executable"]["sha256"].is_string());
    let options = CheckOptions {
        current_version: Some("0.4.7"),
        public_keys: Some(&keys),
        platform: Some(Platform::Win32),
    };
    assert!(check(&doc, &options).unwrap().ok);
    let (tampered, keys) = signed("win32: tampered build block");
    let options = CheckOptions {
        public_keys: Some(&keys),
        ..options
    };
    let result = check(&tampered, &options).unwrap();
    assert!(!result.ok);
    assert_eq!(result.problems, ["подпись Ed25519 не прошла проверку"]);
}

/// «gate falls back…»: Windows-установщик в канале Linux и наоборот не принимаются.
#[test]
fn installer_of_the_other_platform_is_rejected() {
    for (name, expected_platform, other) in [
        ("win32: ok", Platform::Win32, Platform::Linux),
        ("linux: ok", Platform::Linux, Platform::Win32),
    ] {
        let case = items(&FIXTURE["shapes"])
            .iter()
            .find(|case| str_of(&case["id"]) == name)
            .expect("случай");
        let doc = parse(str_of(&case["doc"]));
        let keys = key_pems(&case["keys"]);
        for (target, ok) in [(expected_platform, true), (other, false)] {
            let result = check(
                &doc,
                &CheckOptions {
                    public_keys: Some(&keys),
                    platform: Some(target),
                    ..CheckOptions::default()
                },
            )
            .unwrap();
            assert_eq!(result.ok, ok, "{name} для {target:?}");
        }
    }
}

// ------------------------------------------------------------ delta

#[test]
fn delta_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["delta"]) {
        let id = str_of(&case["id"]);
        if let Some(text) = case["delta"].as_str() {
            let delta = parse(text);
            mismatches.check(
                &json!(delta_problems(&delta, is_allowed_url)),
                &case["problems"],
                format!("{id}: deltaProblems"),
            );
            // `deltaOf` смотрит на документ с этим блоком.
            let mut doc = serde_json::Map::new();
            doc.insert("delta".to_owned(), delta);
            let actual = delta_of(&Value::Object(doc))
                .cloned()
                .unwrap_or(Value::Null);
            mismatches.check(&actual, &case["deltaOf"], format!("{id}: deltaOf"));
        } else if id == "no delta" {
            mismatches.check(
                &delta_of(&json!({})).cloned().unwrap_or(Value::Null),
                &case["deltaOf"],
                id.to_owned(),
            );
        } else {
            mismatches.check(
                &delta_of(&Value::Null).cloned().unwrap_or(Value::Null),
                &case["deltaOf"],
                id.to_owned(),
            );
        }
    }
    mismatches.finish("delta");
}

// ------------------------------------------------------------ проверка файла

fn outcome_json(outcome: &VerifyOutcome) -> Value {
    json!({
        "ok": outcome.ok,
        "problems": outcome.problems,
        "code": outcome.code.map(LuCode::as_str),
    })
}

/// Читает кусками не больше `limit` байт: как поток Node с заданным размером куска.
struct Chunked {
    data: Vec<u8>,
    at: usize,
    limit: usize,
}

impl AsyncRead for Chunked {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let count = (self.data.len() - self.at)
            .min(self.limit)
            .min(buf.remaining());
        let start = self.at;
        buf.put_slice(&self.data[start..start + count]);
        self.at += count;
        Poll::Ready(Ok(()))
    }
}

/// В фикстуре каталог временных файлов — `{dir}`, разделитель после него всегда «/».
fn unmask_dir(value: &Value, dir: &str) -> Value {
    match value {
        Value::String(text) => {
            Value::String(text.replace("{dir}/", &format!("{dir}{}", std::path::MAIN_SEPARATOR)))
        }
        Value::Array(list) => Value::Array(list.iter().map(|item| unmask_dir(item, dir)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| (key.clone(), unmask_dir(item, dir)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn content_of(spec: &Value) -> Vec<u8> {
    let times = spec["times"].as_u64().expect("times") as usize;
    str_of(&spec["repeat"]).repeat(times).into_bytes()
}

#[tokio::test]
async fn verify_installer_stream_matches_node() {
    let dir = tempfile::tempdir().expect("временный каталог");
    let dir_text = dir.path().display().to_string();
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["verify"]["cases"]) {
        let id = str_of(&case["id"]);
        let expected = unmask_dir(&case["result"], &dir_text);
        let installer = &case["installer"];
        let actual = if let Some(stream) = case["stream"].as_str() {
            // Поток длиннее или короче, чем сказал stat: куски по `chunk` байт.
            let content = content_of(&FIXTURE["verify"]["content"]);
            let chunk = case["chunk"].as_u64().expect("chunk") as usize;
            let data = match stream {
                "longer" => {
                    let mut data = content.clone();
                    data.extend(std::iter::repeat_n(
                        7u8,
                        case["extraBytes"].as_u64().unwrap() as usize,
                    ));
                    data
                }
                "shorter" => {
                    content[..content.len() - case["shortBy"].as_u64().unwrap() as usize].to_vec()
                }
                other => panic!("поток {other}"),
            };
            let reader = Chunked {
                data,
                at: 0,
                limit: chunk,
            };
            verify_installer_reader(reader, content.len() as u64, installer, None).await
        } else {
            let file = dir.path().join("installer.exe");
            let _ = std::fs::remove_file(&file);
            if case["source"]["missing"].as_bool() != Some(true) {
                std::fs::write(&file, content_of(&case["source"]["content"])).unwrap();
            }
            let options = &case["options"];
            let cancel = options["aborted"].as_bool().unwrap_or(false).then(|| {
                let token = CancelToken::new();
                token.cancel();
                token
            });
            verify_installer_stream(
                &file,
                installer,
                &VerifyOptions {
                    max_bytes: options["maxBytes"].as_u64(),
                    cancel,
                },
            )
            .await
        };
        mismatches.check(&outcome_json(&actual), &expected, format!("verify: {id}"));
    }
    mismatches.finish("verifyInstallerStream");
}

/// Файл заметно длиннее блока чтения: хеши считаются по всем кускам.
#[tokio::test]
async fn verify_installer_stream_hashes_a_multi_chunk_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big.bin");
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&file, &data).unwrap();
    let sha256 = support::sha256_hex(&data);
    let sha512 = {
        use sha2::Sha512;
        BASE64_STANDARD.encode(Sha512::digest(&data))
    };
    let installer = json!({"size": data.len(), "sha256": sha256, "sha512": sha512});
    let result = verify_installer_stream(&file, &installer, &VerifyOptions::default()).await;
    assert!(result.ok, "{result:?}");
    // Отмена между кусками тоже останавливает чтение.
    let token = CancelToken::new();
    token.cancel();
    let result = verify_installer_stream(
        &file,
        &installer,
        &VerifyOptions {
            max_bytes: None,
            cancel: Some(token),
        },
    )
    .await;
    assert_eq!(result.code, Some(LuCode::InstallerUnreadable));
    // Каталог вместо файла не проходит ни по размеру, ни по чтению.
    let result =
        verify_installer_stream(Path::new(dir.path()), &installer, &VerifyOptions::default()).await;
    assert!(!result.ok);
}

// ------------------------------------------------------------ боевые каналы

fn find_channels(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            find_channels(&path, out);
        } else if entry.file_name() == "launcher-channel.json" {
            out.push(path);
        }
    }
}

/// Боевые `release/**/launcher-channel.json` (каталог — `HLSR_RELEASE_DIR`, по умолчанию
/// `<репозиторий>/release`) проходят проверку формы и подписи боевым ключом, как у клиента 0.4.7.
/// Каталога на машине разработки Windows нет: тест молча ничего не проверяет (строка в «Linux —
/// отложено» отчёта задачи 1).
#[test]
fn local_release_channels_pass_the_production_check() {
    let root = std::env::var_os("HLSR_RELEASE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release"));
    let mut files = Vec::new();
    find_channels(&root, &mut files);
    if files.is_empty() {
        eprintln!(
            "каталога release/ нет ({}): боевые каналы не проверялись",
            root.display()
        );
        return;
    }
    for file in files {
        let text = std::fs::read_to_string(&file).expect("канал читается");
        let doc = parse(&text);
        let linux = file.components().any(|part| part.as_os_str() == "linux");
        let result = check(
            &doc,
            &CheckOptions {
                platform: Some(if linux {
                    Platform::Linux
                } else {
                    Platform::Win32
                }),
                ..CheckOptions::default()
            },
        )
        .unwrap();
        assert!(result.ok, "{}: {:?}", file.display(), result.problems);
    }
}
