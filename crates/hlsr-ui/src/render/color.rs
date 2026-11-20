//! Цвета CSS 0.4.7: `color-mix(in srgb, A p%, B)`
//! (CSS Color 5 — интерполяция премультиплицированных каналов sRGB, как в
//! Chromium) и разбор `rgba(…)`/`#rrggbbaa` из строк `launcher.css`.

use crate::Rgba as Color;

/// Каналы цвета в долях 0…1, прозрачность не премультиплицирована.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RgbaF {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl RgbaF {
    pub const TRANSPARENT: RgbaF = RgbaF {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    pub fn from_color(c: Color) -> RgbaF {
        RgbaF {
            r: f32::from(c.r) / 255.0,
            g: f32::from(c.g) / 255.0,
            b: f32::from(c.b) / 255.0,
            a: f32::from(c.a) / 255.0,
        }
    }

    /// В 8 бит с округлением, как Chromium переводит цвет в `SkColor`.
    pub fn to_color(self) -> Color {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Color::new(q(self.r), q(self.g), q(self.b), q(self.a))
    }
}

/// `color-mix(in srgb, a p, b)`, где `p` — доля первого цвета (0…1), второй
/// берётся с долей `1 − p`. Прозрачность — средняя по долям, каналы —
/// среднее премультиплицированных значений, делённое обратно на прозрачность
/// (CSS Color 5, §2.4). `transparent` = `rgba(0, 0, 0, 0)`.
pub fn color_mix(a: Color, p: f32, b: Color) -> Color {
    mix_rgba(RgbaF::from_color(a), p, RgbaF::from_color(b)).to_color()
}

pub fn mix_rgba(a: RgbaF, p: f32, b: RgbaF) -> RgbaF {
    let p = if p.is_finite() {
        p.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let q = 1.0 - p;
    let alpha = a.a * p + b.a * q;
    if alpha <= 0.0 {
        return RgbaF::TRANSPARENT;
    }
    let channel = |x: f32, y: f32| (x * a.a * p + y * b.a * q) / alpha;
    RgbaF {
        r: channel(a.r, b.r),
        g: channel(a.g, b.g),
        b: channel(a.b, b.b),
        a: alpha,
    }
}

/// Цвет из CSS: `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb(r, g, b)`, `rgba(r, g, b, a)`,
/// `transparent`, `white`, `black`. Прочее — `None`.
pub fn parse_css_color(text: &str) -> Option<Color> {
    let t = text.trim();
    match t {
        "transparent" => return Some(Color::new(0, 0, 0, 0)),
        "white" => return Some(Color::rgb(255, 255, 255)),
        "black" => return Some(Color::rgb(0, 0, 0)),
        _ => {}
    }
    if let Some(hex) = t.strip_prefix('#') {
        let digit = |i: usize, n: usize| u8::from_str_radix(hex.get(i..i + n)?, 16).ok();
        return match hex.len() {
            3 => Some(Color::rgb(
                digit(0, 1)? * 17,
                digit(1, 1)? * 17,
                digit(2, 1)? * 17,
            )),
            6 => Some(Color::rgb(digit(0, 2)?, digit(2, 2)?, digit(4, 2)?)),
            8 => Some(Color::new(
                digit(0, 2)?,
                digit(2, 2)?,
                digit(4, 2)?,
                digit(6, 2)?,
            )),
            _ => None,
        };
    }
    let inner = t
        .strip_prefix("rgba(")
        .or_else(|| t.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let byte =
        |s: &str| -> Option<u8> { Some(s.parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8) };
    match parts[..] {
        [r, g, b] => Some(Color::rgb(byte(r)?, byte(g)?, byte(b)?)),
        [r, g, b, a] => {
            let a = match a.strip_suffix('%') {
                Some(pct) => pct.parse::<f32>().ok()? / 100.0,
                None => a.parse::<f32>().ok()?,
            };
            Some(Color::new(
                byte(r)?,
                byte(g)?,
                byte(b)?,
                (a.clamp(0.0, 1.0) * 255.0).round() as u8,
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argb(c: Color) -> [u8; 4] {
        [c.a, c.r, c.g, c.b]
    }

    #[test]
    fn mix_with_transparent_keeps_color_and_scales_alpha() {
        // color-mix(in srgb, #00abff 12%, transparent) → rgba(0, 171, 255, 0.12):
        // премультипликация не даёт цвету потемнеть к прозрачному.
        let accent = Color::rgb(0x00, 0xAB, 0xFF);
        let out = color_mix(accent, 0.12, Color::new(0, 0, 0, 0));
        assert_eq!(argb(out), [31, 0x00, 0xAB, 0xFF]);
    }

    #[test]
    fn mix_two_translucent_colors_like_css_color_5() {
        // color-mix(in srgb, #00abff 45%, rgba(96, 190, 255, 0.16)) — акцентная
        // рамка темы HLSR. α = 0.45 + 0.16·0.55 = 0.538;
        // r = (0·0.45 + 96·0.088) / 0.538 = 15.70; g = (171·0.45 + 190·0.088) / 0.538 = 174.11.
        let accent = Color::rgb(0x00, 0xAB, 0xFF);
        let border = Color::new(96, 190, 255, 41);
        let out = color_mix(accent, 0.45, border);
        let a = 0.45 + (41.0 / 255.0) * 0.55;
        let r = (96.0 * (41.0 / 255.0) * 0.55) / a;
        let g = (171.0 * 0.45 + 190.0 * (41.0 / 255.0) * 0.55) / a;
        assert_eq!(
            argb(out),
            [
                (a * 255.0_f32).round() as u8,
                r.round() as u8,
                g.round() as u8,
                255
            ]
        );
    }

    #[test]
    fn mix_with_white_lightens() {
        // color-mix(in srgb, #cf3247 72%, white) — шар фона инструмента.
        let orb = Color::rgb(0xCF, 0x32, 0x47);
        let out = color_mix(orb, 0.72, Color::rgb(255, 255, 255));
        assert_eq!(
            argb(out),
            [
                255,
                (207.0_f32 * 0.72 + 255.0 * 0.28).round() as u8,
                (50.0_f32 * 0.72 + 255.0 * 0.28).round() as u8,
                (71.0_f32 * 0.72 + 255.0 * 0.28).round() as u8
            ]
        );
    }

    #[test]
    fn mix_of_two_transparent_is_transparent() {
        let t = Color::new(0, 0, 0, 0);
        assert_eq!(argb(color_mix(t, 0.3, t)), [0, 0, 0, 0]);
    }

    #[test]
    fn parses_launcher_css_colors() {
        assert_eq!(
            parse_css_color("rgba(0, 0, 0, 0.55)").map(argb),
            Some([140, 0, 0, 0])
        );
        assert_eq!(
            parse_css_color("rgba(255,255,255,0.14)").map(argb),
            Some([36, 255, 255, 255])
        );
        assert_eq!(
            parse_css_color("#0b0e13").map(argb),
            Some([255, 11, 14, 19])
        );
        assert_eq!(
            parse_css_color("#00ABFF29").map(argb),
            Some([0x29, 0, 0xAB, 0xFF])
        );
        assert_eq!(
            parse_css_color("#fff").map(argb),
            Some([255, 255, 255, 255])
        );
        assert_eq!(parse_css_color("transparent").map(argb), Some([0, 0, 0, 0]));
        assert_eq!(parse_css_color("var(--x)"), None);
    }
}
