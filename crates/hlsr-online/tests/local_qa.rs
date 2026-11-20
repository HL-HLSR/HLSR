//! QA-подмена API профиля (`online-profile/local-qa.js`) — случаи из кода 0.4.7: подмена только во
//! временной папке `hlsr-profile-qa-*` и только на `http://127.0.0.1`. Пути — POSIX.
#![cfg(all(unix, any(debug_assertions, feature = "dev-hooks")))]

use hlsr_online::online_profile::local_qa::{
    QaEnv, QaError, QaPrepareError, account, friends, prepare, settings,
};
use serde_json::json;
use std::path::PathBuf;

fn env(root: Option<&str>, api: Option<&str>) -> QaEnv {
    QaEnv {
        root: root.map(str::to_owned),
        api: api.map(str::to_owned),
        tmpdir: "/tmp/t".to_owned(),
        cwd: "/tmp/t".to_owned(),
    }
}

#[test]
fn qa_is_off_without_a_root() {
    assert_eq!(settings(&env(None, None)), Ok(None));
    assert_eq!(
        settings(&env(Some(""), Some("http://127.0.0.1:1"))),
        Ok(None)
    );
}

#[test]
fn root_must_be_a_qa_folder_directly_inside_tmpdir() {
    for root in [
        "/tmp/t/other",
        "/tmp/t/hlsr-profile-qa",
        "/tmp/other/hlsr-profile-qa-1",
        "/tmp/t/sub/hlsr-profile-qa-1",
        "/hlsr-profile-qa-1",
        "relative-hlsr-profile-qa-1",
    ] {
        assert_eq!(
            settings(&env(Some(root), None)),
            Err(QaError::NotIsolated),
            "{root}"
        );
    }
    let accepted = settings(&env(Some("/tmp/t/hlsr-profile-qa-1/"), None))
        .unwrap()
        .unwrap();
    assert_eq!(accepted.root, PathBuf::from("/tmp/t/hlsr-profile-qa-1"));
    assert_eq!(
        accepted.account_file,
        PathBuf::from("/tmp/t/hlsr-profile-qa-1/fake-steam.json")
    );
    // Относительный путь разрешается от текущей папки.
    let relative = settings(&env(Some("hlsr-profile-qa-2"), None))
        .unwrap()
        .unwrap();
    assert_eq!(relative.root, PathBuf::from("/tmp/t/hlsr-profile-qa-2"));
}

#[test]
fn api_must_be_a_plain_http_loopback_address() {
    let root = Some("/tmp/t/hlsr-profile-qa-1");
    assert_eq!(
        settings(&env(root, None)).unwrap().unwrap().api_base,
        "http://127.0.0.1:18895"
    );
    for (api, base) in [
        ("http://127.0.0.1:9000", "http://127.0.0.1:9000"),
        ("http://127.0.0.1:9000/", "http://127.0.0.1:9000"),
        ("http://127.0.0.1", "http://127.0.0.1"),
        ("http://127.0.0.1:80", "http://127.0.0.1"),
        ("http://127.0.0.1:9000?x=1", "http://127.0.0.1:9000"),
    ] {
        assert_eq!(
            settings(&env(root, Some(api))).unwrap().unwrap().api_base,
            base,
            "{api}"
        );
    }
    for api in [
        "https://127.0.0.1:9000",
        "http://localhost:9000",
        "http://127.0.0.2:9000",
        "http://[::1]:9000",
        "http://example.com",
        "http://user@127.0.0.1:9000",
        "http://user:pw@127.0.0.1:9000",
        "http://127.0.0.1:9000/api",
    ] {
        assert_eq!(
            settings(&env(root, Some(api))),
            Err(QaError::NotLoopback),
            "{api}"
        );
    }
    assert_eq!(
        settings(&env(root, Some("not a url"))),
        Err(QaError::InvalidUrl)
    );
}

#[test]
fn prepare_creates_the_folder_and_reads_the_fake_files() {
    let base = tempfile::tempdir().unwrap();
    let tmpdir = std::fs::canonicalize(base.path()).unwrap();
    let root = tmpdir.join("hlsr-profile-qa-test");
    let env = QaEnv {
        root: Some(root.to_string_lossy().into_owned()),
        api: None,
        tmpdir: tmpdir.to_string_lossy().into_owned(),
        cwd: tmpdir.to_string_lossy().into_owned(),
    };
    let qa = prepare(&env).unwrap().unwrap();
    assert!(root.is_dir());

    assert_eq!(account(&qa), json!({}));
    assert_eq!(friends(&qa), json!([]));
    std::fs::write(&qa.account_file, "{\"id\":\"76561198000000000\"}").unwrap();
    std::fs::write(root.join("fake-friends.json"), "[{\"id\":1}]").unwrap();
    assert_eq!(account(&qa), json!({"id": "76561198000000000"}));
    assert_eq!(friends(&qa), json!([{"id": 1}]));
    // С меткой порядка байтов JSON.parse не читает.
    std::fs::write(&qa.account_file, "\u{FEFF}{\"id\":1}").unwrap();
    std::fs::write(root.join("fake-friends.json"), "{broken").unwrap();
    assert_eq!(account(&qa), json!({}));
    assert_eq!(friends(&qa), json!([]));

    let refused = QaEnv {
        root: Some("/elsewhere/hlsr-profile-qa-x".to_owned()),
        ..env
    };
    assert!(matches!(
        prepare(&refused),
        Err(QaPrepareError::Qa(QaError::NotIsolated))
    ));
}
