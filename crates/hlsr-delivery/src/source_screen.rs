//! Разрешение, в котором запустится Source-игра (`source-screen.js`).
//!
//! Лаунчер передаёт `-w`/`-h`, только если разрешение задано в настройках игры; иначе движок
//! берёт последнее сохранённое в реестре `HKCU\Software\Valve\Source\<мод>\Settings`
//! (`ScreenWidth`/`ScreenHeight`). Под Proton этот раздел лежит в `user.reg` префикса.
//! Предпросмотр HUD Ghosting по нему строит кадр нужной ширины.
//!
//! В 0.4.7 функция асинхронная только из-за дочернего процесса `reg query`. Здесь реестр
//! читает [`Registry`], работа синхронная: вызывающий сам решает, на каком потоке её вести.

use crate::ghosting_settings::{split_lines, strip_prefix_ascii_ci as strip_prefix_ci};
use crate::ports::{Registry, RegistryHive};
use hlsr_core::js::{self, Platform, is_whitespace};
use serde_json::Value;
use std::path::Path;

/// Ширина и высота кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

/// Откуда взято разрешение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenSource {
    /// Из настроек игры в лаунчере (`"launcher"`).
    Launcher,
    /// Сохранено самой игрой (`"game"`).
    Game,
}

impl ScreenSource {
    /// Значение поля `source` в ответе окну.
    pub fn as_str(self) -> &'static str {
        match self {
            ScreenSource::Launcher => "launcher",
            ScreenSource::Game => "game",
        }
    }
}

/// `{ width, height, source }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub source: ScreenSource,
}

/// `clean(width, height)`: целые числа в пределах 320×240 … 16384×16384.
fn clean(width: f64, height: f64) -> Option<Resolution> {
    let integer = |n: f64| n.is_finite() && n.fract() == 0.0;
    if !integer(width) || !integer(height) {
        return None;
    }
    if width < 320.0 || height < 240.0 || width > 16384.0 || height > 16384.0 {
        return None;
    }
    Some(Resolution {
        width: width as u32,
        height: height as u32,
    })
}

/// `\s+`, затем то, что не пробел: остаток после пробелов или `None`, если пробела не было.
fn skip_whitespace(s: &str) -> Option<&str> {
    let rest = s.trim_start_matches(is_whitespace);
    (rest.len() != s.len()).then_some(rest)
}

/// `([0-9a-f]{1,8})\s*$` с `i`: значение шестнадцатеричной записи до конца строки.
fn hex_dword(s: &str) -> Option<u32> {
    let digits = s.bytes().take_while(u8::is_ascii_hexdigit).count();
    if digits == 0 || digits > 8 || !s[digits..].chars().all(is_whitespace) {
        return None;
    }
    u32::from_str_radix(&s[..digits], 16).ok()
}

/// Какая из двух величин и её значение.
fn screen_value(name: &str) -> Option<bool> {
    if name.eq_ignore_ascii_case("ScreenWidth") {
        Some(true)
    } else if name.eq_ignore_ascii_case("ScreenHeight") {
        Some(false)
    } else {
        None
    }
}

/// Последние прочитанные ширина и высота; чего нет — `NaN`, как `undefined` в `Number()`.
#[derive(Default)]
struct Dimensions {
    width: Option<u32>,
    height: Option<u32>,
}

impl Dimensions {
    fn put(&mut self, is_width: bool, value: u32) {
        if is_width {
            self.width = Some(value);
        } else {
            self.height = Some(value);
        }
    }

    fn resolution(&self) -> Option<Resolution> {
        let as_number = |n: Option<u32>| n.map_or(f64::NAN, f64::from);
        clean(as_number(self.width), as_number(self.height))
    }
}

/// `user.reg` Wine: «[Software\\Valve\\Source\\<мод>\\Settings] 1791620751», затем строки
/// «"ScreenWidth"=dword:000005a0» до пустой строки (раздела).
pub fn from_user_reg(text: &str, game_mod: &str) -> Option<Resolution> {
    let header = format!("[Software\\\\Valve\\\\Source\\\\{game_mod}\\\\Settings]").to_lowercase();
    let lines = split_lines(text);
    let start = lines
        .iter()
        .position(|line| line.to_lowercase().starts_with(&header))?;
    let mut values = Dimensions::default();
    for line in lines[start + 1..]
        .iter()
        .take_while(|line| !line.starts_with('['))
    {
        // `/^"(ScreenWidth|ScreenHeight)"=dword:([0-9a-f]{1,8})\s*$/i`
        let Some(rest) = line.strip_prefix('"') else {
            continue;
        };
        let Some((name, rest)) = rest.split_once('"') else {
            continue;
        };
        let Some(is_width) = screen_value(name) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let Some(rest) = strip_prefix_ci(rest, "dword:") else {
            continue;
        };
        if let Some(value) = hex_dword(rest) {
            values.put(is_width, value);
        }
    }
    values.resolution()
}

