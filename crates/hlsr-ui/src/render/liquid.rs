//! Стекло темы Liquid Glass: кусок подложки блока — размытие, линза кромки, цвет — и поверх него
//! заливка, подкраска `--lg-tint`, блик `--lg-sheen` и блик кромки.
//!
//! Линза — эталон 21st.dev «Liquid Glass» (manuarora700/liquid-glass, aakashmehra/liquid-glass-button):
//! карта смещения скруглённого прямоугольника под `feDisplacementMap`. В 0.4.9 вместо неё был
//! шум `feTurbulence` со сдвигом 70–120 px (`#lg-refract`): он не знал формы блока и давал мутную
//! рябь по всему стеклу, а не линзу.
//!
//! - Форма — расстояние со знаком до скруглённого прямоугольника блока (`sdRoundedBox`).
//! - Кромка шириной `bevel` — купол-«сквиркл» `y = (1 − (1 − x)⁴)^¼`: луч под углом его склона
//!   преломляется по закону Снеллиуса (n = 1,5), изгиб `sin(θᵢ − θₜ)`. Высота поля — интеграл
//!   изгиба по глубине, смещение — его градиент: изгиб на глубине вдоль нормали внутрь, ×1,35 и
//!   до ±1 по осям (кромка сжимается, но не заворачивается). Глубже кромки смещения нет.
//! - `feDisplacementMap scale=… xChannelSelector="R" yChannelSelector="G"`: сдвиг выборки
//!   `scale · (c − ½)`, `c = (128 + 127·d) / 255`. Красный канал берётся со `scale + aberration`,
//!   зелёный — со `scale`, синий — со `scale − aberration` (хроматическая кайма у кромки).
//! - Блик кромки — канал B эталонной карты: нормаль к свету слева сверху `(−0.55, −0.85)`, у края
//!   полностью, к глубине `bevel` — ноль; с обратной стороны — 35 %.
//! - Смещение считается аналитически (градиент поля = изгиб × нормаль), а не разностями на сетке:
//!   оно не зависит от шага сетки куска.
//!
//! Размытие — подложки только под коробкой блока, за её краем зеркало (как у колонки, замер GL);
//! линза гнёт уже размытое (`blur()` раньше `url()` в цепочке `backdrop-filter`), а к краю
//! кромки выборка переходит на резкую подложку ([`SHARP_RIM`]). Выборка за
//! коробкой блока (клетки поля куска) — подложка без фильтра. `saturate()` — матрица
//! `feColorMatrix type="saturate"`, `brightness()` — множитель; в sRGB.
//!
//! Кусок считается на сетке с шагом σ/4 CSS px, как кусок колонки ([`box_backdrop`]): размытое
//! преломлённое на ней не теряет, вид растягивает кусок на блок. Кромка — от 10 CSS px, это
//! 5–7 клеток. Кусок шире блока на две клетки с каждой стороны: видеокарта растягивает его
//! с линейной выборкой, а края картинки (к ним подмешиваются соседи по атласу) остаются за обрезкой
//! блока.

use super::rail_glass::{Placement, area, box_backdrop_at, over};
use crate::{Rgba, Rgba8Image};
use std::sync::{Arc, LazyLock, Mutex};

/// σ подложки в клетках сетки (как у колонки).
pub const SIGMA: f32 = 4.0;
/// Грубая сетка: σ — две клетки, клеток вчетверо меньше. Так считается первый кусок блока в кадре,
/// где блок появился, пока точный считается в фоне (`coarse_piece_stays_close_to_the_fine_one`).
pub const SIGMA_COARSE: f32 = 2.0;
/// Клеток куска за краем блока с каждой стороны.
const PAD: f32 = 2.0;

/// Линза кромки блока (эталон 21st.dev, см. модуль).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lens {
    /// Ширина кромки, которая гнёт свет, CSS px (не больше половины меньшей стороны блока).
    pub bevel: f32,
    /// Сила смещения `feDisplacementMap scale`, CSS px.
    pub scale: f32,
    /// Разнос каналов, CSS px: красный — `scale + aberration`, синий — `scale − aberration`.
    pub aberration: f32,
    /// Прозрачность белого блика у самого края со стороны света.
    pub specular: f32,
}

impl Lens {
    /// Без линзы: только размытие и цвет.
    pub const NONE: Lens = Lens {
        bevel: 0.0,
        scale: 0.0,
        aberration: 0.0,
        specular: 0.0,
    };

    fn is_none(&self) -> bool {
        self.bevel <= 0.0 || (self.scale <= 0.0 && self.specular <= 0.0)
    }
}

/// Окна: широкая кромка, мягкая кайма (эталон: `bevelDepth` 14…37, `scale` 48, `aberration` 4…14;
/// у окна поверх текст, поэтому кайма скромнее). Изгиб купола сходит на нет к середине кромки, так
/// что смещение заметно в её внешней трети: у края до `scale · 0,44` px.
pub const LG_LENS_WINDOW: Lens = Lens {
    bevel: 24.0,
    scale: 56.0,
    aberration: 5.0,
    specular: 0.16,
};

/// Всплывающие панели.
pub const LG_LENS_PANEL: Lens = Lens {
    bevel: 16.0,
    scale: 48.0,
    aberration: 4.0,
    specular: 0.14,
};

