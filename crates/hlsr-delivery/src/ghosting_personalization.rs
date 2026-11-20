//! HUD и прицел Ghosting → cfg, autoexec, импорт из `config.cfg` (`ghosting-personalization.js`).
//!
//! 0.4.9: персонализация пишется в файл игрока (`player_config`), значения из него читает
//! [`import_from_text`]. Путь 0.4.7 (`apply_to_install`, `ensure_autoexec_exec`: отдельный
//! `hlsr-personalization.cfg` и строка exec в autoexec.cfg) зовёт только запуск `hlsr-platform`,
//! пока тот не перейдёт на `default_configs::prepare_for_launch` (задача F2l).

use crate::bind_presets::ghosting_save_deletion;
use crate::errors::RawError;
use crate::game_binds::Encoding;
use crate::ghosting_default_autoexec::default_autoexec;
use crate::ghosting_settings::{
    self as extra_settings, Color, Extras, split_lines, strip_prefix_ascii_ci, words_to_numbers,
};
use crate::node_fs::{mkdir_recursive, read_text, write_text};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform, is_whitespace};
use regress::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub const PERSONALIZATION_FILE: &str = "hlsr-personalization.cfg";
pub const GAME_DIR: &str = "ghostingmod";
pub const CFG_DIR: &str = "ghostingmod/cfg";
pub const EXTRAS_MARKER: &str = "// HLSR_EXTRA_SETTINGS_VERSION 1";

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color { r, g, b }
}

/// Цвета HUD для выбора в окне.
pub const HUD_PRESETS: &[(&str, Color)] = &[
    ("stock", rgb(255, 220, 0)),
    ("white", rgb(255, 255, 255)),
    ("green", rgb(0, 255, 0)),
    ("cyan", rgb(0, 255, 255)),
    ("blue", rgb(80, 160, 255)),
    ("orange", rgb(255, 128, 0)),
    ("pink", rgb(255, 80, 180)),
    ("purple", rgb(180, 80, 255)),
];

/// Пресет прицела: явные значения, а не команды `gh_cross_preset_*`, чтобы предпросмотр в
/// лаунчере совпадал с игрой 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossPreset {
    pub cross_enabled: bool,
    pub cross_color: Color,
    pub cross_alpha: i32,
    pub thickness: i32,
    pub size: i32,
    pub gap: i32,
    /// 0 или 1, как в 0.4.7.
    pub outline: i32,
    pub outline_thickness: i32,
    pub circle_radius: i32,
    pub dot_color: Option<Color>,
    pub dot_size: i32,
    pub top_line: bool,
    pub bottom_line: bool,
    pub left_line: bool,
    pub right_line: bool,
    pub hide_default: bool,
}

const GREEN: Color = rgb(0, 255, 0);

const fn preset(
    cross_color: Color,
    size: i32,
    gap: i32,
    outline: i32,
    circle_radius: i32,
    dot_size: i32,
    lines: bool,
) -> CrossPreset {
    CrossPreset {
        cross_enabled: true,
        cross_color,
        cross_alpha: 255,
        thickness: 2,
        size,
        gap,
        outline,
        outline_thickness: 1,
        circle_radius,
        dot_color: None,
        dot_size,
        top_line: lines,
        bottom_line: lines,
        left_line: lines,
        right_line: lines,
        hide_default: true,
    }
}

/// Пресеты прицела по именам; `None` — «стандартный» и «свой» (значения задаёт игрок).
pub const CROSS_PRESETS: &[(&str, Option<CrossPreset>)] = &[
    ("standard", None),
    ("custom", None),
    ("dot", Some(preset(GREEN, 0, 0, 1, 0, 4, false))),
    ("classic", Some(preset(GREEN, 10, 3, 0, 0, 0, true))),
    ("circle", Some(preset(GREEN, 0, 0, 1, 12, 2, false))),
    (
        "crossdot",
        Some(preset(rgb(255, 255, 0), 8, 4, 1, 0, 2, true)),
    ),
];

