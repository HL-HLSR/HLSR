//! Подписи, адреса канала и флаги против 0.4.7: фикстура `signing.json` из
//! `tools/reference/delivery/signing.js` (модули 0.4.7 в Node 24 на тех же входах;
//! подписи — тестовыми ключами из фиксированных зёрен).

mod support;

use hlsr_core::js::Platform;
use hlsr_core::json_text::{parse_js_json, stringify};
use hlsr_delivery::channel_config::{self, ChannelConfig, QaUrlKind, Source};
use hlsr_delivery::delivery_endpoints::{self as endpoints, LauncherEndpoints};
use hlsr_delivery::environment::Environment;
use hlsr_delivery::errors::Code;
use hlsr_delivery::feature_flag;
use hlsr_delivery::file_ops::hex;
use hlsr_delivery::manifest_signing::{
    self, Check, CheckOptions, Reason, attach, canonicalize, check, key_pair_from_seed, sign,
    verify,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use support::{Mismatches, assert_json_eq, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("signing.json"));

/// Сборка с отладочными возможностями: QA-переопределения канала в ней доступны.
const DEV_BUILD: bool = cfg!(any(debug_assertions, feature = "dev-hooks"));

fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Текст документа из фикстуры: `@game` и `@good` — общие документы, остальное — сам текст JSON.
fn doc_text(reference: &str) -> &str {
    match reference {
        "@game" | "@good" => str_of(&FIXTURE["docs"][&reference[1..]]),
        text => text,
    }
}

fn parse(text: &str) -> Value {
    parse_js_json(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn key(name: &str, field: &str) -> &'static str {
    str_of(&FIXTURE["keys"][name][field])
}

fn pems(names: &Value) -> Vec<String> {
    items(names).iter().map(|n| str_of(n).to_owned()).collect()
}

/// `attach` для документа-объекта.
fn attached(manifest: &Value, private_key_pem: &str) -> Value {
    Value::Object(attach(manifest.as_object().expect("объект"), private_key_pem).unwrap())
}

// ------------------------------------------------------------ canonicalize

#[test]
fn canonicalize_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["canonicalize"]["values"]) {
        let json = str_of(&case["json"]);
        mismatches.check_eq(
            canonicalize(&parse(json)),
            str_of(&case["canonical"]).to_owned(),
            format!("canonicalize(JSON.parse({json}))"),
        );
    }
    mismatches.finish("canonicalize");
}

/// Одиночный суррогат JS пишет как `\udXXX`; `parse_js_json` (как при чтении файла
/// 0.4.7 в Rust) заменяет его на U+FFFD, поэтому канонический текст отличается только этим.
#[test]
fn canonicalize_lone_surrogates_become_replacement_characters() {
    for case in items(&FIXTURE["canonicalize"]["loneSurrogates"]) {
        let json = str_of(&case["json"]);
        let node = str_of(&case["canonical"]);
        let mut expected = String::new();
        let mut rest = node;
        while let Some(at) = rest.find("\\ud") {
            let unit = &rest[at + 2..at + 6];
            let code = u32::from_str_radix(unit, 16).unwrap();
            assert!((0xD800..0xE000).contains(&code), "{node}");
            expected.push_str(&rest[..at]);
            expected.push('\u{FFFD}');
            rest = &rest[at + 6..];
        }
        expected.push_str(rest);
        assert_eq!(canonicalize(&parse(json)), expected, "{json}");
    }
}

#[test]
fn canonicalize_embedded_configs_match_node() {
    let mut mismatches = Mismatches::default();
    let files = items(&FIXTURE["canonicalize"]["configs"]);
    assert!(
        files.len() >= 20,
        "в фикстуре {} файлов config",
        files.len()
    );
    for case in files {
        let relative = str_of(&case["file"]);
        let path = workspace_root().join(relative);
        let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        mismatches.check_eq(
            sha256_hex(&raw),
            str_of(&case["fileSha256"]).to_owned(),
            format!(
                "{relative}: файл изменён с момента записи фикстуры (перезапустите signing.js)"
            ),
        );
        let canonical = canonicalize(&parse(std::str::from_utf8(&raw).unwrap()));
        mismatches.check_eq(
            sha256_hex(canonical.as_bytes()),
            str_of(&case["canonicalSha256"]).to_owned(),
            format!("{relative}: хеш канонической формы"),
        );
        mismatches.check_eq(
            canonical.len() as u64,
            case["canonicalLength"].as_u64().unwrap(),
            format!("{relative}: длина канонической формы"),
        );
    }
    mismatches.finish("canonicalize config/**");
}

