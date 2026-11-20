//! Настройки запуска игры (`launch-args.js`): умолчания, нормализация и санитизация настроек из
//! файла и окна, запрещённые параметры, параметры Proton (`PROTON_X=1 mangohud %command% -novid`),
//! сборка аргументов пользователя поверх доверенных аргументов сборки.
//!
//! Настройки приходят как произвольный JSON (файл `game-settings.json`, ответ окна), поэтому
//! функции принимают `&Value`, а результат — [`Settings`] с теми же полями и порядком ключей
//! ([`Settings::to_value`]), что у объекта 0.4.7. Строка длиной больше 512 единиц UTF-16,
//! управляющие знаки, непарные кавычки и параметры, которые лаунчер задаёт сам (`-game`,
//! `-steam`…), отклоняются с кодом ошибки в [`ArgsError`].

use crate::game_binds::{self, Overrides, overrides_to_value};
use crate::ghosting_personalization::{self, Personalization};
use crate::goldsrc_packs;
use crate::goldsrc_personalization;
use crate::goldsrc_settings::{Extras, PackSelection};
use crate::js_value::{prop, to_number};
use crate::livesplit_overlay::{self, Overlay};
use hlsr_core::js;
use hlsr_core::js::object::JsObject;
use hlsr_core::js::text::utf16_len;
use serde_json::{Map, Value};
use std::collections::HashSet;

pub const MAX_ARGS_LENGTH: usize = 512;

/// Параметры, которые задаёт сам лаунчер: игрок их не меняет.
pub const FORBIDDEN_PARAMS: [&str; 6] = [
    "game",
    "steam",
    "applaunch",
    "appid",
    "basedir",
    "defaultgamedir",
];

/// Аргументы, которые есть в запуске всегда.
pub const ALWAYS_ARGUMENTS: [&str; 1] = ["-insecure"];

/// Переключатель настроек → аргументы запуска; порядок — порядок аргументов.
pub const TOGGLE_ARGUMENTS: [(&str, &[&str]); 8] = [
    ("console", &["-console"]),
    ("novid", &["-novid"]),
    ("borderless", &["-windowed", "-noborder"]),
    ("dx9", &["-dx9"]),
    ("nocrashdialog", &["-nocrashdialog"]),
    ("noasync", &["-noasync"]),
    ("noaafonts", &["-noaafonts"]),
    ("high", &["-high"]),
];

/// Поля HL1 (GoldSrc). Аргументы из них собирает `goldsrc-launch.js`, Source-игры их не читают.
pub const GOLDSRC_FLAGS: [&str; 4] = ["windowed", "developer", "condebug", "nojoy"];

const WIDTH_LIMITS: (f64, f64) = (320.0, 7680.0);
const HEIGHT_LIMITS: (f64, f64) = (200.0, 4320.0);
const REFRESH_LIMITS: (f64, f64) = (24.0, 540.0);

pub const LANGUAGE_AUTO: &str = "auto";

pub const MAX_AFFINITY_CPUS: u32 = 64;

/// Слово `%command%` в параметрах Proton: вокруг него строка делится на обёртку и аргументы игры.
pub const PROTON_COMMAND_TOKEN: &str = "%command%";

/// Эти переменные задаёт сам лаунчер: без них игра уйдёт в чужой префикс или не запустится.
const PROTECTED_ENV: [&str; 9] = [
    "WINEPREFIX",
    "PROTONPATH",
    "PROTON_VERB",
    "GAMEID",
    "STORE",
    "UMU_ID",
    "UMU_USE_STEAM",
    "SteamAppId",
    "SteamGameId",
];
const PROTECTED_ENV_PREFIXES: [&str; 1] = ["STEAM_COMPAT_"];

/// Отказ проверки параметров: `{ field?, code, param?, limit? }` в порядке ключей 0.4.7.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}")]
pub struct ArgsError {
    /// Поле настроек, к которому относится ошибка (`protonOptions`).
    pub field: Option<&'static str>,
    pub code: &'static str,
    /// Параметр или переменная, из-за которых отказ.
    pub param: Option<String>,
    /// Предел длины (`TOO_LONG`).
    pub limit: Option<usize>,
}

