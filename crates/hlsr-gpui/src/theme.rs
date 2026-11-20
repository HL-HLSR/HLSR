//! Тема лаунчера: токены `crates/hlsr-ui/themes.json` (CSS-переменные `.launcher-root` 0.4.9
//! после `rootStyle()`, `tools/reference/export-themes.js`), ступени сетки по размеру окна
//! (`@media` из `launcher.css`), варианты акцента («Бумага»), перетекание при смене темы.
//!
//! Тема `liquid-glass` показывается, пока включён [`LIQUID_GLASS_ENABLED`] (стекло — `liquid.rs`);
//! выключенная — сохранённый выбор читается как `hlsr`, как `themeById` 0.4.9 у неизвестной, prefs
//! не переписываются. Смена темы в 0.4.9 — View Transition 0,42 с `--l-ease` (снимок до и после,
//! перетекание); здесь так же перетекают цвета токенов.

use crate::scale::u;
use gpui::{App, Global, Hsla, Rems, Rgba, Window};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const DATA: &str = include_str!("../../hlsr-ui/themes.json");
/// Тема по умолчанию и замена неизвестной (`DEFAULT_THEME`, `themeById`).
pub const DEFAULT_THEME: &str = "hlsr";
/// Жидкое стекло: размытие и преломление кадра под окнами и панелями (`liquid.rs`).
pub const LIQUID_GLASS: &str = "liquid-glass";
/// Показывать ли тему Liquid Glass (задача liquid, 18.11). `false` — тема скрыта из выбора, а
/// сохранённая читается как `hlsr`.
pub const LIQUID_GLASS_ENABLED: bool = true;
/// `::view-transition-*(root)`: 0.42s cubic-bezier(0.22, 1, 0.36, 1).
const SWITCH: Duration = Duration::from_millis(420);
const SWITCH_EASE: [f32; 4] = [0.22, 1., 0.36, 1.];

