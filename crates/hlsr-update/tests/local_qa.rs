//! Локальный канал QA-сборки: самотест 0.4.7 «Linux: локальный канал и ключ не включаются
//! переменными production-сборки» (`launcher-update-selftest.js`) и сквозная проверка шлюза с
//! подписанным каналом на loopback и ключом из `HLSR_UPDATE_CHANNEL_KEY`. В выпуске без `dev-hooks`
//! модуля нет вовсе — переменные окружения не могут включить ничего.

#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_core::js::Platform;
use hlsr_core::manifest_signing::{attach, key_pair_from_seed};
use hlsr_update::http::{QaHttpGet, is_qa_loopback_url};
use hlsr_update::local_qa::{
    self, LocalQa, configuration, configuration_for, feed_file_url, latest_yml_name,
};
use hlsr_update::ports::{GetError, GetOptions, HttpGet, MemoryLogger};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use support::http::{self, RequestLog};

fn load_for(
    platform: Platform,
    build: Option<&str>,
    feed: Option<&str>,
    key: Option<&str>,
) -> Option<LocalQa> {
    let env = |name: &str| match name {
        "HLSR_BUILD_CHANNEL" => build.map(str::to_owned),
        "HLSR_UPDATE_FEED" => feed.map(str::to_owned),
        "HLSR_UPDATE_CHANNEL_KEY" => key.map(str::to_owned),
        _ => None,
    };
    configuration_for(platform, "0.4.0", &env, Arc::new(MemoryLogger::default()))
}

/// Канал Linux, как в 0.4.7 (`local-qa.js` жил только в Linux-ветке).
fn load(build: Option<&str>, feed: Option<&str>, key: Option<&str>) -> Option<LocalQa> {
    load_for(Platform::Linux, build, feed, key)
}

#[test]
fn the_local_channel_is_enabled_only_by_a_qa_build_with_a_loopback_feed() {
    // Самотест 0.4.7, дословно.
    assert!(
        load(
            Some("production"),
            Some("http://127.0.0.1:18899"),
            Some("untrusted")
        )
        .is_none()
    );
    assert!(
        load(
            Some("qa"),
            Some("http://127.0.0.1.evil.test:18899"),
            Some("untrusted")
        )
        .is_none()
    );
    assert!(
        load(
            Some("qa"),
            Some("http://user@localhost:18899"),
            Some("untrusted")
        )
        .is_none()
    );
    assert!(load(Some("qa"), Some("https://hlsr.pro"), Some("untrusted")).is_none());
    assert!(
        load(
            Some("qa"),
            Some("http://localhost:18899?token=x"),
            Some("untrusted")
        )
        .is_none()
    );
    assert!(
        load(
            Some("qa"),
            Some("http://127.0.0.1:18899"),
            Some("untrusted")
        )
        .is_some()
    );

    // Дополнительно: сборка без признака, без ленты, чужие схемы и хосты.
    assert!(load(None, Some("http://127.0.0.1:18899"), None).is_none());
    assert!(load(Some("QA"), Some("http://127.0.0.1:18899"), None).is_none());
    assert!(load(Some("qa "), Some("http://127.0.0.1:18899"), None).is_none());
    assert!(load(Some("qa"), None, None).is_none());
    assert!(load(Some("qa"), Some(""), None).is_none());
    for feed in [
        "garbage",
        "ftp://127.0.0.1:1",
        "http://[::1]:1/",
        "http://127.0.0.2:1/",
        "http://localhost.evil.test:1/",
        "http://user:pass@127.0.0.1:1/",
        "http://:pass@127.0.0.1:1/",
        "http://127.0.0.1:1/#frag",
        "http://127.0.0.1:1/?a=1",
        "https://127.0.0.1:1/",
    ] {
        assert!(load(Some("qa"), Some(feed), None).is_none(), "{feed}");
    }
}

#[test]
fn the_feed_loses_trailing_slashes_and_keeps_its_path() {
    for (given, feed) in [
        ("http://127.0.0.1:18899", "http://127.0.0.1:18899"),
        ("http://localhost:18899/", "http://localhost:18899"),
        ("http://localhost:18899///", "http://localhost:18899"),
        (
            "http://127.0.0.1:18899/feed/",
            "http://127.0.0.1:18899/feed",
        ),
        ("http://LOCALHOST:18899/", "http://localhost:18899"),
    ] {
        let qa = load(Some("qa"), Some(given), None).unwrap_or_else(|| panic!("{given}"));
        assert_eq!(qa.feed, feed, "{given}");
    }
    assert_eq!(local_qa::BUILD_CHANNEL_VAR, "HLSR_BUILD_CHANNEL");
    assert_eq!(local_qa::FEED_VAR, "HLSR_UPDATE_FEED");
    assert_eq!(local_qa::CHANNEL_KEY_VAR, "HLSR_UPDATE_CHANNEL_KEY");
}

