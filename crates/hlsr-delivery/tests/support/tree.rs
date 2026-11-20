//! Снимок дерева файлов для сценариев сверки — тот же формат, что у
//! `tools/reference/lib/tree.js`:
//! `{path, kind: "file", size, sha256}`, `{path, kind: "dir"}`,
//! `{path, kind: "symlink", target}`; пути от корня через `/`, порядок — по UTF-16.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// Подмена содержимого файла до хеша: `(относительный путь, байты) → байты`.
pub type Transform<'a> = &'a dyn Fn(&str, Vec<u8>) -> Vec<u8>;

/// Снимок дерева `root`.
pub fn snapshot(root: &Path, transform: Option<Transform<'_>>) -> Value {
    let mut entries = Vec::new();
    walk(root, "", transform, &mut entries);
    entries.sort_by(|a, b| {
        hlsr_core::js::cmp_utf16(a["path"].as_str().unwrap(), b["path"].as_str().unwrap())
    });
    Value::Array(entries)
}

fn walk(dir: &Path, prefix: &str, transform: Option<Transform<'_>>, out: &mut Vec<Value>) {
    let mut items: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} не читается: {e}", dir.display()))
        .map(|entry| entry.unwrap())
        .collect();
    items.sort_by_key(|entry| entry.file_name());
    for item in items {
        let name = item.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let full = item.path();
        let kind = item.file_type().unwrap();
        if kind.is_symlink() {
            let target = fs::read_link(&full).unwrap();
            out.push(json!({"path": rel, "kind": "symlink", "target": target.to_string_lossy()}));
        } else if kind.is_dir() {
            out.push(json!({"path": rel, "kind": "dir"}));
            walk(&full, &rel, transform, out);
        } else {
            let mut bytes = fs::read(&full).unwrap();
            if let Some(transform) = transform {
                bytes = transform(&rel, bytes);
            }
            let sha256: String = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            out.push(json!({"path": rel, "kind": "file", "size": bytes.len(), "sha256": sha256}));
        }
    }
}

/// Подмена для [`snapshot`]: в файлах UTF-8 все вхождения `from` (и его записи
/// внутри строки JSON) → `to`. Прочие файлы не меняются.
pub fn replacing(from: &str, to: &str) -> impl Fn(&str, Vec<u8>) -> Vec<u8> + use<> {
    let escaped = serde_json::to_string(from).unwrap();
    let variants = [from.to_owned(), escaped[1..escaped.len() - 1].to_owned()];
    let to = to.to_owned();
    move |_rel, bytes| match String::from_utf8(bytes) {
        Ok(mut text) => {
            for variant in &variants {
                text = text.replace(variant.as_str(), &to);
            }
            text.into_bytes()
        }
        Err(error) => error.into_bytes(),
    }
}