/// Значение переменной темы.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Value {
    /// `#RRGGBBAA`.
    Color(Rgba),
    Gradient(Gradient),
    /// CSS px макета.
    Length(f32),
    Number(f32),
    /// Миллисекунды.
    Duration(f32),
    /// `cubic-bezier(x1, y1, x2, y2)`.
    Easing([f32; 4]),
    /// Не выразить одним значением (тени, несколько градиентов): строка CSS.
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Gradient {
    /// Градусы CSS: 180 — сверху вниз.
    pub angle: f32,
    pub stops: Vec<Stop>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Stop {
    pub color: Rgba,
    /// 0…1.
    pub pos: f32,
}

pub type Tokens = HashMap<String, Value>;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Feature {
    MinWidth,
    MaxWidth,
    MinHeight,
    MaxHeight,
}

/// Условие `@media`: `(min-height: 708px)` и т. п.
#[derive(Debug, Clone, Copy, Deserialize)]
struct Condition {
    feature: Feature,
    px: f32,
}

impl Condition {
    fn matches(self, (width, height): (f32, f32)) -> bool {
        match self.feature {
            Feature::MinWidth => width >= self.px,
            Feature::MaxWidth => width <= self.px,
            Feature::MinHeight => height >= self.px,
            Feature::MaxHeight => height <= self.px,
        }
    }
}

/// Значения сетки при одном наборе выполненных условий.
#[derive(Debug, Clone, Deserialize)]
struct Case {
    matches: Vec<bool>,
    values: Tokens,
}

#[derive(Debug, Clone, Deserialize)]
struct Layout {
    conditions: Vec<Condition>,
    cases: Vec<Case>,
}

/// Варианты акцента темы (`PAPER_ACCENTS`): токены, которые меняются с акцентом.
#[derive(Debug, Clone, Deserialize)]
struct Accents {
    default: String,
    order: Vec<String>,
    values: HashMap<String, Tokens>,
}

#[derive(Deserialize)]
struct Raw {
    order: Vec<String>,
    themes: HashMap<String, Tokens>,
    /// Переменные только для `[data-theme="…"]`.
    #[serde(default)]
    scoped: HashMap<String, Tokens>,
    /// Варианты акцента по id темы.
    #[serde(default)]
    accents: HashMap<String, Accents>,
    layout: Layout,
}

/// `themes.json` (жидкое стекло — по [`LIQUID_GLASS_ENABLED`]).
#[derive(Debug)]
pub struct Table {
    order: Vec<String>,
    themes: HashMap<String, Tokens>,
    accents: HashMap<String, Accents>,
    layout: Layout,
}

impl Table {
    pub fn parse(json: &str) -> Result<Table, String> {
        Table::parse_with(json, LIQUID_GLASS_ENABLED)
    }

    /// Таблица с жидким стеклом (`liquid`) или без него.
    fn parse_with(json: &str, liquid: bool) -> Result<Table, String> {
        let mut raw: Raw = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if !liquid {
            raw.order.retain(|id| id != LIQUID_GLASS);
        }
        if raw.order.first().map(String::as_str) != Some(DEFAULT_THEME) {
            return Err(format!("первая тема не {DEFAULT_THEME}: {:?}", raw.order));
        }
        let mut themes = HashMap::new();
        for id in &raw.order {
            let mut tokens = raw.themes.remove(id).ok_or(format!("нет темы {id}"))?;
            tokens.extend(raw.scoped.remove(id).unwrap_or_default());
            themes.insert(id.clone(), tokens);
        }
        for (number, case) in raw.layout.cases.iter().enumerate() {
            if case.matches.len() != raw.layout.conditions.len() {
                return Err(format!("layout.cases[{number}]: не все условия"));
            }
        }
        raw.accents.retain(|id, _| themes.contains_key(id));
        for (id, set) in &raw.accents {
            if let Some(missing) = set
                .order
                .iter()
                .chain([&set.default])
                .find(|accent| !set.values.contains_key(*accent))
            {
                return Err(format!("accents.{id}: нет значений {missing}"));
            }
        }
        Ok(Table {
            order: raw.order,
            themes,
            accents: raw.accents,
            layout: raw.layout,
        })
    }

    /// Встроенная таблица (разбирается один раз).
    pub fn embedded() -> Arc<Table> {
        static TABLE: OnceLock<Arc<Table>> = OnceLock::new();
        TABLE
            .get_or_init(|| {
                Arc::new(Table::parse(DATA).unwrap_or_else(|e| panic!("themes.json: {e}")))
            })
            .clone()
    }

    /// Темы в порядке выбора.
    pub fn ids(&self) -> &[String] {
        &self.order
    }

    /// Id темы, которую показывать: неизвестная (и выключенная `liquid-glass`) → `hlsr`.
    pub fn theme_id<'a>(&'a self, id: &'a str) -> &'a str {
        if self.themes.contains_key(id) {
            id
        } else {
            DEFAULT_THEME
        }
    }

    /// Варианты акцента темы (пусто — у темы свой акцент).
    pub fn accents(&self, id: &str) -> &[String] {
        self.accents.get(id).map_or(&[], |set| &set.order)
    }

    /// Акцент, который показывать, и его токены: неизвестный — по умолчанию (`themeVars`).
    fn accent<'a>(&'a self, id: &str, accent: Option<&str>) -> Option<(&'a str, &'a Tokens)> {
        let set = self.accents.get(id)?;
        let key = accent
            .filter(|accent| set.values.contains_key(*accent))
            .unwrap_or(&set.default);
        let (key, tokens) = set.values.get_key_value(key)?;
        Some((key, tokens))
    }

    /// Ступень сетки для CSS-окна: случай с теми же выполненными условиями, иначе без условий.
    fn case(&self, viewport: (f32, f32)) -> Option<usize> {
        let matches: Vec<bool> = self
            .layout
            .conditions
            .iter()
            .map(|condition| condition.matches(viewport))
            .collect();
        let cases = &self.layout.cases;
        cases
            .iter()
            .position(|case| case.matches == matches)
            .or_else(|| {
                cases
                    .iter()
                    .position(|case| case.matches.iter().all(|m| !m))
            })
    }

    fn resolve(&self, id: &str, accent: Option<&str>, case: Option<usize>) -> Tokens {
        let mut tokens = self.themes[id].clone();
        if let Some((_, values)) = self.accent(id, accent) {
            tokens.extend(values.clone());
        }
        if let Some(case) = case {
            tokens.extend(self.layout.cases[case].values.clone());
        }
        tokens
    }
}

