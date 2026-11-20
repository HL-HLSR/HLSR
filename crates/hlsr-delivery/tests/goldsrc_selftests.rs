//! Самотесты 0.4.7 о кастомизации HL1 с теми же случаями: `scripts/hl1-selftest.js` (строки
//! 984–1200: cfg BXT, userconfig.cfg лаунчера, паки и BXT в окне) и
//! `scripts/hl1-pack-preview-selftest.js` (раскладка сцены и фрагмент прицела).
//!
//! Не здесь: два теста предпросмотра, которые гоняют `hl1-overlay-renderer.js` (Vue, подпроект 5). Паки с сервером и
//! 7za — `goldsrc_packs_selftests.rs`; тесты `applyToInstall` вместе с биндами и импортом —
//! `cfg_install_selftests.rs` и `config_selftests.rs`.

mod support;

use hlsr_core::js;
use hlsr_delivery::bind_presets::HL1_RUN_ALIAS;
use hlsr_delivery::cfg_search::{CfgSearch, SteamLanguage};
use hlsr_delivery::goldsrc_personalization::{self as gp, Applied};
use hlsr_delivery::goldsrc_settings as gs;
use hlsr_delivery::hl1_default_userconfig as userconfig;
use hlsr_delivery::hl1_pack_preview as preview;
use hlsr_delivery::launch_args;
use hlsr_delivery::manifests;
use hlsr_delivery::mutable_paths;
use hlsr_delivery::ports::FakeRegistry;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use support::{fixture, items, str_of};

fn search() -> CfgSearch {
    CfgSearch::new(SteamLanguage::new(Arc::new(FakeRegistry::default())))
}

fn temp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    (dir, root)
}

fn write(root: &Path, relative: &str, content: &str) {
    let file = root.join(relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, content).unwrap();
}

/// `fakeInstall`: файлы установки HL1, которые лаунчер узнаёт.
fn fake_install(root: &Path) {
    write(root, "Half-Life/hl.exe", "MZ");
    write(
        root,
        "Half-Life/valve_WON/liblist.gam",
        "game \"Half-Life\"\n",
    );
    write(root, "Half-Life/steam_appid.txt", "220");
    write(root, "Bunnymod XT/Injector.exe", "MZ");
}

fn read_latin1(root: &Path, relative: &str) -> String {
    js::latin1_string(&std::fs::read(root.join(relative)).unwrap())
}

fn read_utf8(root: &Path, relative: &str) -> String {
    String::from_utf8(std::fs::read(root.join(relative)).unwrap()).unwrap()
}

fn config() -> &'static manifests::LaunchConfig {
    manifests::launch_config_of("hl1-won").unwrap()
}

fn apply(root: &Path, settings: &Value) -> Applied {
    gp::apply_to_install(&search(), root, config(), settings).unwrap()
}

/// `cfgValues(text)`: `команда "значение"` → значение.
fn cfg_values(text: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for line in text.split("\r\n") {
        let Some((name, rest)) = line.split_once(' ') else {
            continue;
        };
        if name.is_empty() || name.contains(char::is_whitespace) {
            continue;
        }
        if let Some(value) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"'))
            && !value.contains('"')
        {
            values.insert(name.to_owned(), value.to_owned());
        }
    }
    values
}

/// Разрешённые правилами HL1 на speedrun.com команды, которые пишет лаунчер.
const HL1_WHITELIST: &[&str] = &[
    "bxt_hud_game_color",
    "bxt_hud_game_alpha",
    "bxt_hud_color",
    "crosshair",
    "bxt_cross",
    "bxt_cross_color",
    "bxt_cross_alpha",
    "bxt_cross_thickness",
    "bxt_cross_size",
    "bxt_cross_gap",
    "bxt_cross_outline",
    "bxt_cross_circle_radius",
    "bxt_cross_dot_color",
    "bxt_cross_dot_size",
    "bxt_cross_top_line",
    "bxt_cross_bottom_line",
    "bxt_cross_left_line",
    "bxt_cross_right_line",
    "bxt_hud_timer",
    "bxt_hud_speedometer",
    "bxt_hud_jumpspeed",
    "bxt_hud_viewangles",
    "default_fov",
    "bxt_fix_widescreen_fov",
    "bxt_viewmodel_fov",
    "bxt_viewmodel_semitransparent",
    // В whitelist GoldSrc без обязательного значения, то есть любые значения.
    "bxt_hud_timer_anchor",
    "bxt_hud_speedometer_anchor",
    "bxt_hud_jumpspeed_anchor",
    "bxt_hud_viewangles_anchor",
    "bxt_hud_jumpdistance",
    "bxt_hud_precision",
    "bxt_hud_incorrect_fps_indicator",
    "bxt_remove_viewmodel",
    "bxt_viewmodel_ofs_forward",
    "bxt_viewmodel_ofs_right",
    "bxt_viewmodel_ofs_up",
    "bxt_viewmodel_bob_angled",
    "bxt_viewmodel_disable_idle",
    "bxt_viewmodel_disable_equip",
];