/// Локальные `release/**/launcher-channel.json` в git не входят: проверяются те, что есть
/// на диске (`HLSR_RELEASE_DIR` или `release/` в корне), остальные записи фикстуры пропускаются.
#[test]
fn canonicalize_and_check_local_launcher_channels() {
    let section = &FIXTURE["canonicalize"]["release"];
    let launcher_keys = pems(&section["publicKeys"]);
    let dir = std::env::var_os("HLSR_RELEASE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("release"));
    let mut mismatches = Mismatches::default();
    let mut checked = 0;
    for case in items(&section["files"]) {
        let relative = str_of(&case["file"]);
        let Ok(raw) = std::fs::read(dir.join(relative)) else {
            continue;
        };
        if sha256_hex(&raw) != str_of(&case["fileSha256"]) {
            continue;
        }
        checked += 1;
        let manifest = parse(std::str::from_utf8(&raw).unwrap());
        let canonical = canonicalize(&manifest);
        mismatches.check_eq(
            sha256_hex(canonical.as_bytes()),
            str_of(&case["canonicalSha256"]).to_owned(),
            format!("{relative}: хеш канонической формы"),
        );
        let result = check(
            &manifest,
            &CheckOptions {
                required: false,
                public_keys: &launcher_keys,
            },
        );
        mismatches.check(
            &check_json(&result),
            &json!({"ok": case["check"]}),
            format!("{relative}: check()"),
        );
    }
    eprintln!(
        "launcher-channel.json: сверено {checked} из {} (каталог {})",
        items(&section["files"]).len(),
        dir.display()
    );
    mismatches.finish("release/**/launcher-channel.json");
}

// ------------------------------------------------------------ ключи, sign, attach

#[test]
fn key_pairs_from_seeds_have_the_pem_of_node() {
    for name in ["A", "B", "C"] {
        let seed = hex_bytes(str_of(&FIXTURE["keys"][name]["seed"]));
        let pair = key_pair_from_seed(seed.as_slice().try_into().unwrap());
        assert_eq!(pair.public_key_pem, key(name, "publicKeyPem"), "{name}");
        assert_eq!(pair.private_key_pem, key(name, "privateKeyPem"), "{name}");
    }
}

fn hex_bytes(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

/// Ed25519 детерминирован: подпись Rust над тем же документом тем же ключом совпадает с подписью Node.
#[test]
fn signatures_are_those_of_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["sign"]["signatures"]) {
        let json = str_of(&case["json"]);
        let name = str_of(&case["key"]);
        let actual = sign(&parse(json), key(name, "privateKeyPem")).unwrap();
        mismatches.check(&actual, &case["signature"], format!("sign({json}, {name})"));
    }
    mismatches.finish("sign");
}

#[test]
fn attach_replaces_the_signature_and_puts_it_last() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["sign"]["attach"]) {
        let json = str_of(&case["json"]);
        let signed = attached(&parse(json), key("A", "privateKeyPem"));
        mismatches.check_eq(
            stringify(&signed, ""),
            str_of(&case["attached"]).to_owned(),
            format!("attach({json})"),
        );
    }
    mismatches.finish("attach");
}

