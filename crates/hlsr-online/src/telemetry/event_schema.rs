//! Схема событий телеметрии (`telemetry/event-schema.js`): конверт, 36 типов с полями, коды,
//! пределы и проверка события `validate` → `{ok, reason}`. Типы `run_*` и `map_*` описаны, но
//! выключены (`DISABLED_TYPES`): лаунчер их не создаёт.

use hlsr_core::js;
use hlsr_core::json_text::js_key_order;
use serde_json::Value;

pub const SCHEMA_VERSION: u32 = 1;

/// Поля конверта — в порядке записи события.
pub const ENVELOPE: [&str; 10] = [
    "schemaVersion",
    "eventId",
    "installationId",
    "sessionId",
    "type",
    "occurredAt",
    "launcherVersion",
    "locale",
    "windowsMajor",
    "windowsArch",
];

pub const ERROR_CODES: [&str; 21] = [
    "STEAM_NOT_RUNNING",
    "STEAM_NOT_INSTALLED",
    "SDK_2013_NOT_INSTALLED",
    "LICENSE_NOT_AVAILABLE",
    "HTTP_404",
    "HTTP_429",
    "HTTP_5XX",
    "NETWORK_TIMEOUT",
    "NETWORK_UNREACHABLE",
    "POSSIBLE_RESOURCE_BLOCK",
    "CHECKSUM_MISMATCH",
    "SIGNATURE_INVALID",
    "MANIFEST_INVALID",
    "EXTRACT_FAILED",
    "NO_SPACE",
    "REPAIR_FAILED",
    "PROFILE_IMPORT_FAILED",
    "LAUNCH_FAILED",
    "GAME_CRASH",
    "LAUNCHER_CRASH",
    "UNKNOWN_SAFE",
];

pub const EXIT_REASONS: [&str; 5] = ["normal", "user-closed", "crashed", "killed", "unknown"];
pub const PLAN_KINDS: [&str; 3] = ["full", "patch", "patch-chain"];
pub const FALLBACK_REASONS: [&str; 6] = [
    "PATCH_UNAVAILABLE",
    "PATCH_APPLY_FAILED",
    "CHECKSUM_MISMATCH",
    "CHAIN_TOO_LONG",
    "SOURCE_MISSING",
    "UNKNOWN_SAFE",
];

pub const LOCALES: [&str; 2] = ["ru", "en"];
pub const WINDOWS_MAJORS: [&str; 3] = ["10", "11", "unknown"];
pub const WINDOWS_ARCHS: [&str; 1] = ["x64"];

/// Поля, которым разрешён `null` (размер неизвестен).
pub const NULLABLE: [&str; 6] = [
    "downloadBytes",
    "downloadDurationMilliseconds",
    "averageBytesPerSecond",
    "patchBytes",
    "fullArchiveBytes",
    "savedBytes",
];

/// `LIMITS`: верхние границы целых полей.
pub mod limits {
    pub const DURATION_SECONDS: f64 = 2_592_000.0;
    pub const ELAPSED_MILLISECONDS: f64 = 360_000_000.0;
    pub const SEGMENT_MILLISECONDS: f64 = 360_000_000.0;
    pub const RESTART_COUNT: f64 = 100_000.0;
    pub const BYTES: f64 = 68_719_476_736.0;
    pub const DOWNLOAD_DURATION_MILLISECONDS: f64 = 604_800_000.0;
}

/// К чему относится `sessionId` события.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Сессия лаунчера.
    Launcher,
    /// Игровая сессия.
    Game,
    /// Без сессии: `sessionId` — `null`.
    None,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Launcher => "launcher",
            Kind::Game => "game",
            Kind::None => "none",
        }
    }
}

/// Тип события: вид сессии и поля в порядке записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeSpec {
    pub name: &'static str,
    pub kind: Kind,
    pub fields: &'static [&'static str],
}

const GAME: &[&str] = &["gameId", "gameVersion", "buildId"];
const RUN_POSITION: &[&str] = &[
    "gameId",
    "gameVersion",
    "buildId",
    "runId",
    "categoryId",
    "mapId",
    "chapterId",
    "elapsedMilliseconds",
    "restartCount",
];

