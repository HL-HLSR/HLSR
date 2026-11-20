//! Строгий USTAR по описанию записей — тот же построитель, что `tools/reference/update/lib/ustar.js`:
//! архивы для самотестов Linux-установки (байты tar совпадают с Node; gzip свой — flate2).

use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};
use std::io::Write as _;

fn octal_field(value: u64, digits: usize) -> String {
    format!("{value:0digits$o}\0")
}

fn latin1(text: &str) -> Vec<u8> {
    text.chars().map(|c| c as u32 as u8).collect()
}

/// Данные записи: `text` (UTF-8), `hex`, `repeat: [кусок, раз]`.
pub fn data_of(entry: &Value) -> Vec<u8> {
    if let Some(text) = entry["text"].as_str() {
        return text.as_bytes().to_vec();
    }
    if let Some(hex) = entry["hex"].as_str() {
        return (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
    }
    if let Some(repeat) = entry["repeat"].as_array() {
        return repeat[0]
            .as_str()
            .unwrap()
            .repeat(repeat[1].as_u64().unwrap() as usize)
            .into_bytes();
    }
    Vec::new()
}

fn header_of(entry: &Value) -> ([u8; 512], Vec<u8>) {
    let mut header = [0u8; 512];
    let put = |header: &mut [u8; 512], offset: usize, size: usize, bytes: &[u8]| {
        let count = size.min(bytes.len());
        header[offset..offset + count].copy_from_slice(&bytes[..count]);
    };
    let data = data_of(entry);
    let size = entry["size"].as_u64().unwrap_or(data.len() as u64);
    let directory = entry["type"].as_str() == Some("5");
    let mode = entry["mode"]
        .as_u64()
        .unwrap_or(if directory { 0o755 } else { 0o644 });
    let text = |key: &str| entry[key].as_str().map(str::to_owned);
    put(
        &mut header,
        0,
        100,
        text("name").unwrap_or_default().as_bytes(),
    );
    put(
        &mut header,
        100,
        8,
        &latin1(&text("modeField").unwrap_or_else(|| octal_field(mode, 7))),
    );
    put(&mut header, 108, 8, &latin1(&octal_field(0, 7)));
    put(&mut header, 116, 8, &latin1(&octal_field(0, 7)));
    put(
        &mut header,
        124,
        12,
        &latin1(&text("sizeField").unwrap_or_else(|| octal_field(size, 11))),
    );
    put(&mut header, 136, 12, &latin1(&octal_field(0, 11)));
    header[156] = match entry["typeByte"].as_u64() {
        Some(byte) => byte as u8,
        None => text("type").unwrap_or_else(|| "0".to_owned()).as_bytes()[0],
    };
    put(
        &mut header,
        157,
        100,
        text("link").unwrap_or_default().as_bytes(),
    );
    put(
        &mut header,
        257,
        6,
        &latin1(&text("magic").unwrap_or_else(|| "ustar\0".to_owned())),
    );
    put(
        &mut header,
        263,
        2,
        &latin1(&text("version").unwrap_or_else(|| "00".to_owned())),
    );
    put(
        &mut header,
        345,
        155,
        text("prefix").unwrap_or_default().as_bytes(),
    );
    match text("checksumField") {
        Some(field) => put(&mut header, 148, 8, &latin1(&field)),
        None => {
            header[148..156].fill(0x20);
            let sum: i64 = header.iter().map(|byte| i64::from(*byte)).sum::<i64>()
                + entry["checksumDelta"].as_i64().unwrap_or(0);
            put(
                &mut header,
                148,
                8,
                &latin1(&format!("{} ", octal_field(sum as u64, 6))),
            );
        }
    }
    (header, data)
}

/// Байты tar по описанию архива `{entries, end, truncate}`.
pub fn tar_of(spec: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    for item in spec["entries"].as_array().unwrap() {
        if let Some(zeros) = item["zeros"].as_u64() {
            out.extend(std::iter::repeat_n(0u8, 512 * zeros as usize));
            continue;
        }
        if let Some(hex) = item["rawHex"].as_str() {
            out.extend(data_of(&json!({ "hex": hex })));
            continue;
        }
        let (header, data) = header_of(item);
        out.extend_from_slice(&header);
        out.extend_from_slice(&data);
        out.extend(std::iter::repeat_n(0u8, (512 - data.len() % 512) % 512));
    }
    out.extend(std::iter::repeat_n(
        0u8,
        512 * spec["end"].as_u64().unwrap_or(2) as usize,
    ));
    if let Some(truncate) = spec["truncate"].as_u64() {
        out.truncate(truncate as usize);
    }
    out
}

/// gzip (flate2).
pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// ELF64 LE x86-64 в hex (`elfHex` скрипта).
pub fn elf_hex(elf_class: u8, data: u8, machine: u16, length: usize) -> String {
    let mut elf = vec![0u8; length];
    let head = [0x7f, 0x45, 0x4c, 0x46, elf_class, data, 1];
    let count = head.len().min(length);
    elf[..count].copy_from_slice(&head[..count]);
    if length >= 20 {
        elf[18..20].copy_from_slice(&machine.to_le_bytes());
    }
    elf.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Записи архива лаунчера версии `version` (`launcherEntries`).
pub fn launcher_entries(version: &str) -> Vec<Value> {
    let release =
        json!({"version": version, "platform": "linux", "arch": "x64", "executable": "hlsr"});
    vec![
        json!({"name": "./", "type": "5"}),
        json!({"name": "./.hlsr-release.json", "text": release.to_string()}),
        json!({"name": "./hlsr", "mode": 0o755, "hex": elf_hex(2, 1, 62, 40)}),
        json!({"name": "./resources/", "type": "5"}),
        json!({"name": "./resources/app.asar", "text": format!("test application {version}")}),
        json!({"name": "./resources/icons/", "type": "5"}),
        json!({"name": "./resources/icons/hlsr.png", "text": "icon"}),
    ]
}

/// Сумма размеров файлов записей.
pub fn unpacked_size_of(entries: &[Value]) -> u64 {
    entries
        .iter()
        .filter(|entry| entry["type"].as_str() != Some("5"))
        .map(|entry| data_of(entry).len() as u64)
        .sum()
}

/// Архив лаунчера и его описание `{version, installer}` (как `payload.json` без `schema`).
pub struct Release {
    pub version: String,
    pub bytes: Vec<u8>,
    pub manifest: Value,
}

pub fn release(version: &str, entries: &[Value]) -> Release {
    let bytes = gzip(&tar_of(&json!({ "entries": entries })));
    let sha256: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    use base64::Engine as _;
    let sha512 = base64::engine::general_purpose::STANDARD.encode(Sha512::digest(&bytes));
    let manifest = json!({
        "version": version,
        "installer": {
            "format": "tar.gz",
            "filename": format!("HLSR-{version}-linux-x86_64.tar.gz"),
            "size": bytes.len(),
            "sha256": sha256,
            "sha512": sha512,
            "unpackedSize": unpacked_size_of(entries),
        },
    });
    Release {
        version: version.to_owned(),
        bytes,
        manifest,
    }
}