/// Исключение при подписи — тот же код; текст OpenSSL в Rust свой, поэтому сравнивается код.
#[test]
fn sign_rejects_what_node_rejects() {
    for case in items(&FIXTURE["sign"]["errors"]) {
        let name = str_of(&case["name"]);
        let error = sign(&json!({"a": 1}), str_of(&case["pem"]))
            .expect_err(&format!("sign с ключом «{name}» должна отказать"));
        assert_eq!(
            error.code.as_str(),
            str_of(&case["error"]["code"]),
            "{name}"
        );
        assert!(
            error.message.starts_with("Некорректный приватный ключ: "),
            "{name}: {}",
            error.message
        );
        let manifest = json!({"a": 1});
        let error = attach(manifest.as_object().unwrap(), str_of(&case["pem"])).unwrap_err();
        assert_eq!(error.code, Code::ManifestInvalid, "{name}");
    }
}

// ------------------------------------------------------------ verify, check

#[test]
fn verify_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["verify"]) {
        let id = str_of(&case["id"]);
        let manifest = parse(doc_text(str_of(&case["doc"])));
        let signature = parse(str_of(&case["signature"]));
        mismatches.check_eq(
            verify(&manifest, &signature, str_of(&case["publicKey"])),
            case["result"].as_bool().unwrap(),
            format!("verify: {id}"),
        );
    }
    mismatches.finish("verify");
}

fn check_json(result: &Result<Check, hlsr_delivery::errors::DeliveryError>) -> Value {
    match result {
        Ok(check) => json!({"ok": {
            "signed": check.signed,
            "valid": check.valid,
            "reason": check.reason.map(Reason::as_str),
        }}),
        Err(error) => json!({"error": error.to_public()}),
    }
}

#[test]
fn check_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["check"]) {
        let id = str_of(&case["id"]);
        let manifest = parse(doc_text(str_of(&case["doc"])));
        let options = &case["options"];
        let public_keys = options["publicKeys"]
            .as_array()
            .map(|_| pems(&options["publicKeys"]))
            .unwrap_or_default();
        let result = check(
            &manifest,
            &CheckOptions {
                required: options["required"].as_bool().unwrap_or(false),
                public_keys: &public_keys,
            },
        );
        mismatches.check(
            &check_json(&result),
            &case["result"],
            format!("check: {id}"),
        );
    }
    mismatches.finish("check");
}

#[test]
fn trusted_public_keys_are_empty_by_default() {
    let signed = attached(&json!({"a": 1}), key("A", "privateKeyPem"));
    let result = check(&signed, &CheckOptions::default()).unwrap();
    assert_eq!(
        result,
        Check {
            signed: true,
            valid: false,
            reason: Some(Reason::NoTrustedKeys)
        }
    );
    assert_eq!(manifest_signing::ALGORITHM, "ed25519");
    assert_eq!(manifest_signing::SIGNATURE_FIELD, "signature");
}

// ------------------------------------------------------------ delivery-endpoints.js

fn s(value: &str) -> Value {
    Value::String(value.to_owned())
}

fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().map(|v| s(v)).collect())
}

fn launcher_summary(feed: &LauncherEndpoints) -> Value {
    json!({
        "platform": feed.platform.as_str(),
        "primaryBase": feed.primary_base,
        "fallbackBase": feed.fallback_base,
        "channelUrls": strings(&feed.channel_urls),
        "latestYmlUrls": strings(&feed.latest_yml_urls),
        "feedUrls": strings(&feed.feed_urls),
        "installerUrls": strings(&feed.installer_urls("1.0.0", "x.exe")),
        "releaseTag": feed.release_tag("1.0.0"),
        "chunkManifestUrls": strings(&feed.chunk_manifest_urls("m.json")),
        "chunkBaseUrls": feed.chunk_base_urls(),
    })
}

/// Платформа как её понимает `launcherFor`: всё, кроме `"linux"`, — Windows.
fn feed_platform(name: &str) -> Platform {
    if name == "linux" {
        Platform::Linux
    } else {
        Platform::Win32
    }
}