/// «Мягкие» кнопки, тосты, поле поиска: блоки 40–70 px.
pub const LG_LENS_SOFT: Lens = Lens {
    bevel: 12.0,
    scale: 40.0,
    aberration: 3.0,
    specular: 0.12,
};

/// Цепочка `backdrop-filter` одной группы правил Liquid Glass: `blur() <линза> saturate()
/// brightness()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiquidFilter {
    pub blur: f32,
    pub lens: Lens,
    pub saturate: f32,
    pub brightness: f32,
}

/// `--lg-filter`: `blur(14px) <линза окна> saturate(170%) brightness(1.06)`.
pub const LG_WINDOW: LiquidFilter = LiquidFilter {
    blur: 14.0,
    lens: LG_LENS_WINDOW,
    saturate: 1.7,
    brightness: 1.06,
};

/// `--lg-filter-panel`: `blur(8px) <линза панели> saturate(160%)`.
pub const LG_PANEL: LiquidFilter = LiquidFilter {
    blur: 8.0,
    lens: LG_LENS_PANEL,
    saturate: 1.6,
    brightness: 1.0,
};

/// `--lg-filter-soft`: `blur(6px) <линза кнопки> saturate(140%)`.
pub const LG_SOFT: LiquidFilter = LiquidFilter {
    blur: 6.0,
    lens: LG_LENS_SOFT,
    saturate: 1.4,
    brightness: 1.0,
};

/// `backdrop-filter: blur(8px)` без преломления и цвета: подложка просмотра картинок новости
/// (`.news-viewer` NewsImageViewer.vue 0.4.9) — во всех темах, не только Liquid Glass.
pub const BLUR_8: LiquidFilter = LiquidFilter {
    blur: 8.0,
    lens: Lens::NONE,
    saturate: 1.0,
    brightness: 1.0,
};

/// `--lg-tint`: `rgba(255, 255, 255, .07)`.
pub const LG_TINT: Rgba = Rgba::new(255, 255, 255, 18);

/// `--lg-sheen`: `linear-gradient(150deg, rgba(255,255,255,.14) 0%, rgba(255,255,255,.04) 30%,
/// rgba(255,255,255,0) 55%, rgba(255,255,255,.06) 100%)` — (доля, прозрачность белого).
pub const LG_SHEEN: [(f32, f32); 4] = [(0.0, 0.14), (0.3, 0.04), (0.55, 0.0), (1.0, 0.06)];

/// Что рисует блок поверх преломлённой подложки (прямая альфа): `background-color`, затем
/// `background-image: linear-gradient(--lg-tint), --lg-sheen` (у «мягкого» стекла — только своя
/// заливка).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paint {
    pub fill: Rgba,
    pub tint: Rgba,
    pub sheen: bool,
}

/// Готовый кусок: картинка (прямая альфа) и где она лежит в окне (CSS px; шире блока на
/// [`PAD`] клетки с каждой стороны).
#[derive(Debug, Clone)]
pub struct Piece {
    pub image: Rgba8Image,
    pub place: Placement,
}

