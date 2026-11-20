//! Самотесты 0.4.7, которые касаются подписи и адресов: `diagnostics-selftest.js`
//! (подписанные каналы лаунчера и игры, подмена версии), `launcher-update-selftest.js`
//! и `launcher-chunks-selftest.js` (`generateKeyPair` → `sign` → `verify`),
//! `hl1-selftest.js` и `practice-saves-selftest.js` (хосты загрузки из манифестов).
//! Проверки, которым нужны `ChannelService`, `DiagnosticsRunner` и `ManifestService`,
//! переносят задачи с этими модулями.

use hlsr_core::js::Platform;
use hlsr_delivery::delivery_endpoints as endpoints;
use hlsr_delivery::errors::Code;
use hlsr_delivery::manifest_signing::{
    CheckOptions, Reason, attach, check, generate_key_pair, sign, verify,
};
use serde_json::{Value, json};
use std::path::Path;

/// `signedChannels(platform)` из diagnostics-selftest: лаунчер и игра, подписанные свежим ключом.
fn signed_channels(platform: Platform) -> (Value, Value, String) {
    let keys = generate_key_pair();
    let version = "0.3.17";
    let hash = "a".repeat(64);
    let filename = match platform {
        Platform::Linux => format!("HLSR-Setup-{version}-linux-x86_64.AppImage"),
        Platform::Win32 => format!("HLSR-Setup-{version}.exe"),
    };
    let mut launcher = json!({
        "schemaVersion": 1, "version": version,
        "publishedAt": "2026-10-03T00:00:00Z", "minimumSupportedVersion": "0.3.0", "updateRequired": false,
        "installer": {
            "filename": filename, "size": 1024, "sha256": hash,
            "sha512": "AAAA", "urls": ["https://hlsr.pro/downloads/test"],
        },
    });
    launcher["signature"] = sign(&launcher, &keys.private_key_pem).unwrap();
    let provider = json!({"id": "primary", "type": "https-direct", "status": "permanent", "url": "https://hlsr.pro/downloads/test"});
    let mut game = json!({
        "schemaVersion": 1, "gameId": "ghosting-renewed", "channel": "stable", "version": "1.0.0",
        "publishedAt": "2026-10-03T00:00:00Z", "minimumLauncherVersion": "0.3.0", "enabled": true, "updateRequired": false,
        "fullArchive": {"filename": "test.zip", "size": 1024, "sha256": hash, "providers": [provider]},
        "filesManifest": {"filename": "files.json", "version": "1.0.0", "size": 512, "sha256": hash, "providers": [provider]},
    });
    game["signature"] = sign(&game, &keys.private_key_pem).unwrap();
    (launcher, game, keys.public_key_pem)
}

fn required(keys: &[String]) -> CheckOptions<'_> {
    CheckOptions {
        required: true,
        public_keys: keys,
    }
}

/// diagnostics: «verify the correct signed launcher and game channels»,
/// подмена версии → подпись не проходит.
#[test]
fn correct_signed_launcher_and_game_channels_verify_and_tampering_does_not() {
    for platform in [Platform::Linux, Platform::Win32] {
        let (launcher, game, public_key) = signed_channels(platform);
        let keys = [public_key.clone()];

        assert!(verify(&launcher, &launcher["signature"], &public_key));
        let result = check(&game, &required(&keys)).unwrap();
        assert!(result.signed && result.valid && result.reason.is_none());

        let mut tampered = game.clone();
        tampered["version"] = json!("1.0.1");
        let error = check(&tampered, &required(&keys)).unwrap_err();
        assert_eq!(error.code, Code::ManifestInvalid);
        assert_eq!(error.message, "Подпись манифеста не прошла проверку");
        let result = check(
            &tampered,
            &CheckOptions {
                required: false,
                public_keys: &keys,
            },
        )
        .unwrap();
        assert_eq!(result.reason, Some(Reason::BadSignature));
    }
}

/// diagnostics: «empty, truncated or redirected metadata cannot pass as a healthy channel» —
/// пустой документ и подпись `null` не принимаются.
#[test]
fn empty_or_unsigned_metadata_cannot_pass() {
    let (launcher, _, public_key) = signed_channels(Platform::Linux);
    let keys = [public_key.clone()];

    let error = check(&json!({}), &required(&keys)).unwrap_err();
    assert_eq!(error.code, Code::ManifestInvalid);
    assert_eq!(
        error.message,
        "Манифест без подписи отклонён: включена обязательная проверка подписи"
    );

    let mut unsigned = launcher;
    unsigned["signature"] = Value::Null;
    assert!(!verify(&unsigned, &unsigned["signature"], &public_key));
    let result = check(
        &unsigned,
        &CheckOptions {
            required: false,
            public_keys: &keys,
        },
    )
    .unwrap();
    assert!(!result.signed && !result.valid);
    assert_eq!(result.reason, Some(Reason::Unsigned));
}

