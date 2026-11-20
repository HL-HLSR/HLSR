//! Самотесты 0.4.7 о настройках игр с теми же случаями: `scripts/personalization-code-selftest.js`
//! (все шесть тестов), параметры запуска из `scripts/hl1-selftest.js` (строки 366–387, 1196) и
//! `scripts/linux-selftest.js` (параметры Proton, 190–220), умолчания оверлея LiveSplit из
//! `scripts/livesplit-overlay-selftest.js` (первый тест) и `isGameAvailable("70")`.
//!
//! Не здесь: контроллер доставки (`gameSettings` get/save/reset, `personalizationCode`, `INVALID_ASSET`,
//! `NOT_INSTALLED`, выбор файла — `scripts/ghosting-integration-selftest.js`, задача 22), сборка
//! аргументов HL1 `goldsrc-launch.js` (задача 21), геометрия и `overlay.ini` оверлея (подпроект 3).
//! Тесты `negative_crosshair_gap` (`hl1-selftest.js` 2037–2044) — в `cfg_install_selftests.rs` и
//! `ghosting_selftests.rs`.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use flate2::Compression;
use flate2::write::DeflateEncoder;
use hlsr_delivery::game_availability;
use hlsr_delivery::game_settings_store::GameSettingsStore;
use hlsr_delivery::ghosting_settings as ghosting;
use hlsr_delivery::goldsrc_settings as goldsrc;
use hlsr_delivery::launch_args as la;
use hlsr_delivery::livesplit_overlay as overlay;
use hlsr_delivery::personalization_code::{self as codes, CodeError};
use hlsr_delivery::ports::MemoryLogger;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::sync::Arc;

// ============================================================ personalization-code-selftest.js

/// `forged(gameId, payload, suffix)`: верная сумма, сжатие этого же крейта.
fn forged(game: &str, payload: &[u8], suffix: &[u8]) -> String {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(payload).unwrap();
    let mut packed = encoder.finish().unwrap();
    packed.extend_from_slice(suffix);
    let mut hash = Sha256::new();
    hash.update(format!("HLSR-P1.{game}.").as_bytes());
    hash.update(&packed);
    let sum: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "HLSR-P1.{game}.{}.{}",
        URL_SAFE_NO_PAD.encode(&packed),
        &sum[..16]
    )
}

fn code_of(result: Result<impl std::fmt::Debug, CodeError>) -> &'static str {
    result.expect_err("ожидался отказ").code()
}

#[test]
fn ghosting_code_keeps_colors_layout_weapon_and_custom_values_without_launcher_settings() {
    let source = json!({
        "launcherTheme": "xen", "language": "ru", "launchArgs": "-console", "binds": {"SPACE": "+jump"},
        "personalization": {
            "hudEnabled": true, "hudColor": {"r": 17, "g": 140, "b": 225}, "hudAlpha": 185,
            "crossEnabled": true, "crossPreset": "custom", "crossSize": 14, "crossDotColor": {"r": 19, "g": 255, "b": 28},
            "extra": {"gh_health_x": -123, "gh_health_y": 57, "gh_health_align": "6", "gh_keyshud": true, "gh_keyshud_x": 88,
                "gh_ghost_color": {"r": 123, "g": 32, "b": 198}, "viewmodel_fov": 83, "viewmodel_offset_x": 1.5,
                "gh_name": "Игрок λ", "gh_hud_font": "Arial", "gh_chat_channel": "my_channel"},
        },
    });
    let before = source.to_string();
    let code = codes::encode("218", &source).unwrap();
    let look = codes::decode("218", &json!(code)).unwrap();
    assert_eq!(source.to_string(), before, "экспорт не меняет черновик");
    let look = look.to_value();
    assert_eq!(
        look.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["personalization"]
    );
    let p = &look["personalization"];
    assert_eq!(p["hudColor"], source["personalization"]["hudColor"]);
    assert_eq!(
        p["crossDotColor"],
        source["personalization"]["crossDotColor"]
    );
    assert_eq!(p["crossSize"], json!(14));
    let mut expected = ghosting::defaults().to_value();
    for (key, value) in source["personalization"]["extra"].as_object().unwrap() {
        expected[key.as_str()] = value.clone();
    }
    assert_eq!(p["extra"], expected);
    assert_eq!(
        codes::encode("ghosting-renewed", &source).unwrap(),
        code,
        "внешний и внутренний ID дают один код"
    );
    // Код можно вставить с переносами строк из сообщения.
    let wrapped: String = code
        .as_bytes()
        .chunks(60)
        .map(|part| format!("{}\n", std::str::from_utf8(part).unwrap()))
        .collect();
    assert_eq!(
        codes::decode("218", &json!(format!(" \n{wrapped}\t")))
            .unwrap()
            .to_value(),
        look
    );
}

