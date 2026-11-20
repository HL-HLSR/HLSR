//! Оверлей LiveSplit: фикстура `livesplit-overlay.json` 0.4.7 из
//! `tools/reference/platform/livesplit-overlay.js` (`shared/livesplit-overlay.js` в 0.4.9 тот же:
//! `toIni`, `keyAlpha`, `cropRect`, `placement`, `positionOf`, `keyImage`, `backgroundColor`;
//! `game-delivery/livesplit-overlay.js`: `processNameOf`, `parseSnapshot`, файл статуса) и
//! самотесты `livesplit-overlay-selftest.js` 0.4.9 (прокси в папке игры вместо инжектора).
//! Сценарии класса над папкой из фикстуры 0.4.7 (инжектор, `hlsr-overlay.dll`) в 0.4.9 устарели и
//! не сверяются.

mod support;

use hlsr_core::js::Platform;
use hlsr_core::logger::{Logger, MemoryLogger};
use hlsr_delivery::livesplit_overlay::{default_overlay, overlay_changes, sanitize_overlay};
use hlsr_platform::livesplit_overlay::{
    self as lso, LiveSplitOverlay, OverlayOptions, Target, WaitOptions, merge_overrides,
    parse_snapshot, process_name_of, wait_for_status,
};
use hlsr_platform::livesplit_overlay_shared::{
    PixelOrder, SENTINEL, background_color, crop_rect, is_sentinel, key_alpha, key_image,
    placement, position_of, to_ini,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("livesplit-overlay.json"));

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn f(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("не число: {value}"))
}

fn ser(value: &Value) -> String {
    hlsr_core::json_text::stringify(value, "")
}

#[test]
fn ini_matches_0_4_7() {
    let cases = items(&FIXTURE["ini"]);
    assert_eq!(cases.len(), 150);
    let mut m = Mismatches::default();
    for case in cases {
        let input = &case["input"];
        m.check_eq(
            to_ini(&sanitize_overlay(input)),
            str_of(&case["ini"]).to_owned(),
            format!("toIni({})", ser(input)),
        );
    }
    m.finish("overlay.ini");
}

#[test]
fn key_alpha_matches_0_4_7() {
    // Те же цвета, что в скрипте: 15 заданных и 13 случайных — берём их из числа значений.
    let cases = items(&FIXTURE["keyAlpha"]);
    assert_eq!(cases.len(), 48);
    let colors = colors();
    let mut m = Mismatches::default();
    for case in cases {
        let overlay = sanitize_overlay(&case["input"]);
        let values = items(&case["values"]);
        assert_eq!(values.len(), colors.len());
        for (color, expected) in colors.iter().zip(values) {
            m.check_eq(
                key_alpha(&overlay, color[0], color[1], color[2]),
                f(expected),
                format!("keyAlpha({}, {color:?})", ser(&case["input"])),
            );
        }
    }
    m.finish("keyAlpha");
}

/// Цвета проб `keyAlpha` из скрипта: первые 15 заданы, остальные — из генератора с зерном; их
/// значения лежат в фикстуре в поле `colors`.
fn colors() -> Vec<[f64; 3]> {
    items(&FIXTURE["colors"])
        .iter()
        .map(|color| {
            let list = items(color);
            [f(&list[0]), f(&list[1]), f(&list[2])]
        })
        .collect()
}

#[test]
fn crop_rect_matches_0_4_7() {
    let cases = items(&FIXTURE["cropRect"]);
    assert_eq!(cases.len(), 270);
    let mut m = Mismatches::default();
    for case in cases {
        let overlay = sanitize_overlay(&case["input"]);
        let rect = crop_rect(
            &overlay,
            case["width"].as_i64().unwrap(),
            case["height"].as_i64().unwrap(),
        );
        m.check(
            &json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height}),
            &case["rect"],
            format!(
                "cropRect({}, {}, {})",
                ser(&case["input"]),
                case["width"],
                case["height"]
            ),
        );
    }
    m.finish("cropRect");
}