/// Имя пресета прицела; унаследованные имена (`constructor`, `__proto__`) пресетами не считаются.
fn cross_preset_id(id: &str) -> Option<&'static str> {
    CROSS_PRESETS
        .iter()
        .find(|(name, _)| *name == id)
        .map(|(name, _)| *name)
}

/// Настройки персонализации, нормализованные `sanitizePersonalization`.
#[derive(Debug, Clone, PartialEq)]
pub struct Personalization {
    pub hud_enabled: bool,
    pub hud_color: Color,
    pub hud_alpha: i32,
    pub hud_affects_crosshair: bool,
    pub hide_quick_info: bool,
    pub cross_preset: &'static str,
    pub cross_enabled: bool,
    pub cross_color: Color,
    pub cross_alpha: i32,
    pub cross_thickness: i32,
    pub cross_size: i32,
    pub cross_gap: i32,
    pub cross_outline: bool,
    pub cross_outline_thickness: i32,
    pub cross_circle_radius: i32,
    pub cross_dot_color: Option<Color>,
    pub cross_dot_size: i32,
    pub cross_top_line: bool,
    pub cross_bottom_line: bool,
    pub cross_left_line: bool,
    pub cross_right_line: bool,
    pub cross_hide_default: bool,
    pub extra: Extras,
}

impl Personalization {
    /// Объект настроек в порядке ключей 0.4.7 (от него зависят байты `game-settings.json`).
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        let mut put = |key: &str, value: Value| {
            object.insert(key.to_owned(), value);
        };
        put("hudEnabled", Value::Bool(self.hud_enabled));
        put("hudColor", self.hud_color.to_value());
        put("hudAlpha", Value::from(self.hud_alpha));
        put(
            "hudAffectsCrosshair",
            Value::Bool(self.hud_affects_crosshair),
        );
        put("hideQuickInfo", Value::Bool(self.hide_quick_info));
        put("crossPreset", Value::String(self.cross_preset.to_owned()));
        put("crossEnabled", Value::Bool(self.cross_enabled));
        put("crossColor", self.cross_color.to_value());
        put("crossAlpha", Value::from(self.cross_alpha));
        put("crossThickness", Value::from(self.cross_thickness));
        put("crossSize", Value::from(self.cross_size));
        put("crossGap", Value::from(self.cross_gap));
        put("crossOutline", Value::Bool(self.cross_outline));
        put(
            "crossOutlineThickness",
            Value::from(self.cross_outline_thickness),
        );
        put("crossCircleRadius", Value::from(self.cross_circle_radius));
        put(
            "crossDotColor",
            self.cross_dot_color.map_or(Value::Null, Color::to_value),
        );
        put("crossDotSize", Value::from(self.cross_dot_size));
        put("crossTopLine", Value::Bool(self.cross_top_line));
        put("crossBottomLine", Value::Bool(self.cross_bottom_line));
        put("crossLeftLine", Value::Bool(self.cross_left_line));
        put("crossRightLine", Value::Bool(self.cross_right_line));
        put("crossHideDefault", Value::Bool(self.cross_hide_default));
        put("extra", self.extra.to_value());
        Value::Object(object)
    }
}

/// `colorToString`: `"r g b"`.
pub fn color_to_string(color: Color) -> String {
    color.to_cfg()
}

/// `defaultPersonalization()`.
pub fn default_personalization() -> Personalization {
    Personalization {
        hud_enabled: false,
        hud_color: rgb(255, 220, 0),
        hud_alpha: -1,
        hud_affects_crosshair: false,
        hide_quick_info: false,
        cross_preset: "standard",
        cross_enabled: false,
        cross_color: GREEN,
        cross_alpha: 255,
        cross_thickness: 2,
        cross_size: 10,
        cross_gap: 3,
        cross_outline: false,
        cross_outline_thickness: 1,
        cross_circle_radius: 0,
        cross_dot_color: None,
        cross_dot_size: 0,
        cross_top_line: true,
        cross_bottom_line: true,
        cross_left_line: true,
        cross_right_line: true,
        cross_hide_default: true,
        extra: Extras::new(),
    }
}

