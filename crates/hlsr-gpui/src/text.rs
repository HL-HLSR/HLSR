//! Текст: шрифты 0.4.9 (Rubik 300/400/500/700, Font Awesome 7, HLSR Symbols, Cascadia Mono),
//! размеры в CSS px макета, строка с разрядкой и тенью [`Spaced`].
//!
//! Что в GPUI есть само: межстрочный — `.line_height(u(px))` (или [`TextExt::text_px`]),
//! многоточие — `.truncate()` / `.text_ellipsis()`, `line-clamp` — [`TextExt::clamp`],
//! `tabular-nums` — [`TextExt::tnum`], `letter-spacing` — `.letter_spacing(u(px))` (патч
//! `vendor/gpui-pre`, наследуется; у корня — [`BODY_SPACING`], у кнопок —
//! [`TextExt::button_text`]). Нет `text-shadow`: его делает [`Spaced`] (тень жёсткая — копия
//! строки со сдвигом, без размытия). Вес 600 у Rubik нет: Chromium берёт 700, здесь так же.
//! `text-wrap: pretty` и `balance` — абзац [`Wrapped`].

use crate::scale::u;
use gpui::{
    App, AvailableSpace, Bounds, Element, ElementId, FontFeatures, FontWeight, GlobalElementId,
    Hsla, InspectorElementId, LayoutId, LineLayout, Pixels, SharedString, Size, Style, Styled,
    TextRun, Window, WrappedLine, font, point, px,
};
use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

pub const RUBIK: &str = "Rubik";
/// Font Awesome solid (900) и regular (400).
pub const FA: &str = "Font Awesome 7 Free";
pub const FA_BRANDS: &str = "Font Awesome 7 Brands";
pub const SYMBOLS: &str = "HLSR Symbols";
pub const MONO: &str = "Cascadia Mono";

/// `body { letter-spacing: .01em }` 0.4.9 при кегле 14 px: потомки наследуют вычисленные 0,14 CSS px
/// при любом своём кегле. Задаётся у корня окна лаунчера (у установщика и обновления base.css нет).
pub const BODY_SPACING: f32 = 0.14;

macro_rules! font_file {
    ($name:literal) => {
        include_bytes!(concat!("../../hlsr-ui/fonts/", $name))
    };
}

/// Файлы Font Awesome: solid, regular, brands (контуры значков — `icons::fa_turned`).
pub(crate) const FA_FILES: [&[u8]; 3] = [
    font_file!("fa-solid-900.ttf"),
    font_file!("fa-regular-400.ttf"),
    font_file!("fa-brands-400.ttf"),
];

/// Шрифты вида — те же файлы, что у Electron и Slint (`crates/hlsr-ui/fonts`).
pub fn load_fonts(cx: &mut App) {
    let fonts: [&'static [u8]; 9] = [
        font_file!("Rubik-Light.ttf"),
        font_file!("Rubik-Regular.ttf"),
        font_file!("Rubik-Medium.ttf"),
        font_file!("Rubik-Bold.ttf"),
        FA_FILES[0],
        FA_FILES[1],
        FA_FILES[2],
        font_file!("HLSRSymbols-Regular.ttf"),
        font_file!("CascadiaMono-Regular.ttf"),
    ];
    cx.text_system()
        .add_fonts(fonts.into_iter().map(Cow::Borrowed).collect())
        .expect("шрифты не загрузились");
}

/// `word-break: break-all`: строка переносится на любой графеме и заполняется до края. Перенос
/// GPUI идёт по UAX #14 (путь рвётся после «/»), поэтому между графемами — U+200B: место переноса
/// нулевой ширины.
/// shortcut: U+200B остаётся в строке — годится только для показа (строка-потомок GPUI не доходит
/// до AccessKit и не выделяется); для `aria_label`, копирования и сравнений брать исходное имя.
pub fn break_all(text: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    text.graphemes(true).collect::<Vec<_>>().join("\u{200b}")
}