impl ArgsError {
    pub fn new(code: &'static str) -> ArgsError {
        ArgsError {
            field: None,
            code,
            param: None,
            limit: None,
        }
    }

    fn with_param(code: &'static str, param: &str) -> ArgsError {
        ArgsError {
            param: Some(param.to_owned()),
            ..ArgsError::new(code)
        }
    }

    /// `Object.assign({ field }, error)`.
    pub fn with_field(self, field: &'static str) -> ArgsError {
        ArgsError {
            field: Some(field),
            ..self
        }
    }

    /// Объект ошибки.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        if let Some(field) = self.field {
            object.insert("field".into(), Value::String(field.to_owned()));
        }
        object.insert("code".into(), Value::String(self.code.to_owned()));
        if let Some(param) = &self.param {
            object.insert("param".into(), Value::String(param.clone()));
        }
        if let Some(limit) = self.limit {
            object.insert("limit".into(), Value::from(limit));
        }
        Value::Object(object)
    }
}

/// `languageOf(input)`: язык игры из настроек — две–двадцать четыре строчные латинские буквы
/// или `auto`.
pub fn language_of(input: &Value) -> String {
    let value = prop(input, "language")
        .and_then(Value::as_str)
        .map(|text| js::trim(text).to_lowercase())
        .unwrap_or_default();
    if (2..=24).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_lowercase()) {
        value
    } else {
        LANGUAGE_AUTO.to_owned()
    }
}

/// `affinityMaskWithoutCpu0(cpuCount)`: десятичная запись маски ЦП без нулевого процессора;
/// `None`, если процессоров меньше двух. `cpu_count` — результат `Number(...)`.
pub fn affinity_mask_without_cpu0(cpu_count: f64) -> Option<String> {
    // `Number(cpuCount) || 0`: NaN и 0 дают 0.
    let number = if cpu_count.is_nan() { 0.0 } else { cpu_count };
    let count = number.floor().max(0.0).min(f64::from(MAX_AFFINITY_CPUS)) as u32;
    if count < 2 {
        return None;
    }
    let all = (1u128 << count) - 1;
    Some((all & !1).to_string())
}

/// Переменная окружения, которую лаунчер задаёт сам.
pub fn is_protected_env(name: &str) -> bool {
    PROTECTED_ENV.contains(&name)
        || PROTECTED_ENV_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// `paramNameOf(token)`: имя параметра без приставки (`--`, `-`, `+`, `/`) и значения (`=`, `:`),
/// строчными буквами; у слова без приставки имени нет. Регулярное выражение 0.4.7:
/// `/^(?:--|[-+/])([^=:]+)/`.
pub fn param_name_of(token: &str) -> Option<String> {
    fn name(rest: &str) -> Option<&str> {
        let end = rest.find(['=', ':']).unwrap_or(rest.len());
        (end > 0).then(|| &rest[..end])
    }
    // Сначала `--`; если после него нет имени (`--=x`), подходит одиночный `-` и вторая черта
    // становится началом имени.
    if let Some(found) = token.strip_prefix("--").and_then(name) {
        return Some(found.to_lowercase());
    }
    let rest = token.strip_prefix(['-', '+', '/'])?;
    name(rest).map(str::to_lowercase)
}

/// `isForbidden(token)`.
pub fn is_forbidden(token: &str) -> bool {
    param_name_of(token).is_some_and(|name| FORBIDDEN_PARAMS.contains(&name.as_str()))
}

/// Разбор строки аргументов: слова через пробел, кавычки склеивают слова, пустые слова пропадают.
pub fn parse_args_text(raw: &str) -> Result<Vec<String>, ArgsError> {
    if utf16_len(raw) > MAX_ARGS_LENGTH {
        return Err(ArgsError {
            limit: Some(MAX_ARGS_LENGTH),
            ..ArgsError::new("TOO_LONG")
        });
    }
    if raw.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') {
        return Err(ArgsError::new("CONTROL_CHARS"));
    }
    let mut args = Vec::new();
    let mut current = String::new();
    let mut has_current = false;
    let mut in_quotes = false;
    for ch in raw.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
            has_current = has_current || !current.is_empty();
            continue;
        }
        if ch == ' ' && !in_quotes {
            if has_current && !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
            current.clear();
            has_current = false;
            continue;
        }
        current.push(ch);
        has_current = true;
    }
    if in_quotes {
        return Err(ArgsError::new("UNBALANCED_QUOTES"));
    }
    if has_current && !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

