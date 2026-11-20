//! `game-presence/map-assets.js`: карты HL2 с картинкой в приложении Discord, разбор имени карты,
//! FPS и времени забега из сообщений игры. Нужен и модели Discord, и мосту игры (подпроект 3),
//! поэтому лежит в `hlsr-core`; `hlsr-online::game_presence::map_assets` повторяет его.

use crate::js;

/// `MAP_ASSET_KEYS`: карты, для которых у приложения Discord есть картинка.
pub const MAP_ASSET_KEYS: [&str; 66] = [
    "d1_canals_01",
    "d1_canals_01a",
    "d1_canals_02",
    "d1_canals_03",
    "d1_canals_05",
    "d1_canals_06",
    "d1_canals_07",
    "d1_canals_08",
    "d1_canals_09",
    "d1_canals_10",
    "d1_canals_11",
    "d1_canals_12",
    "d1_canals_13",
    "d1_eli_01",
    "d1_eli_02",
    "d1_town_01",
    "d1_town_01a",
    "d1_town_02",
    "d1_town_02a",
    "d1_town_04",
    "d1_town_05",
    "d1_trainstation_01",
    "d1_trainstation_02",
    "d1_trainstation_03",
    "d1_trainstation_04",
    "d1_trainstation_05",
    "d1_trainstation_06",
    "d2_coast_01",
    "d2_coast_03",
    "d2_coast_04",
    "d2_coast_05",
    "d2_coast_07",
    "d2_coast_09",
    "d2_coast_10",
    "d2_coast_11",
    "d2_coast_12",
    "d2_prison_01",
    "d2_prison_02",
    "d2_prison_03",
    "d2_prison_04",
    "d2_prison_05",
    "d2_prison_06",
    "d2_prison_07",
    "d2_prison_08",
    "d3_breen_01",
    "d3_c17_01",
    "d3_c17_02",
    "d3_c17_03",
    "d3_c17_04",
    "d3_c17_05",
    "d3_c17_06a",
    "d3_c17_06b",
    "d3_c17_07",
    "d3_c17_08",
    "d3_c17_09",
    "d3_c17_10a",
    "d3_c17_10b",
    "d3_c17_11",
    "d3_c17_12",
    "d3_c17_12b",
    "d3_c17_13",
    "d3_citadel_01",
    "d3_citadel_02",
    "d3_citadel_03",
    "d3_citadel_04",
    "d3_citadel_05",
];

/// Карта, на которой показывается картинка итогового времени.
pub const FINAL_TIME_MAP: &str = "d3_breen_01";
pub const FINAL_TIME_KEY: &str = "d3_breen_01_finaltime";

/// `ALL_ASSET_KEYS`: карты, итоговое время, меню и значок лаунчера.
pub fn all_asset_keys() -> Vec<&'static str> {
    let mut keys = MAP_ASSET_KEYS.to_vec();
    keys.extend([FINAL_TIME_KEY, "background", "hlsr"]);
    keys
}

pub const FPS_MAX: f64 = 1000.0;
/// Сутки в мс: дольше забег не показывается.
pub const ELAPSED_MAX_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;
/// Предел длины сырого имени (единицы UTF-16).
const RAW_MAP_MAX_LENGTH: usize = 200;

/// `normalizeMapName(raw)`: имя без `maps/` и `.bsp`, строчными, только `[a-z0-9_]{1,64}`;
/// иначе `None`. Длина сырого имени — до 200 единиц UTF-16 вместе с пробелами по краям.
pub fn normalize_map_name(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let length = raw.encode_utf16().count();
    if length == 0 || length > RAW_MAP_MAX_LENGTH {
        return None;
    }
    let value = js::trim(raw).to_lowercase().replace('\\', "/");
    let value = value.strip_prefix("maps/").unwrap_or(&value);
    let value = value.strip_suffix(".bsp").unwrap_or(value);
    let valid = (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    valid.then(|| value.to_owned())
}

/// `isKnownMap(name)`: у карты есть картинка.
pub fn is_known_map(name: Option<&str>) -> bool {
    known(name).is_some()
}

fn known(name: Option<&str>) -> Option<&'static str> {
    let name = name?;
    MAP_ASSET_KEYS.iter().copied().find(|key| *key == name)
}

/// `largeImageKeyFor(mapName, finalTime)`: картинка карты; итоговое время — только на
/// [`FINAL_TIME_MAP`]; неизвестная карта — значок лаунчера.
pub fn large_image_key_for(map_name: Option<&str>, final_time: bool) -> &'static str {
    match known(map_name) {
        None => "hlsr",
        Some(FINAL_TIME_MAP) if final_time => FINAL_TIME_KEY,
        Some(key) => key,
    }
}

/// `normalizeFps(raw)`: число от 0 до 1000, округлённое как `Math.round`; иначе `None`.
pub fn normalize_fps(raw: Option<f64>) -> Option<i64> {
    let raw = raw.filter(|x| x.is_finite() && (0.0..=FPS_MAX).contains(x))?;
    // Не больше 1000 — в i64 без потерь.
    Some(js::math_round(raw) as i64)
}

/// `normalizeElapsedMs(raw)`: мс от 0 до суток, `Math.floor`; иначе `None`.
pub fn normalize_elapsed_ms(raw: Option<f64>) -> Option<i64> {
    let raw = raw.filter(|x| x.is_finite() && (0.0..=ELAPSED_MAX_MS).contains(x))?;
    Some(raw.floor() as i64)
}