/// Перенос текста с путями как у Chromium (`white-space: normal`): Blink рвёт ASCII только на
/// пробелах и после «-» и «?», а GPUI — по UAX #14, ещё и после «:», «/», «,»… («D:» | «\Games»).
/// После прочих знаков ASCII перед не-пробелом — U+2060 (WORD JOINER): там переноса нет. Слово
/// шире строки GPUI всё равно рвёт по графемам, как `overflow-wrap: anywhere`. U+2060 в Rubik
/// нет, но DirectWrite его не рисует: ни глифа, ни ширины (проверено `layout_line`, как и U+200B
/// у [`break_all`]).
/// shortcut: U+2060 остаётся в строке — только для показа, не для копирования; разрыв Blink перед
/// «(», «[», «{», «<» после части знаков не повторён — в подсказках и ошибках установщика скобка
/// идёт после пробела.
pub fn keep_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        let joined = c.is_ascii_punctuation() && !matches!(c, '-' | '?');
        if joined && chars.peek().is_some_and(|next| !next.is_whitespace()) {
            out.push('\u{2060}');
        }
    }
    out
}

/// `font-variant-numeric: tabular-nums`.
pub fn tnum() -> FontFeatures {
    static TNUM: LazyLock<FontFeatures> =
        LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])));
    TNUM.clone()
}

pub trait TextExt: Styled + Sized {
    /// `font-size` и `line-height` в CSS px макета.
    fn text_px(self, size: f32, line_height: f32) -> Self {
        self.text_size(u(size)).line_height(u(line_height))
    }

    /// `font-variant-numeric: tabular-nums`. В Rubik 0.4.9 таблицы `tnum` нет — цифры те же,
    /// как и в Chromium; работает у шрифтов, где она есть.
    fn tnum(self) -> Self {
        self.font_features(tnum())
    }

    /// `-webkit-line-clamp: n`: не больше `lines` строк, в конце последней — многоточие.
    fn clamp(self, lines: usize) -> Self {
        self.line_clamp(lines).text_ellipsis()
    }

    /// Текст `<button>`, `<input>`, `<select>`, `<textarea>` 0.4.9: UA-стиль Chromium ставит им
    /// `letter-spacing: normal`, разрядка `body` ([`BODY_SPACING`]) внутрь не наследуется.
    fn button_text(self) -> Self {
        self.letter_spacing(px(0.))
    }
}

impl<T: Styled> TextExt for T {}

/// Однострочная подпись со своей разрядкой (CSS `letter-spacing`, не наследуется) и жёсткой
/// тенью: строку с разрядкой раскладывает система текста, глифы рисуются по одному, эмодзи —
/// `paint_emoji`. Размеры — CSS px макета, на экран идут умноженными на зум (`rem / 16`).
pub struct Spaced {
    text: SharedString,
    size: f32,
    line_height: f32,
    weight: FontWeight,
    spacing: f32,
    color: Hsla,
    // text-shadow без размытия: та же строка со сдвигом (dx, dy) своим цветом.
    shadow: Option<(f32, f32, Hsla)>,
}

/// Строка Rubik: кегль и высота строки в CSS px, разрядка в em (как `letter-spacing: .14em`).
pub fn spaced(
    text: impl Into<SharedString>,
    size: f32,
    line_height: f32,
    weight: FontWeight,
    spacing_em: f32,
    color: impl Into<Hsla>,
) -> Spaced {
    Spaced {
        text: text.into(),
        size,
        line_height,
        weight,
        spacing: spacing_em * size,
        color: color.into(),
        shadow: None,
    }
}

impl Spaced {
    /// `text-shadow: dx dy <blur> color` без размытия.
    pub fn shadow(mut self, dx: f32, dy: f32, color: impl Into<Hsla>) -> Self {
        self.shadow = Some((dx, dy, color.into()));
        self
    }
}