/// diagnostics: ссылки на канал лаунчера для каждой ОС (тест проверяет, что запрошены именно они).
#[test]
fn launcher_channel_urls_for_each_platform() {
    assert_eq!(
        endpoints::launcher_for(Platform::Linux).channel_urls,
        [
            "https://hlsr.pro/downloads/launcher/linux/stable/launcher-channel.json",
            "https://github.com/HL-HLSR/HLSR-Releases/releases/download/linux-stable/launcher-channel.json",
        ]
    );
    assert_eq!(
        endpoints::launcher_for(Platform::Win32).channel_urls,
        [
            "https://hlsr.pro/downloads/launcher/stable/launcher-channel.json",
            "https://github.com/HL-HLSR/HLSR-Releases/releases/latest/download/launcher-channel.json",
        ]
    );
}

/// launcher-update и launcher-chunks: ключи из `generateKeyPair` подписывают, а
/// `verify` принимает подпись любого из доверенных ключей, как `launcher-channel.check`.
#[test]
fn generated_keys_sign_and_verify_like_the_launcher_release_scripts() {
    let first = generate_key_pair();
    let second = generate_key_pair();
    assert_ne!(first.public_key_pem, second.public_key_pem);
    assert!(
        first
            .public_key_pem
            .starts_with("-----BEGIN PUBLIC KEY-----\n")
    );
    assert!(first.public_key_pem.ends_with("-----END PUBLIC KEY-----\n"));
    assert!(
        first
            .private_key_pem
            .starts_with("-----BEGIN PRIVATE KEY-----\n")
    );

    let manifest = json!({"schemaVersion": 1, "version": "0.4.4", "installer": {"filename": "HLSR-0.4.4-linux.AppImage", "size": 1}});
    let attach_value = |manifest: &Value, pem: &str| {
        Value::Object(attach(manifest.as_object().unwrap(), pem).unwrap())
    };
    let signed = attach_value(&manifest, &first.private_key_pem);
    assert_eq!(signed["signature"]["algorithm"], "ed25519");

    let trusted = [second.public_key_pem.clone(), first.public_key_pem.clone()];
    let ok = trusted
        .iter()
        .any(|pem| verify(&signed, &signed["signature"], pem));
    assert!(ok, "подпись должна проходить по одному из ключей");
    assert!(!verify(
        &signed,
        &signed["signature"],
        &second.public_key_pem
    ));

    // Повторная подпись заменяет прежнюю и не меняет остальное.
    let resigned = attach_value(&signed, &second.private_key_pem);
    assert_eq!(resigned["version"], "0.4.4");
    assert!(verify(
        &resigned,
        &resigned["signature"],
        &second.public_key_pem
    ));
    assert!(!verify(
        &resigned,
        &resigned["signature"],
        &first.public_key_pem
    ));
}

fn config_manifests() -> Vec<(String, Value)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".manifest.json"))
            {
                let text = std::fs::read_to_string(&path).unwrap();
                out.push((
                    path.display().to_string(),
                    hlsr_core::json_text::parse_js_json(&text).unwrap(),
                ));
            }
        }
    }
    out
}

fn providers_of(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                if key == "providers" {
                    for provider in item.as_array().unwrap() {
                        out.push(provider["url"].as_str().unwrap().to_owned());
                    }
                } else {
                    providers_of(item, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| providers_of(item, out)),
        _ => {}
    }
}

/// hl1 («качается только с закреплённого хоста») и practice-saves (`isAllowedDownloadUrl`
/// для источников архива): все ссылки встроенных манифестов разрешены.
#[test]
fn embedded_manifests_download_only_from_allowed_hosts() {
    let manifests = config_manifests();
    assert!(
        manifests.len() >= 7,
        "найдено {} манифестов",
        manifests.len()
    );
    for (name, manifest) in manifests {
        // FFmpeg «Демо» (0.4.9) качается с GitHub GyanD и gyan.dev: ToolService хосты не
        // сверяет, архив и файлы закреплены sha256. Хосты закреплены здесь, как в
        // demo-tool-ffmpeg-selftest.js 0.4.9.
        if name.ends_with("ffmpeg.manifest.json") {
            let mut urls = Vec::new();
            providers_of(&manifest, &mut urls);
            let hosts: Vec<&str> = urls
                .iter()
                .map(|url| url.split('/').nth(2).unwrap())
                .collect();
            assert_eq!(hosts, ["github.com", "www.gyan.dev"], "{name}");
            assert!(urls.iter().all(|url| url.starts_with("https://")), "{name}");
            continue;
        }
        let mut urls = Vec::new();
        providers_of(&manifest, &mut urls);
        assert!(!urls.is_empty(), "{name}: нет источников");
        for url in urls {
            assert!(endpoints::is_allowed_download_url(&url), "{name}: {url}");
            let host = url.split('/').nth(2).unwrap();
            assert!(
                endpoints::ALLOWED_DOWNLOAD_HOSTS.contains(&host),
                "{name}: {url}"
            );
        }
    }
}
