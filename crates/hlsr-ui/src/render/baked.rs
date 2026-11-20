//! Разбор `box-shadow` из `launcher.css` (`themes.json`, раздел `unsupported`): тени рисует
//! `BoxShadow` GPUI, запечённые слои Slint удалены.

use crate::Rgba;
use crate::render::color::parse_css_color;

/// Одна тень `box-shadow`; длины — в логических px (CSS px).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxShadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Rgba,
    pub inset: bool,
}

/// Разбор значения `box-shadow` из `launcher.css` (`themes.json`, раздел
/// `unsupported`): тени через запятую, в каждой — `inset`, 2–4 длины в px и цвет.
pub fn parse_box_shadows(css: &str) -> Option<Vec<BoxShadow>> {
    split_top_level(css)
        .into_iter()
        .map(|part| parse_one_shadow(part.trim()))
        .collect()
}

fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn parse_one_shadow(text: &str) -> Option<BoxShadow> {
    let mut inset = false;
    let mut lengths = Vec::new();
    let mut color = None;
    let mut rest = text;
    while !rest.is_empty() {
        let token_end = if rest.starts_with("rgb") {
            rest.find(')')? + 1
        } else {
            rest.find(char::is_whitespace).unwrap_or(rest.len())
        };
        let token = &rest[..token_end];
        rest = rest[token_end..].trim_start();
        if token == "inset" {
            inset = true;
        } else if let Some(c) = parse_css_color(token) {
            color = Some(c);
        } else {
            let number = token.strip_suffix("px").unwrap_or(token);
            lengths.push(number.parse::<f32>().ok()?);
        }
    }
    if !(2..=4).contains(&lengths.len()) {
        return None;
    }
    Some(BoxShadow {
        x: lengths[0],
        y: lengths[1],
        blur: lengths.get(2).copied().unwrap_or(0.0).max(0.0),
        spread: lengths.get(3).copied().unwrap_or(0.0),
        color: color.unwrap_or(Rgba::rgb(0, 0, 0)),
        inset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shadow_lists_from_launcher_css() {
        let pop = parse_box_shadows("0 18px 48px rgba(0, 0, 0, 0.55)").unwrap();
        assert_eq!(
            pop,
            vec![BoxShadow {
                x: 0.0,
                y: 18.0,
                blur: 48.0,
                spread: 0.0,
                color: Rgba::new(0, 0, 0, 140),
                inset: false,
            }]
        );
        let rim = parse_box_shadows(
            "inset 2px 2px 1px 0 rgba(255, 255, 255, 0.28), inset -1px -1px 1px 1px rgba(255, 255, 255, 0.14)",
        )
        .unwrap();
        assert_eq!(rim.len(), 2);
        assert!(rim.iter().all(|s| s.inset));
        assert_eq!(
            (rim[1].x, rim[1].y, rim[1].blur, rim[1].spread),
            (-1.0, -1.0, 1.0, 1.0)
        );
        let open = parse_box_shadows("24px 0 48px -32px rgba(0, 0, 0, 0.7)").unwrap();
        assert_eq!(open[0].spread, -32.0);
        assert!(parse_box_shadows("0 0 var(--x)").is_none());
    }
}