impl Element for Spaced {
    type RequestLayoutState = (Arc<LineLayout>, f32);
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let zoom = crate::scale::zoom(window);
        let run = TextRun {
            len: self.text.len(),
            font: gpui::Font {
                weight: self.weight,
                ..font(RUBIK)
            },
            color: self.color,
            letter_spacing: px(self.spacing * zoom),
            ..Default::default()
        };
        let layout =
            window
                .text_system()
                .layout_line(self.text.clone(), px(self.size * zoom), &[run], None);
        let mut style = Style::default();
        style.size.width = layout.width.into();
        style.size.height = px(self.line_height * zoom).into();
        style.flex_shrink = 0.;
        (window.request_layout(style, [], cx), (layout, zoom))
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (layout, zoom): &mut Self::RequestLayoutState,
        _prepaint: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) {
        let zoom = *zoom;
        let size = px(self.size * zoom);
        // Половинный интерлиньяж CSS: глифы по центру строки высотой line-height.
        let baseline = (bounds.size.height - layout.ascent - layout.descent) / 2. + layout.ascent;
        let shadow = self.shadow.map(|(dx, dy, color)| (dx, dy, color, false));
        let passes = shadow.into_iter().chain([(0., 0., self.color, true)]);
        // Ошибка глифа (атлас, растр) — в журнал одна на строку, остальные глифы рисуются.
        let mut failed = None;
        for (dx, dy, color, main) in passes {
            for run in &layout.runs {
                for glyph in &run.glyphs {
                    let at = bounds.origin
                        + point(glyph.position.x + px(dx * zoom), baseline + px(dy * zoom));
                    let painted = if glyph.is_emoji {
                        // shortcut: у эмодзи нет тени (в CSS — силуэт цветом тени); нужна — маска глифа.
                        if !main {
                            continue;
                        }
                        window.paint_emoji(at, run.font_id, glyph.id, size)
                    } else {
                        window.paint_glyph(at, run.font_id, glyph.id, size, color)
                    };
                    if let Err(error) = painted {
                        failed.get_or_insert(error);
                    }
                }
            }
        }
        if let Some(error) = failed {
            tracing::warn!("[text] глиф строки «{}» не нарисован: {error:#}", self.text);
        }
    }
}

/// Абзац с `text-wrap: pretty` или `balance` (Chromium): сначала жадный перенос GPUI, потом
/// переносы последних строк выбирает [`optimal_breaks`] — как Blink `ScoreLineBreaker`. Шрифт,
/// кегль, строка, цвет и выравнивание — от родителя, как у строки-потомка GPUI.
pub struct Wrapped {
    text: SharedString,
    balance: bool,
    /// Замер → `paint` одного кадра (замер taffy зовёт не раз). Между кадрами форму строк
    /// держит кэш строк GPUI (`LineLayoutCache`, прошлый кадр), заново — только переносы.
    layout: Rc<RefCell<Option<WrappedLayout>>>,
}

struct WrappedLayout {
    lines: Vec<WrappedLine>,
    line_height: Pixels,
    wrap_width: Option<Pixels>,
    size: Size<Pixels>,
}

/// `text-wrap: pretty`: последняя строка не из одного короткого слова.
pub fn pretty(text: impl Into<SharedString>) -> Wrapped {
    Wrapped {
        text: text.into(),
        balance: false,
        layout: Rc::default(),
    }
}

/// `text-wrap: balance`: строки абзаца (до 6) близкой длины.
pub fn balance(text: impl Into<SharedString>) -> Wrapped {
    Wrapped {
        balance: true,
        ..pretty(text)
    }
}

impl Element for Wrapped {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(
            style
                .line_height
                .to_pixels(font_size.into(), window.rem_size()),
        );
        let runs = vec![style.to_run(self.text.len(), window.rem_size())];
        let zoom = crate::scale::zoom(window);
        let (text, balance, state) = (self.text.clone(), self.balance, self.layout.clone());
        let id =
            window.request_measured_layout(Style::default(), move |known, available, window, _| {
                let wrap_width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                if let Some(layout) = state.borrow().as_ref()
                    && (wrap_width.is_none() || wrap_width == layout.wrap_width)
                {
                    return layout.size;
                }
                let lines = wrap_lines(&text, font_size, &runs, wrap_width, balance, zoom, window);
                let mut size = Size::<Pixels>::default();
                for line in &lines {
                    let line = line.size(line_height);
                    size.height += line.height;
                    size.width = size.width.max(line.width).ceil();
                }
                state.replace(Some(WrappedLayout {
                    lines,
                    line_height,
                    wrap_width,
                    size,
                }));
                size
            });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _layout: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let state = self.layout.borrow();
        let Some(layout) = state.as_ref() else {
            return;
        };
        let align = window.text_style().text_align;
        let mut origin = bounds.origin;
        for line in &layout.lines {
            if let Err(error) =
                line.paint(origin, layout.line_height, align, Some(bounds), window, cx)
            {
                tracing::warn!("[text] строка «{}» не нарисована: {error:#}", line.text);
            }
            origin.y += line.size(layout.line_height).height;
        }
    }
}

