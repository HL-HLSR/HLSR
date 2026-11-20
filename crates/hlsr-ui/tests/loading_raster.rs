//! Раскладка и растр стрелок заставки (`LoadingIndicator.vue`): рамки 24 стрелок против
//! `getBoundingClientRect` Chromium (`tools/reference/ui/loading-layout.js` →
//! `tests/fixtures/loading-layout.json`) в окнах разного размера при масштабах 1 и 1,25; привязка
//! к пикселям; картинка стрелки со свечением.

use hlsr_ui::render::triangle::{self, Slot};
use hlsr_ui::{Rgba as Color, Rgba8Image};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/loading-layout.json")).expect("loading-layout.json")
}

/// `up outer outer-18` → стрелка.
fn slot(classes: &str) -> Slot {
    let words: Vec<&str> = classes.split_whitespace().collect();
    Slot {
        down: words[0] == "down",
        outer: words[1] == "outer",
        index: words[2].rsplit('-').next().unwrap().parse().unwrap(),
    }
}

fn px(text: &str) -> f64 {
    text.trim_end_matches("px").parse().unwrap()
}

#[test]
fn layout_matches_getboundingclientrect() {
    let fixture = fixture();
    let variants = fixture["variants"].as_array().unwrap();
    assert!(variants.len() >= 8);
    for variant in variants {
        let window = variant["window"].as_array().unwrap();
        let window = (window[0].as_f64().unwrap(), window[1].as_f64().unwrap());
        let scale = variant["scale"].as_f64().unwrap();
        let arrows = triangle::layout(window, scale);
        let expected = variant["arrows"].as_array().unwrap();
        assert_eq!(arrows.len(), 24);
        assert_eq!(expected.len(), 24);
        for (arrow, want) in arrows.iter().zip(expected) {
            let what = format!("{window:?} @{scale} {}", want["cls"]);
            assert_eq!(
                arrow.slot,
                slot(want["cls"].as_str().unwrap()),
                "{what}: место в анимации"
            );
            let rect = want["rect"].as_array().unwrap();
            let rect: Vec<f64> = rect.iter().map(|v| v.as_f64().unwrap()).collect();
            // getBoundingClientRect отдаёт float32 из 1/64 px.
            for (got, want, name) in [
                (arrow.x, rect[0], "x"),
                (arrow.y, rect[1], "y"),
                (arrow.width, rect[2], "ширина"),
                (arrow.height, rect[3], "высота"),
            ] {
                assert!((got - want).abs() < 0.02, "{what}: {name} {got} ≠ {want}");
            }
            // Ширина границы: целые физические пиксели вниз.
            let border = px(want["border"]["borderLeftWidth"].as_str().unwrap());
            assert!(
                (arrow.width / 2.0 - border).abs() < 0.001,
                "{what}: граница {border}"
            );
            let bottom = px(want["border"]["borderBottomWidth"].as_str().unwrap());
            assert!(
                (arrow.height - bottom).abs() < 0.001,
                "{what}: нижняя граница {bottom}"
            );
        }
    }
}

#[test]
fn snapping_rounds_each_edge_to_physical_pixels() {
    // 1,25: x = 765,2 → 956,5 физ. → 957; правый край 800,4 → 1000,5 → 1001.
    let arrow = triangle::Arrow {
        x: 765.2,
        y: 386.0,
        width: 35.2,
        height: 32.0,
        slot: Slot {
            down: true,
            outer: true,
            index: 17,
        },
    };
    let snapped = arrow.snapped(1.25);
    assert!((snapped.x * 1.25 - 957.0).abs() < 1e-9);
    assert!(((snapped.x + snapped.width) * 1.25 - 1001.0).abs() < 1e-9);
    assert!((snapped.y * 1.25 - 483.0).abs() < 1e-9, "{}", snapped.y);
    // Привязанная рамка целая в физических пикселях.
    assert!((snapped.width * 1.25 - 44.0).abs() < 1e-9);
    assert!((snapped.height * 1.25 - 40.0).abs() < 1e-9);
    // Масштаб 1 с целыми рамками ничего не меняет.
    let whole = triangle::Arrow {
        x: 746.0,
        y: 386.0,
        width: 36.0,
        height: 32.0,
        slot: arrow.slot,
    };
    assert_eq!(whole.snapped(1.0), whole);
}

fn alpha(pixels: &Rgba8Image, x: usize, y: usize) -> u8 {
    pixels.as_slice()[y * pixels.width() as usize + x].a
}

#[test]
fn arrow_raster_is_a_triangle_with_glow() {
    let color = Color::rgb(0, 171, 255);
    let up = triangle::arrow_with_glow(36.0, 32.0, false, 27.0, color, 1.0);
    let pad = (-up.x) as usize;
    assert_eq!(pad, 82, "3σ = 81, +1");
    assert_eq!(up.pixels.width() as usize, 36 + 2 * pad);
    assert_eq!(up.pixels.height() as usize, 32 + 2 * pad);
    assert_eq!(
        (up.width, up.height),
        (up.pixels.width() as f32, up.pixels.height() as f32)
    );
    // Основание внизу — сплошное, вершина вверху — узкая; цвет — акцент.
    let at = |x: usize, y: usize| alpha(&up.pixels, pad + x, pad + y);
    assert_eq!(at(18, 31), 255, "середина основания");
    assert_eq!(at(1, 31), 255, "угол основания внутри треугольника");
    assert!(at(18, 1) > 200, "вершина");
    assert!(
        at(1, 1) < at(18, 1),
        "вне треугольника сверху — только свечение"
    );
    let center = up.pixels.as_slice()[(pad + 31) * up.pixels.width() as usize + pad + 18];
    assert_eq!((center.r, center.g, center.b), (0, 171, 255));
    // Свечение выходит за блок и затухает у края картинки.
    assert!(alpha(&up.pixels, pad - 20, pad + 31) > 0);
    assert!(alpha(&up.pixels, 0, 0) < 4);

    // `.down` — тот же рисунок, перевёрнутый по вертикали.
    let flipped = triangle::arrow_with_glow(36.0, 32.0, true, 27.0, color, 1.0);
    let width = up.pixels.width() as usize;
    for (x, y) in [
        (pad + 18, pad),
        (pad + 5, pad + 3),
        (pad + 30, pad + 20),
        (10, 10),
    ] {
        let mirrored = up.pixels.height() as usize - 1 - y;
        assert_eq!(
            alpha(&flipped.pixels, x, y),
            alpha(&up.pixels, x, mirrored),
            "({x}, {y}) из {width}"
        );
    }
}

#[test]
fn arrow_raster_scales_with_the_window() {
    let color = Color::rgb(0, 171, 255);
    // 1,25: рамка 35,2 × 32 px = 44 × 40 физ.; σ = 27 × 1,25 = 33,75 → поля 103.
    let raster = triangle::arrow_with_glow(35.2, 32.0, false, 27.0, color, 1.25);
    assert_eq!(raster.pixels.width(), 44 + 2 * 103);
    assert_eq!(raster.pixels.height(), 40 + 2 * 103);
    assert!((raster.x + 103.0 / 1.25).abs() < 1e-4);
    // Без цвета (прозрачный) картинки нет.
    let none = triangle::arrow_with_glow(36.0, 32.0, false, 27.0, Color::TRANSPARENT, 1.0);
    assert_eq!(none.width, 0.0);
}
