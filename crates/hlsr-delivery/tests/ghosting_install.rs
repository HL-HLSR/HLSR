//! Персонализация Ghosting на диске против 0.4.7: фикстура `ghosting-install.json` из
//! `tools/reference/delivery/ghosting-install.js`. Сценарий — данные (файлы и шаги), его выполняют
//! `ghosting-personalization.js` в Node 24 и этот крейт на одинаковых деревьях во временных
//! папках; сравниваются ответ каждого шага и дерево файлов (путь, размер, sha256, текст UTF-8).
//! Платформа игры всегда Linux — на ней снята фикстура (от платформы зависит только текст бинда
//! удаления сейва; оба варианта проверены в `ghosting_settings.rs`).
//!
//! Сценарии, помеченные `unix`, зависят от прав доступа и на Windows пропускаются.

mod support;

use hlsr_core::js::Platform;
use hlsr_core::json_text::stringify;
use hlsr_delivery::errors::RawError;
use hlsr_delivery::ghosting_default_autoexec::default_autoexec;
use hlsr_delivery::ghosting_personalization as gp;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("ghosting-install.json"));

/// Текст файла попадает в снимок, если файл не больше.
const TEXT_LIMIT: u64 = 6000;

/// Корень сценария. В Linux — настоящий путь, как `realpathSync` в скрипте; в Windows
/// `canonicalize` дал бы путь с `\\?\`, к которому нельзя дописывать `/`.
fn real_root(dir: &Path) -> PathBuf {
    if cfg!(unix) {
        dir.canonicalize().unwrap()
    } else {
        dir.to_path_buf()
    }
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

fn path_in(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part))
}

fn put_file(root: &Path, item: &Value) {
    let file = path_in(root, str_of(&item["file"]));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let bytes = match opt_str(&item["hex"]) {
        Some(hex) => unhex(hex),
        None => opt_str(&item["utf8"]).unwrap_or("").as_bytes().to_vec(),
    };
    std::fs::write(&file, bytes).unwrap();
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(time))
        .unwrap();
}

fn setup(root: &Path, list: &Value) {
    for item in list.as_array().into_iter().flatten() {
        if let Some(dir) = opt_str(&item["dir"]) {
            std::fs::create_dir_all(path_in(root, dir)).unwrap();
        } else {
            put_file(root, item);
        }
    }
}

fn tree_of(root: &Path, with_text: bool) -> Value {
    let mut tree = support::tree::snapshot(root, None);
    if with_text {
        for entry in tree.as_array_mut().unwrap() {
            if entry["kind"] == "file" && entry["size"].as_u64().unwrap() <= TEXT_LIMIT {
                let bytes = std::fs::read(path_in(root, str_of(&entry["path"]))).unwrap();
                entry["utf8"] = json!(String::from_utf8_lossy(&bytes));
            }
        }
    }
    tree
}

fn error_json(error: &RawError) -> Value {
    json!({"error": {"code": error.code, "syscall": error.syscall}})
}

/// Значения `value` сравниваются по записи `JSON.stringify`: порядок ключей важен.
fn ordered(value: Value) -> Value {
    json!({"value": stringify(&value, "")})
}

fn do_step(root: &Path, step: &Value) -> Value {
    match str_of(&step["op"]) {
        "tree" => json!({"tree": tree_of(root, step["text"].as_bool() == Some(true))}),
        "write" => {
            put_file(root, step);
            Value::Null
        }
        "remove" => {
            let target = path_in(root, str_of(&step["file"]));
            match std::fs::symlink_metadata(&target) {
                Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&target).unwrap(),
                Ok(_) => std::fs::remove_file(&target).unwrap(),
                Err(_) => {}
            }
            Value::Null
        }
        "mkdir" => {
            std::fs::create_dir_all(path_in(root, str_of(&step["dir"]))).unwrap();
            Value::Null
        }
        "chmod" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let mode = step["mode"].as_u64().unwrap() as u32;
                std::fs::set_permissions(
                    path_in(root, str_of(&step["file"])),
                    std::fs::Permissions::from_mode(mode),
                )
                .unwrap();
            }
            Value::Null
        }
        "apply" => match gp::apply_to_install(root, &step["settings"], Platform::Linux) {
            Ok(applied) => {
                let relative = applied
                    .file
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                json!({"ok": {
                    "ok": true,
                    "file": relative,
                    "autoexec": {"created": applied.autoexec.created, "appended": applied.autoexec.appended},
                }})
            }
            Err(error) => error_json(&error),
        },
        "ensureDefault" => {
            let platform = match opt_str(&step["platform"]) {
                Some("win32") => Platform::Win32,
                _ => Platform::Linux,
            };
            match gp::ensure_default_autoexec(root, platform) {
                Ok(replaced) => json!({"ok": replaced}),
                Err(error) => error_json(&error),
            }
        }
        "ensureExec" => match gp::ensure_autoexec_exec(root) {
            Ok(done) => json!({"ok": {"created": done.created, "appended": done.appended}}),
            Err(error) => error_json(&error),
        },
        "importConfig" => {
            ordered(gp::import_from_config_cfg(root).map_or(Value::Null, |found| found.to_value()))
        }
        "importExtras" => ordered(
            gp::import_extras_from_config_cfg(root).map_or(Value::Null, |found| found.to_value()),
        ),
        other => panic!("неизвестный шаг {other}"),
    }
}

/// Ожидание из фикстуры в том же виде, что и ответ шага.
fn expected_of(step: &Value, expected: &Value) -> Value {
    match str_of(&step["op"]) {
        "importConfig" | "importExtras" => ordered(expected["value"].clone()),
        _ => expected.clone(),
    }
}

/// Права доступа на время удаления временной папки возвращаются.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fn open_up(dir: &Path) {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    return;
                };
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                        let _ = std::fs::set_permissions(
                            entry.path(),
                            std::fs::Permissions::from_mode(0o755),
                        );
                        open_up(&entry.path());
                    }
                }
            }
            open_up(&self.0);
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_scenario(scenario: &Value, m: &mut Mismatches) {
    let name = str_of(&scenario["name"]);
    if scenario["unix"].as_bool() == Some(true) && !cfg!(unix) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = real_root(dir.path());
    let _cleanup = Cleanup(root.clone());
    setup(&root, &scenario["setup"]);
    assert_eq!(
        items(&scenario["steps"]).len(),
        items(&scenario["results"]).len(),
        "шагов и результатов поровну"
    );
    for (at, (step, expected)) in items(&scenario["steps"])
        .iter()
        .zip(items(&scenario["results"]))
        .enumerate()
    {
        let actual = do_step(&root, step);
        let shown: String = serde_json::to_string(step)
            .unwrap()
            .chars()
            .take(300)
            .collect();
        m.check(
            &actual,
            &expected_of(step, expected),
            format!("{name}, шаг {at}: {shown}"),
        );
    }
}

#[test]
fn personalization_on_disk_matches_node() {
    let mut m = Mismatches::default();
    for scenario in items(&FIXTURE["scenarios"]) {
        run_scenario(scenario, &mut m);
    }
    m.finish("персонализация Ghosting на диске");
}

#[test]
fn stock_autoexec_is_recognized_by_its_hash() {
    let stock = str_of(&FIXTURE["stock"]);
    let hash: String = Sha256::digest(stock.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(hash, str_of(&FIXTURE["stockSha256"]));
    assert_eq!(
        default_autoexec(Platform::Linux).encode_utf16().count(),
        FIXTURE["defaultAutoexecLength"].as_u64().unwrap() as usize
    );
}
