//! Файлы сценариев сверки в виде данных: `{ "text" }`, `{ "hex", "padTo" }`, `{ "png": n }`,
//! `{ "jpeg": n }`, `{ "dir": true }` (так их пишут `tools/reference/platform/steam-friends.js` и
//! `steam-avatars.js`), и сжатый вид строки для сравнения (длина и sha256).

#![allow(dead_code)]

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("шестнадцатеричные цифры"))
        .collect()
}

/// Байты файла по описанию.
pub fn bytes_of(spec: &Value) -> Vec<u8> {
    if spec.is_null() {
        return Vec::new();
    }
    if let Some(text) = spec["text"].as_str() {
        let mut bytes = text.as_bytes().to_vec();
        if let Some(total) = spec["padSpacesTo"].as_u64() {
            bytes.resize(total as usize, b' ');
        }
        return bytes;
    }
    if let Some(hex) = spec["hex"].as_str() {
        let mut bytes = from_hex(hex);
        if let Some(total) = spec["padTo"].as_u64().filter(|t| *t as usize > bytes.len()) {
            bytes.resize(total as usize, 0);
        }
        return bytes;
    }
    if let Some(total) = spec["png"].as_u64() {
        return bytes_of(&json!({"hex": "89504e470d0a1a0a", "padTo": total}));
    }
    if let Some(total) = spec["jpeg"].as_u64() {
        return bytes_of(&json!({"hex": "ffd8ffe0", "padTo": total}));
    }
    panic!("неизвестное описание файла: {spec}");
}

/// Положить файлы `{относительный путь через «/»: описание}` под `root`.
pub fn place(root: &Path, files: &Value) {
    for (relative, spec) in files.as_object().expect("файлы — объект") {
        let full = relative
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part));
        if spec["dir"] == json!(true) {
            std::fs::create_dir_all(&full).unwrap();
            continue;
        }
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, bytes_of(spec)).unwrap();
    }
}

/// `{length, sha256}` строки (длина в единицах UTF-16, как `string.length` в JS; для строк
/// `data:` это то же, что число знаков) или `null`.
pub fn digest(text: Option<&str>) -> Value {
    match text {
        None => Value::Null,
        Some(text) => json!({
            "length": text.encode_utf16().count(),
            "sha256": sha256_hex(text.as_bytes()),
        }),
    }
}

/// sha256 байтов.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