#[test]
fn bxt_cfg_writes_only_allowed_commands_defaults_are_as_in_the_game() {
    let values = cfg_values(&gp::to_cfg_lines(&json!({})));
    let allowed: HashSet<&str> = HL1_WHITELIST.iter().copied().collect();
    for name in values.keys() {
        assert!(allowed.contains(name.as_str()), "{name} нет в белом списке");
    }
    assert_eq!(values.len(), allowed.len());
    // Умолчания BXT из cvars.hpp: пустой цвет и прозрачность 0 — цвет игры.
    assert_eq!(values["bxt_hud_game_color"], "");
    assert_eq!(values["bxt_hud_game_alpha"], "0");
    assert_eq!(values["crosshair"], "1");
    assert_eq!(values["bxt_cross"], "0");
    assert_eq!(values["bxt_hud_speedometer"], "1");
    assert_eq!(values["default_fov"], "90");
    // Положение панелей по умолчанию — как в BXT (cvars.hpp).
    assert_eq!(values["bxt_hud_speedometer_anchor"], "0.5 1");
    assert_eq!(values["bxt_hud_timer_anchor"], "0 0.5");
    assert_eq!(values["bxt_hud_viewangles_anchor"], "1 0");
    assert_eq!(values["bxt_hud_precision"], "6");
    assert_eq!(values["bxt_hud_incorrect_fps_indicator"], "1");
}

#[test]
fn speedometer_position_and_weapon_offset_become_bxt_commands() {
    let values = cfg_values(&gp::to_cfg_lines(&json!({
        "bxt": {"speedometerX": 25, "speedometerY": 60, "viewmodelUp": -3, "removeViewmodel": true, "precision": 99},
    })));
    assert_eq!(values["bxt_hud_speedometer_anchor"], "0.25 0.6");
    assert_eq!(values["bxt_viewmodel_ofs_up"], "-3");
    assert_eq!(values["bxt_remove_viewmodel"], "1");
    // Значения за пределами поля обрезаются: BXT больше 16 знаков не показывает.
    assert_eq!(values["bxt_hud_precision"], "16");
    // Положение — только у включённой панели: у выключенной поле скрыто.
    let field = gs::field("jumpspeedX").unwrap();
    assert!(!gs::available(field, &json!({"jumpspeed": false})));
    assert!(gs::available(field, &json!({"jumpspeed": true})));
}

#[test]
fn hud_colour_bxt_crosshair_and_panels_become_bxt_commands() {
    let values = cfg_values(&gp::to_cfg_lines(&json!({
        "personalization": {
            "hudEnabled": true, "hudColor": {"r": 0, "g": 200, "b": 255}, "hudAlpha": 200, "hudAffectsCrosshair": true,
            "crossEnabled": true, "crossColor": {"r": 255, "g": 0, "b": 0}, "crossOutline": true, "crossOutlineThickness": 2,
            "crossHideDefault": true,
        },
        "bxt": {"timer": true, "speedometer": false, "fov": 300, "viewmodelFov": 90},
    })));
    // Прозрачность 200 внесена в цвет: со своим цветом BXT её не учитывает.
    assert_eq!(values["bxt_hud_game_color"], "0 157 200");
    assert_eq!(values["bxt_hud_color"], "0 157 200");
    assert_eq!(values["bxt_hud_game_alpha"], "0");
    assert_eq!(
        values["bxt_cross_color"], "0 200 255",
        "цвет прицела как у HUD"
    );
    assert_eq!(
        values["bxt_cross_outline"], "2",
        "обводка в BXT — толщина линии"
    );
    assert_eq!(values["crosshair"], "0");
    assert_eq!(values["bxt_hud_timer"], "1");
    assert_eq!(values["bxt_hud_speedometer"], "0");
    assert_eq!(values["default_fov"], "120", "поле зрения ограничено");
    assert_eq!(values["bxt_viewmodel_fov"], "90");
}