/// Кусок стекла блока `rect` (CSS px окна, углы скруглены на `radius` CSS px) над кадром `source`,
/// который лежит в окне в `frame` (cover и параллакс — у хозяина фона). `top(x, y)` — что лежит
/// над кадром в точке окна (CSS px, прямая альфа): затемнение, передний план, подложка
/// окна-диалога. `None` — блок пустой. Сетка — `cells` клеток на σ размытия ([`SIGMA`] или
/// [`SIGMA_COARSE`]).
#[allow(clippy::too_many_arguments)]
pub fn piece_at(
    source: &Rgba8Image,
    frame: Placement,
    rect: Placement,
    radius: f32,
    filter: &LiquidFilter,
    cells: f32,
    top: impl Fn(f32, f32) -> Rgba,
    paint: &Paint,
) -> Option<Piece> {
    if rect.width < 1.0 || rect.height < 1.0 {
        return None;
    }
    let step = (filter.blur / cells).max(1.0);
    let pad = PAD * step;
    // Размытая подложка коробки блока (за краем — зеркало).
    let under = box_backdrop_at(
        source,
        Placement {
            x: frame.x - rect.x,
            y: frame.y - rect.y,
            ..frame
        },
        rect.width,
        rect.height,
        filter.blur,
        cells,
        |x, y| top(x + rect.x, y + rect.y),
    );
    let (uw, uh) = (under.width() as usize, under.height() as usize);
    let (ux, uy) = (rect.width / uw as f32, rect.height / uh as f32);
    // Подложка клетки без фильтра: там, куда выборка ушла за коробку (клетки поля куска).
    let plain = |x: f32, y: f32| {
        let half = step / 2.0;
        let under = area(source, frame, (x - half, y - half), (x + half, y + half));
        let p = over(top(x, y), under);
        [p.r, p.g, p.b].map(|c| f32::from(c) / 255.0)
    };
    let out_w = ((rect.width + 2.0 * pad) / step).ceil().max(1.0) as usize;
    let out_h = ((rect.height + 2.0 * pad) / step).ceil().max(1.0) as usize;
    let place = Placement {
        x: rect.x - pad,
        y: rect.y - pad,
        width: out_w as f32 * step,
        height: out_h as f32 * step,
    };
    let lens = filter.lens;
    let field = field(
        Grid {
            out_w,
            out_h,
            step,
            pad,
        },
        rect,
        radius,
        lens,
    );
    // Сдвиг каналов: красный, зелёный, синий.
    let scales = [
        lens.scale + lens.aberration,
        lens.scale,
        (lens.scale - lens.aberration).max(0.0),
    ];
    let matrix = saturate_matrix(filter.saturate);
    // Ось блика: 150deg — вниз и вправо; длина градиента по CSS.
    let (sin, cos) = 150f32.to_radians().sin_cos();
    let line = (rect.width * sin).abs() + (rect.height * cos).abs();
    let center = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
    let inside = |x: f32, y: f32| {
        (rect.x..rect.x + rect.width).contains(&x) && (rect.y..rect.y + rect.height).contains(&y)
    };
    let under_at =
        |x: f32, y: f32| bilinear(&under, (x - rect.x) / ux - 0.5, (y - rect.y) / uy - 0.5);
    let mut image = Rgba8Image::new(out_w as u32, out_h as u32);
    for (index, out) in image.make_mut_slice().iter_mut().enumerate() {
        let (i, j) = ((index % out_w) as f32, (index / out_w) as f32);
        let (x, y) = (place.x + (i + 0.5) * step, place.y + (j + 0.5) * step);
        let cell = field.get(index).copied().unwrap_or_default();
        let mut color = if !inside(x, y) {
            plain(x, y)
        } else {
            let mut rgb = [0.0; 3];
            let sharp = cell.rim * cell.rim * SHARP_RIM;
            if cell.dx == 0.0 && cell.dy == 0.0 && sharp <= 0.0 {
                rgb = under_at(x, y);
            } else {
                for (k, scale) in scales.iter().enumerate() {
                    let (sx, sy) = (x + scale * cell.dx, y + scale * cell.dy);
                    let (blurred, crisp) = if inside(sx, sy) {
                        let crisp = if sharp > 0.0 { plain(sx, sy)[k] } else { 0.0 };
                        (under_at(sx, sy)[k], crisp)
                    } else {
                        let p = plain(x, y)[k];
                        (p, p)
                    };
                    rgb[k] = blurred + (crisp - blurred) * sharp;
                }
            }
            matrix.map(|row| {
                let v = (row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]).clamp(0.0, 1.0);
                (v * filter.brightness).clamp(0.0, 1.0)
            })
        };
        for layer in [paint.fill, paint.tint] {
            color = blend(color, layer);
        }
        if paint.sheen {
            let t = ((x - center.0) * sin - (y - center.1) * cos) / line + 0.5;
            color = blend(color, Rgba::new(255, 255, 255, alpha_byte(sheen_at(t))));
        }
        if cell.light > 0.0 {
            color = blend(
                color,
                Rgba::new(255, 255, 255, alpha_byte(cell.light * lens.specular)),
            );
        }
        let [r, g, b] = color.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8);
        *out = Rgba::rgb(r, g, b);
    }
    Some(Piece { image, place })
}

/// Сетка куска: `out_w × out_h` клеток с шагом `step`, поле `pad` за краем блока (CSS px).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Grid {
    out_w: usize,
    out_h: usize,
    step: f32,
    pad: f32,
}

/// Клетка карты линзы: сдвиг выборки на единицу `scale` (`c − ½` каналов R и G эталонной карты)
/// и блик кромки (канал B, 0…1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LensCell {
    pub dx: f32,
    pub dy: f32,
    pub light: f32,
    /// Близость к краю: 1 у края, 0 на внутренней границе кромки.
    pub rim: f32,
}

/// Расстояние со знаком до скруглённого прямоугольника с полуосями `half` и радиусом `r` (центр в
/// нуле, внутри — меньше нуля) и внешняя нормаль в точке `(x, y)`.
fn rounded_box(x: f32, y: f32, half: (f32, f32), r: f32) -> (f32, (f32, f32)) {
    let qx = x.abs() - half.0 + r;
    let qy = y.abs() - half.1 + r;
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let d = qx.max(qy).min(0.0) + outside - r;
    let (nx, ny) = if qx > 0.0 && qy > 0.0 {
        (qx / outside, qy / outside)
    } else if qx > qy {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };
    (d, (nx.copysign(x), ny.copysign(y)))
}

/// Изгиб луча купола-«сквиркла» по закону Снеллиуса (n = 1,5): `x` — доля кромки от края (0) до
/// её внутренней границы (1). `bendFromDepth` эталона.
fn bend_exact(x: f32) -> f32 {
    let u = 1.0 - x.clamp(0.0, 1.0);
    let denom = (1.0 - u.powi(4)).powf(0.75);
    if denom < 1e-6 {
        return 0.0;
    }
    let theta = (u.powi(3) / denom).min(8.0).atan();
    let sin_t = theta.sin() / 1.5;
    if sin_t >= 1.0 {
        return theta.sin();
    }
    (theta - sin_t.asin()).sin()
}

/// Точек таблицы изгиба на кромку.
const BEND_STEPS: usize = 256;