#[test]
fn endpoint_constants_match_node() {
    let expected = &FIXTURE["endpoints"]["constants"];
    let strs = |value: &[&str]| Value::Array(value.iter().map(|v| s(v)).collect());
    let actual = json!({
        "PRIMARY_HOST": endpoints::PRIMARY_HOST,
        "FALLBACK_HOST": endpoints::FALLBACK_HOST,
        "FALLBACK_ASSET_HOST": endpoints::FALLBACK_ASSET_HOST,
        "BXT_HOST": endpoints::BXT_HOST,
        "PRIMARY_BASE": endpoints::PRIMARY_BASE,
        "RELEASES_REPO": endpoints::RELEASES_REPO,
        "LAUNCHER_RELEASES_REPO": endpoints::LAUNCHER_RELEASES_REPO,
        "GAME_RELEASES_REPO": endpoints::GAME_RELEASES_REPO,
        "GITHUB_RELEASE_BASE": endpoints::GITHUB_RELEASE_BASE,
        "GAME_GITHUB_RELEASE_BASE": endpoints::GAME_GITHUB_RELEASE_BASE,
        "GITHUB_ASSET_MAX_BYTES": endpoints::GITHUB_ASSET_MAX_BYTES,
        "LAUNCHER_PRIMARY_BASE": endpoints::launcher_for(Platform::Linux).primary_base,
        "LAUNCHER_FALLBACK_BASE": endpoints::launcher_for(Platform::Linux).fallback_base,
        "LAUNCHER_CHUNKS_BASE": endpoints::LAUNCHER_CHUNKS_BASE,
        "ALLOWED_DOWNLOAD_HOSTS": strs(endpoints::ALLOWED_DOWNLOAD_HOSTS),
        "FORBIDDEN_DOWNLOAD_HOSTS": strs(endpoints::FORBIDDEN_DOWNLOAD_HOSTS),
    });
    assert_json_eq(&actual, expected, "константы delivery-endpoints");
    for (name, platform) in [("linux", Platform::Linux), ("win32", Platform::Win32)] {
        let feed = endpoints::launcher_for(platform);
        assert_json_eq(
            &json!({
                "LAUNCHER_PRIMARY_BASE": feed.primary_base,
                "LAUNCHER_FALLBACK_BASE": feed.fallback_base,
            }),
            &FIXTURE["endpoints"]["constantsByPlatform"][name],
            name,
        );
    }
}

fn size_of_case(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(text) => match text.as_str() {
            "undefined" => None,
            "Infinity" => Some(f64::INFINITY),
            "-Infinity" => Some(f64::NEG_INFINITY),
            // NaN, null и строка: Number.isFinite даёт false, как для NaN.
            _ => Some(f64::NAN),
        },
        other => panic!("размер {other}"),
    }
}

#[test]
fn github_asset_limit_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["endpoints"]["fitsGithubAsset"]) {
        // fitsGithubAsset(undefined): Number.isFinite(undefined) === false.
        let size = size_of_case(&case["size"]).unwrap_or(f64::NAN);
        mismatches.check_eq(
            endpoints::fits_github_asset(size),
            case["result"].as_bool().unwrap(),
            format!("fitsGithubAsset({})", case["size"]),
        );
    }
    mismatches.finish("fitsGithubAsset");
}