/// `clampInt` для числа: целое от `min` до `max`; не конечное — `fallback`.
fn clamp_number(n: f64, min: i32, max: i32, fallback: i32) -> i32 {
    if !n.is_finite() {
        return fallback;
    }
    let rounded = js::math_round(n);
    if rounded < f64::from(min) {
        min
    } else if rounded > f64::from(max) {
        max
    } else {
        rounded as i32
    }
}

/// `clampInt(value, min, max, fallback)`: число или непустая строка с числом; прочее —
/// `fallback`.
fn clamp_int(value: Option<&Value>, min: i32, max: i32, fallback: i32) -> i32 {
    let n = match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::INFINITY),
        Some(Value::String(text)) if !js::trim(text).is_empty() => js::to_number(text),
        _ => return fallback,
    };
    clamp_number(n, min, max, fallback)
}

/// `sanitizeColor(value, fallback)`: объект с каналами; каналы, которых нет, берутся из
/// `fallback`.
fn sanitize_color(value: Option<&Value>, fallback: Color) -> Color {
    let channel = |name: &str, default: u8| {
        let raw = value.and_then(Value::as_object).and_then(|o| o.get(name));
        clamp_int(raw, 0, 255, i32::from(default)) as u8
    };
    Color {
        r: channel("r", fallback.r),
        g: channel("g", fallback.g),
        b: channel("b", fallback.b),
    }
}

/// `sanitizePersonalization(input)`: любой вход приводится к допустимым значениям.
pub fn sanitize_personalization(input: &Value) -> Personalization {
    let src = |name: &str| input.as_object().and_then(|object| object.get(name));
    let default = default_personalization();
    let hud_color = sanitize_color(src("hudColor"), default.hud_color);
    let cross_color = sanitize_color(src("crossColor"), default.cross_color);

    let cross_dot_color = match src("crossDotColor") {
        // Пустая строка в игре означает «как основной цвет»; в настройках это `null`,
        // а объект считается заданным цветом.
        Some(Value::Object(_) | Value::Array(_)) => {
            Some(sanitize_color(src("crossDotColor"), cross_color))
        }
        Some(Value::String(text)) if !js::trim(text).is_empty() => {
            let parts = words_to_numbers(text);
            (parts.len() == 3 && parts.iter().all(|part| part.is_finite())).then(|| Color {
                r: clamp_number(parts[0], 0, 255, i32::from(cross_color.r)) as u8,
                g: clamp_number(parts[1], 0, 255, i32::from(cross_color.g)) as u8,
                b: clamp_number(parts[2], 0, 255, i32::from(cross_color.b)) as u8,
            })
        }
        _ => None,
    };

    let is_true = |name: &str| matches!(src(name), Some(Value::Bool(true)));
    let not_false = |name: &str| !matches!(src(name), Some(Value::Bool(false)));
    let preset = src("crossPreset")
        .and_then(Value::as_str)
        .and_then(cross_preset_id)
        .unwrap_or("standard");

    Personalization {
        hud_enabled: is_true("hudEnabled"),
        hud_color,
        hud_alpha: clamp_int(src("hudAlpha"), -1, 255, -1),
        hud_affects_crosshair: is_true("hudAffectsCrosshair"),
        hide_quick_info: is_true("hideQuickInfo"),
        cross_preset: preset,
        cross_enabled: is_true("crossEnabled"),
        cross_color,
        cross_alpha: clamp_int(src("crossAlpha"), 0, 255, 255),
        cross_thickness: clamp_int(src("crossThickness"), 1, 10, 2),
        cross_size: clamp_int(src("crossSize"), 0, 40, 10),
        // Отрицательный зазор принимает только BXT (HL1): линии заходят за центр. Ghosting
        // пишет его не меньше 0, см. `cfg_lines`.
        cross_gap: clamp_int(src("crossGap"), -30, 30, 3),
        cross_outline: is_true("crossOutline"),
        cross_outline_thickness: clamp_int(src("crossOutlineThickness"), 1, 4, 1),
        cross_circle_radius: clamp_int(src("crossCircleRadius"), 0, 40, 0),
        cross_dot_color,
        cross_dot_size: clamp_int(src("crossDotSize"), 0, 12, 0),
        cross_top_line: not_false("crossTopLine"),
        cross_bottom_line: not_false("crossBottomLine"),
        cross_left_line: not_false("crossLeftLine"),
        cross_right_line: not_false("crossRightLine"),
        cross_hide_default: not_false("crossHideDefault"),
        extra: extra_settings::sanitize_extras(src("extra").unwrap_or(&Value::Null)),
    }
}

