//! Значки: Font Awesome 7.3.1 по строке классов 0.4.9 (`<i class="fas fa-play">`, таблица
//! `crates/hlsr-ui/data/fa.json`), одноцветные SVG (`glyphs/…`, `logos/…`) и кольцо `fa-spin`.
//!
//! Правила классов — как `hlsr_ui::fa` (`all.css` пакета): `fas`/`fa-solid`/`fa` — solid 900,
//! `far`/`fa-regular` — regular 400, `fab`/`fa-brands` — brands 400, без префикса — solid;
//! первое известное имя `fa-*` — глиф (с псевдонимами v5); `fa-spin` — кольцо вместо глифа
//! (`launcher.css`). `--fa-width: auto`: ширина по глифу, `line-height: 1`.
//!
//! shortcut: таблица разбирается здесь, а не берётся из `hlsr_ui::fa`: hlsr-gpui пока не
//! зависит от hlsr-ui (до F1 в нём Slint); когда зависимость появится (A1) — взять оттуда.

use crate::scale::u;
use gpui::{
    AnyElement, Div, FontWeight, Hsla, IntoElement, ParentElement, Styled, Svg,
    TimedTransformation, Transformation, div, svg,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const DATA: &str = include_str!("../../hlsr-ui/data/fa.json");

/// Дорожка кольца: окружность 1em, рамка .14em (`border: .14em solid`).
pub(crate) const SPIN_TRACK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100"><circle cx="50" cy="50" r="43" fill="none" stroke="#fff" stroke-width="14"/></svg>"##;
/// Верхняя рамка круга (`border-top-color`): четверть окружности сверху, стыки по 45°.
pub(crate) const SPIN_ARC: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100"><path d="M19.594 19.594A43 43 0 0 1 80.406 19.594" fill="none" stroke="#fff" stroke-width="14"/></svg>"##;
/// `l-spin` 0.85s linear infinite.
const SPIN_PERIOD: Duration = Duration::from_millis(850);
/// `fas fa-chevron-down` (Font Awesome Free 7.3.1, CC BY 4.0) контуром: глиф текста GPUI не
/// повернуть. viewBox — квадрат em глифа (448 × 512 единиц, базовая линия на 448).
pub(crate) const CHEVRON_DOWN: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="448" height="512" viewBox="0 0 448 512"><path fill="#fff" d="M201.4 406.6c12.5 12.5 32.8 12.5 45.3 0l192-192c12.5-12.5 12.5-32.8 0-45.3s-32.8-12.5-45.3 0L224 338.7 54.6 169.4c-12.5-12.5-32.8-12.5-45.3 0s-12.5 32.8 0 45.3l192 192z"/></svg>"##;

/// Начертание шрифта значков.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    Solid,
    Regular,
    Brands,
}

impl Style {
    pub fn family(self) -> &'static str {
        match self {
            Style::Solid | Style::Regular => crate::text::FA,
            Style::Brands => crate::text::FA_BRANDS,
        }
    }

    pub fn weight(self) -> FontWeight {
        match self {
            Style::Solid => FontWeight::BLACK,
            Style::Regular | Style::Brands => FontWeight::NORMAL,
        }
    }

    fn from_name(name: &str) -> Option<Style> {
        match name {
            "solid" => Some(Style::Solid),
            "regular" => Some(Style::Regular),
            "brands" => Some(Style::Brands),
            _ => None,
        }
    }
}

/// Разрешённый значок.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    pub ch: char,
    pub style: Style,
    /// Среди классов есть `fa-spin`: вместо глифа рисуется кольцо.
    pub spin: bool,
}

#[derive(Deserialize)]
struct RawIcon {
    code: String,
}

#[derive(Deserialize)]
struct RawTable {
    icons: HashMap<String, RawIcon>,
    aliases: HashMap<String, String>,
    prefixes: HashMap<String, String>,
}

struct Table {
    icons: HashMap<String, char>,
    aliases: HashMap<String, String>,
    prefixes: HashMap<String, Style>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let raw: RawTable = serde_json::from_str(DATA).expect("data/fa.json");
        let icons = raw
            .icons
            .into_iter()
            .filter_map(|(name, icon)| {
                let code = u32::from_str_radix(&icon.code, 16).ok()?;
                Some((name, char::from_u32(code)?))
            })
            .collect();
        let prefixes = raw
            .prefixes
            .into_iter()
            .filter_map(|(prefix, style)| Some((prefix, Style::from_name(&style)?)))
            .collect();
        Table {
            icons,
            aliases: raw.aliases,
            prefixes,
        }
    })
}

