//! Картинки и шрифты Ghosting и разрешение экрана Source против 0.4.7: фикстура
//! `ghosting-assets.json` из `tools/reference/delivery/ghosting-assets.js`. Сценарий — данные
//! (файлы и шаги), его выполняют модуль 0.4.7 в Node 24 и этот крейт на одинаковых деревьях во
//! временных папках; сравниваются ответ каждого шага и дерево файлов (путь, размер, sha256).
//! PNG, который предпросмотр строит из TGA, сравнивается разобранным (размеры, тип, куски, CRC32,
//! строки после inflate): байты сжатия у zlib и miniz разные.
//!
//! Сценарии, помеченные `unix`, зависят от ОС (ссылки, права доступа, регистр имён, порядок
//! `readdir`, имена с пробелом на конце): на Windows они пропускаются.

mod support;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use flate2::read::ZlibDecoder;
use hlsr_core::js::Platform;
use hlsr_delivery::ghosting_assets::{self, AssetError};
use hlsr_delivery::ports::{FakeRegistry, RegistryHive, RegistryValue};
use hlsr_delivery::source_screen::{self, ScreenSource};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("ghosting-assets.json"));

/// Корень сценария. В Linux — настоящий путь, как `realpathSync` в скрипте; в Windows
/// `canonicalize` дал бы путь с `\\?\`, к которому нельзя дописывать `/`.
fn real_root(dir: &Path) -> PathBuf {
    if cfg!(unix) {
        dir.canonicalize().unwrap()
    } else {
        dir.to_path_buf()
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

// ------------------------------------------------------------- файлы сценария

/// Серый TGA (type 11, 8 бит) из кусков RLE по 128 точек: как `bigRleTga` в скрипте.
fn big_rle_tga(width: usize, height: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; 18];
    bytes[2] = 11;
    bytes[12..14].copy_from_slice(&(width as u16).to_le_bytes());
    bytes[14..16].copy_from_slice(&(height as u16).to_le_bytes());
    bytes[16] = 8;
    let total = width * height;
    for _ in 0..total / 128 {
        bytes.extend_from_slice(&[0xff, 0x55]);
    }
    if !total.is_multiple_of(128) {
        bytes.extend_from_slice(&[0x80 | (total % 128 - 1) as u8, 0x55]);
    }
    bytes
}

fn bytes_of(item: &Value) -> Vec<u8> {
    if let Some(text) = opt_str(&item["hex"]) {
        return unhex(text);
    }
    if let Some(text) = opt_str(&item["utf8"]) {
        return text.as_bytes().to_vec();
    }
    if let Some(size) = item["bigRleTga"].as_array() {
        return big_rle_tga(
            size[0].as_u64().unwrap() as usize,
            size[1].as_u64().unwrap() as usize,
        );
    }
    Vec::new()
}

fn path_in(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part))
}

/// Время записи у всех файлов сценария одно: 1600000000 с.
fn set_mtime(file: &Path) {
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    let handle = std::fs::OpenOptions::new().write(true).open(file).unwrap();
    handle
        .set_times(std::fs::FileTimes::new().set_modified(time))
        .unwrap();
}

fn put_file(root: &Path, item: &Value) {
    let file = path_in(root, str_of(&item["file"]));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, bytes_of(item)).unwrap();
    if let Some(size) = item["truncate"].as_u64() {
        std::fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_len(size)
            .unwrap();
    }
    set_mtime(&file);
}

fn substitute(root: &Path, text: &str) -> String {
    text.replace("<root>", root.to_str().unwrap())
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) {
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(not(unix))]
fn symlink(_target: &str, _link: &Path) {
    unreachable!("сценарии со ссылками на Windows пропускаются");
}

fn setup(root: &Path, list: &Value) {
    for item in list.as_array().into_iter().flatten() {
        if let Some(dir) = opt_str(&item["dir"]) {
            std::fs::create_dir_all(path_in(root, dir)).unwrap();
        } else if let Some(link) = opt_str(&item["symlink"]) {
            symlink(
                &substitute(root, str_of(&item["target"])),
                &path_in(root, link),
            );
        } else {
            put_file(root, item);
        }
    }
}

// --------------------------------------------------------------- результаты

fn tree_of(root: &Path) -> Value {
    let mut tree = support::tree::snapshot(root, None);
    let root_text = root.to_str().unwrap();
    for entry in tree.as_array_mut().unwrap() {
        if let Some(target) = entry["target"].as_str() {
            entry["target"] = json!(target.replace(root_text, "<root>"));
        }
    }
    tree
}

fn outcome<T>(result: Result<T, AssetError>, show: impl FnOnce(T) -> Value) -> Value {
    match result {
        Ok(value) => json!({"ok": show(value)}),
        Err(error) => json!({"error": {"code": error.code, "message": error.message}}),
    }
}