/// `applyPreset(base, presetId)`: значения пресета поверх нормализованных настроек.
pub fn apply_preset(base: &Value, preset_id: &str) -> Personalization {
    let mut clean = sanitize_personalization(base);
    if preset_id == "standard" {
        clean.cross_preset = "standard";
        clean.cross_enabled = false;
        return clean;
    }
    let found = CROSS_PRESETS
        .iter()
        .find(|(name, _)| *name == preset_id)
        .and_then(|(name, preset)| preset.map(|preset| (*name, preset)));
    let Some((name, preset)) = found else {
        return clean;
    };
    clean.cross_preset = name;
    clean.cross_enabled = preset.cross_enabled;
    clean.cross_color = preset.cross_color;
    clean.cross_alpha = preset.cross_alpha;
    clean.cross_thickness = preset.thickness;
    clean.cross_size = preset.size;
    clean.cross_gap = preset.gap;
    clean.cross_outline = preset.outline != 0;
    clean.cross_outline_thickness = preset.outline_thickness;
    clean.cross_circle_radius = preset.circle_radius;
    clean.cross_dot_color = preset.dot_color;
    clean.cross_dot_size = preset.dot_size;
    clean.cross_top_line = preset.top_line;
    clean.cross_bottom_line = preset.bottom_line;
    clean.cross_left_line = preset.left_line;
    clean.cross_right_line = preset.right_line;
    clean.cross_hide_default = preset.hide_default;
    clean
}

fn flag(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

/// `toCfgLines(settings)`: текст `hlsr-personalization.cfg` для любых настроек (сначала они
/// нормализуются).
pub fn to_cfg_lines(settings: &Value) -> String {
    cfg_lines(&sanitize_personalization(settings))
}

/// То же для уже нормализованных настроек.
pub fn cfg_lines(s: &Personalization) -> String {
    let cross_color = if s.hud_affects_crosshair {
        s.hud_color
    } else {
        s.cross_color
    };
    let dot_color = s.cross_dot_color.map(color_to_string).unwrap_or_default();
    let mut lines: Vec<String> = vec![
        "// Generated by HLSR Launcher - Personalization. Do not edit manually.".to_owned(),
        "// ghostingmod/cfg/hlsr-personalization.cfg - applied via autoexec.cfg".to_owned(),
        format!("gh_hud_color \"{}\"", color_to_string(s.hud_color)),
        format!("gh_hud_color_enable \"{}\"", flag(s.hud_enabled)),
        format!("gh_hud_color_alpha \"{}\"", s.hud_alpha),
        format!(
            "gh_hud_color_affects_crosshair \"{}\"",
            flag(s.hud_affects_crosshair)
        ),
        format!(
            "hud_quickinfo \"{}\"",
            if s.hide_quick_info { "0" } else { "1" }
        ),
        format!("gh_cross \"{}\"", flag(s.cross_enabled)),
        format!("gh_cross_color \"{}\"", color_to_string(cross_color)),
        format!("gh_cross_alpha \"{}\"", s.cross_alpha),
        format!("gh_cross_thickness \"{}\"", s.cross_thickness),
        format!("gh_cross_size \"{}\"", s.cross_size),
        format!("gh_cross_gap \"{}\"", s.cross_gap.max(0)),
        format!("gh_cross_outline \"{}\"", flag(s.cross_outline)),
        format!(
            "gh_cross_outline_thickness \"{}\"",
            s.cross_outline_thickness
        ),
        format!("gh_cross_circle_radius \"{}\"", s.cross_circle_radius),
        format!("gh_cross_dot_color \"{dot_color}\""),
        format!("gh_cross_dot_size \"{}\"", s.cross_dot_size),
        format!("gh_cross_top_line \"{}\"", flag(s.cross_top_line)),
        format!("gh_cross_bottom_line \"{}\"", flag(s.cross_bottom_line)),
        format!("gh_cross_left_line \"{}\"", flag(s.cross_left_line)),
        format!("gh_cross_right_line \"{}\"", flag(s.cross_right_line)),
        format!("gh_cross_hide_default \"{}\"", flag(s.cross_hide_default)),
        EXTRAS_MARKER.to_owned(),
    ];
    let extra_lines = extra_settings::serialize_clean(&s.extra);
    if !extra_lines.is_empty() {
        lines.push(extra_lines);
    }
    lines.push(String::new());
    lines.join("\n")
}

fn cfg_dir(install: &Path) -> PathBuf {
    let path = NodePath::native();
    PathBuf::from(path.join(&[&install.to_string_lossy(), CFG_DIR]))
}

fn cfg_path(install: &Path, name: &str) -> PathBuf {
    let path = NodePath::native();
    PathBuf::from(path.join(&[&install.to_string_lossy(), CFG_DIR, name]))
}

/// `personalizationFile(installPath)`.
pub fn personalization_file(install: &Path) -> PathBuf {
    cfg_path(install, PERSONALIZATION_FILE)
}

/// Итог [`ensure_autoexec_exec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoexecExec {
    pub created: bool,
    pub appended: bool,
}

