//! Linux-установка целиком против 0.4.7: фикстура `linux-install.json`
//! (`tools/reference/update/linux-install.js`, снимается только на Linux — ссылки, права, `/proc`).
//! Полный цикл во временном корне: `prepare` → `commit` → следующая версия → `previous`/`current` →
//! удаление позапрошлых версий → `rollback`; отказы `commit`, некорректный `current`, корень-ссылка,
//! `managedRoot` по `realpath`, живая блокировка. Сверяются итоги шагов и дерево корня (вид, режим, размер,
//! sha256, цели ссылок).
//!
//! Самотест `launcher-update-selftest.js` «initial installation, next version, stable path and
//! rollback» (сценарий фикстуры).

#![cfg(target_os = "linux")]

mod support;

use hlsr_update::linux::archive_install::{self, ArchiveInstall};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use support::{Mismatches, fixture, items, str_of};

fn base64(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .unwrap()
}

fn staging(text: &str) -> String {
    match text.find(".staging-") {
        Some(index) if text.len() >= index + 15 => {
            format!("{}.staging-{{rand}}{}", &text[..index], &text[index + 15..])
        }
        _ => text.to_owned(),
    }
}

fn tree(root: &Path) -> Value {
    let mut out = Vec::new();
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<Value>) {
        let mut names = hlsr_core::js::fs::read_dir_names(dir).unwrap();
        names.sort();
        for name in names {
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let full = dir.join(&name);
            let meta = std::fs::symlink_metadata(&full).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            if meta.file_type().is_symlink() {
                out.push(json!({"path": rel, "kind": "symlink", "target": std::fs::read_link(&full).unwrap().to_string_lossy()}));
            } else if meta.is_dir() {
                out.push(json!({"path": rel, "kind": "dir", "mode": mode}));
                walk(&full, &rel, out);
            } else if rel == ".update-lock" {
                out.push(json!({"path": rel, "kind": "lock", "mode": mode}));
            } else {
                let bytes = std::fs::read(&full).unwrap();
                let sha: String = Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                out.push(json!({"path": rel, "kind": "file", "mode": mode, "size": bytes.len(), "sha256": sha}));
            }
        }
    }
    if root.exists() {
        walk(root, "", &mut out);
    }
    for entry in &mut out {
        let path = staging(entry["path"].as_str().unwrap());
        entry["path"] = json!(path);
    }
    Value::Array(out)
}

// Фикстуру снимает только Linux-машина (`node tools/reference/update/linux-install.js`); до её появления
// тест отключён, чтобы `cargo test --workspace` в CI не падал на отсутствующем файле. После съёма —
// `cargo test -p hlsr-update --test linux_install -- --include-ignored`, затем снять `ignore`.
#[ignore = "нет фикстуры linux-install.json: снимается на Linux (tools/reference/update/linux-install.js)"]
#[tokio::test(flavor = "current_thread")]
async fn full_cycle_matches_0_4_7() {
    let fixture = fixture("linux-install.json");
    let releases: HashMap<String, (Vec<u8>, Value)> = items(&fixture["releases"])
        .iter()
        .map(|item| {
            (
                str_of(&item["version"]).to_owned(),
                (base64(str_of(&item["gz"])), item["manifest"].clone()),
            )
        })
        .collect();
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture["scenarios"]) {
        let name = str_of(&scenario["name"]);
        let dir = tempfile::tempdir().unwrap();
        let work = std::fs::canonicalize(dir.path()).unwrap();
        let root = work.join(str_of(&scenario["root"]));
        if scenario["rootLink"].as_bool() == Some(true) {
            std::fs::create_dir(work.join("real")).unwrap();
            std::os::unix::fs::symlink(work.join("real"), &root).unwrap();
        }
        let mut archives = HashMap::new();
        for (version, (bytes, manifest)) in &releases {
            let file = work.join(str_of(&manifest["installer"]["filename"]));
            std::fs::write(&file, bytes).unwrap();
            archives.insert(version.clone(), file);
        }
        let mask = |text: &str| {
            text.replace(&root.to_string_lossy().into_owned(), "{root}")
                .replace(&work.to_string_lossy().into_owned(), "{work}")
        };
        let mut instances: HashMap<String, ArchiveInstall> = HashMap::new();
        let mut steps = Vec::new();
        for step in items(&scenario["steps"]) {
            let key = step["instance"].as_str().unwrap_or("a").to_owned();
            let install = instances
                .entry(key)
                .or_insert_with(|| ArchiveInstall::new(&root));
            let path = |field: &str| root.join(str_of(&step[field]));
            let result: Result<Value, String> = match str_of(&step["op"]) {
                "prepare" => {
                    let version = str_of(&step["version"]);
                    install
                        .prepare(&archives[version], &releases[version].1, None)
                        .await
                        .map(|()| Value::Null)
                }
                "commit" => install
                    .commit()
                    .map(|target| json!(target.to_string_lossy())),
                "rollback" => install.rollback().map(|()| Value::Null),
                "discard" => {
                    install.discard();
                    Ok(Value::Null)
                }
                "readCurrent" => archive_install::read_current(&root).map(|current| json!(current)),
                "managedRoot" => Ok(json!(
                    archive_install::managed_root(&path("path"))
                        .map(|root| root.to_string_lossy().into_owned())
                )),
                "link" => {
                    let temp = root.join(format!(".{}-test", str_of(&step["name"])));
                    std::os::unix::fs::symlink(str_of(&step["target"]), &temp).unwrap();
                    std::fs::rename(&temp, path("name")).unwrap();
                    Ok(Value::Null)
                }
                "mkdir" => {
                    std::fs::create_dir_all(path("path")).unwrap();
                    Ok(Value::Null)
                }
                "write" => {
                    std::fs::write(path("path"), str_of(&step["text"])).unwrap();
                    Ok(Value::Null)
                }
                "lockIdentity" => {
                    let owner: Value = serde_json::from_str(
                        &std::fs::read_to_string(root.join(".update-lock")).unwrap(),
                    )
                    .unwrap();
                    let identity = owner["identity"].as_str().unwrap_or("");
                    let well_formed = identity.rsplit_once(':').is_some_and(|(boot, start)| {
                        boot.len() == 36
                            && boot.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
                            && !start.is_empty()
                            && start.bytes().all(|b| b.is_ascii_digit())
                    });
                    Ok(
                        json!({"pidIsSelf": owner["pid"] == json!(std::process::id()), "identity": well_formed}),
                    )
                }
                other => panic!("шаг {other}"),
            };
            let result = match result {
                Ok(value) => {
                    let text = serde_json::to_string(&value).unwrap();
                    json!({"ok": serde_json::from_str::<Value>(&mask(&text)).unwrap()})
                }
                Err(error) => {
                    let error = if error.contains(" at line ") && error.contains(" column ") {
                        "<json>".to_owned()
                    } else {
                        mask(&error)
                    };
                    json!({"error": error})
                }
            };
            let mut record = step.clone();
            let object = record.as_object_mut().unwrap();
            object.insert("result".into(), result);
            object.insert(
                "stage".into(),
                install.stage().map_or(Value::Null, |stage| {
                    json!(staging(
                        &stage.strip_prefix(&root).unwrap_or(stage).to_string_lossy()
                    ))
                }),
            );
            object.insert("tree".into(), tree(&root));
            steps.push(record);
        }
        for install in instances.values_mut() {
            install.discard();
        }
        mismatches.check(
            &Value::Array(steps),
            &scenario["steps"],
            format!("«{name}»"),
        );
    }
    mismatches.finish("ArchiveInstall (полный цикл)");
}
