//! Декодер GIF против 0.4.7: фикстура `gif.json` из `tools/reference/online/gif.js`
//! (`seventv/gif.js` в Node 24) — те же GIF (собраны настоящим LZW: чересстрочность, локальные
//! палитры, dispose 2, обрывы потока и заголовка, лимиты, настоящий GIF), побайтные кадры и
//! задержки, исключения Node и `sampleFrames` при 2…16 кадрах.

mod support;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hlsr_online::seventv::gif::{decode_gif, sample_frames};
use support::chat::sha256_hex;
use support::{fixture, items};

fn describe(case: &serde_json::Value) -> String {
    case["name"].as_str().unwrap().to_owned()
}

#[test]
fn decode_matches_node() {
    let fixture = fixture("gif.json");
    let mut failures = Vec::new();
    for case in items(&fixture["cases"]) {
        let gif = STANDARD.decode(case["gif"].as_str().unwrap()).unwrap();
        let expected = &case["decode"];
        let name = describe(case);
        match decode_gif(&gif) {
            Err(error) => {
                if expected["error"] != "RangeError" || expected["message"] != error.to_string() {
                    failures.push(format!("{name}: исключение {error}, а Node дал {expected}"));
                }
            }
            Ok(None) => {
                if !expected.is_null() {
                    failures.push(format!("{name}: null, а Node дал кадры или исключение"));
                }
            }
            Ok(Some(decoded)) => {
                if expected.is_null() || expected.get("frames").is_none() {
                    failures.push(format!("{name}: кадры, а Node дал {}", expected));
                    continue;
                }
                if decoded.wide != expected["wide"] || decoded.tall != expected["tall"] {
                    failures.push(format!("{name}: размер {}x{}", decoded.wide, decoded.tall));
                }
                let frames = items(&expected["frames"]);
                if decoded.frames.len() != frames.len() {
                    failures.push(format!(
                        "{name}: кадров {}, а у Node {}",
                        decoded.frames.len(),
                        frames.len()
                    ));
                    continue;
                }
                for (i, (got, want)) in decoded.frames.iter().zip(frames).enumerate() {
                    if got.delay_ms != want["delayMs"] {
                        failures.push(format!(
                            "{name}: кадр {i}, задержка {} вместо {}",
                            got.delay_ms, want["delayMs"]
                        ));
                    }
                    let same = match want["rgba"].as_str() {
                        Some(rgba) => STANDARD.encode(&got.rgba) == rgba,
                        None => sha256_hex(&got.rgba) == want["sha256"].as_str().unwrap(),
                    };
                    if !same {
                        failures.push(format!("{name}: кадр {i}, пиксели не совпали"));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} расхождений с 0.4.7:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn sample_frames_matches_node() {
    let fixture = fixture("gif.json");
    let options: Vec<usize> = items(&fixture["maxFramesOptions"])
        .iter()
        .map(|v| v.as_u64().unwrap() as usize)
        .collect();
    let mut failures = Vec::new();
    let mut checked = 0;
    for case in items(&fixture["cases"]) {
        let Some(sample) = case.get("sample") else {
            continue;
        };
        let gif = STANDARD.decode(case["gif"].as_str().unwrap()).unwrap();
        let decoded = decode_gif(&gif)
            .unwrap()
            .expect("у случая с выборкой есть кадры");
        for max in &options {
            let want = &sample[max.to_string()];
            let got = sample_frames(&decoded, *max);
            let picks: Vec<u64> = got.picks.iter().map(|&p| p as u64).collect();
            let want_picks: Vec<u64> = items(&want["picks"])
                .iter()
                .map(|v| v.as_u64().unwrap())
                .collect();
            if got.interval_ms != want["intervalMs"] || picks != want_picks {
                failures.push(format!(
                    "{} (max {max}): интервал {} вместо {}, кадры {picks:?} вместо {want_picks:?}",
                    describe(case),
                    got.interval_ms,
                    want["intervalMs"]
                ));
            }
            checked += 1;
        }
    }
    assert!(checked > 300, "проверено выборок: {checked}");
    assert!(
        failures.is_empty(),
        "{} расхождений с 0.4.7:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Корпус содержит все классы случаев, которые должны быть в сверке.
#[test]
fn corpus_has_every_class() {
    let fixture = fixture("gif.json");
    let cases = items(&fixture["cases"]);
    let has = |needle: &str| cases.iter().any(|case| describe(case).contains(needle));
    for needle in [
        "чересстрочный",
        "локальн",
        "dispose 2",
        "обрыв LZW",
        "KwKwK",
        "настоящий GIF",
        "600 кадров",
        "обрезан на",
    ] {
        assert!(has(needle), "в корпусе нет случаев «{needle}»");
    }
    assert!(
        cases
            .iter()
            .any(|case| case["decode"].get("error").is_some()),
        "нет случая с исключением Node"
    );
    assert!(
        cases.iter().any(|case| case["decode"].is_null()),
        "нет случая с null"
    );
}

/// Испорченные файлы не роняют декодер и не зависают: байты после заголовка заменяются случайными.
#[test]
fn mutated_gifs_do_not_panic() {
    let fixture = fixture("gif.json");
    let mut state = 0x1234_5678u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let started = std::time::Instant::now();
    let mut decoded = 0;
    for case in items(&fixture["cases"]) {
        let original = STANDARD.decode(case["gif"].as_str().unwrap()).unwrap();
        if original.len() < 20 {
            continue;
        }
        for _ in 0..25 {
            let mut gif = original.clone();
            for _ in 0..1 + next() % 3 {
                // размеры холста (байты 6…9) не трогаем: холст в мегапиксель на 512 кадров — гигабайты
                let at = 10 + next() as usize % (gif.len() - 10);
                gif[at] = next() as u8;
            }
            if let Ok(Some(found)) = decode_gif(&gif) {
                for max in [2, 5, 16] {
                    let sampled = sample_frames(&found, max);
                    assert!(sampled.picks.iter().all(|&pick| pick < found.frames.len()));
                }
                decoded += 1;
            }
        }
    }
    assert!(
        decoded > 500,
        "разобралось только {decoded} испорченных файлов"
    );
    assert!(
        started.elapsed().as_secs() < 60,
        "декодер слишком долго разбирал испорченные файлы"
    );
}