/// `parseArgsString(raw)`: `null` (и отсутствие) — пустой список, не строка — `INVALID_TYPE`.
pub fn parse_args_string(raw: &Value) -> Result<Vec<String>, ArgsError> {
    match raw {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => parse_args_text(text),
        _ => Err(ArgsError::new("INVALID_TYPE")),
    }
}

/// `validateArgsString(raw)`: разбор плюс отказ на запрещённом параметре.
pub fn validate_args_string(raw: &Value) -> Result<Vec<String>, ArgsError> {
    let args = parse_args_string(raw)?;
    if let Some(token) = args.iter().find(|token| is_forbidden(token)) {
        return Err(ArgsError::with_param("FORBIDDEN_PARAM", token));
    }
    Ok(args)
}

/// Параметры Proton, разобранные как поле «Параметры запуска» в Steam.
#[derive(Debug, Clone)]
pub struct ProtonOptions {
    /// Переменные в начале строки — в окружение.
    pub env: JsObject<String>,
    /// Слова до `%command%` — программа-обёртка вокруг `umu-run`.
    pub wrapper: Vec<String>,
    /// Слова после `%command%` — аргументы игры.
    pub args: Vec<String>,
}

impl ProtonOptions {
    /// Объект `env` в порядке ключей JS.
    pub fn env_value(&self) -> Value {
        Value::Object(
            self.env
                .iter()
                .map(|(key, value)| (key.to_owned(), Value::String(value.clone())))
                .collect(),
        )
    }
}