/// Строки абзаца: жадный перенос GPUI, затем [`optimal_breaks`] в каждой строке-абзаце. Новые
/// переносы — `\n` вместо пробела перед словом, и тогда переносы задаёт сам текст (все, и
/// жадные тоже). Перенос GPUI не на пробеле (слово шире строки) — жадный как есть.
fn wrap_lines(
    text: &SharedString,
    font_size: Pixels,
    runs: &[TextRun],
    wrap_width: Option<Pixels>,
    balance: bool,
    zoom: f32,
    window: &mut Window,
) -> Vec<WrappedLine> {
    let system = window.text_system().clone();
    let Ok(lines) = system.shape_text(text.clone(), font_size, runs, wrap_width, None) else {
        return Vec::new();
    };
    let lines: Vec<WrappedLine> = lines.into_iter().collect();
    let Some(width) = wrap_width else {
        return lines;
    };
    let mut bytes = text.as_bytes().to_vec();
    let (mut offset, mut changed) = (0, false);
    for line in &lines {
        let layout = &line.unwrapped_layout;
        let own = line.text.as_bytes();
        let greedy: Vec<usize> = line
            .wrap_boundaries
            .iter()
            .map(|b| layout.runs[b.run_ix].glyphs[b.glyph_ix].index)
            .collect();
        if greedy.iter().any(|&at| at == 0 || own[at - 1] != b' ') {
            return lines;
        }
        let x = |index: usize| f32::from(layout.x_for_index(index)) / zoom;
        let mut breaks = Vec::new();
        let mut i = 0;
        while i < own.len() {
            if own[i] != b' ' {
                i += 1;
                continue;
            }
            let start = i;
            while i < own.len() && own[i] == b' ' {
                i += 1;
            }
            if start > 0 && i < own.len() {
                breaks.push(Break {
                    at: i,
                    end: x(start),
                    start: x(i),
                });
            }
        }
        let end = x(line.text.trim_end_matches(' ').len());
        let chosen = optimal_breaks(
            &breaks,
            end,
            &greedy,
            f32::from(width) / zoom,
            f32::from(font_size) / zoom,
            balance,
        );
        changed |= chosen.is_some();
        for at in chosen.unwrap_or(greedy) {
            bytes[offset + at - 1] = b'\n';
        }
        // `shape_text` режет только по одному `\n`: строка GPUI — весь отрезок, без потерь байт.
        offset += line.len() + 1;
    }
    if !changed {
        return lines;
    }
    // Пробел и `\n` — по байту: длины прогонов те же.
    let Ok(text) = String::from_utf8(bytes) else {
        return lines;
    };
    match system.shape_text(text.into(), font_size, runs, None, None) {
        Ok(wrapped) => wrapped.into_iter().collect(),
        Err(_) => lines,
    }
}

/// Место переноса на пробеле: `at` — начало следующей строки (байт), `end` — x конца строки
/// до пробела, `start` — x начала следующей; CSS px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Break {
    pub at: usize,
    pub end: f32,
    pub start: f32,
}

/// Строк в работе у `pretty` (последние) и у `balance` (весь абзац), как `kMaxLinesForOptimal`
/// и `kMaxLinesForBalance` Blink.
const MAX_LINES_OPTIMAL: usize = 4;
const MAX_LINES_BALANCE: usize = 6;
/// Штраф переноса перед последним словом (`kOrphansPenalty`).
const ORPHANS_PENALTY: f32 = 10_000.;
/// Строка шире места (`kScoreOverfull`).
const SCORE_OVERFULL: f32 = 1e12;