#[test]
fn placement_matches_0_4_7() {
    let cases = items(&FIXTURE["placement"]);
    assert!(cases.len() > 500, "случаев: {}", cases.len());
    let mut m = Mismatches::default();
    for case in cases {
        let overlay = sanitize_overlay(&case["input"]);
        let screen = items(&case["screen"]);
        let picture = items(&case["picture"]);
        let actual = placement(
            &overlay,
            f(&screen[0]),
            f(&screen[1]),
            f(&picture[0]),
            f(&picture[1]),
        )
        .map_or(
            Value::Null,
            |p| json!({"left": p.left, "top": p.top, "width": p.width, "height": p.height}),
        );
        m.check(
            &actual,
            &case["result"],
            format!(
                "placement({}, {}, {})",
                ser(&case["input"]),
                case["screen"],
                case["picture"]
            ),
        );
    }
    m.finish("placement");
}

#[test]
fn position_of_matches_0_4_7() {
    let cases = items(&FIXTURE["positionOf"]);
    assert_eq!(cases.len(), 280);
    let mut m = Mismatches::default();
    for case in cases {
        let a: Vec<f64> = items(&case["args"]).iter().map(f).collect();
        let position = position_of(a[0], a[1], a[2], a[3], a[4], a[5]);
        m.check_eq(
            (position.x, position.y),
            (f(&case["result"]["x"]), f(&case["result"]["y"])),
            format!("positionOf({})", case["args"]),
        );
    }
    m.finish("positionOf");
}

fn order_of(name: &str) -> PixelOrder {
    match name {
        "rgba" => PixelOrder::Rgba,
        _ => PixelOrder::Bgrx,
    }
}

#[test]
fn key_image_matches_0_4_7() {
    let cases = items(&FIXTURE["keyImage"]);
    assert_eq!(cases.len(), 60);
    let mut m = Mismatches::default();
    for case in cases {
        let overlay = sanitize_overlay(&case["input"]);
        let (width, height) = (
            case["width"].as_u64().unwrap() as usize,
            case["height"].as_u64().unwrap() as usize,
        );
        let image = unhex(str_of(&case["image"]));
        let keyed = key_image(
            &overlay,
            &image,
            width,
            height,
            order_of(str_of(&case["order"])),
        );
        m.check(
            &json!({"width": keyed.width, "height": keyed.height, "data": hex(&keyed.data)}),
            &case["result"],
            format!(
                "keyImage({}, {width}x{height}, {})",
                ser(&case["input"]),
                case["order"]
            ),
        );
    }
    m.finish("keyImage");
}

#[test]
fn background_color_matches_0_4_7() {
    let cases = items(&FIXTURE["backgroundColor"]);
    assert_eq!(cases.len(), 41);
    let mut m = Mismatches::default();
    for case in cases {
        let (width, height) = (
            case["width"].as_u64().unwrap() as usize,
            case["height"].as_u64().unwrap() as usize,
        );
        let image = unhex(str_of(&case["image"]));
        let actual = background_color(&image, width, height)
            .map_or(Value::Null, |c| json!({"r": c.r, "g": c.g, "b": c.b}));
        m.check(
            &actual,
            &case["result"],
            format!("backgroundColor({width}x{height}, {})", case["image"]),
        );
    }
    m.finish("backgroundColor");
}

#[test]
fn process_name_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["processNameOf"]) {
        m.check_eq(
            process_name_of(case["executable"].as_str().unwrap_or("")),
            str_of(&case["name"]).to_owned(),
            format!("processNameOf({})", case["executable"]),
        );
    }
    m.finish("имя процесса");
}
#[test]
fn snapshot_parsing_matches_0_4_7() {
    let cases = items(&FIXTURE["parseSnapshot"]);
    assert_eq!(cases.len(), 17);
    let mut m = Mismatches::default();
    for case in cases {
        let buffer = unhex(str_of(&case["buffer"]));
        let actual = parse_snapshot(&buffer).map_or(
            Value::Null,
            |s| json!({"width": s.width, "height": s.height, "pixels": hex(&s.pixels)}),
        );
        m.check(
            &actual,
            &case["result"],
            format!("parseSnapshot({})", case["buffer"]),
        );
    }
    m.finish("разбор снимка");
}