/// `ensureAutoexecExec(installPath)`: строка `exec hlsr-personalization.cfg` в autoexec.cfg —
/// в конце, с окончанием строк файла; уже есть (без учёта регистра и пробелов) — не трогается.
pub fn ensure_autoexec_exec(install: &Path) -> Result<AutoexecExec, RawError> {
    let autoexec = cfg_path(install, "autoexec.cfg");
    let exec_line = "exec hlsr-personalization.cfg";
    let text = match read_text(&autoexec, Encoding::Utf8) {
        Ok(text) => text,
        Err(error) if error.code.as_deref() == Some("ENOENT") => {
            mkdir_recursive(&cfg_dir(install))?;
            write_text(&autoexec, &format!("{exec_line}\n"), Encoding::Utf8)?;
            return Ok(AutoexecExec {
                created: true,
                appended: true,
            });
        }
        Err(error) => return Err(error),
    };
    if split_lines(&text)
        .iter()
        .any(|line| js::trim(line).to_lowercase() == exec_line)
    {
        return Ok(AutoexecExec {
            created: false,
            appended: false,
        });
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let suffix = if text.ends_with('\n') || text.is_empty() {
        ""
    } else {
        eol
    };
    write_text(
        &autoexec,
        &format!("{text}{suffix}{exec_line}{eol}"),
        Encoding::Utf8,
    )?;
    Ok(AutoexecExec {
        created: false,
        appended: true,
    })
}

/// sha256 стокового autoexec.cfg сборки (база 1.0.2–1.0.7) без строк exec лаунчера.
pub const STOCK_AUTOEXEC_SHA256: &[&str] =
    &["8a2f1730360d8a8c4f3bb964144ee9f63a4bb94e6aba5afd5b5372cc3a8c94ba"];

/// `/^exec\s+hlsr-[a-z-]+\.cfg$/i`: флаг `i` без `u` сводит регистр только у ASCII.
fn is_launcher_exec_line(line: &str) -> bool {
    let Some(rest) = strip_prefix_ascii_ci(line, "exec") else {
        return false;
    };
    let after_blanks = rest.trim_start_matches(is_whitespace);
    if after_blanks.len() == rest.len() {
        return false;
    }
    let Some(rest) = strip_prefix_ascii_ci(after_blanks, "hlsr-") else {
        return false;
    };
    let Some(at) = rest
        .len()
        .checked_sub(4)
        .filter(|&at| rest.is_char_boundary(at))
    else {
        return false;
    };
    let (name, extension) = rest.split_at(at);
    extension.eq_ignore_ascii_case(".cfg")
        && !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || byte == b'-')
}