#[test]
fn cfg_is_connected_from_userconfig_once_the_players_file_is_not_touched() {
    let (_dir, root) = temp();
    fake_install(&root);
    write(
        &root,
        "Half-Life/valve_WON/userconfig.cfg",
        "bind p \"run\"\r\nbxt_hud_incorrect_fps_indicator 0",
    );

    let first = apply(&root, &json!({"bxt": {"timer": true}}));
    let second = apply(&root, &json!({}));
    let Applied::Applied {
        appended: first, ..
    } = first
    else {
        panic!("{first:?}");
    };
    let Applied::Applied {
        appended: second, ..
    } = second
    else {
        panic!("{second:?}");
    };
    assert!(first);
    assert!(!second);
    assert_eq!(
        read_latin1(&root, "Half-Life/valve_WON/userconfig.cfg"),
        "bind p \"run\"\r\nbxt_hud_incorrect_fps_indicator 0\r\nexec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n"
    );
    let written = read_latin1(&root, "Half-Life/valve_WON/hlsr-personalization.cfg");
    assert_eq!(
        cfg_values(&written)["bxt_hud_timer"],
        "0",
        "cfg переписывается целиком при каждом запуске"
    );
    assert!(mutable_paths::matches_mutable(
        "Half-Life/valve_WON/hlsr-personalization.cfg"
    ));
}

/// Стандартный userconfig.cfg лаунчера 0.4.0: комментарии были на русском.
fn ru_040_userconfig() -> String {
    [
        "exec WON.cfg // Не удалять: fps_max 100 и clockwindow 0 как в оригинальном движке, без них ран не засчитают".to_owned(),
        String::new(),
        "// Обязательно по правилам speedrun.com".to_owned(),
        "_bxt_save_runtime_data_in_demos 1 // Данные BXT в демках, по ним модераторы проверяют ран".to_owned(),
        "cl_showfps 1 // Счётчик FPS в углу экрана, обязательно включён".to_owned(),
        String::new(),
        "// Старт рана: сложность, первая карта, таймер BXT и запись демок run_1, run_2 и дальше".to_owned(),
        "// Сложность во время рана менять нельзя, поэтому она задаётся здесь, до старта".to_owned(),
        format!("alias run \"{HL1_RUN_ALIAS}\""),
        "bind p \"run\" // Начать ран заново".to_owned(),
        String::new(),
        "// После финиша запись демок останавливает команда stop в консоли".to_owned(),
        "// Демки сдаются вместе с раном, храни демки своего рекорда".to_owned(),
        String::new(),
        "bxt_hud_incorrect_fps_indicator 0".to_owned(),
        String::new(),
        "// Свои бинды и alias пиши ниже. Настройки игры удобнее менять в лаунчере:".to_owned(),
        "// Настройки игры, вкладка «Конфиги»".to_owned(),
        String::new(),
    ]
    .join("\r\n")
}

fn ensure_default(root: &Path) -> bool {
    gp::ensure_default_userconfig(&search(), root, config().game_directory).unwrap()
}

#[test]
fn stock_userconfig_is_replaced_by_the_default_launcher_exec_lines_stay_at_the_end() {
    let (_dir, root) = temp();
    fake_install(&root);
    write(
        &root,
        "Half-Life/valve_WON/userconfig.cfg",
        &format!(
            "{}\r\nexec hlsr-binds.cfg\r\n",
            userconfig::stock().replace('\n', "\r\n")
        ),
    );

    apply(&root, &json!({}));
    let text = read_utf8(&root, "Half-Life/valve_WON/userconfig.cfg");
    assert!(text.starts_with(userconfig::text()));
    assert_eq!(
        &text[userconfig::text().len()..],
        "exec hlsr-binds.cfg\r\nexec hlsr-personalization.cfg\r\n"
    );
    assert!(!ensure_default(&root), "второй раз не переписывается");
}