#[tokio::test]
async fn status_file_matches_0_4_7() {
    let cases = items(&FIXTURE["status"]);
    assert_eq!(cases.len(), 18);
    let mut m = Mismatches::default();
    for case in cases {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("inject.status");
        std::fs::write(&file, unhex(str_of(&case["bytes"]))).unwrap();
        let options = WaitOptions {
            interval: Some(Duration::from_millis(5)),
            cancelled: None,
        };
        let actual = wait_for_status(&file, Duration::from_millis(30), &options).await;
        m.check_eq(
            actual,
            case["result"].as_u64(),
            format!("файл статуса {}", case["bytes"]),
        );
    }
    m.finish("файл статуса");
}

// ------------------------------------------------- самотесты livesplit-overlay-selftest.js

/// `fakeResources(root)`: ресурсы лаунчера с поддельными файлами оверлея; прокси узнаётся по
/// метке внутри файла.
struct World {
    root: tempfile::TempDir,
}

impl World {
    fn new() -> World {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("resources").join(lso::RESOURCE_DIRECTORY);
        std::fs::create_dir_all(&dir).unwrap();
        for name in lso::FILES_LINUX {
            let mut bytes = format!("{name} v1 ").into_bytes();
            bytes.extend_from_slice(lso::PROXY_MARKER);
            std::fs::write(dir.join(name), bytes).unwrap();
        }
        World { root }
    }

    fn overlay(&self, base: &str, platform: Platform) -> LiveSplitOverlay {
        LiveSplitOverlay::new(OverlayOptions {
            base_dir: Some(self.root.path().join(base)),
            resources_path: Some(self.root.path().join("resources")),
            logger: Some(Arc::new(MemoryLogger::default()) as Arc<dyn Logger>),
            platform: Some(platform),
        })
    }

    /// `installOf(root, executable, engine)`: папка игры с `bin\` и папкой exe.
    fn install(&self, executable: &str) -> PathBuf {
        let install = self.root.path().join("game");
        std::fs::create_dir_all(install.join(executable).parent().unwrap()).unwrap();
        std::fs::create_dir_all(install.join("bin")).unwrap();
        install
    }
}

fn source_target(install: &Path) -> Target<'_> {
    Target {
        install_path: install,
        executable: "hl2.exe",
        engine: None,
    }
}

#[test]
fn selftest_defaults_are_disabled_and_junk_is_replaced() {
    let base = default_overlay();
    assert!(!base.enabled);
    assert_eq!(sanitize_overlay(&Value::Null), base);
    let clean = sanitize_overlay(&json!({
        "enabled": "yes", "x": 140, "y": -3, "height": "2", "opacity": 55.6, "key": "chroma",
        "keyColor": {"r": 300, "g": "16", "b": null}, "crop": {"top": 9999, "left": "4"},
    }));
    assert!(!clean.enabled, "включает только true");
    assert_eq!(clean.x, 100.0);
    assert_eq!(clean.y, 0.0);
    assert_eq!(clean.height, 5.0);
    assert_eq!(clean.opacity, 56);
    assert_eq!(clean.key, "none");
    assert_eq!(
        (clean.key_color.r, clean.key_color.g, clean.key_color.b),
        (255, 16, 0)
    );
    assert_eq!(
        (
            clean.crop.top,
            clean.crop.right,
            clean.crop.bottom,
            clean.crop.left
        ),
        (2048, 0, 0, 4)
    );
    let base_value = base.to_value();
    assert_eq!(overlay_changes(&base_value), 0);
    let mut changed = base_value.clone();
    changed["x"] = json!(0);
    changed["key"] = json!("luma");
    assert_eq!(overlay_changes(&changed), 2);
    let mut enabled = base_value;
    enabled["enabled"] = json!(true);
    assert_eq!(
        overlay_changes(&enabled),
        0,
        "выключатель сброс раздела не трогает"
    );
}