/// `ENV_ASSIGNMENT`: `ИМЯ=значение`, имя — `[A-Za-z_][A-Za-z0-9_]*`.
fn env_assignment(token: &str) -> Option<(&str, &str)> {
    let (name, value) = token.split_once('=')?;
    let mut chars = name.chars();
    let first = chars.next()?;
    let valid = (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then_some((name, value))
}

/// `parseProtonOptions(raw)`.
pub fn parse_proton_options(raw: &Value) -> Result<ProtonOptions, ArgsError> {
    let parsed = parse_args_string(raw)?;
    let mut env = JsObject::new();
    let mut wrapper = Vec::new();
    let mut args = Vec::new();
    let mut at = 0;
    while let Some((name, value)) = parsed.get(at).and_then(|token| env_assignment(token)) {
        if is_protected_env(name) {
            return Err(ArgsError::with_param("PROTECTED_ENV", name));
        }
        // `env["__proto__"] = "строка"` в JS ничего не меняет: присвоить прототипу можно только объект.
        if name != "__proto__" {
            env.insert(name, value.to_owned());
        }
        at += 1;
    }
    let mut after_command = false;
    for token in &parsed[at..] {
        if token == PROTON_COMMAND_TOKEN {
            if after_command {
                return Err(ArgsError::new("INVALID"));
            }
            after_command = true;
        } else if after_command {
            if is_forbidden(token) {
                return Err(ArgsError::with_param("FORBIDDEN_PARAM", token));
            }
            args.push(token.clone());
        } else {
            if wrapper.is_empty() && token.starts_with(['-', '+']) {
                return Err(ArgsError::with_param("WRAPPER_INVALID", token));
            }
            wrapper.push(token.clone());
        }
    }
    Ok(ProtonOptions { env, wrapper, args })
}

/// Настройки игры после проверки: те же поля и порядок ключей, что у объекта 0.4.7.
#[derive(Debug, Clone)]
pub struct Settings {
    pub launch_args: String,
    pub proton_options: String,
    pub console: bool,
    pub novid: bool,
    pub borderless: bool,
    pub dx9: bool,
    pub nocrashdialog: bool,
    pub noasync: bool,
    pub noaafonts: bool,
    pub high: bool,
    pub exclude_cpu0: bool,
    pub start_timer: bool,
    /// Vulkan через DXVK (`vulkan_backend`), не аргумент запуска.
    pub vulkan: bool,
    pub language: String,
    pub rinput: bool,
    pub rinput_menu_fix: bool,
    pub windowed: bool,
    pub developer: bool,
    pub condebug: bool,
    pub nojoy: bool,
    /// Разрешение имеет смысл только парой: одно число без другого — 0.
    pub width: u32,
    pub height: u32,
    pub refresh_rate: u32,
    pub packs: PackSelection,
    pub bxt: Extras,
    pub binds: Overrides,
    pub personalization: Personalization,
    pub live_split_overlay: Overlay,
    /// Это `defaultSettings()`: `launchArgs` и `protonOptions` в объекте идут первыми, а не
    /// последними, как у `sanitizeSettings` и `normalizeSettings`.
    defaults_order: bool,
}

impl Settings {
    /// Включён ли переключатель из [`TOGGLE_ARGUMENTS`].
    pub fn toggle(&self, key: &str) -> bool {
        match key {
            "console" => self.console,
            "novid" => self.novid,
            "borderless" => self.borderless,
            "dx9" => self.dx9,
            "nocrashdialog" => self.nocrashdialog,
            "noasync" => self.noasync,
            "noaafonts" => self.noaafonts,
            "high" => self.high,
            other => unreachable!("{other} — не переключатель аргументов"),
        }
    }

    /// Объект настроек в порядке ключей 0.4.7: `launchArgs` и `protonOptions` — в конце, как у
    /// `sanitizeSettings` и `normalizeSettings`, и в начале у [`default_settings`].
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        let mut put = |key: &str, value: Value| {
            object.insert(key.to_owned(), value);
        };
        if self.defaults_order {
            put("launchArgs", Value::String(self.launch_args.clone()));
            put("protonOptions", Value::String(self.proton_options.clone()));
        }
        for (key, _) in TOGGLE_ARGUMENTS {
            put(key, Value::Bool(self.toggle(key)));
        }
        put("excludeCpu0", Value::Bool(self.exclude_cpu0));
        put("startTimer", Value::Bool(self.start_timer));
        put("vulkan", Value::Bool(self.vulkan));
        put("language", Value::String(self.language.clone()));
        put("rinput", Value::Bool(self.rinput));
        put("rinputMenuFix", Value::Bool(self.rinput_menu_fix));
        put("windowed", Value::Bool(self.windowed));
        put("developer", Value::Bool(self.developer));
        put("condebug", Value::Bool(self.condebug));
        put("nojoy", Value::Bool(self.nojoy));
        put("width", Value::from(self.width));
        put("height", Value::from(self.height));
        put("refreshRate", Value::from(self.refresh_rate));
        put("packs", self.packs.to_value());
        put("bxt", self.bxt.to_value());
        put("binds", overrides_to_value(&self.binds));
        put("personalization", self.personalization.to_value());
        put("liveSplitOverlay", self.live_split_overlay.to_value());
        if !self.defaults_order {
            put("launchArgs", Value::String(self.launch_args.clone()));
            put("protonOptions", Value::String(self.proton_options.clone()));
        }
        Value::Object(object)
    }
}

/// `defaultSettings()`.
pub fn default_settings() -> Settings {
    Settings {
        launch_args: String::new(),
        proton_options: String::new(),
        console: false,
        novid: false,
        borderless: false,
        dx9: false,
        nocrashdialog: false,
        noasync: false,
        noaafonts: false,
        high: false,
        exclude_cpu0: false,
        start_timer: false,
        vulkan: false,
        language: LANGUAGE_AUTO.to_owned(),
        rinput: true,
        rinput_menu_fix: false,
        windowed: false,
        developer: false,
        condebug: false,
        nojoy: false,
        width: 0,
        height: 0,
        refresh_rate: 0,
        packs: goldsrc_packs::default_packs(),
        bxt: goldsrc_personalization::default_bxt(),
        binds: Overrides::new(),
        personalization: ghosting_personalization::default_personalization(),
        live_split_overlay: livesplit_overlay::default_overlay(),
        defaults_order: true,
    }
}