/// Бинд удаления сейва из стандартного autoexec под Windows. Под Linux он не работает,
/// поэтому в autoexec, поставленном лаунчером раньше, меняется на рабочий. Остальное в
/// файле игрока не трогается.
pub const WINDOWS_SAVE_DELETION_BIND: &str = "bind h \"save :; reload\"";

fn fix_save_deletion_bind(
    autoexec: &Path,
    text: &str,
    platform: Platform,
) -> Result<bool, RawError> {
    if platform != Platform::Linux {
        return Ok(false);
    }
    let fixed = format!("bind h \"{}\"", ghosting_save_deletion(Platform::Linux));
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let mut changed = false;
    for line in &mut lines {
        let cr = if line.ends_with('\r') { "\r" } else { "" };
        if js::trim(line) != WINDOWS_SAVE_DELETION_BIND {
            continue;
        }
        let indent = &line[..line.find("bind").unwrap_or(0)];
        *line = format!("{indent}{fixed}{cr}");
        changed = true;
    }
    if changed {
        write_text(autoexec, &lines.join("\n"), Encoding::Utf8)?;
    }
    Ok(changed)
}

/// `ensureDefaultAutoexec(installPath, { platform })`: ставит стандартный autoexec лаунчера
/// вместо отсутствующего или стокового. Свой autoexec игрока не трогает (разве что чинит бинд
/// удаления сейва под Linux). Строки exec лаунчера переносятся. `true` — файл заменён.
pub fn ensure_default_autoexec(install: &Path, platform: Platform) -> Result<bool, RawError> {
    let autoexec = cfg_path(install, "autoexec.cfg");
    let text = match read_text(&autoexec, Encoding::Utf8) {
        Ok(text) => Some(text),
        Err(error) if error.code.as_deref() == Some("ENOENT") => None,
        Err(error) => return Err(error),
    };
    let normalized = text.as_ref().map(|text| text.replace("\r\n", "\n"));
    let lines: Vec<&str> = normalized
        .as_deref()
        .map(|text| text.split('\n').collect())
        .unwrap_or_default();
    let exec_lines: Vec<&str> = lines
        .iter()
        .map(|line| js::trim(line))
        .filter(|line| is_launcher_exec_line(line))
        .collect();
    if let Some(text) = &text {
        let own: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|line| !is_launcher_exec_line(js::trim(line)))
            .collect();
        let own = own.join("\n");
        let own = own.trim_end_matches(is_whitespace);
        let hash: String = Sha256::digest(own.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if !STOCK_AUTOEXEC_SHA256.contains(&hash.as_str()) {
            fix_save_deletion_bind(&autoexec, text, platform)?;
            return Ok(false);
        }
    }
    if let Some(parent) = autoexec.parent() {
        mkdir_recursive(parent)?;
    }
    let mut content = default_autoexec(platform);
    for line in exec_lines {
        content.push_str(line);
        content.push_str("\r\n");
    }
    write_text(&autoexec, &content, Encoding::Utf8)?;
    Ok(true)
}

/// Итог [`apply_to_install`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub file: PathBuf,
    pub autoexec: AutoexecExec,
}

/// `applyToInstall(installPath, settings)`: стандартный autoexec (если нужен), файл
/// персонализации и строка exec. `platform` — ОС игры (в 0.4.7 `process.platform`).
pub fn apply_to_install(
    install: &Path,
    settings: &Value,
    platform: Platform,
) -> Result<Applied, RawError> {
    let clean = sanitize_personalization(settings);
    ensure_default_autoexec(install, platform)?;
    let file = personalization_file(install);
    mkdir_recursive(&cfg_dir(install))?;
    write_text(&file, &cfg_lines(&clean), Encoding::Utf8)?;
    let autoexec = ensure_autoexec_exec(install)?;
    Ok(Applied { file, autoexec })
}