/// Тема окна: токены выбранной темы и ступени сетки; во время смены — смесь со старой.
#[derive(Clone)]
pub struct Theme {
    table: Arc<Table>,
    id: String,
    /// Выбранный вариант акцента (`prefs.paperAccent`): помнится и в других темах.
    accent: Option<String>,
    case: Option<usize>,
    tokens: Arc<Tokens>,
    /// Смена темы: токены до неё и когда началась.
    from: Option<(Arc<Tokens>, Instant)>,
    /// Доля Liquid Glass в начале смены ([`Theme::liquid_share`]).
    from_share: f32,
    /// Доля новой темы (после кривой): 1 — смены нет.
    mix: f32,
}

impl Global for Theme {}

impl Theme {
    /// Тема `hlsr` для окна 1600×900.
    pub fn new(table: Arc<Table>) -> Theme {
        let id = DEFAULT_THEME.to_string();
        let case = table.case((1600., 900.));
        let tokens = Arc::new(table.resolve(&id, None, case));
        Theme {
            table,
            id,
            accent: None,
            case,
            tokens,
            from: None,
            from_share: 0.,
            mix: 1.,
        }
    }

    pub fn table(&self) -> &Table {
        &self.table
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Видна тема Liquid Glass (стекло окон и панелей, `liquid.rs`).
    pub fn liquid(&self) -> bool {
        self.id == LIQUID_GLASS
    }

    /// Доля Liquid Glass в том, что видно сейчас: 1 — эта тема, 0 — другая, между — смена туда
    /// или обратно (перетекание 0,42 с, как цвета). В 0.4.9 `startViewTransition` растворяет
    /// снимок старой темы в новом: стекло (`liquid.rs`) есть, пока доля больше нуля.
    pub fn liquid_share(&self) -> f32 {
        let to = if self.liquid() { 1. } else { 0. };
        match self.from {
            Some(_) => self.from_share + (to - self.from_share) * self.mix,
            None => to,
        }
    }

    /// Вариант акцента, который виден сейчас (`None` — у темы свой акцент).
    pub fn accent(&self) -> Option<&str> {
        Some(self.table.accent(&self.id, self.accent.as_deref())?.0)
    }

    /// Выбрать тему (`setTheme`). `animate` — перетекание 0,42 с (без анимаций — сразу).
    /// Возвращает, изменилось ли что-то.
    pub fn set(&mut self, id: &str, accent: Option<&str>, animate: bool, now: Instant) -> bool {
        let id = self.table.theme_id(id).to_string();
        // Без своего акцента остаётся прежний (`accent || prefs.paperAccent`).
        let accent = accent.map(str::to_string).or(self.accent.clone());
        if id == self.id && accent == self.accent {
            return false;
        }
        let before = self.blended();
        self.from_share = self.liquid_share();
        self.id = id;
        self.accent = accent;
        self.tokens = Arc::new(
            self.table
                .resolve(&self.id, self.accent.as_deref(), self.case),
        );
        self.from = animate.then(|| (Arc::new(before), now));
        self.mix = if animate { 0. } else { 1. };
        true
    }

    /// CSS-окно (размер окна / зум): ступень сетки `@media`.
    pub fn set_viewport(&mut self, viewport: (f32, f32)) {
        let case = self.table.case(viewport);
        if case != self.case {
            self.case = case;
            self.tokens = Arc::new(self.table.resolve(&self.id, self.accent.as_deref(), case));
        }
    }

    /// Идёт перетекание из прежней темы.
    pub fn blending(&self) -> bool {
        self.from.is_some()
    }

    /// Шаг перетекания к `now`. `true` — смена ещё идёт, нужен следующий кадр.
    pub fn tick(&mut self, now: Instant) -> bool {
        let Some((_, started)) = &self.from else {
            return false;
        };
        let t = now.saturating_duration_since(*started).as_secs_f32() / SWITCH.as_secs_f32();
        if t >= 1. {
            self.from = None;
            self.mix = 1.;
            return false;
        }
        self.mix = bezier(SWITCH_EASE)(t);
        true
    }

    /// Токены, какими они видны сейчас (цвета смешаны).
    fn blended(&self) -> Tokens {
        self.tokens
            .iter()
            .map(|(key, value)| (key.clone(), self.mixed(key, value)))
            .collect()
    }

    fn mixed(&self, key: &str, value: &Value) -> Value {
        let Some((from, _)) = &self.from else {
            return value.clone();
        };
        match (from.get(key), value) {
            (Some(Value::Color(a)), Value::Color(b)) => Value::Color(lerp(*a, *b, self.mix)),
            (Some(Value::Gradient(a)), Value::Gradient(b)) if a.stops.len() == b.stops.len() => {
                let stops = a
                    .stops
                    .iter()
                    .zip(&b.stops)
                    .map(|(a, b)| Stop {
                        color: lerp(a.color, b.color, self.mix),
                        pos: b.pos,
                    })
                    .collect();
                Value::Gradient(Gradient {
                    angle: b.angle,
                    stops,
                })
            }
            _ => value.clone(),
        }
    }

    fn get(&self, key: &str) -> Option<&Value> {
        let value = self.tokens.get(key);
        debug_assert!(value.is_some(), "в теме {} нет токена {key}", self.id);
        value
    }

    /// Цвет токена (`--l-text` → `color("l-text")`).
    pub fn color(&self, key: &str) -> Hsla {
        self.try_color(key).unwrap_or_else(|| {
            debug_assert!(false, "токен {key} темы {} не цвет", self.id);
            gpui::transparent_black()
        })
    }

    /// Цвет, если он есть в этой теме (`--l-paper-*` и другие токены одной темы).
    pub fn try_color(&self, key: &str) -> Option<Hsla> {
        match self.tokens.get(key)? {
            value @ Value::Color(_) => match self.mixed(key, value) {
                Value::Color(color) => Some(color.into()),
                _ => None,
            },
            _ => None,
        }
    }

    /// `rgba(var(--l-shade-rgb), alpha)`: цвет-канал токена с прозрачностью.
    pub fn rgba(&self, key: &str, alpha: f32) -> Hsla {
        Hsla {
            a: alpha,
            ..self.color(key)
        }
    }

    /// Длина в CSS px макета.
    pub fn len(&self, key: &str) -> f32 {
        match self.get(key) {
            Some(Value::Length(px)) => *px,
            _ => 0.,
        }
    }

    /// Длина как rem: `.w(theme.rem("l-rail-w"))`.
    pub fn rem(&self, key: &str) -> Rems {
        u(self.len(key))
    }

    /// Значение, которое одним токеном не выразить (тени, фильтры): строка CSS.
    pub fn css(&self, key: &str) -> Option<&str> {
        match self.tokens.get(key)? {
            Value::Unsupported(text) => Some(text),
            _ => None,
        }
    }

    pub fn number(&self, key: &str) -> f32 {
        match self.get(key) {
            Some(Value::Number(value)) => *value,
            _ => 0.,
        }
    }

    pub fn duration(&self, key: &str) -> Duration {
        match self.get(key) {
            Some(Value::Duration(ms)) => Duration::from_micros((ms * 1000.).round() as u64),
            _ => Duration::ZERO,
        }
    }

    /// Кривая токена (`--l-ease`) как функция для `with_easing`.
    pub fn easing(&self, key: &str) -> impl Fn(f32) -> f32 + Copy + use<> {
        match self.get(key) {
            Some(Value::Easing(points)) => bezier(*points),
            _ => bezier([0., 0., 1., 1.]),
        }
    }

    /// Градиент токена (цвета смешаны во время смены темы).
    pub fn gradient(&self, key: &str) -> Gradient {
        match self.get(key).map(|value| self.mixed(key, value)) {
            Some(Value::Gradient(gradient)) => gradient,
            _ => Gradient {
                angle: 180.,
                stops: Vec::new(),
            },
        }
    }
}

/// Тема приложения из встроенного `themes.json`.
pub fn init(cx: &mut App) {
    cx.set_global(Theme::new(Table::embedded()));
}

/// Начало кадра корня окна: ступень сетки по CSS-окну и шаг смены темы.
pub fn frame(window: &mut Window, cx: &mut App) {
    let viewport = crate::scale::viewport(window);
    let theme = cx.global_mut::<Theme>();
    theme.set_viewport(viewport);
    let animating = theme.tick(Instant::now());
    if animating {
        // Цвета меняются у всех видов: пока идёт смена, кэша видов нет ([`crate::cache::on`]).
        crate::motion::monitor_frame(window);
    }
    crate::liquid::frame(window, cx);
}

fn lerp(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let mix = |a: f32, b: f32| a + (b - a) * t;
    Rgba {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
        a: mix(a.a, b.a),
    }
}

/// `cubic-bezier(x1, y1, x2, y2)` как функция времени 0…1.
pub fn bezier([x1, y1, x2, y2]: [f32; 4]) -> impl Fn(f32) -> f32 + Copy {
    let curve = |a: f32, b: f32, s: f32| {
        let r = 1. - s;
        3. * r * r * s * a + 3. * r * s * s * b + s * s * s
    };
    move |t: f32| {
        if t <= 0. {
            return 0.;
        }
        if t >= 1. {
            return 1.;
        }
        // x(s) монотонна при x1, x2 ∈ [0, 1]: деление пополам.
        let (mut lo, mut hi) = (0_f32, 1_f32);
        for _ in 0..30 {
            let mid = (lo + hi) / 2.;
            if curve(x1, x2, mid) < t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        curve(y1, y2, (lo + hi) / 2.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEMES_049: [&str; 8] = [
        "hlsr",
        "lambda",
        "resonance",
        "combine",
        "xen",
        "steam2003",
        "midnight",
        "paper",
    ];

    fn hex(color: Hsla) -> String {
        let c = Rgba::from(color);
        let byte = |v: f32| (v * 255.).round() as u8;
        format!(
            "#{:02X}{:02X}{:02X}{:02X}",
            byte(c.r),
            byte(c.g),
            byte(c.b),
            byte(c.a)
        )
    }

    /// Таблица в форме 0.4.9: 8 тем + жидкое стекло, акценты «Бумаги», две ступени.
    fn fixture() -> Arc<Table> {
        fixture_with(false)
    }

    fn fixture_with(liquid: bool) -> Arc<Table> {
        let theme = |accent: &str| {
            format!(
                r##"{{"l-text":{{"color":"#F2F7FBFF"}},"l-accent":{{"color":"{accent}"}},
                "l-rail-w":{{"length":80}},"l-dur":{{"duration":260}},
                "l-ease":{{"easing":[0.22,1,0.36,1]}},"l-bg-zoom":{{"number":1}},
                "l-app-bg":{{"gradient":{{"angle":180,"stops":[{{"color":"{accent}","pos":0}},{{"color":"#000000FF","pos":1}}]}}}}}}"##
            )
        };
        let mut themes: Vec<String> = THEMES_049
            .iter()
            .map(|&id| {
                let accent = if id == "paper" {
                    "#D5876DFF"
                } else {
                    "#00ABFFFF"
                };
                format!("\"{id}\":{}", theme(accent))
            })
            .collect();
        themes.push(format!("\"liquid-glass\":{}", theme("#0A84FFFF")));
        let json = format!(
            r##"{{"order":["hlsr","lambda","resonance","combine","xen","steam2003","midnight","liquid-glass","paper"],
            "themes":{{{}}},
            "scoped":{{"paper":{{"l-paper-text":{{"color":"#EEE3DFFF"}}}},
                       "liquid-glass":{{"lg-tint":{{"unsupported":"x"}}}}}},
            "accents":{{"paper":{{"default":"clay","order":["clay","rose"],"values":{{
                "clay":{{"l-accent":{{"color":"#D5876DFF"}}}},
                "rose":{{"l-accent":{{"color":"#D78AA8FF"}}}}}}}}}},
            "layout":{{"conditions":[{{"feature":"min-height","px":860}}],
                "cases":[{{"matches":[false],"values":{{"l-rail-w":{{"length":80}}}}}},
                         {{"matches":[true],"values":{{"l-rail-w":{{"length":96}}}}}}]}}}}"##,
            themes.join(",")
        );
        Arc::new(Table::parse_with(&json, liquid).unwrap())
    }

