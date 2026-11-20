//! Значки FontAwesome: таблица имён против данных 0.4.7 и разрешение классов (коробка значка и
//! анимации кольца — у вида).

use hlsr_ui::fa::{Glyph, Style, table};
use serde::Deserialize;

fn glyph(classes: &str) -> Glyph {
    table()
        .resolve(classes)
        .unwrap_or_else(|| panic!("нет значка «{classes}»"))
}

#[test]
fn classes_resolve_like_all_css() {
    // fas / far / fab / fa-solid… задают начертание, имя — кодовую точку.
    assert_eq!(glyph("fas fa-play").ch, '\u{f04b}');
    assert_eq!(glyph("fas fa-play").style, Style::Solid);
    assert_eq!(glyph("far fa-circle").style, Style::Regular);
    assert_eq!(glyph("far fa-circle").ch, glyph("fas fa-circle").ch);
    assert_eq!(glyph("fab fa-discord").style, Style::Brands);
    assert_eq!(glyph("fa-brands fa-discord").ch, glyph("fab fa-discord").ch);
    // Без префикса — solid; порядок классов не важен.
    assert_eq!(glyph("fa-play").style, Style::Solid);
    assert_eq!(glyph("fa-play fas"), glyph("fas fa-play"));
    // Псевдонимы v5 из кода 0.4.7: fa-times, fa-sync-alt, fa-dot-circle…
    assert_eq!(glyph("fas fa-times").ch, glyph("fas fa-xmark").ch);
    assert_eq!(glyph("fas fa-sync-alt").ch, glyph("fas fa-rotate").ch);
    assert_eq!(glyph("fas fa-dot-circle").ch, glyph("fas fa-circle-dot").ch);
    // Размерные и служебные классы глиф не меняют.
    assert_eq!(glyph("fas fa-cog fa-fw fa-lg"), glyph("fas fa-cog"));
    // Цифры: fa-0…fa-9 — отдельные значки, не размеры.
    assert_eq!(glyph("fas fa-1").ch, '1');
    // Нет имени — нет значка; пустая строка тоже.
    assert!(table().resolve("fas fa-no-such-icon-x").is_none());
    assert!(table().resolve("").is_none());
    assert!(table().resolve("fas").is_none());
}

#[test]
fn spin_replaces_the_glyph_with_the_ring() {
    let spin = glyph("fas fa-spinner fa-spin");
    assert!(spin.spin);
    // Имя при этом не нужно: кольцо не зависит от значка.
    assert!(table().resolve("fas fa-spin").unwrap().spin);
    assert!(!glyph("fas fa-spinner").spin);
}

#[test]
fn font_families_are_those_of_the_font_awesome_package() {
    assert_eq!(Style::Solid.family(), "Font Awesome 7 Free");
    assert_eq!((Style::Solid.weight(), Style::Regular.weight()), (900, 400));
    assert_eq!(Style::Regular.family(), "Font Awesome 7 Free");
    assert_eq!(Style::Brands.family(), "Font Awesome 7 Brands");
    assert_eq!(Style::Brands.weight(), 400);
}

/// `tests/fixtures/fa-used.json` — `tools/reference/export-fa-used.js`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Used {
    generator: String,
    classes: Vec<UsedClasses>,
    names: Vec<UsedName>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsedClasses {
    prefix: String,
    name: String,
    sources: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsedName {
    name: String,
    sources: Vec<String>,
}

fn used() -> Used {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fa-used.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

#[test]
fn every_icon_name_used_by_0_4_7_resolves_in_its_style() {
    let used = used();
    assert_eq!(used.generator, "tools/reference/export-fa-used.js");
    // Выгрузка не пуста: ~115 имён рендерера и 18 из main (карта §6).
    assert!(used.classes.len() > 100, "{}", used.classes.len());
    assert!(used.names.len() > 20, "{}", used.names.len());
    let table = table();
    for entry in &used.classes {
        let classes = format!("{} fa-{}", entry.prefix, entry.name);
        let glyph = table
            .resolve(&classes)
            .unwrap_or_else(|| panic!("«{classes}» ({}) нет в таблице", entry.sources.join(", ")));
        // Начертание, которого просит префикс, есть у значка (иначе Chromium
        // нарисовал бы пустой квадрат): по cmap шрифтов — fa.json icons[].styles.
        assert!(
            table.has_style(&entry.name, glyph.style),
            "«{classes}» ({}): значка нет в начертании {:?}",
            entry.sources.join(", "),
            glyph.style
        );
    }
    // Имена без префикса в строке (префикс дописывает код): хотя бы есть.
    for entry in &used.names {
        assert!(
            table.code_point(&entry.name).is_some(),
            "fa-{} ({}) нет в таблице",
            entry.name,
            entry.sources.join(", ")
        );
    }
}