/// `integerIn(value, limits)`: целое в пределах или 0.
fn integer_in(value: Option<&Value>, (min, max): (f64, f64)) -> u32 {
    let number = value.map_or(f64::NAN, to_number);
    if number.is_finite() && number.fract() == 0.0 && number >= min && number <= max {
        number as u32
    } else {
        0
    }
}

/// `settingsFieldsOf(input)`: все поля, кроме `launchArgs` и `protonOptions`.
fn settings_fields_of(input: &Value) -> Settings {
    let flag = |key: &str| matches!(prop(input, key), Some(Value::Bool(true)));
    // Разрешение имеет смысл только парой: одно число без другого отбрасываем.
    let width = integer_in(prop(input, "width"), WIDTH_LIMITS);
    let height = integer_in(prop(input, "height"), HEIGHT_LIMITS);
    let pair = width != 0 && height != 0;
    let null = &Value::Null;
    Settings {
        launch_args: String::new(),
        proton_options: String::new(),
        console: flag("console"),
        novid: flag("novid"),
        borderless: flag("borderless"),
        dx9: flag("dx9"),
        nocrashdialog: flag("nocrashdialog"),
        noasync: flag("noasync"),
        noaafonts: flag("noaafonts"),
        high: flag("high"),
        exclude_cpu0: flag("excludeCpu0"),
        start_timer: flag("startTimer"),
        vulkan: flag("vulkan"),
        language: language_of(input),
        rinput: !matches!(prop(input, "rinput"), Some(Value::Bool(false))),
        rinput_menu_fix: flag("rinputMenuFix"),
        windowed: flag("windowed"),
        developer: flag("developer"),
        condebug: flag("condebug"),
        nojoy: flag("nojoy"),
        width: if pair { width } else { 0 },
        height: if pair { height } else { 0 },
        refresh_rate: integer_in(prop(input, "refreshRate"), REFRESH_LIMITS),
        packs: goldsrc_packs::sanitize_packs(prop(input, "packs").unwrap_or(null)),
        bxt: goldsrc_personalization::sanitize_bxt(prop(input, "bxt").unwrap_or(null)),
        binds: game_binds::sanitize_overrides(prop(input, "binds").unwrap_or(null)),
        personalization: ghosting_personalization::sanitize_personalization(
            prop(input, "personalization").unwrap_or(null),
        ),
        live_split_overlay: livesplit_overlay::sanitize_overlay(
            prop(input, "liveSplitOverlay").unwrap_or(null),
        ),
        defaults_order: false,
    }
}

/// `normalizeSettings(raw)`: настройки для записи в файл или отказ на первом же испорченном поле.
pub fn normalize_settings(raw: &Value) -> Result<Settings, ArgsError> {
    let null = &Value::Null;
    let launch_args = prop(raw, "launchArgs").unwrap_or(null);
    validate_args_string(launch_args)?;
    let proton_options = prop(raw, "protonOptions").unwrap_or(null);
    parse_proton_options(proton_options).map_err(|error| error.with_field("protonOptions"))?;
    let trimmed = |value: &Value| value.as_str().map(js::trim).unwrap_or_default().to_owned();
    Ok(Settings {
        launch_args: trimmed(launch_args),
        proton_options: trimmed(proton_options),
        ..settings_fields_of(raw)
    })
}

/// `IMPORTABLE_ARG` (`/^-[a-z0-9_-]+$|^\d+(?:\.\d+)?$/i`): параметр или число — без консольных
/// команд («+exec») и без путей.
fn importable_arg(token: &str) -> bool {
    if let Some(name) = token.strip_prefix('-') {
        return !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    }
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    match token.split_once('.') {
        Some((whole, fraction)) => digits(whole) && digits(fraction),
        None => digits(token),
    }
}

