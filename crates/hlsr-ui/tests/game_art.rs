//! Арт игр и раскладка переднего плана (`launcher-data.js` 0.4.7) против выгрузки Node
//! (`tools/reference/ui/launcher-data.js` → `tests/fixtures/launcher-data.json`): имена файлов,
//! раскладки в cqw/cqh, размеры вшитых файлов.

use hlsr_ui::game_art::{self, FrontLayout, GameArt};
use hlsr_ui::images::{self, ImageSource};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/launcher-data.json")).expect("launcher-data.json")
}

/// Значение CSS из раскладки 0.4.7 в коэффициентах: доли `(percent, cqw, cqh)`. Понимает
/// `calc(...)` из `+`/`-`, `%`, `cqw`, `cqh` и `var(--front-bleed-x)` (12cqw).
fn coefficients(text: &str) -> (f64, f64, f64) {
    let inner = text
        .trim()
        .strip_prefix("calc(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(text.trim());
    let mut sum = (0.0, 0.0, 0.0);
    let mut sign = 1.0;
    // `var(--front-bleed-x)` содержит пробел-разделитель только снаружи; склеиваем скобки.
    let flat = inner.replace("var(--front-bleed-x)", "12cqw");
    for token in flat.split_whitespace() {
        match token {
            "+" => sign = 1.0,
            "-" => sign = -1.0,
            term => {
                let (number, unit) = term.split_at(
                    term.find(|c: char| c.is_ascii_alphabetic() || c == '%')
                        .unwrap(),
                );
                let value = sign * number.parse::<f64>().unwrap();
                match unit {
                    "%" => sum.0 += value / 100.0,
                    "cqw" => sum.1 += value,
                    "cqh" => sum.2 += value,
                    other => panic!("единица {other} в «{text}»"),
                }
                sign = 1.0;
            }
        }
    }
    sum
}

fn cq(text: &str, unit: &str) -> f64 {
    text.trim().strip_suffix(unit).unwrap().parse().unwrap()
}

#[test]
fn layouts_match_launcher_data() {
    let fixture = fixture();
    let art = &fixture["gameArt"];
    for id in ["70", "218", "220", "380", "420"] {
        let expected = &art[id];
        let game = GameArt::of(id).unwrap_or_else(|| panic!("игры {id} нет в таблице"));
        assert_eq!(game.hero, expected["hero"].as_str().unwrap(), "{id}: фон");
        assert_eq!(
            game.front,
            expected["heroFront"].as_str(),
            "{id}: передний план"
        );
        let layout = &expected["heroFrontLayout"];
        let defaults = FrontLayout::DEFAULT;
        // Без раскладки в 0.4.7 действуют умолчания .launcher-bg__front (LauncherBackground.vue:297-298).
        let (size, pos, origin_x, origin_y) = if layout.is_null() {
            ("auto 195cqh", "calc(50% + 20cqw) 3cqh", "72cqw", "85cqh")
        } else {
            (
                layout["--front-size"].as_str().unwrap(),
                layout["--front-pos"].as_str().unwrap(),
                layout["--front-origin-x"].as_str().unwrap(),
                layout["--front-origin-y"].as_str().unwrap(),
            )
        };
        if layout.is_null() {
            assert_eq!(game.layout, defaults, "{id}: умолчания");
        }
        let height = cq(size.strip_prefix("auto ").unwrap(), "cqh");
        let (x_text, y) = pos.rsplit_once(' ').unwrap();
        let (x_percent, x_cqw, x_cqh) = coefficients(x_text);
        assert_eq!(game.layout.height_cqh, height, "{id}: высота");
        assert_eq!(
            (game.layout.x_percent, game.layout.x_cqw, game.layout.x_cqh),
            (x_percent, x_cqw, x_cqh),
            "{id}: положение по x"
        );
        assert_eq!(game.layout.y_cqh, cq(y, "cqh"), "{id}: положение по y");
        assert_eq!(
            game.layout.origin_x_cqw,
            cq(origin_x, "cqw"),
            "{id}: центр поворота x"
        );
        assert_eq!(
            game.layout.origin_y_cqh,
            cq(origin_y, "cqh"),
            "{id}: центр поворота y"
        );
    }
}

#[test]
fn hidden_games_without_art_have_none() {
    // Игры 50 и 130 скрыты (shared/game-availability.js), их фоны (1,4 МБ) не вшиваются.
    let fixture = fixture();
    for id in fixture["availability"]["hidden"].as_array().unwrap() {
        let id = id.as_str().unwrap();
        if matches!(id, "50" | "130") {
            assert!(GameArt::of(id).is_none(), "{id}");
        }
    }
    assert!(GameArt::of("999").is_none());
}

#[test]
fn startup_game_has_the_default_layout_art() {
    let fixture = fixture();
    let default = fixture["availability"]["default"].as_str().unwrap();
    let art = GameArt::of(default).unwrap();
    assert_eq!(art.hero, "launcher/hl2-ghosting-bg.png");
    assert_eq!(art.front, Some("launcher/hl2-ghosting-front.webp"));
}

#[test]
fn bundled_files_are_the_files_of_0_4_7() {
    let fixture = fixture();
    let sizes = fixture["sizes"].as_object().unwrap();
    game_art::register_bundles();
    for id in ["70", "218", "220"] {
        let art = GameArt::of(id).unwrap();
        for name in [Some(art.hero), art.front].into_iter().flatten() {
            let bytes = game_art::bytes(name).unwrap_or_else(|| panic!("{name} не вшит"));
            assert_eq!(bytes.len() as u64, sizes[name].as_u64().unwrap(), "{name}");
            // Файл сборки открывается загрузчиком: тот же формат, что у Chromium (PNG, JPEG, WebP).
            let decoded = images::decode(bytes, None, images::Fit::Contain)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(decoded.width > 0 && decoded.height > 0, "{name}");
            assert!(
                matches!(
                    ImageSource::parse(&format!("bundle:{name}")),
                    Some(ImageSource::Bundle(_))
                ),
                "{name}"
            );
        }
    }
}

#[test]
fn art_has_the_aspect_the_layouts_assume() {
    // Раскладки рассчитаны под кадры 16:9 (HL2) и вырезки в рост окна: размеры вырезок — по
    // файлам (G9: 2172×2896 у Ghosting, 2560×1440 у HL2).
    game_art::register_bundles();
    let size = |name: &str| {
        let decoded =
            images::decode(game_art::bytes(name).unwrap(), None, images::Fit::Contain).unwrap();
        (decoded.width, decoded.height)
    };
    assert_eq!(size("launcher/hero-hl2.jpg"), (2560, 1440));
    assert_eq!(size("launcher/hl2-front.webp"), (2560, 1440));
    assert_eq!(size("launcher/hl2-ghosting-front.webp"), (2172, 2896));
}

#[test]
fn place_follows_css_background_position() {
    // HL2 в окне 1600×900: `auto 100cqh`, `calc(var(--front-bleed-x) + 106cqw - 178cqh) 9cqh`
    // от рамки слоя, начинающейся на 12cqw левее окна: x = 106·16 − 178·9 = 94, y = 81.
    let hl2 = GameArt::of("220").unwrap().layout;
    let (x, y, width, height) = hl2.place((1600.0, 900.0), 16.0 / 9.0);
    assert_eq!((x, y, width, height), (94.0, 81.0, 1600.0, 900.0));
    // Ghosting (умолчания): `auto 195cqh`, `calc(50% + 20cqw) 3cqh`; 50 % — от свободного места
    // рамки (окно + 2 × 12cqw).
    let ghosting = GameArt::of("218").unwrap().layout;
    let aspect = 2172.0 / 2896.0;
    let (x, y, width, height) = ghosting.place((1600.0, 900.0), aspect);
    assert_eq!(height, 1755.0);
    assert!((width - 1316.25).abs() < 1e-9);
    assert!((x - (-192.0 + 0.5 * (1984.0 - 1316.25) + 320.0)).abs() < 1e-9);
    assert_eq!(y, 27.0);
    // Размеры от окна: при окне 1200×676 всё масштабируется по cqw и cqh.
    let hl1 = GameArt::of("70").unwrap().layout;
    let (x, y, width, height) = hl1.place((1200.0, 676.0), 1.0);
    assert!((height - 1.7 * 676.0).abs() < 1e-9);
    assert!((width - height).abs() < 1e-9);
    assert!((x - (-144.0 + 0.5 * (1488.0 - width) + 22.0 * 12.0)).abs() < 1e-9);
    assert!((y - 67.6).abs() < 1e-9);
}