/// `/^\s*([A-Za-z0-9_.]+)\s+(.*)\s*$/`
static CFG_VALUE_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([A-Za-z0-9_.]+)\s+(.*)\s*$").expect("выражение 0.4.7 разбирается")
});

/// `parseCfgValue(text, name)`: значение первой строки с этой командой; кавычки и
/// комментарий `//` вне кавычек убираются. `None` — команды нет.
fn parse_cfg_value(text: &str, name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    for line in split_lines(text) {
        let trimmed = js::trim(line);
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        let Some(found) = CFG_VALUE_LINE.find(line) else {
            continue;
        };
        let (Some(command), Some(rest)) = (found.group(1), found.group(2)) else {
            continue;
        };
        if line[command].to_lowercase() != lower {
            continue;
        }
        let mut value = js::trim(&line[rest]).to_owned();
        // Убираем хвостовой комментарий вида: gh_cross "0" // comment
        // (кавычки учитываем грубо: режем по // вне кавычек).
        let chars: Vec<char> = value.chars().collect();
        let mut in_quotes = false;
        for at in 0..chars.len().saturating_sub(1) {
            if chars[at] == '"' {
                in_quotes = !in_quotes;
            }
            if !in_quotes && chars[at] == '/' && chars[at + 1] == '/' {
                value = js::trim(&chars[..at].iter().collect::<String>()).to_owned();
                break;
            }
        }
        let count = value.chars().count();
        if count >= 2 && value.starts_with('"') && value.ends_with('"') {
            return Some(value.chars().skip(1).take(count - 2).collect());
        }
        return Some(value);
    }
    None
}

/// `parseColorString(str, fallback)`: `"r g b"` из cfg.
fn parse_color_string(text: Option<&str>, fallback: Color) -> Color {
    let Some(text) = text else {
        return fallback;
    };
    if js::trim(text).is_empty() {
        return fallback;
    }
    let parts = words_to_numbers(text);
    if parts.len() != 3 || !parts.iter().all(|part| part.is_finite()) {
        return fallback;
    }
    Color {
        r: clamp_number(parts[0], 0, 255, i32::from(fallback.r)) as u8,
        g: clamp_number(parts[1], 0, 255, i32::from(fallback.g)) as u8,
        b: clamp_number(parts[2], 0, 255, i32::from(fallback.b)) as u8,
    }
}

/// Чтение `config.cfg` для первичного импорта: если игрок уже настроил цвет и прицел через
/// консоль, они подхватываются как стартовые значения. Нужен ровно один раз: после первого
/// сохранения отдельный файл становится источником истины. Любой сбой — `None`.
pub fn import_from_config_cfg(install: &Path) -> Option<Personalization> {
    if personalization_file(install).exists() {
        return None;
    }
    let text = read_text(&cfg_path(install, "config.cfg"), Encoding::Utf8).ok()?;
    import_from_text(&text)
}

