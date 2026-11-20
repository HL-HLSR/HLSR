use hlsr_ui_build::themes::ThemeTable;
use std::path::{Path, PathBuf};

fn ui(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../hlsr-ui")
        .join(path)
}

fn table() -> ThemeTable {
    ThemeTable::load(&ui("themes.json")).unwrap()
}

#[test]
fn table_has_eight_themes_and_same_keys() {
    let t = table();
    assert_eq!(
        t.order(),
        [
            "hlsr",
            "lambda",
            "resonance",
            "combine",
            "xen",
            "steam2003",
            "midnight",
            "paper"
        ]
    );
    let keys = t.keys("hlsr");
    assert!(keys.len() > 100, "{}", keys.len());
    for id in t.order() {
        assert_eq!(t.keys(id), keys, "{id}");
    }
}

#[test]
fn known_values_from_launcher_themes() {
    let t = table();
    assert_eq!(t.color("lambda", "accent"), Some("#FF7A1CFF".into()));
    assert_eq!(t.color("lambda", "accent-soft"), Some("#FF7A1C29".into())); // rgba(255,122,28,0.16)
    assert_eq!(t.color("hlsr", "accent"), Some("#00ABFFFF".into()));
    assert_eq!(t.color("hlsr", "accent-focus"), Some("#4FC3FFFF".into()));
    // color-mix(in srgb, #FF7A1C 75%, #000)
    assert_eq!(
        t.color("lambda", "btn-progress-bg"),
        Some("#BF5C15FF".into())
    );
    // Каналы r, g, b — цвет без прозрачности.
    assert_eq!(t.color("resonance", "accent-rgb"), Some("#2FCF6CFF".into()));
    // Нет в vars темы hlsr: запасное значение var(--btn-ready-text, #fff).
    assert_eq!(t.color("hlsr", "btn-ready-text"), Some("#FFFFFFFF".into()));
    assert_eq!(
        t.color("midnight", "l-switch-thumb-on"),
        Some("#0A0B0DFF".into())
    );
    // Paper — с акцентом по умолчанию clay (0.4.9).
    assert_eq!(t.color("paper", "accent"), Some("#D5876DFF".into()));
    assert_eq!(t.length("paper", "l-radius"), Some(12.0));
    assert_eq!(t.length("hlsr", "l-radius"), Some(12.0));
}

#[test]
fn values_slint_cannot_hold_are_listed_apart() {
    let t = table();
    for key in ["l-bg-scrim", "l-shadow-pop", "l-footer-lead-in", "l-fast"] {
        assert!(t.unsupported().iter().any(|(name, _)| name == key), "{key}");
        assert!(!t.keys("hlsr").contains(&key), "{key}");
    }
}

#[test]
fn layout_steps_follow_window_size() {
    let t = table();
    let at = |w: f32, h: f32, key: &str| {
        t.layout_for(w, h)
            .into_iter()
            .find(|(name, _)| *name == key)
            .map(|(_, px)| px)
    };
    // 1152x648: max-height 663 и max-width 1295.
    assert_eq!(at(1152.0, 648.0, "l-rail-item"), Some(48.0));
    assert_eq!(at(1152.0, 648.0, "l-hero-top"), Some(184.0));
    assert_eq!(at(1152.0, 648.0, "l-header-gap"), Some(24.0));
    // 1200x676 (FullHD): только узкое окно.
    assert_eq!(at(1200.0, 676.0, "l-rail-item"), Some(56.0));
    assert_eq!(at(1200.0, 676.0, "l-topbar-h"), Some(80.0));
    assert_eq!(at(1200.0, 676.0, "l-header-gap"), Some(24.0));
    // 1280x720: min-height 708.
    assert_eq!(at(1280.0, 720.0, "l-hero-top"), Some(208.0));
    assert_eq!(at(1280.0, 720.0, "l-rail-tool-gap"), Some(8.0));
    // 1600x900: все ступени по высоте; --l-header-inset пересчитан от 48px.
    assert_eq!(at(1600.0, 900.0, "l-topbar-h"), Some(96.0));
    assert_eq!(at(1600.0, 900.0, "l-header-gap"), Some(32.0));
    assert_eq!(at(1600.0, 900.0, "l-header-inset"), Some(12.0));
    // Дробная высота между ступенями — без ступени, как @media.
    assert_eq!(at(1400.0, 663.5, "l-rail-item"), Some(56.0));
    // Переменная вне ступеней в списке не появляется.
    assert_eq!(at(1600.0, 900.0, "accent"), None);
}

#[test]
fn broken_tables_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("themes.json");
    let base = |hlsr: &str, other: &str| {
        format!(
            r#"{{"order":["hlsr","lambda"],"themes":{{"hlsr":{hlsr},"lambda":{other}}},"unsupported":{{}},"scoped":{{}},"layout":{{"conditions":[],"cases":[]}}}}"#
        )
    };

    std::fs::write(
        &path,
        base(
            r##"{"a":{"color":"#000000FF"}}"##,
            r##"{"a":{"color":"#000000FF"}}"##,
        ),
    )
    .unwrap();
    let t = ThemeTable::load(&path).unwrap();
    assert_eq!(t.keys("lambda"), ["a"]);

    // Ключи тем различаются.
    std::fs::write(
        &path,
        base(
            r##"{"a":{"color":"#000000FF"}}"##,
            r##"{"b":{"color":"#000000FF"}}"##,
        ),
    )
    .unwrap();
    assert!(ThemeTable::load(&path).is_err());
    // Неизвестный тип значения.
    std::fs::write(
        &path,
        base(
            r##"{"a":{"shadow":"0 0 1px"}}"##,
            r##"{"a":{"shadow":"0 0 1px"}}"##,
        ),
    )
    .unwrap();
    assert!(ThemeTable::load(&path).is_err());
    // Тип различается между темами.
    std::fs::write(
        &path,
        base(
            r##"{"a":{"color":"#000000FF"}}"##,
            r##"{"a":{"length":1}}"##,
        ),
    )
    .unwrap();
    assert!(ThemeTable::load(&path).is_err());
    // Цвет не #RRGGBBAA.
    std::fs::write(
        &path,
        base(r##"{"a":{"color":"#000"}}"##, r##"{"a":{"color":"#000"}}"##),
    )
    .unwrap();
    assert!(ThemeTable::load(&path).is_err());
    // Градиент меняется с темой, но точки в разных местах.
    let g = |pos: f64| {
        format!(
            r##"{{"a":{{"gradient":{{"angle":180,"stops":[{{"color":"#000000FF","pos":0}},{{"color":"#FFFFFFFF","pos":{pos}}}]}}}}}}"##
        )
    };
    std::fs::write(&path, base(&g(1.0), &g(0.5))).unwrap();
    assert!(ThemeTable::load(&path).is_err());
}