/// Изгиб с линейной выборкой из таблицы: тригонометрия на клетку не нужна.
fn bend(x: f32) -> f32 {
    static TABLE: LazyLock<[f32; BEND_STEPS + 1]> =
        LazyLock::new(|| std::array::from_fn(|i| bend_exact(i as f32 / BEND_STEPS as f32)));
    let at = x.clamp(0.0, 1.0) * BEND_STEPS as f32;
    let i = (at as usize).min(BEND_STEPS - 1);
    let f = at - i as f32;
    TABLE[i] + (TABLE[i + 1] - TABLE[i]) * f
}

/// Доля резкой подложки у самого края кромки (к её внутренней границе — квадратично до нуля).
/// В эталоне матовости почти нет (`frost` 0,4 px), и кромка гнёт резкую картинку — это и есть
/// вид «жидкого стекла»; окнам лаунчера нужна матовая середина под текстом, поэтому резкость —
/// только у края, а середина размыта, как в 0.4.9.
const SHARP_RIM: f32 = 0.85;

/// Усиление смещения эталона (`gain`).
const GAIN: f32 = 1.35;
/// Свет блика: слева сверху (эталон, ось y вниз).
const LIGHT: (f32, f32) = (-0.55, -0.85);

/// Карта линзы в точке блока `(x, y)` от его центра (CSS px), полуоси `half`, радиус угла `r`,
/// ширина кромки `bevel`.
pub fn lens_cell(x: f32, y: f32, half: (f32, f32), r: f32, bevel: f32) -> LensCell {
    let (d, (nx, ny)) = rounded_box(x, y, half, r);
    if d > 0.0 || bevel <= 0.0 {
        return LensCell::default();
    }
    let depth = -d;
    if depth >= bevel {
        return LensCell::default();
    }
    // Градиент поля высоты — изгиб на этой глубине вдоль нормали внутрь.
    let b = bend(depth / bevel) * GAIN;
    let axis = |n: f32| (-n * b).clamp(-1.0, 1.0) * 127.0 / 255.0;
    let len = LIGHT.0.hypot(LIGHT.1);
    let dot = (nx * LIGHT.0 + ny * LIGHT.1) / len;
    let rim = 1.0 - depth / bevel;
    LensCell {
        dx: axis(nx),
        dy: axis(ny),
        light: (dot.max(0.0) + (-dot).max(0.0) * 0.35) * rim,
        rim,
    }
}

/// Карта линзы клеток куска (сетка `grid`) блока `rect` с радиусом угла `radius`. Зависит только
/// от размеров, не от места блока: блок, чья подложка движется (параллакс), берёт её готовой.
/// Помнятся последние [`FIELDS_KEPT`].
fn field(grid: Grid, rect: Placement, radius: f32, lens: Lens) -> Field {
    // Без линзы карты нет: подложка во всё окно — сотни тысяч клеток.
    if lens.is_none() {
        return Arc::from([]);
    }
    type Key = (usize, usize, [u32; 6]);
    static CACHE: Mutex<Vec<(Key, Field)>> = Mutex::new(Vec::new());
    let half = (rect.width / 2.0, rect.height / 2.0);
    let r = radius.clamp(0.0, half.0.min(half.1));
    let bevel = lens.bevel.min(half.0.min(half.1));
    let key = (
        grid.out_w,
        grid.out_h,
        [grid.step, grid.pad, rect.width, rect.height, r, bevel].map(f32::to_bits),
    );
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(at) = cache.iter().position(|(k, _)| *k == key) {
        let hit = cache.remove(at);
        let field = hit.1.clone();
        cache.push(hit);
        return field;
    }
    let field: Field = (0..grid.out_w * grid.out_h)
        .map(|index| {
            let (i, j) = ((index % grid.out_w) as f32, (index / grid.out_w) as f32);
            let x = (i + 0.5) * grid.step - grid.pad - half.0;
            let y = (j + 0.5) * grid.step - grid.pad - half.1;
            lens_cell(x, y, half, r, bevel)
        })
        .collect();
    if cache.len() >= FIELDS_KEPT {
        cache.remove(0);
    }
    cache.push((key, field.clone()));
    field
}

/// Карта линзы клеток куска; пустая — линзы нет.
type Field = Arc<[LensCell]>;

/// Сколько карт помнить: окна, панели и мягкие кнопки на экране.
const FIELDS_KEPT: usize = 16;