#[test]
fn selftest_ini_is_in_the_format_the_dll_reads() {
    let ini = to_ini(&sanitize_overlay(&json!({
        "enabled": true, "x": 12.5, "key": "color",
        "keyColor": {"r": 1, "g": 171, "b": 255}, "crop": {"top": 3},
    })));
    let lines: Vec<&str> = ini.split("\r\n").collect();
    for expected in [
        "enabled=1",
        "x=12.5",
        "key=1",
        "key_color=01abff",
        "crop_top=3",
        "luma_min=0.1",
    ] {
        assert!(lines.contains(&expected), "нет строки {expected}");
    }
    assert_eq!(
        lines.last(),
        Some(&""),
        "файл заканчивается переводом строки"
    );
    assert!(to_ini(&sanitize_overlay(&json!({"key": "luma"}))).contains("key=2\r\n"));
}

#[test]
fn selftest_masks_like_obs() {
    let color = sanitize_overlay(&json!({"key": "color", "keyColor": {"r": 0, "g": 255, "b": 0}}));
    assert_eq!(
        key_alpha(&color, 0.0, 1.0, 0.0),
        0.0,
        "цвет ключа вырезается"
    );
    assert_eq!(
        key_alpha(&color, 1.0, 1.0, 1.0),
        1.0,
        "белый текст остаётся"
    );
    let edge = key_alpha(&color, 0.3, 1.0, 0.3);
    assert!(edge > 0.0 && edge < 1.0, "у границы — сглаживание");

    let luma = sanitize_overlay(&json!({"key": "luma", "lumaMin": 0.1, "lumaMinSmooth": 0.05}));
    assert_eq!(
        key_alpha(&luma, 0.05, 0.05, 0.05),
        0.0,
        "тёмный фон вырезается"
    );
    assert_eq!(
        key_alpha(&luma, 1.0, 1.0, 1.0),
        1.0,
        "светлый текст остаётся"
    );
    let bright =
        sanitize_overlay(&json!({"key": "luma", "lumaMin": 0, "lumaMinSmooth": 0, "lumaMax": 0.8}));
    assert_eq!(
        key_alpha(&bright, 1.0, 1.0, 1.0),
        0.0,
        "выше максимума вырезается"
    );
    assert_eq!(
        key_alpha(&default_overlay(), 0.0, 0.0, 0.0),
        1.0,
        "без маски всё непрозрачно"
    );
}

#[test]
fn selftest_placement_does_not_depend_on_resolution() {
    let value = sanitize_overlay(&json!({"x": 100, "y": 0, "height": 50}));
    let at =
        |sw, sh, w, h| placement(&value, sw, sh, w, h).map(|p| (p.left, p.top, p.width, p.height));
    assert_eq!(at(1600.0, 900.0, 200.0, 300.0), Some((1300, 0, 300, 450)));
    assert_eq!(at(800.0, 450.0, 200.0, 300.0), Some((650, 0, 150, 225)));
    // Слишком широкая картинка ужимается по ширине кадра.
    let wide = placement(
        &sanitize_overlay(&json!({"x": 0, "y": 50, "height": 100})),
        400.0,
        300.0,
        800.0,
        300.0,
    )
    .unwrap();
    assert_eq!((wide.width, wide.height, wide.top), (400, 150, 75));
    assert_eq!(at(8.0, 8.0, 10.0, 10.0), None);

    let rect = placement(
        &sanitize_overlay(&json!({"x": 30, "y": 70, "height": 40})),
        1600.0,
        900.0,
        200.0,
        300.0,
    )
    .unwrap();
    let back = position_of(
        rect.left as f64,
        rect.top as f64,
        1600.0,
        900.0,
        rect.width as f64,
        rect.height as f64,
    );
    assert_eq!((back.x, back.y), (30.0, 70.0));
    let clamped = position_of(-50.0, 5000.0, 1600.0, 900.0, 100.0, 100.0);
    assert_eq!((clamped.x, clamped.y), (0.0, 100.0), "за краями — к краю");
}

