//! Картинки в чат игры против 0.4.7: фикстура `native-image.json` из
//! `tools/reference/online/native-image.js` — `decodePng` и `resizeRgba` с настоящим
//! `nativeImage` Chromium (Electron 44.5.1, Skia, libpng, libjpeg-turbo). PNG и масштабирование
//! совпадают побайтно; JPEG — с допуском (другой декодер).

mod support;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hlsr_online::seventv::{decode_png, native_image, resize_rgba};
use serde_json::Value;
use support::chat::sha256_hex;
use support::recipes::generate;
use support::{fixture, items, str_of};

/// Допуск на канал у JPEG: libjpeg-turbo и zune-jpeg округляют по-разному.
const JPEG_TOLERANCE: u8 = 16;
/// Допуск у PNG с гаммой: Skia считает поправку в f32 с приближённой степенью.
const GAMMA_TOLERANCE: u8 = 1;

/// Сравнить пиксели с ожидаемым значением (`{rgba}` или `{sha256, bytes}`): текст расхождения.
fn compare(actual: &[u8], expected: &Value, tolerance: u8) -> Option<String> {
    if let Some(rgba) = expected["rgba"].as_str() {
        let want = STANDARD.decode(rgba).unwrap();
        if want.len() != actual.len() {
            return Some(format!("{} байт вместо {}", actual.len(), want.len()));
        }
        let mut worst = 0u8;
        let mut off = 0usize;
        let mut first = None;
        for (i, (a, b)) in actual.iter().zip(&want).enumerate() {
            let d = a.abs_diff(*b);
            if d > tolerance {
                off += 1;
                first.get_or_insert(i);
            }
            worst = worst.max(d);
        }
        return (off > 0).then(|| {
            let at = first.unwrap();
            format!(
                "{off} байт вне допуска {tolerance}, наибольшая разница {worst}; первый — {at} (пиксель {}, канал {}): {} вместо {}",
                at / 4,
                at % 4,
                actual[at],
                want[at]
            )
        });
    }
    let hash = sha256_hex(actual);
    (hash != expected["sha256"].as_str().unwrap()
        || actual.len() as u64 != expected["bytes"].as_u64().unwrap())
    .then(|| {
        format!(
            "SHA-256 {hash} ({} байт) вместо {} ({} байт)",
            actual.len(),
            expected["sha256"],
            expected["bytes"]
        )
    })
}

#[test]
fn decode_png_matches_chromium() {
    let fixture = fixture("native-image.json");
    let mut failures = Vec::new();
    for case in items(&fixture["png"]) {
        let name = str_of(&case["name"]);
        let bytes = STANDARD.decode(str_of(&case["png"])).unwrap();
        let px = case["px"].as_u64().unwrap() as u32;
        let jpeg = name.starts_with("JPEG");
        let tolerance = if jpeg {
            JPEG_TOLERANCE
        } else if name.contains("gAMA") {
            GAMMA_TOLERANCE
        } else {
            0
        };

        // Размер до масштабирования — как у `image.getSize()`.
        let raw = native_image::create_from_buffer(&bytes);
        match (&raw, &case["raw"]) {
            (None, Value::Null) => {}
            (Some(raw), expected)
                if raw.wide == expected["width"] && raw.tall == expected["height"] => {}
            (raw, expected) => failures.push(format!(
                "{name}: исходный размер {:?}, у Chromium {expected}",
                raw.as_ref().map(|raw| (raw.wide, raw.tall))
            )),
        }

        let got = decode_png(&bytes, px);
        let expected = &case["result"];
        match (got, expected.is_null()) {
            (None, true) => {}
            (None, false) => failures.push(format!("{name}: None, а Chromium дал картинку")),
            (Some(_), true) => failures.push(format!("{name}: картинка, а Chromium дал null")),
            (Some(got), false) => {
                if got.wide != expected["wide"] || got.tall != expected["tall"] {
                    failures.push(format!(
                        "{name}: размер {}x{} вместо {}x{}",
                        got.wide, got.tall, expected["wide"], expected["tall"]
                    ));
                    continue;
                }
                if let Some(text) = compare(&got.rgba, expected, tolerance) {
                    failures.push(format!("{name}: {text}"));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} расхождений с Chromium:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn resize_rgba_matches_skia() {
    let fixture = fixture("native-image.json");
    let mut failures = Vec::new();
    for case in items(&fixture["resize"]) {
        let name = str_of(&case["name"]);
        let recipe = &case["recipe"];
        let src = generate(recipe);
        let (wide, tall) = (
            recipe["wide"].as_u64().unwrap() as u32,
            recipe["tall"].as_u64().unwrap() as u32,
        );
        let (out_wide, out_tall) = (
            case["outWide"].as_u64().unwrap() as u32,
            case["outTall"].as_u64().unwrap() as u32,
        );
        let got = resize_rgba(&src, wide, tall, out_wide, out_tall);
        match (got, case["result"].is_null()) {
            (None, true) => {}
            (None, false) => failures.push(format!("{name}: None, а Chromium дал картинку")),
            (Some(_), true) => failures.push(format!("{name}: картинка, а Chromium дал null")),
            (Some(got), false) => {
                if let Some(text) = compare(&got, &case["result"], 0) {
                    failures.push(format!("{name}: {text}"));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} расхождений с Skia:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
