//! Модель Discord против 0.4.7 и 0.4.9. Фикстура `discord-model.json` из
//! `tools/reference/online/discord-model.js` (0.4.7, Node 24) — константы, карты и FPS,
//! `kindFromDelivery`, снимок диагностики, сценарии `discord-rpc.json` с переносом из
//! `settings.json`. Фикстура `discord-model-049.json` из `discord-model-049.js` (0.4.9) — новые
//! виды и константы, нормализация присутствия и активность на корпусе 0.4.7 плюс разделы и
//! эпизоды, `same`, `sameGame` и сценарии моста (раздел, рендер демки, статус чужой игры, эпизод).

mod support;

use hlsr_core::js;
use hlsr_core::json_text::js_number;
use hlsr_online::discord_rpc::constants::{
    self, ALLOWED_SMALL_IMAGE_KEYS, ALLOWED_SMALL_IMAGE_TEXTS, APPLICATION_ID, BUTTON_KEYS, Code,
    State, assets, channels, game_text, timings,
};
use hlsr_online::discord_rpc::diagnostics_model::{self, Diagnostics};
use hlsr_online::discord_rpc::discord_rich_presence_bridge::{
    BridgeOptions, ClientFactory, DiscordRichPresenceBridge,
};
use hlsr_online::discord_rpc::preference;
use hlsr_online::discord_rpc::presence_model::{
    self, ACTIVITY_KINDS, EPISODE_TITLES, FORBIDDEN_ACTIVITY_KEYS, GAME_ALIASES, GAMES, Kind,
    LAUNCHER_DETAILS, Locale, Presence,
};
use hlsr_online::game_presence::map_assets;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use support::{FakeClock, Mismatches, fixture, items, str_of};

/// `Date.UTC(2026, 9, 16, 12, 0, 0, 0)`.
const NOW: i64 = 1_792_152_000_000;

/// `{"undefined": true}` в фикстуре — `undefined` JS.
fn defined(value: &Value) -> Option<&Value> {
    (*value != json!({"undefined": true})).then_some(value)
}

fn object(pairs: impl IntoIterator<Item = (String, Value)>) -> Value {
    Value::Object(pairs.into_iter().collect::<Map<_, _>>())
}

/// Константы 0.4.7 с заменой и дополнением из 0.4.9 (`discord-model-049.json`).
fn constants_049() -> Value {
    let mut constants = fixture("discord-model.json")["constants"].clone();
    let fresh = fixture("discord-model-049.json");
    for (key, value) in fresh["constants"].as_object().unwrap() {
        constants[key] = value.clone();
    }
    constants
}