#[test]
fn selftest_snapshot_is_cropped_and_keyed() {
    // 4x10 BGRX: левый столбец зелёный, остальные белые.
    let (width, height) = (4_usize, 10_usize);
    let mut bgrx = vec![0_u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let pixel: [u8; 4] = if x == 0 {
                [0, 255, 0, 0]
            } else {
                [255, 255, 255, 0]
            };
            bgrx[(y * width + x) * 4..][..4].copy_from_slice(&pixel);
        }
    }
    let overlay = sanitize_overlay(&json!({"key": "color", "crop": {"top": 1, "bottom": 1}}));
    let keyed = key_image(&overlay, &bgrx, width, height, PixelOrder::Bgrx);
    assert_eq!(
        keyed.width, 4,
        "обрезка по ширине оставила бы меньше 8 пикселей — не применяется"
    );
    assert_eq!(keyed.height, 8);
    assert_eq!(keyed.data[3], 0, "зелёный вырезан");
    assert_eq!(&keyed.data[4..8], &[255, 255, 255, 255]);

    // RGBA (примерный LiveSplit из canvas): красный остаётся красным.
    let rgba = key_image(
        &sanitize_overlay(&json!({})),
        &[255, 0, 0, 255],
        1,
        1,
        PixelOrder::Rgba,
    );
    assert_eq!(rgba.data, vec![255, 0, 0, 255]);
}

#[test]
fn selftest_unpainted_place_is_transparent_and_background_is_not_taken_from_it() {
    // Масштаб Windows 125%: LiveSplit нарисовал себя в левые верхние 8x8 из 10x10, полосы справа
    // и снизу остались залиты SENTINEL (common.h).
    let (width, height) = (10_usize, 10_usize);
    let mut bgrx = vec![0_u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let pixel: [u8; 4] = if x < 8 && y < 8 {
                [0, 255, 0, 0]
            } else {
                SENTINEL
            };
            bgrx[(y * width + x) * 4..][..4].copy_from_slice(&pixel);
        }
    }
    for key in ["none", "color", "luma"] {
        let keyed = key_image(
            &sanitize_overlay(&json!({"key": key})),
            &bgrx,
            width,
            height,
            PixelOrder::Bgrx,
        );
        assert_eq!(
            keyed.data[(9 * width + 9) * 4 + 3],
            0,
            "{key}: полоса прозрачна"
        );
    }
    let plain = key_image(
        &sanitize_overlay(&json!({"key": "none"})),
        &bgrx,
        width,
        height,
        PixelOrder::Bgrx,
    );
    assert_eq!(plain.data[3], 255, "сам LiveSplit виден");
    let background = background_color(&bgrx, width, height).unwrap();
    assert_eq!((background.r, background.g, background.b), (0, 255, 0));
    assert!(is_sentinel(&bgrx, (9 * width + 9) * 4));

    // C-код делает то же: SENTINEL в common.h совпадает с цветом в Rust.
    let common = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/livesplit-overlay/common.h"),
    )
    .unwrap();
    assert!(common.contains("#define SENTINEL 0x5A00FF5Au"));
    assert!(common.contains("if (p == SENTINEL) { out[x] = 0; continue; }"));
    // 0x5A00FF5A в BGRX по байтам: 5A FF 00 5A.
    assert_eq!(SENTINEL, 0x5A00_FF5A_u32.to_le_bytes());
}