/// Строка классов `<i class="…">` → значок. `None` — имени значка нет.
pub fn fa(classes: &str) -> Option<Glyph> {
    let table = table();
    let mut style = None;
    let mut ch = None;
    let mut spin = false;
    for token in classes.split_whitespace() {
        if let Some(found) = table.prefixes.get(token) {
            style.get_or_insert(*found);
        } else if token == "fa-spin" {
            spin = true;
        } else if let Some(name) = token.strip_prefix("fa-")
            && ch.is_none()
        {
            let name = table.aliases.get(name).map_or(name, String::as_str);
            ch = table.icons.get(name).copied();
        }
    }
    // Кольцо не зависит от имени значка.
    let ch = ch.or(spin.then_some(' '))?;
    Some(Glyph {
        ch,
        style: style.unwrap_or(Style::Solid),
        spin,
    })
}

/// Глиф значка `<i class="…">` текстом `size` CSS px: цвет наследуется (`currentColor`),
/// так что следует за `hover` родителя. Нет такого значка — пустое место. `fa-spin` — [`icon`].
pub fn fa_glyph(classes: &str, size: f32) -> Div {
    let el = div().flex_none().text_size(u(size)).line_height(u(size));
    match fa(classes) {
        Some(glyph) if !glyph.spin => el
            .font_family(glyph.style.family())
            .font_weight(glyph.style.weight())
            .child(glyph.ch.to_string()),
        _ => el,
    }
}

/// Значок, повёрнутый на `degrees` вокруг середины своей коробки (`transform: rotate()` у `<i>`):
/// у текста GPUI поворота нет, поэтому повёрнутый глиф — его контур из шрифта в SVG (маска цвета
/// `color`, коробка та же: ширина глифа × 1em). Контур и при угле 0: смена глифа на контур в
/// начале и в конце поворота меняла бы растеризацию.
pub fn fa_turned(classes: &str, size: f32, degrees: f32, color: Hsla) -> AnyElement {
    let turn = degrees.rem_euclid(360.);
    let outline = fa(classes).filter(|glyph| !glyph.spin).and_then(glyph_svg);
    match outline {
        Some((data, width)) => svg()
            .data(&data)
            .flex_none()
            .w(u(size * width))
            .h(u(size))
            .text_color(color)
            .with_transformation(Transformation::rotate(gpui::radians(turn.to_radians())))
            .into_any_element(),
        None => fa_glyph(classes, size).text_color(color).into_any_element(),
    }
}

/// Контур глифа в SVG и ширина его коробки в em. Коробка — строка `line-height: 1`: базовая
/// линия, как у текста, на половине лишнего над `ascent` (единицы шрифта, y вниз).
fn glyph_svg(glyph: Glyph) -> Option<(Arc<[u8]>, f32)> {
    type Outline = Option<(Arc<[u8]>, f32)>;
    static CACHE: OnceLock<Mutex<HashMap<(Style, char), Outline>>> = OnceLock::new();
    let mut cache =
        (CACHE.get_or_init(Default::default).lock()).unwrap_or_else(|error| error.into_inner());
    cache
        .entry((glyph.style, glyph.ch))
        .or_insert_with(|| {
            use skrifa::MetadataProvider;
            use skrifa::instance::{LocationRef, Size};
            use skrifa::outline::{DrawSettings, pen::SvgPen};
            let file = match glyph.style {
                Style::Solid => crate::text::FA_FILES[0],
                Style::Regular => crate::text::FA_FILES[1],
                Style::Brands => crate::text::FA_FILES[2],
            };
            let font = skrifa::FontRef::new(file).ok()?;
            let id = font.charmap().map(glyph.ch)?;
            let (size, at) = (Size::unscaled(), LocationRef::default());
            let metrics = font.metrics(size, at);
            let em = f32::from(metrics.units_per_em);
            let advance = font.glyph_metrics(size, at).advance_width(id)?;
            let mut pen = SvgPen::with_precision(1);
            (font.outline_glyphs().get(id)?)
                .draw(DrawSettings::unhinted(size, at), &mut pen)
                .ok()?;
            let top = (em - (metrics.ascent - metrics.descent)) / 2. + metrics.ascent;
            let data = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 {} {advance} {em}'>\
                 <path transform='scale(1,-1)' d='{}'/></svg>",
                -top, &*pen
            );
            Some((Arc::from(data.into_bytes()), advance / em))
        })
        .clone()
}

