//! Самотест 0.4.7 для патчей, перенесённый в Rust: `scripts/user-configs-selftest.js`, тест «патч
//! не трогает изменённые игроком cfg, а стоковые обновляет» — часть `PatchService.withoutUserConfigs`
//! (часть `keptByPatch` — в `foundation_selftests.rs`, проверка файлов — у `IntegrityService`).

use hlsr_delivery::patch_service::{PatchOptions, PatchService};
use hlsr_delivery::paths::resolve_library;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

fn sha(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn entry(relative: &str, text: &str) -> Value {
    json!({"path": relative, "size": text.len(), "sha256": sha(text)})
}

fn paths_of(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().or_else(|| item["path"].as_str()).unwrap())
        .collect()
}

#[test]
fn patch_keeps_player_edited_configs_and_updates_stock_ones() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for (relative, text) in [
        ("mod/stock.cfg", "старый\n"),
        ("mod/edited.cfg", "мой бинд\n"),
        ("mod/own.cfg", "мой файл\n"),
        ("mod/gone.cfg", "удалить\n"),
        ("mod/gone-edited.cfg", "удалить, но я менял\n"),
        ("mod/same.cfg", "новый\n"),
        ("mod/client.dll", "чужой бинарь"),
    ] {
        let full = root.join(relative);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let previous = json!({"files": [
        entry("mod/stock.cfg", "старый\n"),
        entry("mod/edited.cfg", "стоковый\n"),
        entry("mod/gone.cfg", "удалить\n"),
        entry("mod/gone-edited.cfg", "удалить\n"),
        entry("mod/same.cfg", "старый\n"),
        entry("mod/client.dll", "бинарь"),
    ]});
    let patch = json!({
        "fromVersion": "1.0.0",
        "files": [
            entry("mod/stock.cfg", "новый\n"),
            entry("mod/edited.cfg", "новый\n"),
            entry("mod/own.cfg", "из сборки\n"),
            entry("mod/same.cfg", "новый\n"),
            entry("mod/missing.cfg", "новый\n"),
            entry("mod/client.dll", "новый бинарь"),
        ],
        "deletedFiles": ["mod/gone.cfg", "mod/gone-edited.cfg"],
    });

    let library = tempfile::tempdir().unwrap();
    let calls: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let service = PatchService::new(
        resolve_library(&library.path().to_string_lossy()).unwrap(),
        PatchOptions {
            files_manifest_of: Some({
                let (calls, previous) = (calls.clone(), previous.clone());
                Arc::new(move |game_id: &str, version: &str| {
                    calls
                        .lock()
                        .unwrap()
                        .push((game_id.to_owned(), version.to_owned()));
                    Some(previous.clone())
                })
            }),
            ..PatchOptions::default()
        },
    );
    let applied = service.without_user_configs("hl1-won", root, &patch);
    assert_eq!(
        *calls.lock().unwrap(),
        [("hl1-won".to_owned(), "1.0.0".to_owned())]
    );
    assert_eq!(
        paths_of(&applied["files"]),
        [
            "mod/stock.cfg",
            "mod/same.cfg",
            "mod/missing.cfg",
            "mod/client.dll"
        ]
    );
    assert_eq!(paths_of(&applied["deletedFiles"]), ["mod/gone.cfg"]);
    assert_eq!(
        patch["files"].as_array().unwrap().len(),
        6,
        "исходный манифест патча не меняется"
    );
}
