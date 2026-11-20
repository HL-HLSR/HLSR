//! Самотесты 0.4.7 о персонализации Ghosting с теми же случаями:
//! `scripts/ghosting-settings-selftest.js`, `scripts/ghosting-assets-selftest.js`, часть
//! `scripts/ghosting-integration-selftest.js` и два теста Ghosting из `scripts/hl1-selftest.js`.
//!
//! Не здесь: тесты, которым нужны `LaunchService` (первый запуск, перенос extras до записи
//! метки, `steam_appid.txt`, Steam не запущен — задачи 20 и 21), контроллер доставки
//! (`gameSettings`, `personalizationCode`, выбор файла в диалоге — задача 22). Там они
//! переносятся вместе с модулями; `GameSettingsStore` и `personalization-code` —
//! `game_settings_selftests.rs`. Тест «переводы
//! есть у каждого поля» читает строки из `crates/hlsr-ui/locales` (в 0.4.7 это
//! `ghosting-settings-locales.js`, нужный только окну; в крейте доставки его нет).

use hlsr_core::js::Platform;
use hlsr_delivery::bind_presets::{catalog_for, ghosting_save_deletion};
use hlsr_delivery::ghosting_assets::{self, AssetError, MAX_ASSET_BYTES};
use hlsr_delivery::ghosting_personalization as gp;
use hlsr_delivery::ghosting_settings::{self as gs, ExtraValue, FieldType};
use hlsr_delivery::launch_args;
use hlsr_delivery::ports::{FakeRegistry, RegistryHive, RegistryValue};
use hlsr_delivery::source_screen::{self, Screen, ScreenSource};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Корень проверки. В Linux — настоящий путь; в Windows `canonicalize` дал бы путь с `\\?\`,
/// к которому нельзя дописывать `/`.
fn real_root(dir: &Path) -> PathBuf {
    if cfg!(unix) {
        dir.canonicalize().unwrap()
    } else {
        dir.to_path_buf()
    }
}

fn installation() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = real_root(dir.path());
    std::fs::create_dir_all(root.join(gp::CFG_DIR)).unwrap();
    (dir, root)
}

fn cfg_file(root: &Path, name: &str) -> PathBuf {
    root.join(gp::CFG_DIR).join(name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    String::from_utf8(std::fs::read(path).unwrap()).unwrap()
}

fn sanitize(input: &Value) -> Value {
    gs::sanitize_extras(input).to_value()
}

fn import(text: &str) -> Value {
    gs::import_extras(text).to_value()
}

// ---------------------------------------------------- ghosting-settings-selftest.js

fn locale(language: &str) -> Value {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("hlsr-ui")
        .join("locales")
        .join(format!("{language}.json"));
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

#[test]
fn all_exposed_fields_have_translations_and_stable_default_cfg_roundtrips() {
    let keys: HashSet<&str> = gs::fields().map(|field| field.key).collect();
    assert_eq!(keys.len(), gs::fields().count());
    let (ru, en) = (locale("ru"), locale("en"));
    for section in gs::SECTIONS {
        let mut wanted = vec![section.title];
        for field in section.fields {
            wanted.push(field.label);
            wanted.extend(field.hint);
            if let FieldType::Select { options } = field.kind {
                wanted.extend(options.iter().map(|option| option.label));
            }
            wanted.extend(field.scale.into_iter().flatten());
        }
        for key in wanted {
            for (language, strings) in [("ru", &ru), ("en", &en)] {
                let text = strings[key].as_str().unwrap_or_default();
                assert!(!text.is_empty(), "{language}: {key}");
            }
        }
    }
    let defaults = gs::defaults().to_value();
    assert_eq!(
        import(&gs::serialize_extras(&defaults)),
        defaults,
        "умолчания записываются и читаются без потерь"
    );
    let mut first = gs::defaults();
    first.insert(
        "gh_ghost_color",
        ExtraValue::Color(gs::Color { r: 0, g: 0, b: 0 }),
    );
    assert_eq!(
        gs::defaults().get("gh_ghost_color"),
        Some(&ExtraValue::Color(gs::Color {
            r: 237,
            g: 133,
            b: 60
        })),
        "цвета в предпросмотре не меняют схему"
    );
}

#[test]
fn only_explicitly_managed_extras_are_written_legacy_personalization_is_preserved() {
    let old = json!({"hudEnabled": true, "crossEnabled": true, "crossColor": {"r": 14, "g": 52, "b": 91}, "crossSize": 8});
    let migrated = gp::sanitize_personalization(&old);
    assert!(migrated.extra.is_empty());
    assert_eq!(migrated.cross_size, 8);
    assert_eq!(migrated.cross_color.to_value(), old["crossColor"]);
    let cfg = gp::cfg_lines(&migrated);
    assert!(!cfg.contains("gh_timer \""));
    assert!(!cfg.contains("viewmodel_fov \""));
    let mut with_extra = old.clone();
    with_extra["extra"] = json!({"gh_timer": false});
    assert_eq!(
        gp::apply_preset(&with_extra, "dot").extra.to_value(),
        json!({"gh_timer": false})
    );
}

#[test]
fn crosshair_preset_ids_ignore_inherited_object_properties() {
    for id in [
        "constructor",
        "__proto__",
        "toString",
        "hasOwnProperty",
        "valueOf",
    ] {
        assert_eq!(
            gp::sanitize_personalization(&json!({"crossPreset": id})).cross_preset,
            "standard",
            "{id}"
        );
        assert_eq!(
            gp::apply_preset(&json!({"crossEnabled": true}), id),
            gp::sanitize_personalization(&json!({"crossEnabled": true})),
            "{id}"
        );
    }
    assert_eq!(
        gp::sanitize_personalization(&json!({"crossPreset": "custom"})).cross_preset,
        "custom"
    );
    assert_eq!(
        gp::apply_preset(&json!({}), "circle").cross_circle_radius,
        12
    );
}

#[test]
fn false_zero_decimals_colors_and_last_cfg_assignment_survive_roundtrip() {
    let input = json!({
        "gh_timer": false,
        "gh_timer_x": 0,
        "viewmodel_offset_x": -2.4,
        "gh_hud_fontscale": 1.35,
        "gh_chat_emoji_scale": 1.15,
        "gh_trail_color": {"r": 11, "g": 222, "b": 33},
        "gh_health_align": "9",
        "gh_hud_font": "Lucida Console",
    });
    assert_eq!(import(&gs::serialize_extras(&input)), input);
    let parsed = import("gh_timer 1\ngh_timer \"0\" // override\ngh_timer_x 0\n");
    assert_eq!(parsed, json!({"gh_timer": false, "gh_timer_x": 0}));
}

#[test]
fn unknown_commands_and_malicious_text_never_reach_a_generated_cfg() {
    for value in [
        "Impact\";quit",
        "Impact\nquit",
        "Impact\rquit",
        "Impact;quit",
        "Impact\u{0}quit",
        "Impact\u{2028}quit",
    ] {
        assert_eq!(
            sanitize(&json!({"gh_hud_font": value})),
            json!({}),
            "{value:?}"
        );
    }
    // Infinity в JSON не записать: так же, как в 0.4.7, не принимается строка, дающая Infinity.
    assert_eq!(
        sanitize(&json!({
            "exec": "evil.cfg", "quit": true, "gh_spray_set": "evil",
            "gh_timer": "false", "viewmodel_fov": "Infinity",
        })),
        json!({})
    );
    let proto: Value =
        serde_json::from_str(r#"{"__proto__":{"gh_timer":true},"constructor":"quit"}"#).unwrap();
    assert_eq!(sanitize(&proto), json!({}));
    assert_eq!(
        import("gh_hud_font \"Impact\"; quit\nexec evil.cfg\ngh_timer \"0;quit\"\n"),
        json!({})
    );
    assert_eq!(
        sanitize(&json!({"viewmodel_fov": 500}))["viewmodel_fov"],
        100
    );
    assert_eq!(sanitize(&json!({"gh_opacity": -30}))["gh_opacity"], 0);
    assert_eq!(
        sanitize(&json!({"gh_spray_scale": 0}))["gh_spray_scale"],
        0.05
    );
    assert_eq!(
        sanitize(&json!({"gh_trail_length": 2.8}))["gh_trail_length"],
        3
    );
}

#[test]
fn spray_and_youtube_fields_use_real_commands_and_validate_user_paths_and_urls() {
    let clean = sanitize(&json!({
        "gh_spray_file": "C:\\Pictures\\my spray.png",
        "gh_chat_connect": "https://youtu.be/abcdefghijk?t=42",
    }));
    assert_eq!(clean["gh_spray_file"], "C:/Pictures/my spray.png");
    assert_eq!(
        clean["gh_chat_connect"],
        "https://www.youtube.com/watch?v=abcdefghijk"
    );
    let cfg = gs::serialize_extras(&clean);
    assert!(cfg.contains("gh_spray_set \"C:/Pictures/my spray.png\""));
    assert!(cfg.contains("gh_chat_connect \"https://www.youtube.com/watch?v=abcdefghijk\""));
    assert_eq!(import(&cfg), clean);
    let reset = gs::serialize_extras(&json!({"gh_spray_file": "", "gh_chat_connect": ""}));
    assert!(reset.contains("gh_spray_clear"));
    assert!(reset.contains("gh_chat_disconnect youtube"));
    assert_eq!(
        import(&reset),
        json!({"gh_chat_connect": "", "gh_spray_file": ""})
    );
    for url in [
        "https://youtube.com.attacker.test/watch?v=abcdefghijk",
        "file:///etc/passwd",
        "https://youtube.com@attacker.test/watch?v=abcdefghijk",
        "https://youtube.com/watch?v=bad",
    ] {
        assert_eq!(
            sanitize(&json!({"gh_chat_connect": url})),
            json!({}),
            "{url}"
        );
    }
    assert_eq!(
        sanitize(&json!({"gh_chat_channel": "Some_Runner"})),
        json!({"gh_chat_channel": "some_runner"})
    );
    assert_eq!(
        sanitize(&json!({"gh_chat_channel": "runner;quit"})),
        json!({})
    );
}

#[test]
fn apply_retains_user_autoexec_and_is_idempotent() {
    let (_dir, root) = installation();
    let original = "// My settings\nbind F6 \"save quick\"\nexec speedrun.cfg";
    write(&cfg_file(&root, "autoexec.cfg"), original);
    let input = json!({"extra": {"gh_timer": false, "viewmodel_offset_z": -0.7}});
    gp::apply_to_install(&root, &input, Platform::Linux).unwrap();
    let file = cfg_file(&root, gp::PERSONALIZATION_FILE);
    let first = read(&file);
    gp::apply_to_install(&root, &input, Platform::Linux).unwrap();
    assert_eq!(read(&file), first);
    assert_eq!(
        read(&cfg_file(&root, "autoexec.cfg")),
        format!("{original}\nexec hlsr-personalization.cfg\n")
    );
    assert_eq!(import(&first), input["extra"]);
}

#[test]
fn chat_connections_respect_disabled_readers_and_retain_their_saved_selection() {
    let url = "https://www.youtube.com/watch?v=abcdefghijk";
    let input = json!({"gh_chat": false, "gh_chat_channel": "runner", "gh_chat_connect": url});
    let disabled = gs::serialize_extras(&input);
    assert!(
        !disabled
            .split('\n')
            .any(|line| line.starts_with("gh_chat_connect "))
    );
    assert_eq!(import(&disabled), input);
    let youtube_disabled =
        gs::serialize_extras(&json!({"gh_chat_youtube": false, "gh_chat_connect": url}));
    assert!(
        !youtube_disabled
            .split('\n')
            .any(|line| line.starts_with("gh_chat_connect "))
    );
    let twitch = gs::serialize_extras(&json!({"gh_chat": true, "gh_chat_channel": "runner"}));
    assert!(twitch.find("gh_chat \"1\"") < twitch.find("gh_chat_connect \"runner\""));
    assert_eq!(
        import(&twitch),
        json!({"gh_chat": true, "gh_chat_channel": "runner"})
    );
    assert!(
        gs::serialize_extras(&json!({"gh_chat_channel": ""})).contains("gh_chat_disconnect twitch")
    );
}

#[test]
fn managed_spray_paths_resolve_from_the_games_working_directory_and_roundtrip() {
    let (_dir, root) = installation();
    let imported = format!("materials/vgui/logos/hlsr/{}.png", "a".repeat(64));
    let physical = root.join("ghostingmod").join(&imported);
    write(&physical, "test image path");
    let cfg = gs::serialize_extras(&json!({"gh_spray_file": imported}));
    let emitted = cfg
        .strip_prefix("gh_spray_set \"")
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("одна команда gh_spray_set");
    assert_eq!(emitted, format!("ghostingmod/{imported}"));
    assert_eq!(
        std::fs::canonicalize(root.join(emitted)).unwrap(),
        std::fs::canonicalize(&physical).unwrap()
    );
    assert_eq!(import(&cfg), json!({"gh_spray_file": imported}));
    assert_eq!(
        gs::serialize_extras(&json!({"gh_spray_file": "custom/picture.png"})),
        "gh_spray_set \"custom/picture.png\""
    );
}

#[test]
fn upgrade_imports_extra_settings_even_when_an_older_personalization_cfg_exists() {
    let (_dir, root) = installation();
    write(
        &cfg_file(&root, "config.cfg"),
        "gh_keyshud \"1\"\ngh_hud_fontscale \"1.4\"\ngh_cross_size \"30\"\n",
    );
    let file = cfg_file(&root, gp::PERSONALIZATION_FILE);
    write(
        &file,
        "// Old HLSR file\ngh_cross_size \"8\"\ngh_keyshud \"0\"\n",
    );
    assert!(
        gp::import_from_config_cfg(&root).is_none(),
        "существующая основная персонализация остаётся главной"
    );
    assert_eq!(
        gp::import_extras_from_config_cfg(&root).unwrap().to_value(),
        json!({"gh_keyshud": false, "gh_hud_fontscale": 1.4})
    );
    let saved = json!({"crossSize": 8, "extra": {"gh_keyshud": false}});
    gp::apply_to_install(&root, &saved, Platform::Linux).unwrap();
    assert!(gp::import_extras_from_config_cfg(&root).is_none());
    assert!(read(&file).contains("gh_cross_size \"8\""));
}

#[test]
fn first_import_accepts_extras_without_color_settings_explicit_reset_cannot_resurrect_old_cfg_values()
 {
    let (_dir, root) = installation();
    write(
        &cfg_file(&root, "config.cfg"),
        "gh_keyshud \"1\"\nviewmodel_fov \"80\"\n",
    );
    assert_eq!(
        gp::import_from_config_cfg(&root).unwrap().extra.to_value(),
        json!({"gh_keyshud": true, "viewmodel_fov": 80})
    );
    let reset = gs::reset_extras(&json!({"gh_keyshud": true, "viewmodel_fov": 80})).to_value();
    assert_eq!(reset, json!({"gh_keyshud": false, "viewmodel_fov": 62}));
    gp::apply_to_install(&root, &json!({"extra": reset}), Platform::Linux).unwrap();
    let file = cfg_file(&root, gp::PERSONALIZATION_FILE);
    assert_eq!(import(&read(&file)), reset);
    assert!(gp::import_extras_from_config_cfg(&root).is_none());
    gp::apply_to_install(&root, &json!({"extra": {}}), Platform::Linux).unwrap();
    assert_eq!(import(&read(&file)), json!({}));
    assert!(gp::import_extras_from_config_cfg(&root).is_none());
}

#[test]
fn settings_that_only_work_with_another_one_are_off_until_it_is_set() {
    let field = |key: &str| gs::field(key).unwrap();
    let values = gs::defaults().to_value();
    for item in gs::fields().filter(|item| item.requires.is_some()) {
        assert!(gs::field(item.requires.unwrap()).is_some(), "{}", item.key);
    }
    let with = |pairs: Value| {
        let mut merged = values.clone();
        for (key, value) in pairs.as_object().unwrap() {
            merged[key.as_str()] = value.clone();
        }
        merged
    };
    assert!(!gs::available(field("gh_hud_fontscale"), &values));
    assert!(gs::available(
        field("gh_hud_fontscale"),
        &with(json!({"gh_hud_font": "Impact"}))
    ));
    assert!(gs::available(
        field("gh_showfps_font"),
        &with(json!({"cl_showfps": "2"}))
    ));
    // Требования идут цепочкой: стиль splits требует splits, а они — зеркало.
    let bold = field("gh_timer_mirror_splits_bold");
    assert!(!gs::available(bold, &values));
    assert!(gs::available(bold, &with(json!({"gh_timer_mirror": true}))));
    assert!(!gs::available(
        bold,
        &with(json!({"gh_timer": false, "gh_timer_mirror": true}))
    ));
    assert!(gs::available(field("gh_timer"), &values));
}

#[test]
fn fps_counter_and_position_are_on_by_default() {
    let values = gs::defaults().to_value();
    assert_eq!(values["cl_showfps"], "1");
    assert_eq!(values["cl_showpos"], true);
    assert_eq!(
        gs::reset_extras(&json!({"cl_showfps": "0", "cl_showpos": false})).to_value(),
        json!({"cl_showfps": "1", "cl_showpos": true})
    );
}

#[test]
fn field_of_view_is_written_under_all_three_source_names_and_reads_back() {
    let cfg = gs::serialize_extras(&json!({"fov_desired": 120}));
    let lines: Vec<&str> = cfg.split('\n').collect();
    for line in ["default_fov \"120\"", "fov_desired \"120\"", "fov \"120\""] {
        assert!(lines.contains(&line), "{line}");
    }
    assert_eq!(import(&cfg), json!({"fov_desired": 120}));
    assert_eq!(
        sanitize(&json!({"fov_desired": 200})),
        json!({"fov_desired": 150}),
        "не шире самого широкого кадра предпросмотра"
    );
}

// --------------------------------------------------- hl1-selftest.js (Ghosting)

#[test]
fn ghosting_stock_autoexec_is_replaced_by_the_launchers_standard_one_own_is_not() {
    let (_dir, root) = installation();
    let stock = "fov_desired 90\ncon_enable 1\ncl_showfps 1\ncl_showpos 1\nfps_max 300\n\ndemo_interplimit 4000\n\nexec gh_optimized.cfg";
    let autoexec = cfg_file(&root, "autoexec.cfg");
    write(
        &autoexec,
        &format!("{stock}\nexec hlsr-personalization.cfg\n"),
    );
    assert!(gp::ensure_default_autoexec(&root, Platform::Linux).unwrap());
    let text = read(&autoexec);
    assert!(text.contains("bind k \"load quick01\""));
    assert!(
        text.ends_with("exec hlsr-personalization.cfg\r\n"),
        "строка exec лаунчера сохраняется"
    );
    assert!(
        !gp::ensure_default_autoexec(&root, Platform::Linux).unwrap(),
        "второй раз уже не стоковый"
    );

    write(&autoexec, "fps_max 999\n");
    assert!(!gp::ensure_default_autoexec(&root, Platform::Linux).unwrap());
    assert_eq!(read(&autoexec), "fps_max 999\n");

    let fresh = tempfile::tempdir().unwrap();
    assert!(
        gp::ensure_default_autoexec(fresh.path(), Platform::Linux).unwrap(),
        "нет файла — ставится стандартный"
    );
    let catalog = catalog_for("ghosting-renewed", Platform::Linux).unwrap();
    assert!(
        catalog.preset_of("load quick01").is_some(),
        "бинды стандартного autoexec есть в заготовках"
    );
}

#[test]
fn ghosting_save_deletion_goes_through_save_slash_on_linux_the_old_bind_in_autoexec_is_replaced() {
    let expected = ghosting_save_deletion(Platform::Linux);
    assert_eq!(expected, "save /; reload");
    assert!(
        catalog_for("ghosting-renewed", Platform::Linux)
            .unwrap()
            .preset_of(&expected)
            .is_some(),
        "заготовка удаления сейва"
    );
    let fresh = tempfile::tempdir().unwrap();
    gp::ensure_default_autoexec(fresh.path(), Platform::Linux).unwrap();
    assert!(
        read(&fresh.path().join("ghostingmod/cfg/autoexec.cfg"))
            .contains(&format!("bind h \"{expected}\"\r\n"))
    );

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("ghostingmod/cfg/autoexec.cfg");
    let own = "fps_max 999\r\n\tbind h \"save :; reload\"\r\nbind j \"save :; reload; echo 1\"\r\nexec hlsr-personalization.cfg\r\n";
    write(&file, own);
    assert!(!gp::ensure_default_autoexec(root.path(), Platform::Win32).unwrap());
    assert_eq!(read(&file), own, "под Windows файл не меняется");
    assert!(!gp::ensure_default_autoexec(root.path(), Platform::Linux).unwrap());
    assert_eq!(
        read(&file),
        own.replace(
            "\tbind h \"save :; reload\"",
            &format!("\tbind h \"{expected}\"")
        ),
        "меняется только бинд лаунчера, отступы и окончания строк те же"
    );
}

#[test]
fn negative_crosshair_gap_is_kept_by_settings_and_written_as_zero_for_ghosting() {
    let adopted = json!({"crossEnabled": true, "crossGap": -4});
    let settings = launch_args::sanitize_settings(&json!({"personalization": adopted})).settings;
    assert_eq!(
        settings.personalization.cross_gap, -4,
        "сохранение не обрезает зазор"
    );
    assert!(
        gp::cfg_lines(&settings.personalization)
            .split('\n')
            .any(|line| line == "gh_cross_gap \"0\"")
    );
    assert_eq!(
        launch_args::sanitize_settings(&json!({"personalization": {"crossGap": -99}}))
            .settings
            .personalization
            .cross_gap,
        -30
    );
}

// ----------------------------------------------------- ghosting-assets-selftest.js

const PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aGhsAAAAASUVORK5CYII=";

fn png() -> Vec<u8> {
    use base64::prelude::{BASE64_STANDARD, Engine as _};
    BASE64_STANDARD.decode(PNG_BASE64).unwrap()
}

fn font(otf: bool) -> Vec<u8> {
    let mut bytes = vec![0u8; 32];
    if otf {
        bytes[..4].copy_from_slice(b"OTTO");
    } else {
        bytes[..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    }
    bytes[4..6].copy_from_slice(&1u16.to_be_bytes());
    bytes[12..16].copy_from_slice(b"head");
    bytes[20..24].copy_from_slice(&28u32.to_be_bytes());
    bytes[24..28].copy_from_slice(&4u32.to_be_bytes());
    bytes
}

fn bmp() -> Vec<u8> {
    let mut bytes = vec![0u8; 58];
    bytes[..2].copy_from_slice(b"BM");
    bytes[2..6].copy_from_slice(&58u32.to_le_bytes());
    bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
    bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
    bytes[18..22].copy_from_slice(&1i32.to_le_bytes());
    bytes[22..26].copy_from_slice(&1i32.to_le_bytes());
    bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
    bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
    bytes
}

fn tga() -> Vec<u8> {
    let mut bytes = vec![0u8; 21];
    bytes[2] = 2;
    bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
    bytes[14..16].copy_from_slice(&1u16.to_le_bytes());
    bytes[16] = 24;
    bytes
}

struct Assets {
    _dir: tempfile::TempDir,
    root: PathBuf,
    install: PathBuf,
    sources: PathBuf,
}

impl Assets {
    fn new() -> Assets {
        let dir = tempfile::tempdir().unwrap();
        let root = real_root(dir.path());
        let install = root.join("game with spaces");
        let sources = root.join("sources");
        std::fs::create_dir_all(install.join("ghostingmod")).unwrap();
        std::fs::create_dir_all(&sources).unwrap();
        Assets {
            _dir: dir,
            root,
            install,
            sources,
        }
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.sources.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn import(
        &self,
        source: &Path,
        kind: &str,
    ) -> Result<ghosting_assets::ImportedAsset, AssetError> {
        ghosting_assets::import_asset(
            self.install.to_str().unwrap(),
            source.to_str().unwrap(),
            kind,
        )
    }

    fn preview(&self, kind: &str, value: &str) -> Result<String, AssetError> {
        ghosting_assets::read_preview_asset(self.install.to_str().unwrap(), kind, value, None)
            .map(|asset| asset.data_url)
    }
}

fn code<T: std::fmt::Debug>(result: Result<T, AssetError>) -> &'static str {
    result.expect_err("ожидался отказ").code
}

fn is_hex_name(text: &str, extensions: &[&str]) -> bool {
    let Some((name, extension)) = text.rsplit_once('.') else {
        return false;
    };
    name.len() == 64
        && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && extensions.contains(&extension)
}

#[test]
fn imports_each_supported_format_to_a_stable_mod_relative_path_and_preserves_the_source() {
    let f = Assets::new();
    let assets: Vec<(&str, &str, Vec<u8>)> = vec![
        ("font", "my font.TTF", font(false)),
        ("font", "font.otf", font(true)),
        ("spray", "my picture.png", png()),
        ("spray", "picture.bmp", bmp()),
        ("spray", "picture.tga", tga()),
    ];
    for (kind, name, bytes) in assets {
        let source = f.file(name, &bytes);
        let before = std::fs::metadata(&source).unwrap().modified().unwrap();
        let result = f.import(&source, kind).unwrap();
        let (directory, file) = result.value.rsplit_once('/').unwrap();
        assert_eq!(
            directory,
            if kind == "font" {
                "resource/hlsr-fonts"
            } else {
                "materials/vgui/logos/hlsr"
            }
        );
        assert!(
            is_hex_name(
                file,
                if kind == "font" {
                    &["ttf", "otf"]
                } else {
                    &["png", "bmp", "tga"]
                }
            ),
            "{}",
            result.value
        );
        assert_eq!(result.name, name);
        assert_eq!(result.bytes, bytes.len() as u64);
        assert_eq!(result.sha256.len(), 64);
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(&source).unwrap().modified().unwrap(),
            before
        );
        let managed = f.install.join("ghostingmod").join(&result.value);
        assert_eq!(std::fs::read(&managed).unwrap(), bytes);
        assert_eq!(f.import(&source, kind).unwrap(), result);
        let managed_before = std::fs::metadata(&managed).unwrap();
        assert_eq!(f.import(&managed, kind).unwrap().value, result.value);
        let managed_after = std::fs::metadata(&managed).unwrap();
        assert_eq!(
            managed_after.modified().unwrap(),
            managed_before.modified().unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(
                managed_after.ino(),
                managed_before.ino(),
                "файл не переписан"
            );
        }
    }
}

#[test]
fn rejects_unsupported_kinds_relative_paths_missing_installations_and_wrong_extensions() {
    let f = Assets::new();
    let source = f.file("picture.png", &png());
    let (install, src) = (f.install.to_str().unwrap(), source.to_str().unwrap());
    let import = ghosting_assets::import_asset;
    assert_eq!(code(import(install, src, "__proto__")), "ASSET_BAD_KIND");
    assert_eq!(
        code(import(install, "picture.png", "spray")),
        "ASSET_INVALID_PATH"
    );
    assert_eq!(
        code(import("relative/game", src, "spray")),
        "ASSET_INVALID_PATH"
    );
    let missing = f.root.join("missing");
    assert_eq!(
        code(import(missing.to_str().unwrap(), src, "spray")),
        "ASSET_NOT_INSTALLED"
    );
    assert_eq!(code(import(install, src, "font")), "ASSET_INVALID_TYPE");
    let absent = f.sources.join("absent.png");
    assert_eq!(
        code(import(install, absent.to_str().unwrap(), "spray")),
        "ASSET_READ_FAILED"
    );
}

#[test]
fn rejects_empty_spoofed_truncated_and_oversized_source_files() {
    let f = Assets::new();
    let png = png();
    let mut broken_font = font(false);
    broken_font[24..28].copy_from_slice(&9999u32.to_be_bytes());
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("empty.png", Vec::new(), "spray"),
        ("spoof.png", b"not a picture".to_vec(), "spray"),
        ("broken.ttf", b"not a font".to_vec(), "font"),
        ("truncated.png", png[..33].to_vec(), "spray"),
        ("bad.bmp", vec![0; 54], "spray"),
        ("bad.tga", vec![0; 30], "spray"),
        ("tables.ttf", broken_font, "font"),
    ];
    for (name, bytes, kind) in cases {
        assert_eq!(
            code(f.import(&f.file(name, &bytes), kind)),
            "ASSET_INVALID_FILE",
            "{name}"
        );
    }
    let huge = f.file("huge.png", &png);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&huge)
        .unwrap()
        .set_len(MAX_ASSET_BYTES + 1)
        .unwrap();
    assert_eq!(code(f.import(&huge, "spray")), "ASSET_TOO_LARGE");
    assert!(!f.install.join("ghostingmod").join("materials").exists());
}

#[test]
fn rejects_directories_and_source_symlinks() {
    let f = Assets::new();
    let directory = f.sources.join("directory.png");
    std::fs::create_dir(&directory).unwrap();
    assert_eq!(code(f.import(&directory, "spray")), "ASSET_INVALID_FILE");
    #[cfg(unix)]
    {
        let source = f.file("source.png", &png());
        let linked = f.sources.join("linked.png");
        std::os::unix::fs::symlink(&source, &linked).unwrap();
        assert_eq!(code(f.import(&linked, "spray")), "ASSET_INVALID_FILE");
    }
}

#[test]
fn rejects_image_encodings_the_game_cannot_decode() {
    let f = Assets::new();
    let mut interlaced = png();
    interlaced[28] = 1;
    let mut compressed_bmp = bmp();
    compressed_bmp[30..34].copy_from_slice(&1u32.to_le_bytes());
    let mut bmp16 = bmp();
    bmp16[28..30].copy_from_slice(&16u16.to_le_bytes());
    let mut mapped_tga = tga();
    mapped_tga[1] = 1;
    mapped_tga[2] = 1;
    let mut tga16 = tga();
    tga16[16] = 16;
    let truncated_tga = tga()[..19].to_vec();
    let mut bad_rle_tga = tga();
    bad_rle_tga[2] = 10;
    bad_rle_tga[18] = 0xff;
    let cases = [
        ("interlaced.png", interlaced),
        ("compressed.bmp", compressed_bmp),
        ("16bit.bmp", bmp16),
        ("mapped.tga", mapped_tga),
        ("16bit.tga", tga16),
        ("truncated.tga", truncated_tga),
        ("rle.tga", bad_rle_tga),
    ];
    for (name, bytes) in cases {
        assert_eq!(
            code(f.import(&f.file(name, &bytes), "spray")),
            "ASSET_INVALID_FILE",
            "{name}"
        );
    }
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_mod_and_destination_directories_without_writing_outside_the_install() {
    let f = Assets::new();
    let source = f.file("source.png", &png());
    let outside = f.root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    let module = f.install.join("ghostingmod");
    std::fs::remove_dir(&module).unwrap();
    std::os::unix::fs::symlink(&outside, &module).unwrap();
    assert_eq!(code(f.import(&source, "spray")), "ASSET_UNSAFE_PATH");
    std::fs::remove_file(&module).unwrap();
    std::fs::create_dir(&module).unwrap();
    std::os::unix::fs::symlink(&outside, module.join("materials")).unwrap();
    assert_eq!(code(f.import(&source, "spray")), "ASSET_UNSAFE_PATH");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn refuses_a_linked_final_destination_and_leaves_its_target_intact() {
    let f = Assets::new();
    let source = f.file("source.png", &png());
    let result = f.import(&source, "spray").unwrap();
    let destination = f.install.join("ghostingmod").join(&result.value);
    let outside = f.file("unrelated.txt", b"keep me");
    std::fs::remove_file(&destination).unwrap();
    std::os::unix::fs::symlink(&outside, &destination).unwrap();
    assert_eq!(code(f.import(&source, "spray")), "ASSET_UNSAFE_PATH");
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep me");
    assert_eq!(std::fs::read(&source).unwrap(), png());
    let leftovers = std::fs::read_dir(destination.parent().unwrap())
        .unwrap()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"));
    assert!(!leftovers, "временный файл не остался");
}

#[test]
fn overlay_preview_reads_imported_assets_and_rejects_escapes_or_other_file_types() {
    use base64::prelude::{BASE64_STANDARD, Engine as _};
    let f = Assets::new();
    let asset = f.import(&f.file("preview.png", &png()), "spray").unwrap();
    assert_eq!(
        f.preview("spray", &asset.value).unwrap(),
        format!("data:image/png;base64,{}", BASE64_STANDARD.encode(png()))
    );
    assert_eq!(
        code(f.preview("spray", "../sources/preview.png")),
        "ASSET_INVALID_PATH"
    );
    assert_eq!(code(f.preview("font", &asset.value)), "ASSET_INVALID_TYPE");
    // Создавать ссылки в Windows без особых прав нельзя: там этой проверки нет.
    #[cfg(unix)]
    {
        let outside = f.file("outside.png", &png());
        std::os::unix::fs::symlink(&outside, f.install.join("ghostingmod").join("outside.png"))
            .unwrap();
        assert_eq!(code(f.preview("spray", "outside.png")), "ASSET_UNSAFE_PATH");
    }
}

#[test]
fn overlay_preview_finds_files_like_the_game_any_case_custom_and_hl2() {
    use base64::prelude::{BASE64_STANDARD, Engine as _};
    let f = Assets::new();
    let put = |relative: &str, bytes: &[u8]| {
        let file = f.install.join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    };
    let url = format!("data:image/png;base64,{}", BASE64_STANDARD.encode(png()));
    // Под Proton игра не различает регистр, предпросмотр тоже.
    put("ghostingmod/Materials/VGUI/Logos/MySpray.PNG", &png());
    assert_eq!(
        f.preview("spray", "materials/vgui/logos/myspray.png")
            .unwrap(),
        url
    );
    assert_eq!(
        f.preview("spray", "materials\\vgui\\logos\\MYSPRAY.png")
            .unwrap(),
        url
    );
    // Пути поиска из gameinfo.txt: custom/* мода и папка hl2.
    put(
        "ghostingmod/custom/myhud/resource/fonts/hud.ttf",
        &font(false),
    );
    assert_eq!(
        f.preview("font", "resource/fonts/hud.ttf").unwrap(),
        format!(
            "data:font/ttf;base64,{}",
            BASE64_STANDARD.encode(font(false))
        )
    );
    put("hl2/materials/shared.png", &png());
    assert_eq!(f.preview("spray", "materials/shared.png").unwrap(), url);
    assert_eq!(
        code(f.preview("spray", "materials/missing.png")),
        "ASSET_READ_FAILED"
    );
}

#[test]
fn overlay_tga_conversion_retains_rgb_pixels_and_writes_valid_png_chunks() {
    use base64::prelude::{BASE64_STANDARD, Engine as _};
    let f = Assets::new();
    let mut picture = tga();
    picture[18] = 15;
    picture[19] = 80;
    picture[20] = 210;
    let asset = f.import(&f.file("preview.tga", &picture), "spray").unwrap();
    let url = f.preview("spray", &asset.value).unwrap();
    let encoded = url.split(',').nth(1).unwrap();
    let png = BASE64_STANDARD.decode(encoded).unwrap();
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 1);
    assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 1);
    let (mut offset, mut decoded, mut types) = (8, Vec::new(), Vec::new());
    while offset < png.len() {
        let length = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = String::from_utf8(png[offset + 4..offset + 8].to_vec()).unwrap();
        // Браузер отвергает PNG, у которого сумма куска не сходится.
        let crc = u32::from_be_bytes(
            png[offset + 8 + length..offset + 12 + length]
                .try_into()
                .unwrap(),
        );
        assert_eq!(
            crc,
            crc32fast::hash(&png[offset + 4..offset + 8 + length]),
            "{kind}"
        );
        if kind == "IDAT" {
            flate2::read::ZlibDecoder::new(&png[offset + 8..offset + 8 + length])
                .read_to_end(&mut decoded)
                .unwrap();
        }
        types.push(kind);
        offset += length + 12;
    }
    assert_eq!(types, ["IHDR", "IDAT", "IEND"]);
    assert_eq!(decoded, [0, 210, 80, 15, 255]);
}

#[test]
fn overlay_font_family_lookup_stays_in_the_game_prefix() {
    use base64::prelude::{BASE64_STANDARD, Engine as _};
    let f = Assets::new();
    let prefix = f.root.join("pfx");
    let fonts = prefix.join("drive_c/windows/Fonts");
    let preview = |value: &str, prefix: Option<&Path>| {
        ghosting_assets::read_preview_asset(f.install.to_str().unwrap(), "font", value, prefix)
            .map(|asset| asset.data_url)
    };
    // Префикс, который не подготовлен, папки Fonts ещё не имеет.
    assert_eq!(code(preview("Impact", Some(&prefix))), "ASSET_INVALID_FILE");
    std::fs::create_dir_all(&fonts).unwrap();
    std::fs::write(fonts.join("IMPACT.TTF"), font(false)).unwrap();
    assert_eq!(
        preview("Impact", Some(&prefix)).unwrap(),
        format!(
            "data:font/ttf;base64,{}",
            BASE64_STANDARD.encode(font(false))
        )
    );
    assert_eq!(
        code(preview("Verdana", Some(&prefix))),
        "ASSET_INVALID_FILE"
    );
    // Без префикса название семейства — просто файл мода, которого нет.
    assert_eq!(code(preview("Impact", None)), "ASSET_READ_FAILED");
    // Имена, унаследованные от Object.prototype, — не семейства шрифтов.
    for name in ["constructor", "__proto__", "toString", "hasOwnProperty"] {
        assert_eq!(
            code(preview(name, Some(&prefix))),
            "ASSET_READ_FAILED",
            "{name}"
        );
    }
}

#[test]
fn overlay_preview_reports_unavailable_assets_with_stable_error_codes() {
    let f = Assets::new();
    let preview = |install: &str, kind: &str, value: &str| {
        ghosting_assets::read_preview_asset(install, kind, value, None)
    };
    let install = f.install.to_str().unwrap();
    assert_eq!(
        code(preview(
            install,
            "spray",
            "materials/vgui/logos/hlsr/absent.png"
        )),
        "ASSET_READ_FAILED"
    );
    let missing = f.root.join("missing");
    assert_eq!(
        code(preview(
            missing.to_str().unwrap(),
            "spray",
            "materials/x.png"
        )),
        "ASSET_READ_FAILED"
    );
    assert_eq!(
        code(preview(install, "executable", "x.png")),
        "ASSET_INVALID_PATH"
    );
    assert_eq!(
        code(preview("relative/game", "spray", "x.png")),
        "ASSET_INVALID_PATH"
    );
    assert_eq!(
        code(preview(install, "spray", "C:/Windows/x.png")),
        "ASSET_INVALID_PATH"
    );
    assert_eq!(
        code(preview(install, "spray", &"x".repeat(513))),
        "ASSET_INVALID_PATH"
    );
}

// ------------------------------------------- ghosting-integration-selftest.js (файлы)

#[test]
fn preview_assets_are_served_only_for_imported_ghosting_assets() {
    use base64::prelude::{BASE64_STANDARD, Engine as _};
    let f = Assets::new();
    let source = f.root.join("my spray.png");
    std::fs::write(&source, png()).unwrap();
    let imported = f.import(&source, "spray").unwrap();
    assert_eq!(
        f.preview("spray", &imported.value).unwrap(),
        format!("data:image/png;base64,{}", BASE64_STANDARD.encode(png()))
    );
    assert_eq!(
        code(f.preview("spray", "../outside.png")),
        "ASSET_INVALID_PATH"
    );
    assert_eq!(code(f.preview("font", "Impact")), "ASSET_READ_FAILED");
}

// ------------------------------------------------ ghosting-assets-selftest.js (экран)

#[test]
fn ghosting_resolution_for_the_hud_preview() {
    let prefix = tempfile::tempdir().unwrap();
    write(
        &prefix.path().join("user.reg"),
        &[
            "WINE REGISTRY Version 2",
            "",
            "[Software\\\\Valve\\\\Source\\\\hl2\\\\Settings] 1791620700",
            "\"ScreenWidth\"=dword:00000780",
            "\"ScreenHeight\"=dword:00000438",
            "",
            "[Software\\\\Valve\\\\Source\\\\ghostingmod\\\\Settings] 1791620751",
            "#time=1dc39a9c2b4c1e0",
            "\"ScreenHeight\"=dword:00000384",
            "\"ScreenWidth\"=dword:000005a0",
            "",
        ]
        .join("\n"),
    );
    let registry = FakeRegistry::default();
    let screen = |settings: Option<&Value>,
                  game_mod: &str,
                  platform,
                  prefix: Option<&Path>,
                  registry: &FakeRegistry| {
        source_screen::screen_of(settings, game_mod, platform, prefix, registry)
    };
    let linux = Platform::Linux;
    let p = Some(prefix.path());
    let game = |width, height| {
        Some(Screen {
            width,
            height,
            source: ScreenSource::Game,
        })
    };
    assert_eq!(
        screen(
            Some(&json!({"width": 0, "height": 0})),
            "ghostingmod",
            linux,
            p,
            &registry
        ),
        game(1440, 900)
    );
    assert_eq!(
        screen(
            Some(&json!({"width": 2560, "height": 1080})),
            "ghostingmod",
            linux,
            p,
            &registry
        ),
        Some(Screen {
            width: 2560,
            height: 1080,
            source: ScreenSource::Launcher
        })
    );
    assert_eq!(
        screen(Some(&json!({})), "cstrike", linux, p, &registry),
        None
    );
    assert_eq!(screen(Some(&json!({})), "..\\x", linux, p, &registry), None);
    let none = prefix.path().join("none");
    assert_eq!(
        screen(
            Some(&json!({})),
            "ghostingmod",
            linux,
            Some(&none),
            &registry
        ),
        None
    );

    // Windows: reg query HKCU\Software\Valve\Source\ghostingmod\Settings.
    let key = "Software\\Valve\\Source\\ghostingmod\\Settings";
    registry.set(
        RegistryHive::CurrentUser,
        key,
        "ScreenWidth",
        RegistryValue::Dword(0xa00),
    );
    registry.set(
        RegistryHive::CurrentUser,
        key,
        "ScreenHeight",
        RegistryValue::Dword(0x5a0),
    );
    assert_eq!(
        screen(None, "ghostingmod", Platform::Win32, None, &registry),
        game(2560, 1440)
    );
    let missing = FakeRegistry::default();
    assert_eq!(
        screen(None, "ghostingmod", Platform::Win32, None, &missing),
        None
    );
    // Мусор в реестре не даёт кадр нулевой или гигантской ширины.
    assert_eq!(
        source_screen::from_reg_query(
            "    ScreenWidth    REG_DWORD    0x0\r\n    ScreenHeight    REG_DWORD    0x5a0"
        ),
        None
    );
}