const fn spec(name: &'static str, kind: Kind, fields: &'static [&'static str]) -> TypeSpec {
    TypeSpec { name, kind, fields }
}

/// `TYPES` в порядке объявления (`Object.keys(TYPES)`).
pub const TYPES: [TypeSpec; 36] = [
    spec("launcher_session_started", Kind::Launcher, &[]),
    spec("launcher_heartbeat", Kind::Launcher, &[]),
    spec(
        "launcher_session_ended",
        Kind::Launcher,
        &["durationSeconds"],
    ),
    spec("launcher_crash_detected", Kind::Launcher, &[]),
    spec("launcher_install_completed", Kind::None, &[]),
    spec("launcher_uninstall_initiated", Kind::None, &[]),
    spec("launcher_uninstall_completed", Kind::None, &[]),
    spec("game_launch_started", Kind::Game, GAME),
    spec("game_launch_succeeded", Kind::Game, GAME),
    spec(
        "game_launch_failed",
        Kind::Game,
        &["gameId", "gameVersion", "buildId", "errorCode"],
    ),
    spec("game_heartbeat", Kind::Game, GAME),
    spec(
        "game_session_ended",
        Kind::Game,
        &[
            "gameId",
            "gameVersion",
            "buildId",
            "durationSeconds",
            "exitReason",
        ],
    ),
    spec(
        "game_crashed",
        Kind::Game,
        &[
            "gameId",
            "gameVersion",
            "buildId",
            "durationSeconds",
            "errorCode",
        ],
    ),
    spec(
        "run_started",
        Kind::Game,
        &[
            "gameId",
            "gameVersion",
            "buildId",
            "runId",
            "categoryId",
            "mapId",
            "chapterId",
        ],
    ),
    spec("run_restarted", Kind::Game, RUN_POSITION),
    spec(
        "run_completed",
        Kind::Game,
        &[
            "gameId",
            "gameVersion",
            "buildId",
            "runId",
            "categoryId",
            "elapsedMilliseconds",
            "restartCount",
            "isPersonalBest",
        ],
    ),
    spec("run_abandoned", Kind::Game, RUN_POSITION),
    spec(
        "run_personal_best",
        Kind::Game,
        &[
            "gameId",
            "gameVersion",
            "buildId",
            "runId",
            "categoryId",
            "elapsedMilliseconds",
        ],
    ),
    spec(
        "run_heartbeat",
        Kind::Game,
        &[
            "gameId",
            "gameVersion",
            "buildId",
            "runId",
            "categoryId",
            "mapId",
            "chapterId",
            "elapsedMilliseconds",
        ],
    ),
    spec(
        "map_entered",
        Kind::Game,
        &[
            "gameId",
            "gameVersion",
            "buildId",
            "runId",
            "mapId",
            "chapterId",
            "elapsedMilliseconds",
        ],
    ),
    spec(
        "map_left",
        Kind::Game,
        &[
            "gameId",
            "gameVersion",
            "buildId",
            "runId",
            "mapId",
            "chapterId",
            "elapsedMilliseconds",
            "segmentMilliseconds",
        ],
    ),
    spec(
        "game_update_available",
        Kind::Launcher,
        &["gameId", "fromVersion", "toVersion"],
    ),
    spec(
        "game_download_started",
        Kind::Launcher,
        &["gameId", "fromVersion", "toVersion", "planKind"],
    ),
    spec(
        "game_download_completed",
        Kind::Launcher,
        &[
            "gameId",
            "fromVersion",
            "toVersion",
            "planKind",
            "downloadBytes",
            "downloadDurationMilliseconds",
            "averageBytesPerSecond",
            "patchBytes",
            "fullArchiveBytes",
            "savedBytes",
        ],
    ),
    spec(
        "game_download_cancelled",
        Kind::Launcher,
        &[
            "gameId",
            "fromVersion",
            "toVersion",
            "planKind",
            "downloadBytes",
        ],
    ),
    spec(
        "game_update_installed",
        Kind::Launcher,
        &["gameId", "fromVersion", "toVersion", "planKind"],
    ),
    spec(
        "game_update_failed",
        Kind::Launcher,
        &[
            "gameId",
            "fromVersion",
            "toVersion",
            "planKind",
            "errorCode",
            "fallbackReason",
        ],
    ),
    spec(
        "game_repair_started",
        Kind::Launcher,
        &["gameId", "gameVersion"],
    ),
    spec(
        "game_repair_completed",
        Kind::Launcher,
        &["gameId", "gameVersion", "durationSeconds"],
    ),
    spec(
        "game_repair_failed",
        Kind::Launcher,
        &["gameId", "gameVersion", "errorCode"],
    ),
    spec(
        "launcher_update_available",
        Kind::Launcher,
        &["fromVersion", "toVersion"],
    ),
    spec(
        "launcher_download_started",
        Kind::Launcher,
        &["fromVersion", "toVersion"],
    ),
    spec(
        "launcher_download_completed",
        Kind::Launcher,
        &[
            "fromVersion",
            "toVersion",
            "downloadBytes",
            "downloadDurationMilliseconds",
            "averageBytesPerSecond",
        ],
    ),
    spec(
        "launcher_download_cancelled",
        Kind::Launcher,
        &["fromVersion", "toVersion"],
    ),
    spec(
        "launcher_update_installed",
        Kind::Launcher,
        &["fromVersion", "toVersion"],
    ),
    spec(
        "launcher_update_failed",
        Kind::Launcher,
        &["fromVersion", "toVersion", "errorCode"],
    ),
];

/// Описанные, но выключенные типы: `push` их отвергает.
pub const DISABLED_TYPES: [&str; 8] = [
    "run_started",
    "run_restarted",
    "run_completed",
    "run_abandoned",
    "run_personal_best",
    "run_heartbeat",
    "map_entered",
    "map_left",
];

/// Тип по имени (`TYPES[type]`).
pub fn type_spec(name: &str) -> Option<&'static TypeSpec> {
    TYPES.iter().find(|spec| spec.name == name)
}