/// Blink `ScoreLineBreaker` (score_line_breaker.cc) без переносов по слогам и без
/// выключки: начала строк вместо жадных `greedy`, или `None` — оставить жадные.
/// `pretty` работает, только если последняя строка — одно слово короче трети ширины, и
/// перебирает последние 4 строки; `balance` — весь абзац до 6 строк, и у последней строки
/// тоже считается недобор. Счёт строки — недобор² (у последней в `pretty` — 0), плюс штраф
/// строки `4 · ширина · кегль` и штраф переноса перед последним словом; число строк не
/// меняется (иначе — жадные).
pub(crate) fn optimal_breaks(
    breaks: &[Break],
    end: f32,
    greedy: &[usize],
    width: f32,
    font_size: f32,
    balance: bool,
) -> Option<Vec<usize>> {
    let lines = greedy.len() + 1;
    if lines <= 1 {
        return None;
    }
    let greedy_ix: Vec<usize> = greedy
        .iter()
        .map(|at| breaks.iter().position(|b| b.at == *at))
        .collect::<Option<_>>()?;
    let window = if balance {
        if lines > MAX_LINES_BALANCE {
            return None;
        }
        lines
    } else {
        // `ShouldOptimize`: последняя строка короче трети и без мест переноса внутри.
        let last = *greedy_ix.last()?;
        if end - breaks[last].start >= width / 3. || last + 1 < breaks.len() {
            return None;
        }
        lines.min(MAX_LINES_OPTIMAL)
    };
    let first = lines - window;
    // Кандидаты: начало окна, места переноса в нём, конец абзаца.
    let (start, from) = match first {
        0 => (0., 0),
        n => {
            let ix = greedy_ix[n - 1];
            (breaks[ix].start, ix + 1)
        }
    };
    let mut cands = vec![Break {
        at: 0,
        end: 0.,
        start,
    }];
    cands.extend_from_slice(&breaks[from..]);
    cands.push(Break {
        at: usize::MAX,
        end,
        start: end,
    });
    // `kMinCandidates`: меньше трёх мест переноса — нечего выбирать.
    if cands.len() < 3 + 2 {
        return None;
    }
    let mut penalty = vec![0f32; cands.len()];
    let orphan = cands.len() - 2;
    penalty[orphan] = ORPHANS_PENALTY;
    let line_penalty = 4. * width * font_size;
    let n = cands.len();
    let (mut score, mut prev, mut count) = (vec![0f32; n], vec![0usize; n], vec![0usize; n]);
    for i in 1..n {
        let is_end = i == n - 1;
        let (mut best, mut best_j) = (f32::INFINITY, 0);
        for j in 0..i {
            let delta = width - (cands[i].end - cands[j].start);
            let width_score = if delta < 0. {
                SCORE_OVERFULL
            } else if is_end && !balance {
                0.
            } else {
                delta * delta
            };
            let total = score[j] + width_score;
            if total < best {
                (best, best_j) = (total, j);
            }
        }
        score[i] = best + penalty[i] + line_penalty;
        prev[i] = best_j;
        count[i] = count[best_j] + 1;
    }
    if count[n - 1] != window {
        return None;
    }
    let mut chosen = Vec::with_capacity(window - 1);
    let mut i = prev[n - 1];
    while i > 0 {
        chosen.push(cands[i].at);
        i = prev[i];
    }
    chosen.reverse();
    let mut out = greedy[..first].to_vec();
    out.extend(chosen);
    (out != greedy).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::{RUBIK, keep_words};
    use gpui::{TestAppContext, TextRun, font, px};
    use std::path::{Path, PathBuf};

    fn run(len: usize, spacing: f32) -> TextRun {
        TextRun {
            len,
            font: font(RUBIK),
            letter_spacing: px(spacing),
            ..Default::default()
        }
    }

    /// Разрядка (патч `vendor/gpui-pre`): шаг после каждого знака и после последнего; пробел в
    /// конце строки висит, как у Chromium. Система текста тестов — моноширинная (`NoopTextSystem`).
    #[gpui::test]
    fn letter_spacing_and_hanging_space(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|window, _| {
            let text = window.text_system();
            let plain = text.layout_line("abc", px(10.), &[run(3, 0.)], None);
            let spaced = text.layout_line("abc", px(10.), &[run(3, 2.)], None);
            let em = plain.runs[0].glyphs[1].position.x;
            let xs: Vec<_> = spaced.runs[0].glyphs.iter().map(|g| g.position.x).collect();
            assert_eq!(xs, [px(0.), em + px(2.), em + em + px(4.)]);
            assert_eq!(spaced.width, plain.width + px(6.));

            // «aa bb» ровно во всю ширину: пробел за ним не уносит «bb» на следующую строку.
            let lines = text
                .shape_text(
                    "aa bb cc".into(),
                    px(10.),
                    &[run(8, 0.)],
                    Some(em * 5.),
                    None,
                )
                .unwrap();
            let wrap = &lines[0].wrap_boundaries[0];
            let glyph = &lines[0].unwrapped_layout.runs[wrap.run_ix].glyphs[wrap.glyph_ix];
            assert_eq!(glyph.index, 6);

            // С разрядкой «aa bb» уже не входит в те же 5 em.
            let lines = text
                .shape_text(
                    "aa bb cc".into(),
                    px(10.),
                    &[run(8, 1.)],
                    Some(em * 5.),
                    None,
                )
                .unwrap();
            let wrap = &lines[0].wrap_boundaries[0];
            let glyph = &lines[0].unwrapped_layout.runs[wrap.run_ix].glyphs[wrap.glyph_ix];
            assert_eq!(glyph.index, 3);
        });
    }

    use super::{Break, optimal_breaks};

    /// Слова шириной `words` через пробел 5 px: места переноса (начало слова = его номер × 10)
    /// и конец абзаца.
    fn words(words: &[f32]) -> (Vec<Break>, f32) {
        let mut x = 0.;
        let mut breaks = Vec::new();
        for (i, w) in words.iter().enumerate() {
            if i > 0 {
                breaks.push(Break {
                    at: i * 10,
                    end: x,
                    start: x + 5.,
                });
                x += 5.;
            }
            x += w;
        }
        (breaks, x)
    }

    /// `pretty`: одно короткое слово внизу — переносится ещё одно (Blink: недобор 70² меньше
    /// штрафа 10 000), а не всё поровну, как у `balance`.
    #[test]
    fn pretty_takes_one_word_down_and_balance_evens() {
        let (breaks, end) = words(&[50., 50., 50., 50., 30.]);
        // Жадно: 4 слова (215 из 230) и «30» одно.
        assert_eq!(
            optimal_breaks(&breaks, end, &[40], 230., 12., false),
            Some(vec![30])
        );
        // `balance`: 105 и 140 вместо 160 и 85.
        assert_eq!(
            optimal_breaks(&breaks, end, &[40], 230., 12., true),
            Some(vec![20])
        );
        // Слово, которое пришлось бы унести, широкое: недобор 120² дороже штрафа — как было.
        let (breaks, end) = words(&[50., 50., 50., 115., 30.]);
        assert_eq!(optimal_breaks(&breaks, end, &[40], 290., 12., false), None);
    }

    /// `pretty` не трогает абзац, если внизу два слова или строка не короче трети.
    #[test]
    fn pretty_keeps_greedy_without_short_orphan() {
        let (breaks, end) = words(&[50., 50., 50., 50., 20., 20.]);
        assert_eq!(optimal_breaks(&breaks, end, &[40], 230., 12., false), None);
        let (breaks, end) = words(&[50., 50., 50., 50., 90.]);
        assert_eq!(optimal_breaks(&breaks, end, &[40], 230., 12., false), None);
        // Одна строка — переносить нечего.
        assert_eq!(optimal_breaks(&breaks, end, &[], 500., 12., false), None);
    }

    /// С переносом GPUI (моноширинная раскладка тестовой системы текста): жадные переносы
    /// узнаются по началу слова, новые — `\n` вместо пробела.
    #[gpui::test]
    fn pretty_rewraps_gpui_lines(cx: &mut TestAppContext) {
        use super::wrap_lines;
        let cx = cx.add_empty_window();
        cx.update(|window, _| {
            let text = "aaaa bbbb cccc dddd ee";
            let run = run(text.len(), 0.);
            let size = px(16.);
            let char_w = window
                .text_system()
                .layout_line("a", size, std::slice::from_ref(&run), None)
                .width;
            let mut lines = |balance| {
                wrap_lines(
                    &text.into(),
                    size,
                    std::slice::from_ref(&run),
                    Some(char_w * 20.5),
                    balance,
                    1.,
                    window,
                )
                .iter()
                .map(|line| line.text.to_string())
                .collect::<Vec<_>>()
            };
            assert_eq!(lines(false), ["aaaa bbbb cccc", "dddd ee"]);
            assert_eq!(lines(true), ["aaaa bbbb", "cccc dddd ee"]);
        });
    }

    /// `balance` выравнивает и длинную последнюю строку; больше 6 строк — жадно.
    #[test]
    fn balance_evens_lines() {
        let (breaks, end) = words(&[40.; 6]);
        // Жадно 5 слов (220 из 230) и одно; поровну — по три.
        assert_eq!(
            optimal_breaks(&breaks, end, &[50], 230., 12., true),
            Some(vec![30])
        );
        let (breaks, end) = words(&[40.; 14]);
        let greedy = [20, 40, 60, 80, 100, 120];
        assert_eq!(optimal_breaks(&breaks, end, &greedy, 85., 12., true), None);
    }

    /// Путь не рвётся после «:» и «\», пробел и «-» остаются местами переноса.
    #[test]
    fn keep_words_joins_path_punctuation() {
        assert_eq!(
            keep_words(r"C:\HLSR, D:\Games-1"),
            "C:\u{2060}\\\u{2060}HLSR, D:\u{2060}\\\u{2060}Games-1"
        );
        assert_eq!(keep_words("Вход • 15,2 ГБ"), "Вход • 15,\u{2060}2 ГБ");
    }

    /// Роли элементов, которые в 0.4.9 — `<button>`/`<select>`: им нужен `button_text()`.
    const BUTTON_ROLES: &[&str] = &[
        "Button",
        "Tab",
        "MenuItem",
        "RadioButton",
        "ListBoxOption",
        "Link",
    ];
    /// Свои окна установщика и обновления — без разрядки `body`.
    const OWN_WINDOWS: &[&str] = &["installer/", "updater/"];
    /// Не `<button>` в 0.4.9 — разрядку `body` наследуют: `<span role="link">` бегуна.
    const SPACED: &[(&str, &str)] = &[("home/records.rs", "Link")];

    /// Новая кнопка без `button_text()` молча берёт разрядку `body` 0,14 px: каждая цепочка с
    /// ролью кнопки сбрасывает её (или стоит в исключениях выше).
    #[test]
    fn button_roles_reset_body_spacing() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rs_files(&root, &mut files);
        let mut found = Vec::new();
        for path in files {
            let name = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if OWN_WINDOWS.iter().any(|dir| name.starts_with(dir)) {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for (at, line) in lines.iter().enumerate() {
                let Some(role) = line
                    .trim_start()
                    .strip_prefix(".role(Role::")
                    .and_then(|rest| rest.strip_suffix(')'))
                else {
                    continue;
                };
                if BUTTON_ROLES.contains(&role)
                    && !SPACED.contains(&(name.as_str(), role))
                    && !in_chain(&lines, at, ".button_text()")
                {
                    found.push(format!("{name}:{}: {}", at + 1, line.trim()));
                }
            }
        }
        assert!(
            found.is_empty(),
            "кнопке нужен .button_text() (letter-spacing: normal у <button>):\n{}",
            found.join("\n")
        );
    }

    fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rs_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// Есть ли `call` в той же цепочке методов, что и строка `at`: после rustfmt методы
    /// цепочки — строки с тем же отступом, начинающиеся с «.».
    fn in_chain(lines: &[&str], at: usize, call: &str) -> bool {
        let depth = indent(lines[at]);
        let side = |lines: &mut dyn Iterator<Item = &&str>| {
            for line in lines {
                let text = line.trim_start();
                if text.is_empty() || text.starts_with("//") {
                    continue;
                }
                let k = indent(line);
                if k < depth || (k == depth && !text.starts_with('.')) {
                    return false;
                }
                if k == depth && text.starts_with(call) {
                    return true;
                }
            }
            false
        };
        side(&mut lines[..at].iter().rev()) || side(&mut lines[at + 1..].iter())
    }

    fn indent(line: &str) -> usize {
        line.len() - line.trim_start().len()
    }
}