#[test]
fn constants_match_node() {
    let text = |locale: Locale| {
        let mut out = Map::new();
        out.insert("launcherDetails".into(), json!(LAUNCHER_DETAILS));
        for kind in Kind::ALL {
            if let Some(text) = presence_model::kind_text(locale, kind) {
                out.insert(kind.as_str().into(), json!(text));
            }
        }
        let buttons = object(BUTTON_KEYS.iter().map(|key| {
            (
                (*key).to_owned(),
                json!(presence_model::button_label(locale, key)),
            )
        }));
        out.insert("buttons".into(), buttons);
        Value::Object(out)
    };
    let actual = json!({
        "APPLICATION_ID": APPLICATION_ID,
        "ASSETS": {"large": assets::LARGE, "menu": assets::MENU, "smallIdle": assets::SMALL_IDLE,
            "smallRunning": assets::SMALL_RUNNING},
        "ALLOWED_SMALL_IMAGE_KEYS": ALLOWED_SMALL_IMAGE_KEYS,
        "ALLOWED_SMALL_IMAGE_TEXTS": ALLOWED_SMALL_IMAGE_TEXTS,
        "GAME_TEXT": {"title": game_text::TITLE, "menuState": game_text::MENU_STATE,
            "playingState": game_text::PLAYING_STATE, "runningSmall": game_text::RUNNING_SMALL,
            "fpsLabel": game_text::FPS_LABEL, "gameLabel": game_text::GAME_LABEL},
        "STATES": object(State::ALL.map(|s| (s.name(), json!(s.as_str())))),
        "CODES": object(Code::ALL.map(|c| (c.name().to_owned(), json!(c.as_str())))),
        "BUTTON_KEYS": BUTTON_KEYS,
        "TIMINGS": {"DEBOUNCE_MS": timings::DEBOUNCE_MS, "RETRY_BASE_MS": timings::RETRY_BASE_MS,
            "RETRY_MAX_MS": timings::RETRY_MAX_MS, "RETRY_JITTER": timings::RETRY_JITTER,
            "LOG_THROTTLE_MS": timings::LOG_THROTTLE_MS},
        "CHANNELS": {"get": channels::GET, "set": channels::SET, "state": channels::STATE,
            "activity": channels::ACTIVITY},
        "buttonUrls": constants::button_urls().iter()
            .map(|(key, url)| json!({"key": key, "url": url})).collect::<Vec<_>>(),
        "KINDS": object(Kind::ALL.map(|k| (k.as_str().to_uppercase(), json!(k.as_str())))),
        "KIND_LIST": Kind::ALL.map(Kind::as_str),
        "GAME_KINDS": Kind::GAME.map(Kind::as_str),
        "TEXT": {"ru": text(Locale::Ru), "en": text(Locale::En)},
        "GAMES": object(GAMES.map(|(id, title)| (id.to_owned(), json!({"title": title})))),
        "GAME_ALIASES": object(GAME_ALIASES.map(|(id, key)| (id.to_owned(), json!(key)))),
        "FORBIDDEN_ACTIVITY_KEYS": FORBIDDEN_ACTIVITY_KEYS,
        "DISABLED": Diagnostics::DISABLED.to_json(),
        "ALLOWED_KEYS": diagnostics_model::ALLOWED_KEYS,
        "BRIDGE_KEYS": diagnostics_model::BRIDGE_KEYS,
        "RECONNECT_STATES": diagnostics_model::RECONNECT_STATES.map(State::as_str),
        "SCHEMA_VERSION": preference::SCHEMA_VERSION,
        "FILE": preference::FILE,
        "LEGACY_FILE": preference::LEGACY_FILE,
        "LEGACY_KEY": preference::LEGACY_KEY,
        "DEFAULT_ENABLED": preference::DEFAULT_ENABLED,
        "SOURCES": {"DEFAULT": preference::sources::DEFAULT, "USER": preference::sources::USER,
            "MIGRATED": preference::sources::MIGRATED},
        "MAP_ASSET_KEYS": map_assets::MAP_ASSET_KEYS.to_vec(),
        "ALL_ASSET_KEYS": map_assets::all_asset_keys(),
        "FINAL_TIME_MAP": map_assets::FINAL_TIME_MAP,
        "FINAL_TIME_KEY": map_assets::FINAL_TIME_KEY,
        "FPS_MAX": js_number(map_assets::FPS_MAX),
        "ELAPSED_MAX_MS": js_number(map_assets::ELAPSED_MAX_MS),
        // 0.4.9: дописаны в конец константам 0.4.7 (`constants_049`).
        "LAUNCHER_KINDS": Kind::LAUNCHER.map(Kind::as_str),
        "ACTIVITY_KINDS": object(ACTIVITY_KINDS.map(|(key, kind)| (key.to_owned(), json!(kind.as_str())))),
        "EPISODE_TITLES": object(EPISODE_TITLES.map(|(id, title)| (id.to_owned(), json!(title)))),
    });
    let mut mismatches = Mismatches::default();
    mismatches.check(&actual, &constants_049(), "константы");
    mismatches.finish("константы Discord");
}

#[test]
fn maps_and_fps_match_node() {
    let fixture = fixture("discord-model.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["maps"]) {
        let name = map_assets::normalize_map_name(case["raw"].as_str());
        let name_ref = name.as_deref();
        let actual = json!({
            "raw": case["raw"],
            "name": name,
            "known": map_assets::is_known_map(name_ref),
            "key": map_assets::large_image_key_for(name_ref, false),
            "finalKey": map_assets::large_image_key_for(name_ref, true),
        });
        mismatches.check(&actual, case, format!("карта {}", case["raw"]));
    }
    for case in items(&fixture["fps"]) {
        let raw = case[0].as_f64().filter(|_| case[0].is_number());
        let actual = json!([
            case[0],
            map_assets::normalize_fps(raw),
            map_assets::normalize_elapsed_ms(raw.map(|x| x * 1e5)),
        ]);
        mismatches.check(&actual, case, format!("fps {}", case[0]));
    }
    mismatches.finish("карты и FPS");
}

/// `opts.version ? "HLSR v" + String(version) : "HLSR"`.
fn version_of(value: Option<&Value>) -> Option<String> {
    value.filter(|v| js::truthy(v)).map(js::to_string)
}