/// `UUID_RE`: `8-4-4-4-12` шестнадцатеричных цифр без учёта регистра, версия 1–5, вариант 8, 9, a, b.
pub fn is_uuid(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 5
        && parts.iter().zip([8, 4, 4, 4, 12]).all(|(part, length)| {
            part.len() == length && part.bytes().all(|b| b.is_ascii_hexdigit())
        })
        && matches!(parts[2].as_bytes()[0], b'1'..=b'5')
        && matches!(
            parts[3].as_bytes()[0],
            b'8' | b'9' | b'a' | b'b' | b'A' | b'B'
        )
}

/// `VERSION_RE`: `^[0-9A-Za-z][0-9A-Za-z.+-]{0,63}$`.
pub fn is_version(text: &str) -> bool {
    let bytes = text.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
}

/// `BUILD_RE` (`^[a-z0-9]+([._-][a-z0-9]+)*$`) и длина 3…128.
fn is_build_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    let word = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    (3..=128).contains(&bytes.len())
        && bytes
            .split(|b| matches!(b, b'.' | b'_' | b'-'))
            .all(|part| !part.is_empty() && part.iter().all(word))
}

/// `ID_RE`: `^[0-9a-z][0-9a-z._-]{0,63}$` без учёта регистра.
fn is_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `TIMESTAMP_RE`: `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?Z$` (`\d` — только ASCII).
fn is_timestamp(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    if bytes.len() < 20 || bytes[bytes.len() - 1] != b'Z' {
        return false;
    }
    let head = digits(0..4)
        && bytes[4] == b'-'
        && digits(5..7)
        && bytes[7] == b'-'
        && digits(8..10)
        && bytes[10] == b'T'
        && digits(11..13)
        && bytes[13] == b':'
        && digits(14..16)
        && bytes[16] == b':'
        && digits(17..19);
    let fraction = &bytes[19..bytes.len() - 1];
    head && (fraction.is_empty()
        || (fraction[0] == b'.'
            && (2..=4).contains(&fraction.len())
            && fraction[1..].iter().all(u8::is_ascii_digit)))
}

/// `Number.isInteger(value) && value >= 0 && value <= max`.
fn is_bounded_int(value: &Value, max: f64) -> bool {
    value
        .as_f64()
        .filter(|_| value.is_number())
        .is_some_and(|x| x.is_finite() && x.fract() == 0.0 && x >= 0.0 && x <= max)
}

/// `list.indexOf(value) !== -1`: строгое равенство — только строка из списка.
fn one_of(list: &[&str], value: &Value) -> bool {
    value.as_str().is_some_and(|text| list.contains(&text))
}