    #[test]
    fn liquid_glass_follows_the_flag() {
        let table = fixture_with(true);
        let mut order = THEMES_049.to_vec();
        order.insert(7, LIQUID_GLASS);
        assert_eq!(table.ids(), order);
        assert_eq!(table.theme_id(LIQUID_GLASS), LIQUID_GLASS);
        let mut theme = Theme::new(table);
        assert!(theme.set(LIQUID_GLASS, None, false, Instant::now()));
        assert!(theme.liquid());
        assert_eq!(theme.try_color("lg-tint"), None, "строка CSS, не цвет");
    }

    #[test]
    fn eight_themes_without_liquid_glass() {
        let table = fixture();
        assert_eq!(table.ids(), THEMES_049);
        assert_eq!(table.theme_id("liquid-glass"), "hlsr");
        assert_eq!(table.theme_id("нет такой"), "hlsr");
        assert_eq!(table.theme_id("paper"), "paper");
        let mut theme = Theme::new(table);
        let now = Instant::now();
        assert!(!theme.set("liquid-glass", None, false, now));
        assert_eq!(theme.id(), "hlsr");
    }

    #[test]
    fn paper_accent_defaults_to_first_and_survives_theme_switch() {
        let mut theme = Theme::new(fixture());
        let now = Instant::now();
        assert!(theme.set("paper", None, false, now));
        assert_eq!(theme.accent(), Some("clay"));
        assert_eq!(hex(theme.color("l-accent")), "#D5876DFF");
        assert_eq!(hex(theme.color("l-paper-text")), "#EEE3DFFF");
        assert!(theme.set("paper", Some("rose"), false, now));
        assert_eq!(hex(theme.color("l-accent")), "#D78AA8FF");
        assert!(!theme.set("paper", Some("rose"), false, now));
        // Другая тема: акцента нет, токены одной «Бумаги» пропадают; выбор акцента помнится.
        assert!(theme.set("lambda", None, false, now));
        assert_eq!(theme.accent(), None);
        assert_eq!(hex(theme.color("l-accent")), "#00ABFFFF");
        assert_eq!(theme.try_color("l-paper-text"), None);
        assert!(theme.set("paper", None, false, now));
        assert_eq!(theme.accent(), Some("rose"));
        assert!(theme.set("paper", Some("нет"), false, now));
        assert_eq!(theme.accent(), Some("clay"));
    }