#[test]
fn launcher_endpoints_match_node() {
    let mut mismatches = Mismatches::default();
    for (name, expected) in FIXTURE["endpoints"]["launcherFor"].as_object().unwrap() {
        let feed = endpoints::launcher_for(feed_platform(name));
        let ctx = format!("launcherFor({name:?})");
        mismatches.check_eq(
            feed.platform.as_str(),
            str_of(&expected["platform"]),
            format!("{ctx}.platform"),
        );
        mismatches.check_eq(
            feed.primary_base,
            str_of(&expected["primaryBase"]),
            format!("{ctx}.primaryBase"),
        );
        mismatches.check_eq(
            feed.fallback_base,
            str_of(&expected["fallbackBase"]),
            format!("{ctx}.fallbackBase"),
        );
        for (field, actual) in [
            ("channelUrls", &feed.channel_urls),
            ("latestYmlUrls", &feed.latest_yml_urls),
            ("feedUrls", &feed.feed_urls),
        ] {
            mismatches.check(&strings(actual), &expected[field], format!("{ctx}.{field}"));
        }
        for case in items(&expected["installerUrls"]) {
            let (version, installer) = (str_of(&case["version"]), str_of(&case["name"]));
            mismatches.check(
                &strings(&feed.installer_urls(version, installer)),
                &case["urls"],
                format!("{ctx}.installerUrls({version:?}, {installer:?})"),
            );
        }
        for case in items(&expected["releaseTags"]) {
            mismatches.check_eq(
                feed.release_tag(str_of(&case[0])),
                str_of(&case[1]).to_owned(),
                format!("{ctx}.releaseTag({})", case[0]),
            );
        }
        for case in items(&expected["chunkManifestUrls"]) {
            mismatches.check(
                &strings(&feed.chunk_manifest_urls(str_of(&case[0]))),
                &case[1],
                format!("{ctx}.chunkManifestUrls({})", case[0]),
            );
        }
        mismatches.check(
            &json!(feed.chunk_base_urls()),
            &expected["chunkBaseUrls"],
            format!("{ctx}.chunkBaseUrls()"),
        );
    }
    for (name, platform) in [("linux", Platform::Linux), ("win32", Platform::Win32)] {
        mismatches.check(
            &launcher_summary(&endpoints::launcher_for(platform)),
            &FIXTURE["endpoints"]["launcher"][name],
            format!("launcher ({name})"),
        );
    }
    mismatches.check_eq(
        endpoints::launcher(),
        endpoints::launcher_for(Platform::CURRENT),
        "launcher = launcherFor(process.platform)",
    );
    mismatches.finish("launcherFor");
}

#[test]
fn game_endpoints_match_node() {
    let game = &FIXTURE["endpoints"]["game"];
    let mut mismatches = Mismatches::default();
    for case in items(&game["channelUrl"]) {
        mismatches.check_eq(
            endpoints::game::channel_url(str_of(&case[0])),
            str_of(&case[1]).to_owned(),
            format!("game.channelUrl({})", case[0]),
        );
    }
    for case in items(&game["releaseTag"]) {
        mismatches.check_eq(
            endpoints::game::release_tag(str_of(&case[0]), str_of(&case[1])),
            str_of(&case[2]).to_owned(),
            format!("game.releaseTag({}, {})", case[0], case[1]),
        );
    }
    for case in items(&game["primaryFileUrl"]) {
        mismatches.check_eq(
            endpoints::game::primary_file_url(str_of(&case[0]), str_of(&case[1]), str_of(&case[2])),
            str_of(&case[3]).to_owned(),
            format!("game.primaryFileUrl({}, {}, {})", case[0], case[1], case[2]),
        );
    }
    for case in items(&game["fallbackFileUrl"]) {
        mismatches.check_eq(
            endpoints::game::fallback_file_url(
                str_of(&case[0]),
                str_of(&case[1]),
                str_of(&case[2]),
            ),
            str_of(&case[3]).to_owned(),
            format!(
                "game.fallbackFileUrl({}, {}, {})",
                case[0], case[1], case[2]
            ),
        );
    }
    mismatches.check_eq(
        endpoints::game::release_repository(),
        str_of(&game["releaseRepository"]),
        "game.releaseRepository()",
    );
    for case in items(&game["providersFor"]) {
        let providers = endpoints::game::providers_for(
            str_of(&case["gameId"]),
            str_of(&case["version"]),
            str_of(&case["filename"]),
            size_of_case(&case["size"]),
        );
        mismatches.check(
            &Value::Array(providers),
            &case["providers"],
            format!(
                "game.providersFor({}, size {})",
                case["gameId"], case["size"]
            ),
        );
    }
    mismatches.finish("delivery-endpoints game");
}

#[test]
fn allowed_download_urls_match_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["endpoints"]["isAllowedDownloadUrl"]) {
        let url = str_of(&case["url"]);
        mismatches.check_eq(
            endpoints::is_allowed_download_url(url),
            case["result"].as_bool().unwrap(),
            format!("isAllowedDownloadUrl({url:?})"),
        );
    }
    mismatches.finish("isAllowedDownloadUrl");
}