fn alpha_byte(a: f32) -> u8 {
    (a * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Прозрачность блика в доле `t` линии градиента (до первого и после последнего стопа — крайние).
fn sheen_at(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    for pair in LG_SHEEN.windows(2) {
        let ((p0, a0), (p1, a1)) = (pair[0], pair[1]);
        if t <= p1 {
            return a0 + (a1 - a0) * (t - p0) / (p1 - p0);
        }
    }
    LG_SHEEN[LG_SHEEN.len() - 1].1
}

/// Непрозрачный цвет (0…1) под полупрозрачным `top`.
fn blend(under: [f32; 3], top: Rgba) -> [f32; 3] {
    let a = f32::from(top.a) / 255.0;
    let top = [top.r, top.g, top.b].map(|c| f32::from(c) / 255.0);
    [0, 1, 2].map(|k| under[k] + (top[k] - under[k]) * a)
}

/// Цвет картинки (0…1) в точке пикселей `(x, y)` (центры пикселей — целые), у края — крайний.
fn bilinear(image: &Rgba8Image, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |x: usize, y: usize| {
        let p = image.as_slice()[y * w + x];
        [p.r, p.g, p.b].map(|c| f32::from(c) / 255.0)
    };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    [0, 1, 2].map(|k| {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        top + (bottom - top) * fy
    })
}

/// Матрица `saturate(s)` (Filter Effects, `feColorMatrix type="saturate"`).
fn saturate_matrix(s: f32) -> [[f32; 3]; 3] {
    [
        [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
    ]
}

/// Пиксель картинки `image` (место в окне `place`, CSS px) в точке окна, прямая альфа; вне
/// картинки — прозрачный. Картинки `ImageLoader` предумножены — делится на альфу.
pub fn sample(image: &Rgba8Image, place: Placement, x: f32, y: f32) -> Rgba {
    let (w, h) = (image.width() as f32, image.height() as f32);
    if place.width <= 0.0 || place.height <= 0.0 || w == 0.0 || h == 0.0 {
        return Rgba::TRANSPARENT;
    }
    let u = (x - place.x) / place.width * w;
    let v = (y - place.y) / place.height * h;
    if u < 0.0 || v < 0.0 || u >= w || v >= h {
        return Rgba::TRANSPARENT;
    }
    let p = image.as_slice()[v as usize * image.width() as usize + u as usize];
    if !image.is_premultiplied() || p.a == 255 || p.a == 0 {
        return p;
    }
    let k = |c: u8| ((u32::from(c) * 255 + u32::from(p.a) / 2) / u32::from(p.a)).min(255) as u8;
    Rgba::new(k(p.r), k(p.g), k(p.b), p.a)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Placement = Placement {
        x: 0.0,
        y: 0.0,
        width: 1600.0,
        height: 900.0,
    };

    fn flat(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Rgba8Image {
        let mut image = Rgba8Image::new(width, height);
        for (index, p) in image.make_mut_slice().iter_mut().enumerate() {
            let [r, g, b] = pixel(index as u32 % width, index as u32 / width);
            *p = Rgba::rgb(r, g, b);
        }
        image
    }

    fn clear() -> Paint {
        Paint {
            fill: Rgba::TRANSPARENT,
            tint: Rgba::TRANSPARENT,
            sheen: false,
        }
    }

    fn none(_: f32, _: f32) -> Rgba {
        Rgba::TRANSPARENT
    }

    /// Радиус угла блока в тестах, CSS px.
    const RADIUS: f32 = 12.0;

    /// Точный кусок (сетка [`SIGMA`]) блока со скруглением [`RADIUS`].
    fn piece(
        source: &Rgba8Image,
        frame: Placement,
        rect: Placement,
        filter: &LiquidFilter,
        top: impl Fn(f32, f32) -> Rgba,
        paint: &Paint,
    ) -> Option<Piece> {
        piece_at(source, frame, rect, RADIUS, filter, SIGMA, top, paint)
    }

    /// Цвет куска в точке окна с линейной выборкой, как его растягивает видеокарта.
    fn linear(piece: &Piece, x: f32, y: f32) -> [f32; 3] {
        let (w, h) = (piece.image.width() as usize, piece.image.height() as usize);
        let u =
            ((x - piece.place.x) / piece.place.width * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let v =
            ((y - piece.place.y) / piece.place.height * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (i, j) = (u as usize, v as usize);
        let (fu, fv) = (u - i as f32, v - j as f32);
        let at = |i: usize, j: usize| piece.image.as_slice()[j.min(h - 1) * w + i.min(w - 1)];
        let mix = |c: fn(Rgba) -> u8| {
            let row = |j| f32::from(c(at(i, j))) * (1.0 - fu) + f32::from(c(at(i + 1, j))) * fu;
            row(j) * (1.0 - fv) + row(j + 1) * fv
        };
        [mix(|p| p.r), mix(|p| p.g), mix(|p| p.b)]
    }

    /// `.news-viewer`: только размытие — ровный цветной кадр не меняется (ни сдвигов, ни
    /// насыщенности), резкая граница расплывается на σ 8 px, сверху ложится заливка.
    #[test]
    fn blur_8_only_blurs_under_the_fill() {
        let even = flat(160, 90, |_, _| [200, 50, 30]);
        let plain = piece(&even, WINDOW, WINDOW, &BLUR_8, none, &clear()).unwrap();
        let [r, g, b] = linear(&plain, 400.0, 300.0);
        assert!((r - 200.0).abs() < 1.0 && (g - 50.0).abs() < 1.0 && (b - 30.0).abs() < 1.0);

        let edge = flat(
            160,
            90,
            |x, _| if x < 80 { [0, 0, 0] } else { [255, 255, 255] },
        );
        let fill = Paint {
            fill: Rgba::new(5, 8, 11, 224),
            ..clear()
        };
        let piece = piece(&edge, WINDOW, WINDOW, &BLUR_8, none, &fill).unwrap();
        let at = |x: f32| linear(&piece, x, 450.0)[0];
        let k = 224.0 / 255.0;
        let (dark, light) = (5.0 * k, 255.0 + (5.0 - 255.0) * k);
        assert!((at(100.0) - dark).abs() < 1.0, "{}", at(100.0));
        assert!((at(1500.0) - light).abs() < 1.0, "{}", at(1500.0));
        let middle = (dark + light) / 2.0;
        assert!((at(800.0) - middle).abs() < 3.0, "{}", at(800.0));
        // σ слева от границы: около 16 % светлого.
        let near = at(792.0);
        assert!(near > dark + 2.0 && near < middle - 5.0, "{near}");
    }

    #[test]
    fn coarse_piece_stays_close_to_the_fine_one() {
        // Настоящий кадр игры (`hero-hl2.jpg`) во всё окно, блок с преломлением каждой группы.
        // Замер (release, ещё окно 1040×720 и кнопка 64×72): среднее 0,35–0,7 уровня, 99 % точек —
        // до 5 уровней, наибольшее — 12 у края блока, где сдвиг выносит выборку за блок. Шахматка
        // 40 px двух цветов (худшее): среднее 3–5 уровней.
        let frame = image::load_from_memory(include_bytes!("../../assets/launcher/hero-hl2.jpg"))
            .unwrap()
            .to_rgba8();
        let source = flat(frame.width(), frame.height(), |x, y| {
            let [r, g, b, _] = frame.get_pixel(x, y).0;
            [r, g, b]
        });
        let rect = Placement {
            x: 500.0,
            y: 300.0,
            width: 300.0,
            height: 200.0,
        };
        for filter in [LG_WINDOW, LG_PANEL, LG_SOFT] {
            let fine = piece(&source, WINDOW, rect, &filter, none, &clear()).unwrap();
            let coarse = piece_at(
                &source,
                WINDOW,
                rect,
                RADIUS,
                &filter,
                SIGMA_COARSE,
                none,
                &clear(),
            )
            .unwrap();
            let mut diffs = Vec::new();
            for j in 0..rect.height as usize {
                for i in 0..rect.width as usize {
                    let (x, y) = (rect.x + i as f32 + 0.5, rect.y + j as f32 + 0.5);
                    let (a, b) = (linear(&fine, x, y), linear(&coarse, x, y));
                    diffs.push((0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f32::max));
                }
            }
            diffs.sort_by(f32::total_cmp);
            let mean = diffs.iter().sum::<f32>() / diffs.len() as f32;
            let p99 = diffs[diffs.len() * 99 / 100];
            assert!(mean < 1.0 && p99 < 6.0, "σ {}: {mean} {p99}", filter.blur);
        }
    }

    #[test]
    fn piece_covers_the_box_with_two_cells_around() {
        let source = flat(160, 90, |_, _| [40, 80, 120]);
        let rect = Placement {
            x: 280.0,
            y: 90.0,
            width: 1040.0,
            height: 720.0,
        };
        let piece = piece(&source, WINDOW, rect, &LG_WINDOW, none, &clear()).unwrap();
        // σ 14 → клетка 3,5 CSS px; по 2 клетки с каждой стороны.
        assert_eq!(piece.place.x, 273.0);
        assert_eq!(piece.place.y, 83.0);
        assert_eq!((piece.image.width(), piece.image.height()), (302, 210));
        assert!(piece.place.width >= rect.width + 14.0);
        let empty = Placement { width: 0.5, ..rect };
        assert!(self::piece(&source, WINDOW, empty, &LG_WINDOW, none, &clear()).is_none());
    }

    #[test]
    fn flat_backdrop_is_only_coloured_by_the_filter() {
        // Серый остаётся серым (насыщенность, линза и белый блик кромки серого не меняют),
        // brightness(1.06) светлее.
        let source = flat(160, 90, |_, _| [100, 100, 100]);
        let rect = Placement {
            x: 400.0,
            y: 200.0,
            width: 300.0,
            height: 200.0,
        };
        let window = piece(&source, WINDOW, rect, &LG_WINDOW, none, &clear()).unwrap();
        // Поле куска за коробкой — подложка без фильтра (100), внутри — с brightness (106), у кромки
        // светлее на блик.
        let pixels = window.image.as_slice();
        assert!(
            pixels
                .iter()
                .all(|p| p.r == p.g && p.g == p.b && p.r >= 100)
        );
        let middle = pixels[pixels.len() / 2 + window.image.width() as usize / 2];
        assert_eq!(middle, Rgba::rgb(106, 106, 106));
        let panel = piece(&source, WINDOW, rect, &LG_PANEL, none, &clear()).unwrap();
        let w = panel.image.width() as usize;
        let middle = panel.image.as_slice()[panel.image.as_slice().len() / 2 + w / 2];
        assert_eq!(middle, Rgba::rgb(100, 100, 100));
        assert!(
            panel
                .image
                .as_slice()
                .iter()
                .all(|p| p.r == p.b && p.r >= 100)
        );
        // Насыщенность раздвигает каналы цветного.
        let tinted = flat(160, 90, |_, _| [120, 100, 80]);
        let soft = piece(&tinted, WINDOW, rect, &LG_SOFT, none, &clear()).unwrap();
        let pixels = soft.image.as_slice();
        let p = pixels[pixels.len() / 2 + soft.image.width() as usize / 2];
        assert!(p.r > 120 && p.b < 80, "{p:?}");
    }

    #[test]
    fn the_lens_bends_only_the_rim() {
        // Вертикальные полосы по 40 CSS px, левый край блока за 10 px до границы полос: глубже
        // кромки кусок тот же, что без линзы, на кромке — полосы сдвинуты внутрь блока.
        let source = flat(1600, 900, |x, _| {
            if x / 40 % 2 == 0 {
                [30, 30, 30]
            } else {
                [220, 220, 220]
            }
        });
        let rect = Placement {
            x: 310.0,
            y: 100.0,
            width: 600.0,
            height: 600.0,
        };
        let flat_lens = LiquidFilter {
            lens: Lens {
                scale: 0.0,
                aberration: 0.0,
                specular: 0.0,
                ..LG_LENS_SOFT
            },
            ..LG_SOFT
        };
        let bare = LiquidFilter {
            lens: Lens {
                specular: 0.0,
                ..LG_LENS_SOFT
            },
            ..LG_SOFT
        };
        let lens = piece(&source, WINDOW, rect, &bare, none, &clear()).unwrap();
        let plain = piece(&source, WINDOW, rect, &flat_lens, none, &clear()).unwrap();
        let w = lens.image.width() as usize;
        let row = lens.image.height() as usize / 2;
        let step = LG_SOFT.blur / SIGMA;
        let mut rim = 0u8;
        for i in 0..w {
            let x = lens.place.x + (i as f32 + 0.5) * step;
            let (a, b) = (
                lens.image.as_slice()[row * w + i],
                plain.image.as_slice()[row * w + i],
            );
            let depth = (x - rect.x).min(rect.x + rect.width - x);
            if depth > LG_LENS_SOFT.bevel {
                assert_eq!(a, b, "глубже кромки, x {x}");
            } else if depth > 0.0 {
                rim = rim.max(a.r.abs_diff(b.r));
            }
        }
        assert!(rim > 40, "кромка не гнёт: {rim}");
    }

    #[test]
    fn lens_pulls_inward_and_lights_the_top_left() {
        let half = (100.0, 50.0);
        let (r, bevel) = (20.0, 14.0);
        // Снаружи и глубже кромки — ничего.
        assert_eq!(lens_cell(150.0, 0.0, half, r, bevel), LensCell::default());
        assert_eq!(lens_cell(0.0, 0.0, half, r, bevel), LensCell::default());
        assert_eq!(lens_cell(-80.0, 0.0, half, r, bevel), LensCell::default());
        // Левая кромка тянет выборку вправо (внутрь), правая — влево, верхняя — вниз.
        let left = lens_cell(-98.0, 0.0, half, r, bevel);
        let right = lens_cell(98.0, 0.0, half, r, bevel);
        let top = lens_cell(0.0, -48.0, half, r, bevel);
        let bottom = lens_cell(0.0, 48.0, half, r, bevel);
        assert!(left.dx > 0.1 && left.dy == 0.0, "{left:?}");
        assert!((left.dx + right.dx).abs() < 1e-6);
        assert!(top.dy > 0.1 && top.dx == 0.0, "{top:?}");
        // Не больше ±127/255 по оси, как байт карты.
        assert!(left.dx <= 127.0 / 255.0);
        // Угол тянет по диагонали к центру.
        let corner = lens_cell(-90.0, -40.0, half, r, bevel);
        assert!(corner.dx > 0.0 && corner.dy > 0.0, "{corner:?}");
        // Свет слева сверху: верх и левый край ярче низа и правого.
        assert!(top.light > bottom.light && left.light > right.light);
        assert!(bottom.light > 0.0);
        // Изгиб: ноль в глубине, наибольший у края.
        assert_eq!(bend_exact(1.0), 0.0);
        assert!(bend_exact(0.02) > bend_exact(0.5) && bend_exact(0.5) > 0.0);
        for k in 0..=20 {
            let x = k as f32 / 20.0;
            assert!((bend(x) - bend_exact(x)).abs() < 0.02, "{x}");
        }
    }

    #[test]
    fn aberration_splits_channels_at_the_rim() {
        // Серый, светлеющий слева направо от левого края блока (2 уровня на px): у левой кромки
        // красный канал берётся глубже (светлее) синего, в середине каналы равны.
        let source = flat(1600, 900, |x, _| {
            let v = (x.saturating_sub(380) * 2).min(255) as u8;
            [v, v, v]
        });
        let rect = Placement {
            x: 400.0,
            y: 200.0,
            width: 400.0,
            height: 300.0,
        };
        let filter = LiquidFilter {
            saturate: 1.0,
            lens: Lens {
                specular: 0.0,
                ..LG_LENS_PANEL
            },
            ..LG_PANEL
        };
        let out = piece(&source, WINDOW, rect, &filter, none, &clear()).unwrap();
        let w = out.image.width() as usize;
        let row = out.image.height() as usize / 2;
        let at = |i: usize| out.image.as_slice()[row * w + i];
        let edge = at(3);
        assert!(edge.r > edge.b, "{edge:?}");
        let middle = at(w / 2);
        assert_eq!(middle.r, middle.b);
    }

    #[test]
    fn samples_beyond_the_box_show_the_backdrop_without_the_filter() {
        // Кнопка 64 × 64 у нижнего края светлого: над ней (60 px) светло, под ней темно. Линза
        // тянет выборку внутрь коробки, поле куска за коробкой — подложка без фильтра (тёмная):
        // светлое сверху в кусок не попадает.
        let source = flat(1600, 900, |_, y| {
            if y < 760 {
                [220, 220, 220]
            } else {
                [20, 20, 20]
            }
        });
        let rect = Placement {
            x: 1200.0,
            y: 800.0,
            width: 64.0,
            height: 64.0,
        };
        let filter = LiquidFilter {
            lens: Lens {
                specular: 0.0,
                ..LG_LENS_SOFT
            },
            ..LG_SOFT
        };
        let soft = piece(&source, WINDOW, rect, &filter, none, &clear()).unwrap();
        assert!(
            soft.image.as_slice().iter().all(|p| p.r < 40),
            "сверху подмешалось светлое"
        );
    }

    #[test]
    fn top_layers_lie_over_the_frame_before_the_blur() {
        // Подложка окна-диалога наполовину чёрная: кусок вдвое темнее (без фильтра цвета).
        let source = flat(160, 90, |_, _| [200, 200, 200]);
        let rect = Placement {
            x: 400.0,
            y: 200.0,
            width: 300.0,
            height: 200.0,
        };
        let filter = LiquidFilter {
            saturate: 1.0,
            brightness: 1.0,
            lens: Lens::NONE,
            ..LG_PANEL
        };
        let half = |_: f32, _: f32| Rgba::new(0, 0, 0, 128);
        let out = piece(&source, WINDOW, rect, &filter, half, &clear()).unwrap();
        assert!(out.image.as_slice().iter().all(|p| p.r.abs_diff(100) <= 1));
    }

    #[test]
    fn fill_tint_and_sheen_follow_css() {
        // rgba(18,22,30,.58) поверх чёрного, затем белый .07 и блик 150deg: левый верх светлее
        // правого низа (.14 против .06), середина — без блика.
        let source = flat(160, 90, |_, _| [0, 0, 0]);
        let rect = Placement {
            x: 400.0,
            y: 200.0,
            width: 400.0,
            height: 300.0,
        };
        let paint = Paint {
            fill: Rgba::new(18, 22, 30, 148),
            tint: LG_TINT,
            sheen: true,
        };
        let out = piece(&source, WINDOW, rect, &LG_WINDOW, none, &paint).unwrap();
        let w = out.image.width() as usize;
        let at = |i: usize, j: usize| out.image.as_slice()[j * w + i];
        let (corner, middle, end) = (
            at(2, 2),
            at(w / 2, out.image.height() as usize / 2),
            at(w - 3, out.image.height() as usize - 3),
        );
        // Без блика: 0 → 10/13/17 (заливка) → +7 % белого.
        let base: f32 = 30.0 * 148.0 / 255.0;
        let expect = (base + (255.0 - base) * (18.0 / 255.0)).round() as u8;
        // В середине блика почти нет (.008 у t = ½).
        assert!(middle.b.abs_diff(expect) <= 3, "{middle:?} против {expect}");
        assert!(
            corner.b > end.b && end.b > middle.b,
            "{corner:?} {middle:?} {end:?}"
        );
    }

    #[test]
    fn sheen_stops_match_the_gradient() {
        assert!((sheen_at(0.0) - 0.14).abs() < 1e-6);
        assert!((sheen_at(0.15) - 0.09).abs() < 1e-6);
        assert_eq!(sheen_at(0.55), 0.0);
        assert!((sheen_at(1.0) - 0.06).abs() < 1e-6);
        assert!((sheen_at(2.0) - 0.06).abs() < 1e-6);
    }

    #[test]
    fn sample_reads_straight_alpha_and_nothing_outside() {
        let mut image = Rgba8Image::new(2, 1);
        image.make_mut_slice()[1] = Rgba::new(50, 25, 0, 128);
        let image = image.premultiplied();
        let place = Placement {
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 10.0,
        };
        assert_eq!(sample(&image, place, 5.0, 15.0), Rgba::TRANSPARENT);
        assert_eq!(
            sample(&image, place, 25.0, 15.0),
            Rgba::new(100, 50, 0, 128)
        );
    }
    /// Цена куска (замер для отчёта, не проверка): `cargo test --release -p hlsr-ui --lib
    /// piece_cost -- --ignored --nocapture`. Кадр 2560 × 1440 в окне 1600 × 900.
    #[test]
    #[ignore]
    fn piece_cost() {
        let source = flat(2560, 1440, |x, y| [(x % 251) as u8, (y % 241) as u8, 90]);
        let cases = [
            ("окно 1040×720", LG_WINDOW, (280.0, 90.0, 1040.0, 720.0)),
            ("меню 260×170", LG_PANEL, (1150.0, 80.0, 260.0, 170.0)),
            ("кнопка 64×72", LG_SOFT, (1192.0, 808.0, 64.0, 72.0)),
            ("сборка 304×64", LG_SOFT, (1272.0, 728.0, 304.0, 64.0)),
        ];
        for (name, filter, (x, y, width, height)) in cases {
            let rect = Placement {
                x,
                y,
                width,
                height,
            };
            let start = std::time::Instant::now();
            let runs = 20;
            for _ in 0..runs {
                std::hint::black_box(piece(&source, WINDOW, rect, &filter, none, &clear()));
            }
            println!(
                "{name}: {:.2} мс",
                start.elapsed().as_secs_f64() * 1000.0 / runs as f64
            );
        }
    }
}