/// Подписанные адреса остаются боевыми, байты берутся на ленте по имени файла.
#[test]
fn signed_urls_are_mapped_to_files_of_the_feed() {
    let feed = "http://127.0.0.1:18899/feed";
    assert_eq!(
        feed_file_url(
            feed,
            "https://hlsr.pro/downloads/launcher/linux/stable/HLSR-Setup-0.5.0-linux-x86_64.AppImage"
        )
        .as_deref(),
        Some("http://127.0.0.1:18899/feed/HLSR-Setup-0.5.0-linux-x86_64.AppImage")
    );
    // `encodeURIComponent` кодирует и «%», и пробелы, но не `!'()*`.
    assert_eq!(
        feed_file_url(feed, "https://hlsr.pro/a/b%20c (1)!.bin").as_deref(),
        Some("http://127.0.0.1:18899/feed/b%2520c%2520(1)!.bin")
    );
    assert_eq!(
        feed_file_url(feed, "https://hlsr.pro/a/b/").as_deref(),
        Some("http://127.0.0.1:18899/feed/b")
    );
    assert_eq!(feed_file_url(feed, "not a url"), None);
}

#[test]
fn qa_loopback_urls_match_the_original_expression() {
    for url in [
        "http://127.0.0.1/x",
        "http://127.0.0.1:18899/x",
        "http://localhost/",
        "http://localhost:1/x?y=1",
    ] {
        assert!(is_qa_loopback_url(url), "{url}");
    }
    for url in [
        "http://127.0.0.1",
        "http://127.0.0.1:/x",
        "http://127.0.0.1:abc/x",
        "http://127.0.0.1.evil.test/x",
        "http://localhost.evil.test/x",
        "http://user@localhost/x",
        "https://127.0.0.1/x",
        "HTTP://127.0.0.1/x",
        "http://127.0.0.2/x",
        "http://[::1]/x",
        " http://127.0.0.1/x",
    ] {
        assert!(!is_qa_loopback_url(url), "{url}");
    }
}

fn linux_channel(keys: &hlsr_core::manifest_signing::KeyPair, yml: &str) -> serde_json::Value {
    channel_of(
        keys,
        yml,
        "HLSR-Setup-0.5.0-linux-x86_64.AppImage",
        "https://hlsr.pro/downloads/launcher/linux/stable/HLSR-Setup-0.5.0-linux-x86_64.AppImage",
    )
}

fn windows_channel(keys: &hlsr_core::manifest_signing::KeyPair, yml: &str) -> serde_json::Value {
    channel_of(
        keys,
        yml,
        "HLSR-Setup-0.5.0.exe",
        "https://hlsr.pro/downloads/launcher/stable/HLSR-Setup-0.5.0.exe",
    )
}