#[test]
fn default_userconfig_of_the_previous_launcher_version_is_updated() {
    let (_dir, root) = temp();
    fake_install(&root);
    let ru = ru_040_userconfig();
    let previous: Vec<&str> = ru
        .split("\r\n")
        .filter(|line| !line.starts_with("cl_showfps "))
        .collect();
    write(
        &root,
        "Half-Life/valve_WON/userconfig.cfg",
        &format!(
            "{}exec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n",
            previous.join("\r\n")
        ),
    );

    assert!(ensure_default(&root));
    assert_eq!(
        read_utf8(&root, "Half-Life/valve_WON/userconfig.cfg"),
        format!(
            "{}exec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n",
            userconfig::text()
        )
    );
}

#[test]
fn default_userconfig_of_0_4_0_with_russian_comments_is_replaced() {
    let (_dir, root) = temp();
    fake_install(&root);
    std::fs::write(
        root.join("Half-Life/valve_WON/userconfig.cfg"),
        format!(
            "{}\r\nexec hlsr-binds.cfg\r\nexec hlsr-personalization.cfg\r\n",
            ru_040_userconfig()
        ),
    )
    .unwrap();

    assert!(ensure_default(&root));
    assert_eq!(
        read_utf8(&root, "Half-Life/valve_WON/userconfig.cfg"),
        format!(
            "{}exec hlsr-binds.cfg\r\nexec hlsr-personalization.cfg\r\n",
            userconfig::text()
        )
    );
}

#[test]
fn default_userconfig_follows_the_won_rules_without_semicolons_in_comments() {
    let text = userconfig::text();
    let lines: Vec<&str> = text.split("\r\n").collect();
    assert!(
        lines[0].starts_with("exec WON.cfg"),
        "WON.cfg первой строкой"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("_bxt_save_runtime_data_in_demos 1")),
        "данные BXT в демках"
    );
    assert!(
        text.bytes()
            .all(|b| matches!(b, b'\t' | b'\r' | b'\n' | 0x20..=0x7e)),
        "только ASCII, без русских комментариев"
    );
    assert!(
        lines.iter().any(|line| line.starts_with("cl_showfps 1")),
        "счётчик FPS включён"
    );
    assert!(lines.contains(&format!("alias run \"{HL1_RUN_ALIAS}\"").as_str()));
    for line in &lines {
        if let Some(comment) = line.find("//") {
            assert!(!line[comment..].contains(';'), "{line}");
        }
    }
}

#[test]
fn missing_userconfig_is_created_and_the_players_own_stays() {
    let (_dir, root) = temp();
    fake_install(&root);
    assert!(ensure_default(&root));
    assert_eq!(
        read_utf8(&root, "Half-Life/valve_WON/userconfig.cfg"),
        userconfig::text()
    );

    let own = format!(
        "{}\r\nbind f \"+duck\"\r\n",
        userconfig::stock().replace('\n', "\r\n")
    );
    std::fs::write(
        root.join("Half-Life/valve_WON/userconfig.cfg"),
        js::latin1_bytes(&own),
    )
    .unwrap();
    assert!(!ensure_default(&root));
    assert_eq!(
        read_latin1(&root, "Half-Life/valve_WON/userconfig.cfg"),
        own
    );
}

#[test]
fn a_source_game_gets_no_bxt_cfg() {
    let (_dir, root) = temp();
    let ghosting = manifests::launch_config_of("ghosting-renewed").unwrap();
    let result = gp::apply_to_install(&search(), &root, ghosting, &json!({})).unwrap();
    assert_eq!(result, Applied::Skipped);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn packs_and_bxt_are_one_list_in_the_window_and_saved_separately() {
    let flat = gs::to_flat(
        &json!({"hud": "ag-hud", "crosshair": "crosshair-changer", "changerShape": "1px", "changerColor": "red"}),
        &json!({"timer": true, "fov": 100}),
    );
    let back = gs::from_flat(&flat.to_value());
    assert_eq!(
        back.packs.to_value(),
        json!({"hud": "ag-hud", "crosshair": "crosshair-changer", "changerShape": "1px", "changerColor": "red"})
    );
    assert_eq!(back.bxt.to_value()["timer"], json!(true));
    assert_eq!(back.bxt.to_value()["fov"], json!(100));

    let shape = gs::field("changerShape").unwrap();
    assert!(gs::available(
        shape,
        &json!({"packCrosshair": "crosshair-changer"})
    ));
    assert!(!gs::available(
        shape,
        &json!({"packCrosshair": "execut4ble-crosshair"})
    ));

    // Лишние поля bxt при сохранении отбрасываются (`launchArgs.sanitizeSettings`).
    let mut with_bogus = back.bxt.to_value();
    with_bogus["bogus"] = json!(1);
    let saved =
        launch_args::sanitize_settings(&json!({"packs": back.packs.to_value(), "bxt": with_bogus}))
            .settings
            .to_value();
    assert_eq!(saved["packs"], back.packs.to_value());
    assert_eq!(saved["bxt"], back.bxt.to_value());
}

// ----------------------------------------------------- hl1-pack-preview-selftest.js

fn preview_fixture() -> &'static Value {
    static FIXTURE: std::sync::LazyLock<Value> =
        std::sync::LazyLock::new(|| fixture("goldsrc-settings.json")["preview"].clone());
    &FIXTURE
}