#[test]
fn selftest_process_name_and_snapshot_parsing() {
    assert_eq!(process_name_of("bin\\HL2.EXE"), "hl2.exe");

    let mut header = Vec::new();
    header.extend_from_slice(b"HLSRLS1\0");
    header.extend_from_slice(&2_u32.to_le_bytes());
    header.extend_from_slice(&1_u32.to_le_bytes());
    let good = [header.clone(), vec![7; 8]].concat();
    let snapshot = parse_snapshot(&good).unwrap();
    assert_eq!(snapshot.width, 2);
    assert_eq!(snapshot.pixels.len(), 8);
    assert!(
        parse_snapshot(&[header.clone(), vec![0; 4]].concat()).is_none(),
        "обрезанный снимок"
    );
    assert!(parse_snapshot(b"garbage").is_none());
}

#[test]
fn selftest_foreign_wine_overrides_stay() {
    assert_eq!(merge_overrides(Some(""), "d3d9=n,b"), "d3d9=n,b");
    assert_eq!(merge_overrides(None, "d3d9=n,b"), "d3d9=n,b");
    assert_eq!(
        merge_overrides(Some("dxgi=n"), "d3d9=n,b"),
        "dxgi=n;d3d9=n,b"
    );
}

#[test]
fn selftest_settings_are_written_live_and_proxy_goes_to_game_folders() {
    let world = World::new();
    let service = world.overlay("data", Platform::Win32);
    let install = world.install("hl2.exe");
    let target = source_target(&install);
    assert!(
        service
            .prepare("ghosting-renewed", &json!({}), Some(&target), false)
            .unwrap()
            .is_none()
    );
    assert!(
        !service
            .write_config("ghosting-renewed", &json!({}))
            .unwrap(),
        "игра без оверлея — папки нет"
    );

    let prepared = service
        .prepare(
            "ghosting-renewed",
            &json!({"liveSplitOverlay": {"enabled": true, "x": 0}}),
            Some(&target),
            false,
        )
        .unwrap()
        .expect("оверлей включён");
    assert_eq!(prepared.proxy, "d3d9.dll");
    assert_eq!(
        prepared.dll_override, None,
        "на Windows замена DLL не нужна"
    );
    let ini = std::fs::read_to_string(prepared.dir.join("overlay.ini")).unwrap();
    assert!(ini.starts_with("enabled=1\r\nx=0\r\n"), "{ini:?}");
    // Source: рядом с hl2.exe и в bin\.
    let sidecar = format!(
        "{}\n",
        hlsr_core::js::path::NodePath::native().resolve(&[&prepared.dir.to_string_lossy()])
    );
    for folder in [install.clone(), install.join("bin")] {
        assert!(folder.join("d3d9.dll").exists(), "{}", folder.display());
        assert_eq!(
            std::fs::read_to_string(folder.join(lso::SIDECAR)).unwrap(),
            sidecar
        );
    }
    assert!(
        !prepared.dir.join(lso::HELPER64).exists(),
        "на Windows 64-битные файлы не нужны"
    );

    service
        .write_config(
            "ghosting-renewed",
            &json!({"liveSplitOverlay": {"enabled": true, "x": 42}}),
        )
        .unwrap();
    let ini = std::fs::read_to_string(prepared.dir.join("overlay.ini")).unwrap();
    assert!(ini.contains("\r\nx=42\r\n"), "{ini:?}");

    // Обновлённый прокси в ресурсах заменяет копию в папке игры.
    let mut v2 = b"v2 ".to_vec();
    v2.extend_from_slice(lso::PROXY_MARKER);
    std::fs::write(service.resource("proxy-d3d9.dll"), &v2).unwrap();
    service
        .prepare(
            "ghosting-renewed",
            &json!({"liveSplitOverlay": {"enabled": true}}),
            Some(&target),
            false,
        )
        .unwrap();
    assert_eq!(std::fs::read(install.join("d3d9.dll")).unwrap(), v2);

    // Оверлей выключили — прокси и файл с путём убраны.
    assert!(
        service
            .prepare(
                "ghosting-renewed",
                &json!({"liveSplitOverlay": {"enabled": false}}),
                Some(&target),
                false,
            )
            .unwrap()
            .is_none()
    );
    assert!(!install.join("d3d9.dll").exists());
    assert!(!install.join(lso::SIDECAR).exists());

    assert!(service.dir_for("../evil").is_none());
    let missing = LiveSplitOverlay::new(OverlayOptions {
        base_dir: Some(world.root.path().to_path_buf()),
        resources_path: Some(world.root.path().join("nowhere")),
        platform: Some(Platform::Win32),
        ..OverlayOptions::default()
    });
    let error = missing
        .prepare(
            "hl1-won",
            &json!({"liveSplitOverlay": {"enabled": true}}),
            Some(&target),
            false,
        )
        .unwrap_err();
    assert!(error.message.contains("не найдены"), "{}", error.message);
}