    #[test]
    fn switch_blends_colors_for_420ms_with_l_ease() {
        let mut theme = Theme::new(fixture());
        let start = Instant::now();
        theme.set("paper", None, true, start);
        assert_eq!(hex(theme.color("l-accent")), "#00ABFFFF");
        assert!(theme.tick(start + Duration::from_millis(100)));
        let ease = bezier(SWITCH_EASE)(100. / 420.);
        let mid = Rgba::from(theme.color("l-accent"));
        assert!(
            (mid.r - (0. + (213. / 255.) * ease)).abs() < 0.01,
            "{mid:?}"
        );
        let stop = theme.gradient("l-app-bg").stops[0].color;
        assert!((stop.r - mid.r).abs() < 1e-6);
        // Длины не перетекают.
        assert_eq!(theme.len("l-rail-w"), 96.);
        assert!(!theme.tick(start + SWITCH));
        assert_eq!(hex(theme.color("l-accent")), "#D5876DFF");
        assert!(!theme.tick(start + SWITCH * 2));
    }

    #[test]
    fn liquid_share_follows_the_switch_both_ways() {
        let mut theme = Theme::new(fixture_with(true));
        let start = Instant::now();
        assert_eq!(theme.liquid_share(), 0.);
        theme.set(LIQUID_GLASS, None, true, start);
        assert_eq!(theme.liquid_share(), 0.);
        theme.tick(start + Duration::from_millis(100));
        let ease = bezier(SWITCH_EASE)(100. / 420.);
        assert!((theme.liquid_share() - ease).abs() < 1e-6);
        // Обратно посреди смены: от текущей доли к нулю.
        let back = start + Duration::from_millis(100);
        theme.set("hlsr", None, true, back);
        assert!((theme.liquid_share() - ease).abs() < 1e-6);
        assert!(!theme.liquid());
        theme.tick(back + Duration::from_millis(200));
        let left = ease * (1. - bezier(SWITCH_EASE)(200. / 420.));
        assert!((theme.liquid_share() - left).abs() < 1e-6);
        assert!(!theme.tick(back + SWITCH));
        assert_eq!(theme.liquid_share(), 0.);
        // Без анимаций — сразу.
        theme.set(LIQUID_GLASS, None, false, back);
        assert_eq!(theme.liquid_share(), 1.);
    }