fn meta() -> preview::PackMeta {
    let meta = &preview_fixture()["meta"];
    let strings = |list: &Value| -> Vec<String> {
        items(list).iter().map(|s| str_of(s).to_owned()).collect()
    };
    preview::PackMeta {
        shapes: strings(&meta["shapes"]),
        colors: strings(&meta["colors"]),
        changer_cell: meta["changerCell"].as_f64().unwrap(),
        changer_inset: meta["changerInset"].as_f64().unwrap(),
        changer_size: meta["changerSize"].as_f64().unwrap(),
    }
}

#[test]
fn every_installer_shape_and_colour_has_its_own_fragment_of_the_real_sprite() {
    let meta = meta();
    let fixture_meta = &preview_fixture()["meta"];
    assert_eq!(json!(meta.shapes), json!(gs::CHANGER_SHAPES));
    assert_eq!(json!(meta.colors), json!(gs::CHANGER_COLORS));
    assert_eq!(&fixture_meta["shapes"], &json!(gs::CHANGER_SHAPES));
    let mut tiles = HashSet::new();
    for shape in gs::CHANGER_SHAPES {
        for color in gs::CHANGER_COLORS {
            let layer = preview::crosshair_layer(
                &json!({"packCrosshair": "crosshair-changer", "changerShape": shape, "changerColor": color}),
                &meta,
            )
            .expect("слой прицела");
            assert_eq!(layer.image, "cross-changer");
            tiles.insert(format!("{:?}", layer.rect));
            let [x, y, w, h] = layer.rect;
            assert!(x >= 0.0 && y >= 0.0 && x + w <= 224.0 && y + h <= 288.0);
        }
    }
    assert_eq!(tiles.len(), 63);
    assert_eq!(
        preview::crosshair_layer(&json!({"packCrosshair": "default"}), &meta),
        None
    );
}

#[test]
fn big_digits_armour_and_both_ammo_kinds_stay_inside_the_won_frame() {
    let expected = preview_fixture();
    let rects: preview::Rects = expected["meta"]["hud"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, rect)| {
            let n = |at: usize| rect[at].as_f64().unwrap();
            (name.clone(), [n(0), n(1), n(2), n(3)])
        })
        .collect();
    let commands =
        preview::hud_commands(&rects, preview::DEFAULT_WIDTH, preview::DEFAULT_HEIGHT).unwrap();
    for command in &commands {
        let (at, [width, height]) = match command {
            preview::Command::Sprite { rect, at, .. } => {
                let rect = rect.expect("у спрайтов сцены есть прямоугольник");
                (*at, [rect[2], rect[3]])
            }
            preview::Command::Fill { at, size } => (*at, *size),
        };
        assert!(
            at[0] >= 0.0 && at[1] >= 0.0 && at[0] + width <= 1600.0 && at[1] + height <= 900.0,
            "{command:?}"
        );
    }
    let has = |name: &str| {
        commands
            .iter()
            .any(|command| matches!(command, preview::Command::Sprite { name: n, .. } if n == name))
    };
    for name in [
        "suit_full",
        "suit_empty",
        "ammo",
        "ammo2",
        "flash_full",
        "flash_empty",
    ] {
        assert!(has(name), "{name}");
    }
    let zeros = commands
        .iter()
        .filter(|command| matches!(command, preview::Command::Sprite { name, .. } if name == "number_0"))
        .count();
    assert!(zeros >= 4);
}