#[test]
fn selftest_hl1_gets_opengl32_next_to_hl_exe_and_foreign_dll_is_untouched() {
    let world = World::new();
    let service = world.overlay("data", Platform::Win32);
    let install = world.install("Half-Life/hl.exe");
    let goldsrc = Target {
        install_path: &install,
        executable: "Half-Life/hl.exe",
        engine: Some("goldsrc"),
    };
    let settings = json!({"liveSplitOverlay": {"enabled": true}});
    let prepared = service
        .prepare("hl1-won", &settings, Some(&goldsrc), false)
        .unwrap()
        .unwrap();
    assert_eq!(prepared.proxy, "opengl32.dll");
    assert_eq!(prepared.placed, [install.join("Half-Life")]);
    assert!(install.join("Half-Life").join("opengl32.dll").exists());

    // ReShade или DXVK пользователя: файл без нашей метки не затирается и не удаляется.
    let foreign = install.join("d3d9.dll");
    std::fs::write(&foreign, "dxvk").unwrap();
    let source = source_target(&install);
    let placed = service
        .prepare("ghosting-renewed", &settings, Some(&source), false)
        .unwrap()
        .unwrap();
    assert_eq!(placed.placed, [install.join("bin")]);
    assert_eq!(std::fs::read_to_string(&foreign).unwrap(), "dxvk");
    service
        .prepare(
            "ghosting-renewed",
            &json!({"liveSplitOverlay": {"enabled": false}}),
            Some(&source),
            false,
        )
        .unwrap();
    assert_eq!(std::fs::read_to_string(&foreign).unwrap(), "dxvk");
}

#[test]
fn selftest_linux_helper_lives_next_to_data_and_game_gets_wine_override() {
    let world = World::new();
    let service = world.overlay("data", Platform::Linux);
    let install = world.install("hl2.exe");
    let prepared = service
        .prepare(
            "ghosting-renewed",
            &json!({"liveSplitOverlay": {"enabled": true}}),
            Some(&source_target(&install)),
            false,
        )
        .unwrap()
        .unwrap();
    assert_eq!(prepared.dll_override.as_deref(), Some("d3d9=n,b"));
    for name in [lso::HELPER64, lso::SHARE_DLL] {
        assert!(prepared.dir.join(name).exists(), "{name}");
    }
}

/// Путь `Z:\…` строится из пути Linux: на Windows эмуляция дала бы «Z:C:\…», проверять там нечего.
#[cfg(unix)]
#[test]
fn selftest_linux_sidecar_has_data_path_as_wine_sees_it() {
    let world = World::new();
    let service = world.overlay("data", Platform::Linux);
    let install = world.install("hl2.exe");
    service
        .prepare(
            "ghosting-renewed",
            &json!({"liveSplitOverlay": {"enabled": true}}),
            Some(&source_target(&install)),
            false,
        )
        .unwrap();
    let text = std::fs::read_to_string(install.join(lso::SIDECAR)).unwrap();
    assert!(
        text.starts_with("Z:\\") && text.ends_with('\n') && text.lines().count() == 1,
        "{text:?}"
    );
}