    #[test]
    fn layout_steps_follow_viewport() {
        let mut theme = Theme::new(fixture());
        assert_eq!(theme.len("l-rail-w"), 96.);
        theme.set_viewport((1600., 700.));
        assert_eq!(theme.len("l-rail-w"), 80.);
        theme.set_viewport((1600., 860.));
        assert_eq!(theme.rem("l-rail-w"), u(96.));
        assert_eq!(theme.duration("l-dur"), Duration::from_millis(260));
        assert_eq!(theme.number("l-bg-zoom"), 1.);
        assert_eq!(theme.easing("l-ease")(1.), 1.);
    }

    #[test]
    fn bezier_matches_css_points() {
        let linear = bezier([0., 0., 1., 1.]);
        assert!((linear(0.3) - 0.3).abs() < 1e-4);
        let ease = bezier(SWITCH_EASE);
        assert_eq!(ease(0.), 0.);
        assert_eq!(ease(1.), 1.);
        // cubic-bezier(.22,1,.36,1): x(s) = 0,5 при s ≈ 0,662, y = 1 − (1 − s)³ ≈ 0,9614.
        assert!((ease(0.5) - 0.9614).abs() < 0.001, "{}", ease(0.5));
    }

    /// Встроенный `themes.json`: все темы разбираются, жидкое стекло — по флагу, у каждой есть
    /// токены, которые читает основа вида.
    #[test]
    fn embedded_themes_load() {
        let table = Table::embedded();
        let mut order = THEMES_049.to_vec();
        if LIQUID_GLASS_ENABLED {
            order.insert(7, LIQUID_GLASS);
        }
        assert_eq!(table.ids(), order);
        assert_eq!(table.accents("paper"), ["clay", "rose", "amber", "sea"]);
        let now = Instant::now();
        for id in table.ids() {
            let mut theme = Theme::new(table.clone());
            theme.set(id, None, false, now);
            for key in [
                "l-bg",
                "l-text",
                "l-accent",
                "l-surface",
                "l-border",
                "l-shade-rgb",
            ] {
                assert!(theme.try_color(key).is_some(), "{id}: {key}");
            }
            assert!(theme.len("l-radius") > 0., "{id}");
        }
    }
}