#[test]
fn presence_matches_node() {
    let fixture = fixture("discord-model-049.json");
    let mut mismatches = Mismatches::default();
    let mut normalized: Vec<Presence> = Vec::new();
    for (index, case) in items(&fixture["presence"]).iter().enumerate() {
        let model = presence_model::normalize(&case["state"]);
        let version = version_of(defined(&case["version"]));
        let activity = presence_model::to_activity(&case["state"], version.as_deref());
        mismatches.check(
            &model.to_json(),
            &case["normalized"],
            format!("normalize #{index} {}", case["state"]),
        );
        mismatches.check(
            &activity.to_json(),
            &case["activity"],
            format!("toActivity #{index} {}", case["state"]),
        );
        // Повторная нормализация ничего не меняет — служба отдаёт уже нормализованную модель.
        assert_eq!(
            presence_model::normalize(&model.to_json()),
            model,
            "normalize не идемпотентна: #{index}"
        );
        let json = activity.to_json();
        for key in FORBIDDEN_ACTIVITY_KEYS {
            assert!(json.get(key).is_none(), "в активности #{index} есть {key}");
        }
        normalized.push(model);
    }
    for case in items(&fixture["same"]) {
        let at = |value: &Value| value.as_u64().map(|i| &normalized[i as usize]);
        let actual = presence_model::same(at(&case[0]), at(&case[1]));
        mismatches.check(
            &json!([case[0], case[1], actual]),
            case,
            format!("same {} {}", case[0], case[1]),
        );
    }
    for case in items(&fixture["sameGame"]) {
        let actual = presence_model::same_game(defined(&case[0]), defined(&case[1]));
        mismatches.check(
            &json!([case[0], case[1], actual]),
            case,
            format!("sameGame {} {}", case[0], case[1]),
        );
    }
    for case in items(&support::fixture("discord-model.json")["kindFromDelivery"]) {
        let signal = defined(&case[0]).cloned().unwrap_or(Value::Null);
        let actual = presence_model::kind_from_delivery(&signal).as_str();
        mismatches.check(
            &json!([case[0], actual]),
            case,
            format!("kindFromDelivery {}", case[0]),
        );
    }
    mismatches.finish("присутствие Discord");
}

/// Сценарии моста 0.4.9: после каждого шага — модель, которую мост отдал службе, и её активность.
#[tokio::test]
async fn bridge_scenarios_match_node() {
    let fixture = fixture("discord-model-049.json");
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture["bridge"]) {
        let name = str_of(&scenario["name"]);
        let dir = tempfile::tempdir().unwrap();
        let mut options = BridgeOptions::new(
            dir.path(),
            Arc::new(hlsr_delivery::ports::MemoryLogger::default()),
        );
        options.now = FakeClock::new(NOW).clock();
        options.client = ClientFactory::Missing;
        let bridge = DiscordRichPresenceBridge::new(options);
        let mut results = Vec::new();
        for step in items(&scenario["steps"]) {
            let arg = &step[1];
            match str_of(&step[0]) {
                "signal" => bridge.on_signal(arg),
                "activity" => bridge.set_activity(defined(arg)),
                "demo" => bridge.set_demo_rendering(*arg == Value::Bool(true)),
                "game" => bridge.set_game_state(arg),
                other => panic!("неизвестный шаг {other}"),
            }
            results.push(bridge.pending_presence().map_or(Value::Null, |model| {
                json!({"model": model.to_json(), "activity": model.activity(None).to_json()})
            }));
        }
        mismatches.check(&Value::Array(results), &scenario["results"], name);
    }
    mismatches.finish("мост Discord");
}

#[test]
fn diagnostics_match_node() {
    let fixture = fixture("discord-model.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["diagnostics"]) {
        let actual = diagnostics_model::normalize(&case[0]).to_json();
        mismatches.check(&json!([case[0], actual]), case, format!("{}", case[0]));
    }
    mismatches.finish("диагностика Discord");
}

#[test]
fn preference_scenarios_match_node() {
    let fixture = fixture("discord-model.json");
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture["preference"]) {
        let name = str_of(&scenario["name"]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        support::setup(root, &scenario["setup"]);
        let mut results = Vec::new();
        for step in items(&scenario["steps"]) {
            let now = NOW + results.len() as i64 * 1000;
            let result = match str_of(&step["op"]) {
                "files" => Ok(support::files(root)),
                "read" => Ok(preference::read(root).to_json()),
                "set" => preference::set(root, step["enabled"] == Value::Bool(true), now)
                    .map(|p| p.to_json()),
                "reset" => preference::reset(root, now).map(|p| p.to_json()),
                "adoptDefault" => preference::adopt_default(root, None, now).map(|p| p.to_json()),
                "migrate" => preference::migrate(root, now).map(|p| p.to_json()),
                other => panic!("неизвестный шаг {other}"),
            };
            results.push(result.unwrap_or_else(|e| panic!("{name}: {}", e.message)));
        }
        mismatches.check(&Value::Array(results), &scenario["results"], name);
    }
    mismatches.finish("настройка discord-rpc.json");
}