#[test]
fn selftest_window_capture_on_windows_needs_no_own_exe() {
    assert!(
        !lso::FILES_WINDOWS.iter().any(|name| name.ends_with(".exe")),
        "в Windows-сборке оверлея нет своих exe"
    );
    let world = World::new();
    let windows = world
        .overlay("w", Platform::Win32)
        .capture_tool()
        .unwrap()
        .unwrap();
    assert_eq!(windows.injector64, None);
    assert_eq!(windows.share_dll, None);
    assert_eq!(
        std::fs::read_dir(&windows.dir).unwrap().count(),
        0,
        "в папку снимка ничего не копируется"
    );
    assert_eq!(windows.snapshot.file_name().unwrap(), "snapshot.bin");
    let linux = world
        .overlay("l", Platform::Linux)
        .capture_tool()
        .unwrap()
        .unwrap();
    assert_eq!(
        linux.injector64.unwrap().file_name().unwrap(),
        lso::HELPER64
    );
    assert_eq!(
        linux.share_dll.unwrap().file_name().unwrap(),
        lso::SHARE_DLL
    );
}

#[test]
fn selftest_legacy_capture_exe_is_removed_from_data() {
    let world = World::new();
    let service = world.overlay("data", Platform::Win32);
    service.remove_legacy_capture();
    assert!(
        !world.root.path().join("data").exists(),
        "папки нет — создавать её незачем"
    );
    let dir = service.dir_for("capture").unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(lso::LEGACY_CAPTURE), "old").unwrap();
    std::fs::write(dir.join("snapshot.bin"), "keep").unwrap();
    service.remove_legacy_capture();
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["snapshot.bin"]);
}

#[tokio::test]
async fn selftest_helper_status_file() {
    let dir = tempfile::tempdir().unwrap();
    let service = LiveSplitOverlay::new(OverlayOptions {
        base_dir: Some(dir.path().to_path_buf()),
        resources_path: Some(dir.path().to_path_buf()),
        ..OverlayOptions::default()
    });
    let file = dir.path().join("inject.status");
    let later = file.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        std::fs::write(later, "0\n").unwrap();
    });
    let options = WaitOptions {
        interval: Some(Duration::from_millis(10)),
        cancelled: None,
    };
    assert_eq!(
        service
            .wait_for_status(&file, Duration::from_secs(30), &options)
            .await,
        Some(0)
    );
    let none = dir.path().join("none");
    assert_eq!(
        service
            .wait_for_status(&none, Duration::from_millis(50), &options)
            .await,
        None
    );
    let cancelled = WaitOptions {
        interval: Some(Duration::from_millis(10)),
        cancelled: Some(&|| true),
    };
    assert_eq!(
        service
            .wait_for_status(&none, Duration::from_secs(30), &cancelled)
            .await,
        None
    );
}
#[test]
fn selftest_overlay_is_off_in_games_livesplit_is_not_opened_for() {
    // livesplit-splits-selftest.js: «оверлей LiveSplit выключен в играх, для которых LiveSplit не открыт».
    let dir = tempfile::tempdir().unwrap();
    for game in ["hl1-won", "hl2-4260"] {
        std::fs::create_dir_all(dir.path().join("livesplit-overlay").join(game)).unwrap();
    }
    let overlay = LiveSplitOverlay::new(OverlayOptions {
        base_dir: Some(dir.path().to_path_buf()),
        ..OverlayOptions::default()
    });
    let settings_of = |_: &str| Some(json!({"liveSplitOverlay": {"enabled": true}}));
    let ini = |game: &str| {
        std::fs::read_to_string(
            dir.path()
                .join("livesplit-overlay")
                .join(game)
                .join("overlay.ini"),
        )
        .unwrap()
    };
    let line = |text: &str, wanted: &str| text.lines().any(|line| line == wanted);

    assert!(overlay.bind_to(Some("hl1-won"), &settings_of));
    assert!(line(&ini("hl1-won"), "enabled=1"));
    assert!(line(&ini("hl2-4260"), "enabled=0"));

    assert!(!overlay.bind_to(Some("hl1-won"), &settings_of));

    // LiveSplit закрыли — оверлей снова во всех играх.
    overlay.bind_to(None, &settings_of);
    assert!(line(&ini("hl2-4260"), "enabled=1"));
}