#[test]
fn half_life_code_carries_crosshair_packs_bxt_and_fov() {
    let source = json!({
        "personalization": {"hudEnabled": true, "hudColor": {"r": 240, "g": 150, "b": 15}, "crossEnabled": true, "crossGap": 9},
        "packs": {"hud": "ag-hud", "crosshair": "crosshair-changer", "changerColor": "pink", "changerShape": "3px"},
        "bxt": {"timer": true, "jumpspeed": true, "fov": 111, "viewmodelFov": 77, "fixWidescreenFov": true},
    });
    let look = codes::decode("hl1-won", &json!(codes::encode("70", &source).unwrap()))
        .unwrap()
        .to_value();
    assert_eq!(look["packs"], source["packs"]);
    let flat = goldsrc::from_flat(&goldsrc::to_flat(&source["packs"], &source["bxt"]).to_value());
    assert_eq!(look["bxt"], flat.bxt.to_value());
    assert_eq!(look["personalization"]["crossGap"], json!(9));
    assert_eq!(look["personalization"]["extra"], json!({}));
}

#[test]
fn code_of_another_game_another_version_and_a_damaged_string_are_rejected() {
    let code = codes::encode("218", &json!({})).unwrap();
    let id = |text: &str| json!(text);
    assert_eq!(
        code_of(codes::decode("70", &id(&code))),
        "PERSONALIZATION_CODE_WRONG_GAME"
    );
    assert_eq!(
        code_of(codes::decode(
            "218",
            &id(&code.replace("HLSR-P1", "HLSR-P2"))
        )),
        "PERSONALIZATION_CODE_VERSION_UNSUPPORTED"
    );
    assert_eq!(
        code_of(codes::decode("220", &id(&code))),
        "PERSONALIZATION_CODE_GAME_UNSUPPORTED"
    );
    assert_eq!(
        code_of(codes::decode("constructor", &id(&code))),
        "PERSONALIZATION_CODE_GAME_UNSUPPORTED"
    );
    let zeroed = {
        let at = code.rfind('.').unwrap();
        format!("{}.0000000000000000", &code[..at])
    };
    let texts = [
        Value::Null,
        json!(""),
        json!("text"),
        json!(&code[..code.len() - 1]),
        json!(format!("{code}0")),
        json!(zeroed),
    ];
    for text in texts {
        assert_eq!(
            code_of(codes::decode("218", &text)),
            "PERSONALIZATION_CODE_INVALID",
            "{text}"
        );
    }
}