/// `importFromConfigCfg(installPath, { text })` 0.4.9: то же по переданному тексту (файл игрока).
pub fn import_from_text(text: &str) -> Option<Personalization> {
    let known = [
        "gh_hud_color",
        "gh_hud_color_enable",
        "hud_quickinfo",
        "gh_cross",
        "gh_cross_color",
    ];
    let extra = extra_settings::import_extras(text);
    if !known.iter().any(|key| parse_cfg_value(text, key).is_some()) && extra.is_empty() {
        return None;
    }
    let default = default_personalization();
    let value_of = |name: &str| parse_cfg_value(text, name);
    let text_value = |name: &str| value_of(name).map_or(Value::Null, Value::String);
    let equals = |name: &str, wanted: &str| value_of(name).as_deref() == Some(wanted);
    // `(parseCfgValue(...) || "1") === "1"`: пустая строка тоже значит «1».
    let line_on = |name: &str| {
        value_of(name)
            .filter(|v| !v.is_empty())
            .as_deref()
            .unwrap_or("1")
            == "1"
    };
    let hud_color = parse_color_string(value_of("gh_hud_color").as_deref(), default.hud_color);
    let cross_color =
        parse_color_string(value_of("gh_cross_color").as_deref(), default.cross_color);
    let dot_raw = value_of("gh_cross_dot_color")
        .filter(|raw| !js::trim(raw).is_empty())
        .map_or(Value::Null, Value::String);
    let mut input = Map::new();
    let mut put = |key: &str, value: Value| {
        input.insert(key.to_owned(), value);
    };
    put(
        "hudEnabled",
        Value::Bool(equals("gh_hud_color_enable", "1")),
    );
    put("hudColor", hud_color.to_value());
    put("hudAlpha", text_value("gh_hud_color_alpha"));
    put(
        "hudAffectsCrosshair",
        Value::Bool(equals("gh_hud_color_affects_crosshair", "1")),
    );
    put("hideQuickInfo", Value::Bool(equals("hud_quickinfo", "0")));
    put("crossEnabled", Value::Bool(equals("gh_cross", "1")));
    put("crossColor", cross_color.to_value());
    put("crossAlpha", text_value("gh_cross_alpha"));
    put("crossThickness", text_value("gh_cross_thickness"));
    put("crossSize", text_value("gh_cross_size"));
    put("crossGap", text_value("gh_cross_gap"));
    put("crossOutline", Value::Bool(equals("gh_cross_outline", "1")));
    put(
        "crossOutlineThickness",
        text_value("gh_cross_outline_thickness"),
    );
    put("crossCircleRadius", text_value("gh_cross_circle_radius"));
    put("crossDotColor", dot_raw);
    put("crossDotSize", text_value("gh_cross_dot_size"));
    put("crossTopLine", Value::Bool(line_on("gh_cross_top_line")));
    put(
        "crossBottomLine",
        Value::Bool(line_on("gh_cross_bottom_line")),
    );
    put("crossLeftLine", Value::Bool(line_on("gh_cross_left_line")));
    put(
        "crossRightLine",
        Value::Bool(line_on("gh_cross_right_line")),
    );
    put(
        "crossHideDefault",
        Value::Bool(line_on("gh_cross_hide_default")),
    );
    put("crossPreset", Value::String("custom".to_owned()));
    put("extra", extra.to_value());
    Some(sanitize_personalization(&Value::Object(input)))
}

/// Обновление уже установленной сборки: новые настройки подхватываются, не заменяя
/// сохранённый HUD и прицел. Метка пишется и для пустого набора, поэтому явный сброс или
/// очистка не оживит старые значения из `config.cfg`. `None` — импортировать нечего.
pub fn import_extras_from_config_cfg(install: &Path) -> Option<Extras> {
    let own_text = match read_text(&personalization_file(install), Encoding::Utf8) {
        Ok(text) => text,
        Err(error) if error.code.as_deref() == Some("ENOENT") => String::new(),
        Err(_) => return None,
    };
    if split_lines(&own_text)
        .iter()
        .any(|line| js::trim(line) == EXTRAS_MARKER)
    {
        return None;
    }
    let game_text = match read_text(&cfg_path(install, "config.cfg"), Encoding::Utf8) {
        Ok(text) => text,
        Err(error) if error.code.as_deref() == Some("ENOENT") => String::new(),
        Err(_) => return None,
    };
    let mut extra = extra_settings::import_extras(&game_text);
    extra.assign(&extra_settings::import_extras(&own_text));
    (!extra.is_empty()).then_some(extra)
}

/// `importExtrasFromConfigCfg(installPath, { text })` 0.4.9: extras из переданного текста.
pub fn import_extras_from_text(text: &str) -> Option<Extras> {
    let extra = extra_settings::import_extras(text);
    (!extra.is_empty()).then_some(extra)
}