fn channel_of(
    keys: &hlsr_core::manifest_signing::KeyPair,
    yml: &str,
    filename: &str,
    url: &str,
) -> serde_json::Value {
    let doc = json!({
        "schemaVersion": 1,
        "version": "0.5.0",
        "publishedAt": "2026-10-16T09:00:00.000Z",
        "minimumSupportedVersion": "0.3.16",
        "updateRequired": false,
        "installer": {
            "filename": filename,
            "size": 1234,
            "sha256": "a".repeat(64),
            "sha512": format!("{}==", "A".repeat(86)),
            "urls": [url],
        },
        "latestYmlSha256": Sha256::digest(yml.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>(),
    });
    serde_json::Value::Object(attach(doc.as_object().unwrap(), &keys.private_key_pem).unwrap())
}

#[tokio::test]
async fn the_local_gate_trusts_the_key_from_the_environment_and_nothing_else() {
    let keys = key_pair_from_seed(&[7u8; 32]);
    let yml = "version: 0.5.0\nfiles: []\n";
    let log = RequestLog::default();
    let server = http::start_server("feed", log.clone()).await;
    {
        let mut routes = server.routes.lock().unwrap();
        routes.insert(
            "/launcher-channel.json".to_owned(),
            json!([{"body": linux_channel(&keys, yml).to_string()}]),
        );
        routes.insert("/latest-linux.yml".to_owned(), json!([{"body": yml}]));
    }
    let feed = format!("http://{}", server.authority);
    // Ключ в окружении записан одной строкой: переводы строк — буквами `\n`.
    let key_line = keys.public_key_pem.trim_end().replace('\n', "\\n");

    let qa = load(Some("qa"), Some(&feed), Some(&key_line)).unwrap();
    let outcome = qa.gate.evaluate(None).await.unwrap();
    assert!(outcome.ok, "{:?}", outcome.problems);
    assert!(outcome.update_available);
    assert_eq!(outcome.source_url, format!("{feed}/launcher-channel.json"));
    assert_eq!(
        log.lock()
            .unwrap()
            .iter()
            .map(|entry| entry["url"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        ["/launcher-channel.json", "/latest-linux.yml"]
    );

    // Без ключа в окружении остаётся боевой ключ: тестовая подпись не проходит.
    let qa = load(Some("qa"), Some(&feed), None).unwrap();
    let outcome = qa.gate.evaluate(None).await.unwrap();
    assert!(!outcome.ok);
    assert_eq!(outcome.problems, ["подпись Ed25519 не прошла проверку"]);
}

/// Канал Windows: `latest.yml` вместо `latest-linux.yml`, установщик `.exe`; ключ и подпись те же.
#[tokio::test]
async fn the_windows_local_gate_reads_latest_yml_and_a_windows_installer() {
    let keys = key_pair_from_seed(&[8u8; 32]);
    let yml = "version: 0.5.0\nfiles: []\n";
    let log = RequestLog::default();
    let server = http::start_server("feed", log.clone()).await;
    {
        let mut routes = server.routes.lock().unwrap();
        routes.insert(
            "/launcher-channel.json".to_owned(),
            json!([{"body": windows_channel(&keys, yml).to_string()}]),
        );
        routes.insert("/latest.yml".to_owned(), json!([{"body": yml}]));
    }
    let feed = format!("http://{}", server.authority);
    let key_line = keys.public_key_pem.trim_end().replace('\n', "\\n");

    let qa = load_for(Platform::Win32, Some("qa"), Some(&feed), Some(&key_line)).unwrap();
    assert_eq!(qa.platform, Platform::Win32);
    let outcome = qa.gate.evaluate(None).await.unwrap();
    assert!(outcome.ok, "{:?}", outcome.problems);
    assert!(outcome.update_available);
    assert_eq!(
        log.lock()
            .unwrap()
            .iter()
            .map(|entry| entry["url"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        ["/launcher-channel.json", "/latest.yml"]
    );

    // Канал другой ОС не принимается: установщик Windows для Linux-шлюза — не его файл.
    let linux = load_for(Platform::Linux, Some("qa"), Some(&feed), Some(&key_line)).unwrap();
    let outcome = linux.gate.evaluate(None).await.unwrap();
    assert!(!outcome.ok, "Linux-шлюз принял установщик Windows");
}

/// Имя `latest*.yml` берётся из боевых адресов ОС, а `configuration` — для ОС сборки.
#[test]
fn the_platform_picks_the_yml_name_and_the_default_is_the_build_os() {
    assert_eq!(latest_yml_name(Platform::Linux), "latest-linux.yml");
    assert_eq!(latest_yml_name(Platform::Win32), "latest.yml");
    let env = |name: &str| match name {
        "HLSR_BUILD_CHANNEL" => Some("qa".to_owned()),
        "HLSR_UPDATE_FEED" => Some("http://127.0.0.1:18899".to_owned()),
        _ => None,
    };
    let qa = configuration("0.4.0", &env, Arc::new(MemoryLogger::default())).unwrap();
    assert_eq!(qa.platform, Platform::CURRENT);
    for platform in [Platform::Linux, Platform::Win32] {
        let qa =
            configuration_for(platform, "0.4.0", &env, Arc::new(MemoryLogger::default())).unwrap();
        assert_eq!(qa.platform, platform);
    }
}

#[tokio::test]
async fn qa_get_reads_loopback_without_limits_and_delegates_the_rest() {
    let log = RequestLog::default();
    let server = http::start_server("feed", log.clone()).await;
    server
        .routes
        .lock()
        .unwrap()
        .insert("/big".to_owned(), json!([{"bodySize": 300 * 1024}]));
    server
        .routes
        .lock()
        .unwrap()
        .insert("/gone".to_owned(), json!([{"status": 404, "body": ""}]));
    let client = QaHttpGet::new();
    let options = GetOptions::default();

    // Лимит 256 КиБ боевого клиента к loopback не применяется.
    let body = client
        .get(&format!("http://{}/big", server.authority), &options)
        .await
        .unwrap();
    assert_eq!(body.len(), 300 * 1024);
    assert_eq!(
        client
            .get(&format!("http://{}/gone", server.authority), &options)
            .await,
        Err(GetError::Status(404))
    );
    // Остальное идёт боевым путём и боевым allowlist.
    assert_eq!(
        client.get("https://example.com/x", &options).await,
        Err(GetError::NotAllowed("https://example.com/x".to_owned()))
    );
    assert_eq!(
        client.get("http://hlsr.pro/x", &options).await,
        Err(GetError::NotAllowed("http://hlsr.pro/x".to_owned()))
    );
    // Заголовка user-agent у запроса к ленте нет: `http.get` без заголовков.
    assert!(
        log.lock().unwrap()[0]["headers"]
            .get("user-agent")
            .is_none()
    );
}