/// Разбор PNG, который построил предпросмотр: тот же вид, что `pngDetails` в скрипте.
fn png_details(bytes: &[u8]) -> Value {
    let mut details = Map::new();
    details.insert(
        "signatureOk".into(),
        json!(bytes[..8] == [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    );
    let mut chunks = Vec::new();
    let mut crc_ok = true;
    let mut offset = 8;
    let mut decoded: Option<Vec<u8>> = None;
    while offset < bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = String::from_utf8_lossy(&bytes[offset + 4..offset + 8]).into_owned();
        let body = &bytes[offset + 4..offset + 8 + length];
        let stored = u32::from_be_bytes(
            bytes[offset + 8 + length..offset + 12 + length]
                .try_into()
                .unwrap(),
        );
        if stored != crc32fast::hash(body) {
            crc_ok = false;
        }
        let data = &bytes[offset + 8..offset + 8 + length];
        if kind == "IHDR" {
            details.insert(
                "width".into(),
                json!(u32::from_be_bytes(data[0..4].try_into().unwrap())),
            );
            details.insert(
                "height".into(),
                json!(u32::from_be_bytes(data[4..8].try_into().unwrap())),
            );
            details.insert("bitDepth".into(), json!(data[8]));
            details.insert("colorType".into(), json!(data[9]));
            details.insert("compression".into(), json!(data[10]));
            details.insert("filter".into(), json!(data[11]));
            details.insert("interlace".into(), json!(data[12]));
        }
        if kind == "IDAT" {
            let mut out = Vec::new();
            ZlibDecoder::new(data).read_to_end(&mut out).unwrap();
            decoded = Some(out);
        }
        chunks.push(json!(kind));
        offset += length + 12;
    }
    let decoded = decoded.expect("в PNG нет IDAT");
    details.insert("chunks".into(), Value::Array(chunks));
    details.insert("crcOk".into(), json!(crc_ok));
    details.insert(
        "rows".into(),
        if decoded.len() <= 4096 {
            json!(hex(&decoded))
        } else {
            Value::Null
        },
    );
    details.insert("rowsLength".into(), json!(decoded.len()));
    details.insert("rowsSha256".into(), json!(sha256(&decoded)));
    Value::Object(details)
}

fn preview_result(step: &Value, data_url: &str) -> Value {
    let rest = data_url.strip_prefix("data:").expect("data:-адрес");
    let (mime, encoded) = rest.split_once(";base64,").expect("base64");
    let bytes = BASE64_STANDARD.decode(encoded).unwrap();
    if step["tga"].as_bool() == Some(true) {
        return json!({"mime": mime, "png": png_details(&bytes)});
    }
    json!({"mime": mime, "length": bytes.len(), "sha256": sha256(&bytes)})
}

fn registry_of(step: &Value) -> FakeRegistry {
    let registry = FakeRegistry::default();
    let key = format!(
        "Software\\Valve\\Source\\{}\\Settings",
        str_of(&step["mod"])
    );
    if let Some(values) = step["registry"].as_object() {
        for (name, value) in values {
            let value = match value {
                Value::Number(number) => RegistryValue::Dword(number.as_u64().unwrap() as u32),
                other => RegistryValue::Sz(str_of(other).to_owned()),
            };
            registry.set(RegistryHive::CurrentUser, &key, name, value);
        }
    }
    registry
}

fn screen_json(found: Option<source_screen::Screen>) -> Value {
    found.map_or(Value::Null, |screen| {
        json!({"width": screen.width, "height": screen.height, "source": screen.source.as_str()})
    })
}

fn do_step(root: &Path, step: &Value) -> Value {
    let text = |name: &str| substitute(root, str_of(&step[name]));
    match str_of(&step["op"]) {
        "tree" => json!({"tree": tree_of(root)}),
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
        "symlink" => {
            symlink(&text("target"), &path_in(root, str_of(&step["link"])));
            Value::Null
        }
        "import" => outcome(
            ghosting_assets::import_asset(&text("install"), &text("source"), str_of(&step["kind"])),
            |imported| {
                json!({
                    "value": imported.value, "name": imported.name,
                    "bytes": imported.bytes, "sha256": imported.sha256,
                })
            },
        ),
        "preview" => {
            let prefix =
                opt_str(&step["prefix"]).map(|prefix| PathBuf::from(substitute(root, prefix)));
            outcome(
                // Порядок папок custom/* — язык по умолчанию процесса: без `locale` фикстура снята
                // с en-US (скрипт ставит LC_ALL), с `locale` Node подменяет язык у localeCompare.
                ghosting_assets::read_preview_asset_in(
                    &text("install"),
                    str_of(&step["kind"]),
                    str_of(&step["value"]),
                    prefix.as_deref(),
                    opt_str(&step["locale"]).unwrap_or("en-US"),
                ),
                |asset| preview_result(step, &asset.data_url),
            )
        }
        "screen" => {
            let platform = if str_of(&step["platform"]) == "win32" {
                Platform::Win32
            } else {
                Platform::Linux
            };
            let prefix =
                opt_str(&step["prefix"]).map(|prefix| PathBuf::from(substitute(root, prefix)));
            let registry = registry_of(step);
            let found = source_screen::screen_of(
                Some(&step["settings"]),
                str_of(&step["mod"]),
                platform,
                prefix.as_deref(),
                &registry,
            );
            json!({"screen": screen_json(found)})
        }
        other => panic!("неизвестный шаг {other}"),
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
        let shown = serde_json::to_string(step).unwrap();
        let shown: String = shown.chars().take(300).collect();
        m.check(&actual, expected, format!("{name}, шаг {at}: {shown}"));
    }
}

#[test]
fn scenarios_match_node() {
    let mut m = Mismatches::default();
    for scenario in items(&FIXTURE["scenarios"]) {
        run_scenario(scenario, &mut m);
    }
    m.finish("импорт и предпросмотр картинок и шрифтов, разрешение экрана");
}

#[test]
fn size_limit_is_16_mib() {
    assert_eq!(
        ghosting_assets::MAX_ASSET_BYTES,
        FIXTURE["maxAssetBytes"].as_u64().unwrap()
    );
}

#[test]
fn screen_source_names_are_the_same_as_in_node() {
    assert_eq!(ScreenSource::Launcher.as_str(), "launcher");
    assert_eq!(ScreenSource::Game.as_str(), "game");
}