// ------------------------------------------------------------ channel-config.js, feature-flag.js

fn environment(vars: &Value, production: bool) -> Environment {
    Environment::new(
        vars.as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| (name.clone(), str_of(value).to_owned())),
        production,
    )
}

/// Случаи с `production: false` исполняются там, где QA-переопределения компилируются.
fn runnable(production: bool) -> bool {
    production || DEV_BUILD
}

fn source_json(source: Option<Source>) -> Value {
    source.map_or(Value::Null, |source| s(source.as_str()))
}

fn config_json(config: &ChannelConfig) -> Value {
    json!({
        "gameId": config.game_id,
        "url": config.url,
        "channel": config.channel,
        "requireSignature": config.require_signature,
        "publicKeys": config.public_keys,
        "source": source_json(config.source),
    })
}

#[test]
fn builtin_channels_match_node() {
    let mut actual = Map::new();
    for game_id in channel_config::GAME_IDS {
        let base = channel_config::builtin_channel(game_id).unwrap();
        actual.insert(
            game_id.to_owned(),
            json!({
                "url": base.url,
                "channel": base.channel,
                "requireSignature": base.require_signature,
                "publicKeys": base.public_keys,
                "keyIds": base.key_ids,
                "releaseProfile": {"enabled": base.release_profile_enabled},
            }),
        );
    }
    assert_json_eq(
        &Value::Object(actual),
        &FIXTURE["channelConfig"]["channels"],
        "CHANNELS",
    );
    assert!(channel_config::builtin_channel("hl1-won").is_none());
    let constants = &FIXTURE["channelConfig"]["constants"];
    assert_eq!(channel_config::QA_URL_ENV, str_of(&constants["QA_URL_ENV"]));
    assert_eq!(channel_config::QA_KEY_ENV, str_of(&constants["QA_KEY_ENV"]));
    assert_eq!(
        channel_config::QA_CHANNEL_ENV,
        str_of(&constants["QA_CHANNEL_ENV"])
    );
    assert_json_eq(
        &json!(channel_config::QA_ALLOWED_HTTPS_HOSTS),
        &constants["QA_ALLOWED_HTTPS_HOSTS"],
        "QA_ALLOWED_HTTPS_HOSTS",
    );
}

#[test]
fn release_profile_is_active_only_in_production() {
    for case in items(&FIXTURE["channelConfig"]["hasActiveReleaseProfile"]) {
        let production = case["production"].as_bool().unwrap();
        assert_eq!(
            channel_config::has_active_release_profile(&Environment::new(
                Vec::<(String, String)>::new(),
                production
            )),
            case["result"].as_bool().unwrap(),
            "production {production}"
        );
    }
}

#[test]
fn qa_url_kind_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["channelConfig"]["qaUrlKind"]) {
        let production = case["production"].as_bool().unwrap();
        if !runnable(production) {
            continue;
        }
        let url = str_of(&case["url"]);
        let kind = channel_config::qa_url_kind(url, &environment(&case["env"], production));
        let actual = kind.map_or(Value::Null, |kind| s(kind.as_str()));
        mismatches.check(
            &actual,
            &case["result"],
            format!(
                "qaUrlKind({url:?}, {}) при production={production}",
                case["env"]
            ),
        );
    }
    mismatches.finish("qaUrlKind");
    assert_eq!(QaUrlKind::Https.as_str(), "https");
    assert_eq!(QaUrlKind::LocalHttp.as_str(), "local-http");
}