#[test]
fn even_a_code_with_a_valid_sum_does_not_allow_foreign_fields_commands_or_invalid_values() {
    let base = codes::decode("218", &json!(codes::encode("218", &json!({})).unwrap()))
        .unwrap()
        .to_value();
    let edit = |change: &dyn Fn(&mut Value)| {
        let mut value = base.clone();
        change(&mut value);
        value
    };
    let theme = edit(&|v| v["launcherTheme"] = json!("combine"));
    let args = edit(&|v| v["launchArgs"] = json!("+quit"));
    let command = edit(&|v| v["personalization"]["extra"]["exec"] = json!("unknown.cfg"));
    let separator = edit(&|v| v["personalization"]["extra"]["gh_name"] = json!("runner\";quit"));
    let number = edit(&|v| v["personalization"]["extra"]["viewmodel_fov"] = json!(10000));
    let kind = edit(&|v| v["personalization"]["hudEnabled"] = json!("yes"));
    let color = edit(&|v| v["personalization"]["hudColor"]["r"] = json!(-100));
    let prototype = base.to_string().replace(
        "\"personalization\":{",
        "\"personalization\":{\"__proto__\":{\"polluted\":true},",
    );
    let mut payloads: Vec<Vec<u8>> = [theme, args, command, separator, number, kind, color]
        .iter()
        .map(|v| v.to_string().into_bytes())
        .collect();
    payloads.push(prototype.into_bytes());
    payloads.push(b"[]".to_vec());
    payloads.push(br#"{"personalization":[]}"#.to_vec());
    for payload in payloads {
        let code = forged("218", &payload, &[]);
        assert_eq!(
            code_of(codes::decode("218", &json!(code))),
            "PERSONALIZATION_CODE_INVALID",
            "{}",
            String::from_utf8_lossy(&payload)
        );
    }
}

#[test]
fn length_inflation_and_extra_data_in_the_compressed_part_are_limited() {
    assert_eq!(
        code_of(codes::decode(
            "218",
            &json!("x".repeat(codes::MAX_CODE_LENGTH + 1))
        )),
        "PERSONALIZATION_CODE_TOO_LARGE"
    );
    let bomb = forged("218", &vec![0x61; codes::MAX_PAYLOAD_BYTES + 1], &[]);
    assert_eq!(
        code_of(codes::decode("218", &json!(bomb))),
        "PERSONALIZATION_CODE_TOO_LARGE"
    );
    let look = codes::decode("218", &json!(codes::encode("218", &json!({})).unwrap()))
        .unwrap()
        .to_value();
    let extra = forged("218", look.to_string().as_bytes(), b"extra");
    assert_eq!(
        code_of(codes::decode("218", &json!(extra))),
        "PERSONALIZATION_CODE_INVALID"
    );
}

#[test]
fn saving_an_imported_look_keeps_launch_settings_binds_and_other_games() {
    let dir = tempfile::tempdir().unwrap();
    let logger = Arc::new(MemoryLogger::default());
    let store = GameSettingsStore::new(dir.path().join("game-settings.json"), logger);
    store
        .save(
            "ghosting-renewed",
            &json!({"launchArgs": "-console", "language": "en", "rinput": false, "startTimer": true,
                "width": 1280, "height": 720, "binds": {"space": {"command": "+jump", "base": "+duck"}}}),
        )
        .unwrap();
    store
        .save(
            "hl1-won",
            &json!({"personalization": {"crossGap": 7}, "bxt": {"timer": true}}),
        )
        .unwrap();
    let before = store.get("ghosting-renewed").to_value();
    let other = store.get("hl1-won").to_value();
    assert_eq!(
        before["binds"].as_object().unwrap().len(),
        1,
        "в исходных настройках есть свой бинд"
    );
    let look = codes::decode(
        "218",
        &json!(
            codes::encode(
                "218",
                &json!({"personalization": {"hudEnabled": true, "extra": {"gh_health_x": 93}}})
            )
            .unwrap()
        ),
    )
    .unwrap()
    .to_value();
    // `store.save("ghosting-renewed", { ...before, ...look })`
    let mut merged = before.as_object().unwrap().clone();
    merged.extend(look.as_object().unwrap().clone());
    let result = store
        .save("ghosting-renewed", &Value::Object(merged))
        .unwrap();
    let mut result = result.to_value();
    assert_eq!(result["personalization"]["extra"]["gh_health_x"], json!(93));
    result["personalization"] = before["personalization"].clone();
    assert_eq!(result, before);
    assert_eq!(store.get("hl1-won").to_value(), other);
}

// ============================================================ hl1-selftest.js: параметры запуска

#[test]
fn hl1_launch_settings_resolution_comes_in_pairs_and_rinput_is_on_by_default() {
    let value = la::sanitize_settings(&json!({"width": 1920, "refreshRate": 7})).settings;
    assert!(value.rinput, "RInput по умолчанию включён");
    assert_eq!(value.width, 0, "разрешение без высоты отбрасывается");
    assert_eq!(value.refresh_rate, 0, "частота вне диапазона отбрасывается");
    assert!(
        !la::sanitize_settings(&json!({"rinput": false}))
            .settings
            .rinput
    );
}

#[test]
fn hl1_won_is_available_in_the_interface() {
    assert!(game_availability::is_game_available("70"));
}

// ============================================================ linux-selftest.js: параметры Proton

fn proton(text: &str) -> Result<la::ProtonOptions, la::ArgsError> {
    la::parse_proton_options(&json!(text))
}

fn proton_value(options: &la::ProtonOptions) -> Value {
    json!({"env": options.env_value(), "wrapper": options.wrapper, "args": options.args})
}

#[test]
fn proton_options_variables_wrapper_and_game_arguments_like_in_steam() {
    let all = proton(
        "PROTON_USE_WINED3D=1 DXVK_HUD=\"fps, frametimes\" mangohud --dlsym %command% -novid",
    )
    .unwrap();
    assert_eq!(
        proton_value(&all),
        json!({
            "env": {"PROTON_USE_WINED3D": "1", "DXVK_HUD": "fps, frametimes"},
            "wrapper": ["mangohud", "--dlsym"],
            "args": ["-novid"],
        })
    );
    assert_eq!(
        proton_value(&proton("mangohud").unwrap()),
        json!({"env": {}, "wrapper": ["mangohud"], "args": []}),
        "без %command% остаток строки — обёртка"
    );
    assert_eq!(
        proton_value(&proton("PROTON_LOG=1 %command%").unwrap()),
        json!({"env": {"PROTON_LOG": "1"}, "wrapper": [], "args": []})
    );
    assert_eq!(
        proton_value(&proton("").unwrap()),
        json!({"env": {}, "wrapper": [], "args": []})
    );
    let error = |text: &str| proton(text).unwrap_err().to_value();
    assert_eq!(
        error("WINEPREFIX=/tmp/x %command%"),
        json!({"code": "PROTECTED_ENV", "param": "WINEPREFIX"})
    );
    assert_eq!(
        error("STEAM_COMPAT_DATA_PATH=/x"),
        json!({"code": "PROTECTED_ENV", "param": "STEAM_COMPAT_DATA_PATH"})
    );
    assert_eq!(
        error("-novid"),
        json!({"code": "WRAPPER_INVALID", "param": "-novid"})
    );
    assert_eq!(
        error("%command% -game cstrike"),
        json!({"code": "FORBIDDEN_PARAM", "param": "-game"})
    );
    assert!(proton("%command% %command%").is_err());
    assert_eq!(
        error("mangohud \"%command%")["code"],
        json!("UNBALANCED_QUOTES")
    );

    assert_eq!(
        la::normalize_settings(&json!({"protonOptions": "  mangohud %command%  "}))
            .unwrap()
            .proton_options,
        "mangohud %command%"
    );
    assert_eq!(
        la::normalize_settings(&json!({"protonOptions": "GAMEID=1"}))
            .unwrap_err()
            .to_value(),
        json!({"field": "protonOptions", "code": "PROTECTED_ENV", "param": "GAMEID"}),
        "ошибка указывает на поле Proton"
    );
    let sanitized =
        la::sanitize_settings(&json!({"launchArgs": "-high", "protonOptions": "WINEPREFIX=/x"}));
    assert_eq!(
        sanitized.settings.proton_options, "",
        "испорченная строка из файла не доходит до запуска"
    );
    assert_eq!(sanitized.settings.launch_args, "-high");
    assert_eq!(sanitized.dropped.unwrap().field, Some("protonOptions"));
    assert_eq!(la::default_settings().proton_options, "");
}

// ============================================================ livesplit-overlay-selftest.js

#[test]
fn livesplit_overlay_defaults_are_off_and_junk_becomes_the_standard_values() {
    let base = overlay::default_overlay();
    assert!(!base.enabled);
    assert_eq!(
        overlay::sanitize_overlay(&Value::Null).to_value(),
        base.to_value()
    );
    let clean = overlay::sanitize_overlay(&json!({
        "enabled": "yes", "x": 140, "y": -3, "height": "2", "opacity": 55.6, "key": "chroma",
        "keyColor": {"r": 300, "g": "16", "b": null}, "crop": {"top": 9999, "left": "4"},
    }))
    .to_value();
    assert_eq!(clean["enabled"], json!(false), "включает только true");
    assert_eq!(clean["x"], json!(100));
    assert_eq!(clean["y"], json!(0));
    assert_eq!(clean["height"], json!(5));
    assert_eq!(clean["opacity"], json!(56));
    assert_eq!(clean["key"], json!("none"));
    assert_eq!(clean["keyColor"], json!({"r": 255, "g": 16, "b": 0}));
    assert_eq!(
        clean["crop"],
        json!({"top": 2048, "right": 0, "bottom": 0, "left": 4})
    );
    let with = |change: &dyn Fn(&mut Map<String, Value>)| {
        let mut value = base.to_value().as_object().unwrap().clone();
        change(&mut value);
        Value::Object(value)
    };
    assert_eq!(overlay::overlay_changes(&base.to_value()), 0);
    assert_eq!(
        overlay::overlay_changes(&with(&|v| {
            v.insert("x".into(), json!(0));
            v.insert("key".into(), json!("luma"));
        })),
        2
    );
    assert_eq!(
        overlay::overlay_changes(&with(&|v| {
            v.insert("enabled".into(), json!(true));
        })),
        0,
        "выключатель сброс раздела не трогает"
    );
}