/// `importedSettings(raw, current)`: настройки из чужого файла (.hlsrbuild, .hlsrpack). Такой файл
/// выбирают один раз, а параметры сработают при каждом запуске игры, поэтому команды Proton всегда
/// остаются свои, а параметры запуска принимаются, только если в них нет команд и путей.
pub fn imported_settings(raw: &Value, current: &Settings) -> Value {
    let mut input = match raw {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    let launch_args = input.get("launchArgs").cloned().unwrap_or(Value::Null);
    let safe = parse_args_string(&launch_args)
        .is_ok_and(|args| args.iter().all(|token| importable_arg(token)));
    input.insert(
        "protonOptions".to_owned(),
        Value::String(current.proton_options.clone()),
    );
    input.insert(
        "launchArgs".to_owned(),
        if safe {
            launch_args
        } else {
            Value::String(current.launch_args.clone())
        },
    );
    Value::Object(input)
}

/// Итог [`sanitize_settings`].
#[derive(Debug, Clone)]
pub struct Sanitized {
    pub settings: Settings,
    /// Первая отброшенная строка параметров: из-за неё поле очищено, переключатели остались.
    pub dropped: Option<ArgsError>,
}

/// `sanitizeSettings(raw)`: настройки из файла или окна; испорченные `launchArgs` и
/// `protonOptions` очищаются, переключатели сохраняются.
pub fn sanitize_settings(raw: &Value) -> Sanitized {
    let null = &Value::Null;
    let mut settings = settings_fields_of(raw);
    let launch_args = prop(raw, "launchArgs").unwrap_or(null);
    let checked = validate_args_string(launch_args);
    let mut dropped = checked.as_ref().err().cloned();
    if checked.is_ok()
        && let Value::String(text) = launch_args
    {
        settings.launch_args = js::trim(text).to_owned();
    }
    let proton_options = prop(raw, "protonOptions").unwrap_or(null);
    match parse_proton_options(proton_options) {
        Ok(_) => {
            if let Value::String(text) = proton_options {
                settings.proton_options = js::trim(text).to_owned();
            }
        }
        Err(error) if dropped.is_none() => dropped = Some(error.with_field("protonOptions")),
        Err(_) => {}
    }
    Sanitized { settings, dropped }
}

/// `buildUserArguments(settings, knownArgs)`: аргументы игрока без повторов друг друга и
/// доверенных `known_args` (повтор определяется без учёта регистра).
pub fn build_user_arguments(settings: &Value, known_args: &[String]) -> Vec<String> {
    let value = sanitize_settings(settings).settings;
    let mut seen: HashSet<String> = known_args.iter().map(|arg| arg.to_lowercase()).collect();
    let mut result = Vec::new();
    let mut push = |token: &str| {
        if token.starts_with('-') && !seen.insert(token.to_lowercase()) {
            return;
        }
        result.push(token.to_owned());
    };
    for (key, tokens) in TOGGLE_ARGUMENTS {
        if value.toggle(key) {
            for token in tokens {
                push(token);
            }
        }
    }
    if let Ok(tokens) = validate_args_string(&Value::String(value.launch_args)) {
        for token in &tokens {
            push(token);
        }
    }
    result
}

/// `mergeLaunchArguments(trustedArgs, settings)`: доверенные аргументы сборки, `-insecure` и
/// аргументы игрока.
pub fn merge_launch_arguments(trusted_args: &[String], settings: &Value) -> Vec<String> {
    let mut base = trusted_args.to_vec();
    let mut seen: HashSet<String> = base.iter().map(|arg| arg.to_lowercase()).collect();
    for token in ALWAYS_ARGUMENTS {
        if seen.insert(token.to_lowercase()) {
            base.push(token.to_owned());
        }
    }
    let user = build_user_arguments(settings, &base);
    base.extend(user);
    base
}