/// `checkField(name, value)`: значение поля по его правилу; незнакомое поле — `false`.
pub fn check_field(name: &str, value: &Value) -> bool {
    let text = || value.as_str();
    match name {
        "gameId" | "mapId" | "chapterId" | "categoryId" => text().is_some_and(is_id),
        "gameVersion" | "fromVersion" | "toVersion" => text().is_some_and(is_version),
        // `value.length` — единицы UTF-16, но BUILD_RE пускает только ASCII.
        "buildId" => text().is_some_and(is_build_id),
        "runId" => text().is_some_and(is_uuid),
        "errorCode" => one_of(&ERROR_CODES, value),
        "exitReason" => one_of(&EXIT_REASONS, value),
        "planKind" => one_of(&PLAN_KINDS, value),
        "fallbackReason" => one_of(&FALLBACK_REASONS, value),
        "durationSeconds" => is_bounded_int(value, limits::DURATION_SECONDS),
        "elapsedMilliseconds" => is_bounded_int(value, limits::ELAPSED_MILLISECONDS),
        "segmentMilliseconds" => is_bounded_int(value, limits::SEGMENT_MILLISECONDS),
        "restartCount" => is_bounded_int(value, limits::RESTART_COUNT),
        "downloadBytes"
        | "patchBytes"
        | "fullArchiveBytes"
        | "savedBytes"
        | "averageBytesPerSecond" => is_bounded_int(value, limits::BYTES),
        "downloadDurationMilliseconds" => {
            is_bounded_int(value, limits::DOWNLOAD_DURATION_MILLISECONDS)
        }
        "isPersonalBest" => value.is_boolean(),
        _ => false,
    }
}

/// Итог [`validate`]: `{ok: true}` или `{ok: false, reason}`.
pub type Validation = Result<(), String>;

/// `validate(event)`: событие по схеме или первая причина отказа (`not-object`, `unknown-type`,
/// `unknown-field:<ключ>`, `schema-version`, `event-id`, …, `missing:<поле>`, `null:<поле>`,
/// `bad:<поле>`) — в том же порядке проверок, что у 0.4.7.
pub fn validate(event: &Value) -> Validation {
    let fail = |reason: &str| Err(reason.to_owned());
    let Value::Object(map) = event else {
        return fail("not-object");
    };
    // `TYPES[event.type]`: ключ — строка значения (`String(type)`).
    let Some(spec) = map
        .get("type")
        .and_then(|value| type_spec(&js::to_string(value)))
    else {
        return fail("unknown-type");
    };
    for (key, _) in js_key_order(map) {
        if !ENVELOPE.contains(&key.as_str()) && !spec.fields.contains(&key.as_str()) {
            return Err(format!("unknown-field:{key}"));
        }
    }
    let field = |key: &str| map.get(key);
    let text = |key: &str| field(key).and_then(Value::as_str);
    if !field("schemaVersion")
        .and_then(Value::as_f64)
        .is_some_and(|version| version == f64::from(SCHEMA_VERSION))
    {
        return fail("schema-version");
    }
    if !text("eventId").is_some_and(is_uuid) {
        return fail("event-id");
    }
    if !text("installationId").is_some_and(is_uuid) {
        return fail("installation-id");
    }
    if spec.kind == Kind::None {
        if field("sessionId") != Some(&Value::Null) {
            return fail("session-forbidden");
        }
    } else if !text("sessionId").is_some_and(is_uuid) {
        return fail("session-id");
    }
    if !text("occurredAt").is_some_and(is_timestamp) {
        return fail("occurred-at");
    }
    if !text("launcherVersion").is_some_and(is_version) {
        return fail("launcher-version");
    }
    let listed = |key: &str, list: &[&str]| field(key).is_some_and(|value| one_of(list, value));
    if !listed("locale", &LOCALES) {
        return fail("locale");
    }
    if !listed("windowsMajor", &WINDOWS_MAJORS) {
        return fail("windows-major");
    }
    if !listed("windowsArch", &WINDOWS_ARCHS) {
        return fail("windows-arch");
    }
    for name in spec.fields {
        match field(name) {
            None => return Err(format!("missing:{name}")),
            Some(Value::Null) if NULLABLE.contains(name) => {}
            Some(Value::Null) => return Err(format!("null:{name}")),
            Some(value) if !check_field(name, value) => return Err(format!("bad:{name}")),
            Some(_) => {}
        }
    }
    Ok(())
}

/// `isEnabledType(type)`: тип описан и не выключен.
pub fn is_enabled_type(name: &str) -> bool {
    type_spec(name).is_some() && !DISABLED_TYPES.contains(&name)
}

/// `gameTelemetryConnected()`: игровые события включены.
pub fn game_telemetry_connected() -> bool {
    is_enabled_type("game_heartbeat")
}

/// `emittedTypes()`: типы, которые лаунчер создаёт (без `launcher_uninstall_completed`).
pub fn emitted_types() -> Vec<&'static str> {
    TYPES
        .iter()
        .map(|spec| spec.name)
        .filter(|&name| name != "launcher_uninstall_completed" && is_enabled_type(name))
        .collect()
}
