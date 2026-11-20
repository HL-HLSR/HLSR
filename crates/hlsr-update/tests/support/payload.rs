//! Упаковка установщика для тестов (`pack_for_tests`): `HLSR.exe` дерева сборки, за ним нагрузка
//! (длина списка, `install-files.json`, файлы списка кроме `HLSR.exe` в порядке списка) и хвост.
//! Выпускной упаковщик — `tools/release` (задача 8).

use hlsr_update::windows::file_list;
use hlsr_update::windows::layout;
use hlsr_update::windows::payload::Trailer;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Собрать `HLSR-Setup.exe` из распакованной сборки `tree` (с `resources/install-files.json`).
pub fn pack_for_tests(tree: &Path, out: &Path) {
    let list_bytes = std::fs::read(tree.join(file_list::RELATIVE)).expect("install-files.json");
    let list: Value = serde_json::from_slice(&list_bytes).expect("список — JSON");
    let exe = std::fs::read(tree.join(layout::EXE)).expect("HLSR.exe в дереве");
    let mut payload = Vec::new();
    payload.extend_from_slice(&(list_bytes.len() as u64).to_le_bytes());
    payload.extend_from_slice(&list_bytes);
    for item in list["files"].as_array().expect("files") {
        let path = item["path"].as_str().expect("path");
        if path == layout::EXE {
            continue;
        }
        let mut full = tree.to_path_buf();
        for part in path.split('/') {
            full.push(part);
        }
        payload.extend_from_slice(&std::fs::read(full).expect("файл списка"));
    }
    let trailer = Trailer {
        offset: exe.len() as u64,
        length: payload.len() as u64,
        sha256: Sha256::digest(&payload).into(),
    };
    let mut bytes = exe;
    bytes.extend_from_slice(&payload);
    bytes.extend_from_slice(&trailer.to_bytes());
    std::fs::write(out, bytes).expect("запись установщика");
}