#[test]
fn resolve_and_describe_match_node() {
    let mut mismatches = Mismatches::default();
    let mut ran = 0;
    for case in items(&FIXTURE["channelConfig"]["resolve"]) {
        let production = case["production"].as_bool().unwrap();
        if !runnable(production) {
            continue;
        }
        ran += 1;
        let game_id = str_of(&case["gameId"]);
        let env = environment(&case["env"], production);
        let context = format!("{game_id:?}, env {}, production={production}", case["env"]);
        let resolved = channel_config::resolve(game_id, &env)
            .map_or(Value::Null, |config| config_json(&config));
        mismatches.check(&resolved, &case["resolve"], format!("resolve({context})"));
        let described = channel_config::describe(game_id, &env);
        mismatches.check(
            &json!({
                "gameId": described.game_id,
                "configured": described.configured,
                "source": source_json(described.source),
                "requireSignature": described.require_signature,
                "trustedKeys": described.trusted_keys,
            }),
            &case["describe"],
            format!("describe({context})"),
        );
    }
    // В выпуске без dev-hooks — только случаи production (их 56 из 112).
    assert!(ran >= 56, "выполнено только {ran} случаев");
    mismatches.finish("channel-config resolve/describe");
}

#[test]
fn feature_flags_match_node() {
    let flag = &FIXTURE["featureFlag"]["constants"];
    assert_eq!(feature_flag::FLAG, str_of(&flag["FLAG"]));
    assert_eq!(
        feature_flag::LOCAL_HTTP_FLAG,
        str_of(&flag["LOCAL_HTTP_FLAG"])
    );
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["featureFlag"]["cases"]) {
        let production = case["production"].as_bool().unwrap();
        if !runnable(production) {
            continue;
        }
        let env = environment(&case["env"], production);
        let context = format!("env {}, production={production}", case["env"]);
        mismatches.check_eq(
            feature_flag::is_remote_delivery_enabled(&env),
            case["isRemoteDeliveryEnabled"].as_bool().unwrap(),
            format!("isRemoteDeliveryEnabled({context})"),
        );
        mismatches.check_eq(
            feature_flag::is_release_profile_active(&env),
            case["isReleaseProfileActive"].as_bool().unwrap(),
            format!("isReleaseProfileActive({context})"),
        );
        mismatches.check_eq(
            feature_flag::is_local_http_allowed(&env),
            case["isLocalHttpAllowed"].as_bool().unwrap(),
            format!("isLocalHttpAllowed({context})"),
        );
        mismatches.check(
            &json!(feature_flag::allowed_provider_types(&env)),
            &case["allowedProviderTypes"],
            format!("allowedProviderTypes({context})"),
        );
        let described = feature_flag::describe(&env);
        mismatches.check(
            &json!({
                "flag": described.flag,
                "remoteEnabled": described.remote_enabled,
                "releaseProfileActive": described.release_profile_active,
                "localHttpAllowed": described.local_http_allowed,
                "allowedProviderTypes": described.allowed_provider_types,
            }),
            &case["describe"],
            format!("describe({context})"),
        );
    }
    mismatches.finish("feature-flag");
}

/// QA-канал и локальный HTTP существуют только в отладочной сборке и с `dev-hooks`:
/// в выпуске их не включит и окружение, объявленное не-production.
#[test]
fn debug_features_exist_only_in_debug_builds() {
    let qa = Environment::new(
        [
            (
                channel_config::QA_URL_ENV,
                "http://127.0.0.1:9000/stable.json",
            ),
            (feature_flag::LOCAL_HTTP_FLAG, "1"),
        ],
        false,
    );
    assert_eq!(feature_flag::is_local_http_allowed(&qa), DEV_BUILD);
    let config = channel_config::resolve("ghosting-renewed", &qa).unwrap();
    assert_eq!(config.source, DEV_BUILD.then_some(Source::Qa));
    assert_eq!(config.url.is_some(), DEV_BUILD);
    assert_eq!(
        feature_flag::allowed_provider_types(&qa).contains(&"local-http"),
        DEV_BUILD
    );
}

#[test]
fn production_build_is_a_release_without_dev_hooks() {
    assert_eq!(channel_config::is_production_build(), !DEV_BUILD);
    assert_eq!(Environment::process().is_production(), !DEV_BUILD);
    // Переменная процесса читается, а не копируется при создании.
    let path = std::env::var("PATH").ok();
    assert_eq!(Environment::process().var("PATH"), path);
    assert_eq!(
        Environment::new([("A", "1")], true).var("A"),
        Some("1".to_owned())
    );
    assert_eq!(Environment::new([("A", "1")], true).var("B"), None);
}