/// Значок с кольцом `fa-spin`: SVG цвет не наследует, поэтому `color` явный.
pub fn icon(classes: &str, size: f32, color: Hsla) -> AnyElement {
    match fa(classes) {
        Some(glyph) if glyph.spin => spinner(size, color).into_any_element(),
        _ => fa_glyph(classes, size).text_color(color).into_any_element(),
    }
}

/// Кольцо `fa-spin`: квадрат 1em, дорожка цветом 28 %, верхняя четверть вращается 0,85 с.
/// При reduced-motion GPUI рисует его неподвижным (в 0.4.9 кольцо тогда мигает).
pub fn spinner(size: f32, color: Hsla) -> Div {
    div()
        .relative()
        .flex_none()
        .size(u(size))
        .child(
            svg()
                .path("hlsr/spin-track.svg")
                .absolute()
                .size_full()
                .text_color(color.opacity(0.28)),
        )
        .child(
            svg()
                .path("hlsr/spin-arc.svg")
                .absolute()
                .size_full()
                .text_color(color)
                .with_timed_transformation(TimedTransformation::rotation(
                    SPIN_PERIOD,
                    Transformation::default(),
                )),
        )
}

/// `fas fa-chevron-down` `size` CSS px с `transform: rotate(degrees)` вокруг середины (стрелка
/// меню, которая переворачивается при открытии). Коробка — как у глифа [`fa_glyph`]: ширина по
/// глифу, высота 1em; SVG цвет не наследует, поэтому `color` явный.
pub fn chevron(size: f32, degrees: f32, color: Hsla) -> Svg {
    svg()
        .path("hlsr/chevron-down.svg")
        .flex_none()
        .w(u(size * 448. / 512.))
        .h(u(size))
        .text_color(color)
        .with_transformation(Transformation::rotate(gpui::radians(degrees.to_radians())))
}

/// Одноцветный SVG из `crates/hlsr-ui/assets` (`glyphs/discord.svg`, `logos/ghosting.svg`):
/// маска, цвет — `color` (как `fill: currentColor`).
pub fn svg_icon(path: &'static str, size: f32, color: Hsla) -> Svg {
    svg().path(path).flex_none().size(u(size)).text_color(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_resolve_like_fa_css() {
        let play = fa("fas fa-play").unwrap();
        assert_eq!(
            (play.ch, play.style, play.spin),
            ('\u{f04b}', Style::Solid, false)
        );
        assert_eq!(fa("fa-play").unwrap().style, Style::Solid);
        assert_eq!(fa("far fa-heart").unwrap().style, Style::Regular);
        assert_eq!(fa("fab fa-discord").unwrap().style, Style::Brands);
        // Псевдоним v5 и посторонние классы.
        assert_eq!(
            fa("fas fa-fw fa-times").unwrap().ch,
            fa("fas fa-xmark").unwrap().ch
        );
        let spin = fa("fas fa-circle-notch fa-spin").unwrap();
        assert!(spin.spin);
        assert!(fa("fas fa-spin").unwrap().spin);
        assert_eq!(fa("fas fa-нет"), None);
        // Значки, новые в 0.4.9.
        for name in [
            "ellipsis",
            "film",
            "expand",
            "compress",
            "trophy",
            "crown",
            "calendar-check",
        ] {
            assert!(fa(&format!("fas fa-{name}")).is_some(), "{name}");
        }
        assert!(fa("fab fa-telegram").is_some());
    }

    /// Повёрнутый значок — контур глифа: коробка ширина глифа × 1em, контур внутри неё.
    #[test]
    fn turned_glyph_is_its_outline_in_the_text_box() {
        let (data, width) = glyph_svg(fa("fas fa-chevron-up").unwrap()).unwrap();
        let text = std::str::from_utf8(&data).unwrap();
        assert!(
            text.contains("<path") && text.contains("viewBox='0 -"),
            "{text}"
        );
        assert!((0.5..=1.25).contains(&width), "{width}");
        // Тот же знак второй раз — из кеша.
        let again = glyph_svg(fa("fas fa-chevron-up").unwrap()).unwrap();
        assert!(Arc::ptr_eq(&data, &again.0));
    }
}
