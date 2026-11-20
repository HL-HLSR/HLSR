//! `prefers-reduced-motion` ОС, как его видит Chromium (`gfx::Animation::
//! PrefersReducedMotion`): Windows — анимация в клиентской области выключена
//! (`SPI_GETCLIENTAREAANIMATION`); Linux — GTK `gtk-enable-animations` ложно
//! (XSETTINGS `Gtk/EnableAnimations`, иначе `gtk-3.0/settings.ini`).
//! Читается при запуске.

/// ОС просит меньше движения.
pub fn os_prefers_reduced_motion() -> bool {
    platform::prefers_reduced_motion()
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };

    pub fn prefers_reduced_motion() -> bool {
        let mut enabled: i32 = 1;
        // SAFETY: SPI_GETCLIENTAREAANIMATION пишет один BOOL по переданному указателю.
        let ok = unsafe {
            SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut enabled).cast(), 0)
        };
        ok != 0 && enabled == 0
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::PathBuf;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    const ENABLE_ANIMATIONS: &[u8] = b"Gtk/EnableAnimations";

    pub fn prefers_reduced_motion() -> bool {
        let enabled = xsettings_enable_animations().or_else(settings_ini_enable_animations);
        enabled == Some(false)
    }

    /// `Gtk/EnableAnimations` из XSETTINGS владельца `_XSETTINGS_S<экран>`.
    fn xsettings_enable_animations() -> Option<bool> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let atom = |name: &[u8]| Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom);
        let selection = atom(format!("_XSETTINGS_S{screen}").as_bytes())?;
        let owner = conn
            .get_selection_owner(selection)
            .ok()?
            .reply()
            .ok()?
            .owner;
        if owner == x11rb::NONE {
            return None;
        }
        let property = atom(b"_XSETTINGS_SETTINGS")?;
        let reply = conn
            .get_property(false, owner, property, AtomEnum::ANY, 0, u32::MAX / 4)
            .ok()?
            .reply()
            .ok()?;
        parse_xsettings(&reply.value, ENABLE_ANIMATIONS).map(|value| value != 0)
    }

    /// Целое значение `name` из данных XSETTINGS (спецификация freedesktop).
    pub(super) fn parse_xsettings(data: &[u8], name: &[u8]) -> Option<i32> {
        let big = *data.first()? == 1;
        let u16_at = |at: usize| -> Option<u16> {
            let bytes: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
            Some(if big {
                u16::from_be_bytes(bytes)
            } else {
                u16::from_le_bytes(bytes)
            })
        };
        let u32_at = |at: usize| -> Option<u32> {
            let bytes: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
            Some(if big {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            })
        };
        let pad = |len: usize| len.div_ceil(4) * 4;
        let count = u32_at(8)?;
        let mut at = 12;
        for _ in 0..count {
            let kind = *data.get(at)?;
            let name_len = usize::from(u16_at(at + 2)?);
            let found = data.get(at + 4..at + 4 + name_len)? == name;
            at += 4 + pad(name_len) + 4;
            match kind {
                0 => {
                    let value = u32_at(at)? as i32;
                    if found {
                        return Some(value);
                    }
                    at += 4;
                }
                1 => at += 4 + pad(usize::try_from(u32_at(at)?).ok()?),
                2 => at += 8,
                _ => return None,
            }
        }
        None
    }

    /// `gtk-enable-animations` из `$XDG_CONFIG_HOME/gtk-3.0/settings.ini`.
    fn settings_ini_enable_animations() -> Option<bool> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        let text = std::fs::read_to_string(config.join("gtk-3.0").join("settings.ini")).ok()?;
        parse_settings_ini(&text)
    }

    pub(super) fn parse_settings_ini(text: &str) -> Option<bool> {
        let mut in_settings = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_settings = line == "[Settings]";
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if in_settings && key.trim() == "gtk-enable-animations" {
                return match value.trim() {
                    "true" | "1" | "TRUE" | "True" => Some(true),
                    "false" | "0" | "FALSE" | "False" => Some(false),
                    _ => None,
                };
            }
        }
        None
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::platform::{parse_settings_ini, parse_xsettings};

    fn setting(kind: u8, name: &[u8], value: &[u8]) -> Vec<u8> {
        let mut out = vec![kind, 0];
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name);
        out.resize(out.len().div_ceil(4) * 4, 0);
        out.extend_from_slice(&7u32.to_le_bytes());
        out.extend_from_slice(value);
        out
    }

    #[test]
    fn xsettings_int_after_string_and_color() {
        let mut data = vec![0, 0, 0, 0];
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&3u32.to_le_bytes());
        let mut string = 5u32.to_le_bytes().to_vec();
        string.extend_from_slice(b"Adw\0\0\0\0\0");
        data.extend(setting(1, b"Net/ThemeName", &string));
        data.extend(setting(2, b"Gtk/Color", &[0; 8]));
        data.extend(setting(0, b"Gtk/EnableAnimations", &0i32.to_le_bytes()));
        assert_eq!(parse_xsettings(&data, b"Gtk/EnableAnimations"), Some(0));
        assert_eq!(parse_xsettings(&data, b"Gtk/Nope"), None);
    }

    #[test]
    fn settings_ini_section() {
        assert_eq!(
            parse_settings_ini("[Settings]\ngtk-enable-animations=false\n"),
            Some(false)
        );
        assert_eq!(
            parse_settings_ini("[Other]\ngtk-enable-animations=false\n"),
            None
        );
        assert_eq!(
            parse_settings_ini("[Settings]\ngtk-enable-animations = 1\n"),
            Some(true)
        );
    }
}