/// Вывод `reg query`: «    ScreenWidth    REG_DWORD    0xa00».
pub fn from_reg_query(text: &str) -> Option<Resolution> {
    let mut values = Dimensions::default();
    for line in split_lines(text) {
        // `/^\s*(ScreenWidth|ScreenHeight)\s+REG_DWORD\s+0x([0-9a-f]{1,8})\s*$/i`
        let line = line.trim_start_matches(is_whitespace);
        let (is_width, rest) = if let Some(rest) = strip_prefix_ci(line, "ScreenWidth") {
            (true, rest)
        } else if let Some(rest) = strip_prefix_ci(line, "ScreenHeight") {
            (false, rest)
        } else {
            continue;
        };
        let Some(rest) = skip_whitespace(rest) else {
            continue;
        };
        let Some(rest) = strip_prefix_ci(rest, "REG_DWORD") else {
            continue;
        };
        let Some(rest) = skip_whitespace(rest) else {
            continue;
        };
        let Some(rest) = strip_prefix_ci(rest, "0x") else {
            continue;
        };
        if let Some(value) = hex_dword(rest) {
            values.put(is_width, value);
        }
    }
    values.resolution()
}

fn read_user_reg(prefix: &Path, game_mod: &str) -> Option<Resolution> {
    let bytes = std::fs::read(prefix.join("user.reg")).ok()?;
    from_user_reg(&js::utf8_string(&bytes), game_mod)
}

/// `reg query HKCU\Software\Valve\Source\<мод>\Settings`: обе величины читает [`Registry`];
/// выводы склеиваются, и разбирает их тот же `from_reg_query`, что и вывод настоящего `reg.exe`.
fn query_registry(registry: &dyn Registry, game_mod: &str) -> Option<Resolution> {
    let key = format!("Software\\Valve\\Source\\{game_mod}\\Settings");
    let text: String = ["ScreenWidth", "ScreenHeight"]
        .iter()
        .filter_map(|name| registry.query(RegistryHive::CurrentUser, &key, name))
        .collect();
    from_reg_query(&text)
}

/// Разрешение для предпросмотра: настройки лаунчера важнее сохранённого игрой; под Proton
/// сохранённое читается из `user.reg` префикса, в Windows — из реестра. `None` — неизвестно,
/// тогда предпросмотр берёт разрешение монитора.
pub fn screen_of(
    settings: Option<&Value>,
    game_mod: &str,
    platform: Platform,
    prefix: Option<&Path>,
    registry: &dyn Registry,
) -> Option<Screen> {
    if let Some(Value::Object(object)) = settings
        && let Some(found) = clean(
            js::value::to_number(object.get("width")),
            js::value::to_number(object.get("height")),
        )
    {
        return Some(Screen {
            width: found.width,
            height: found.height,
            source: ScreenSource::Launcher,
        });
    }
    // `/^[\w.-]+$/`
    let valid = !game_mod.is_empty()
        && game_mod
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'));
    if !valid {
        return None;
    }
    let found = if platform == Platform::Win32 {
        query_registry(registry, game_mod)
    } else {
        prefix
            .filter(|prefix| !prefix.as_os_str().is_empty())
            .and_then(|prefix| read_user_reg(prefix, game_mod))
    }?;
    Some(Screen {
        width: found.width,
        height: found.height,
        source: ScreenSource::Game,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dword_needs_one_to_eight_digits_and_only_blanks_after() {
        assert_eq!(hex_dword("a00"), Some(0xa00));
        assert_eq!(hex_dword("a00 \t"), Some(0xa00));
        assert_eq!(hex_dword("123456789"), None);
        assert_eq!(hex_dword(""), None);
        assert_eq!(hex_dword("a00x"), None);
    }
}
