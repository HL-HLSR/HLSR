//! Превью персонализации Ghosting (`ghosting-overlay-renderer.js` 0.4.7, задача 34).
//! Слои сняты в Ghosting Renewed 3.9, d2_coast_09, 1600×900. Мир масштабируется вокруг
//! центра; оружие берётся из двух соседних ракурсов X и перепроецируется по глубине
//! (сетка с шагом 6 px по цветному ракурсу, карта глубины). HUD смешивается отдельно в
//! линейном свете: маски R — свечение, G — тень, B — подложка (`hud.png`).
//!
//! WebGL 0.4.7 (перепроекция сетки) здесь — растеризатор треугольников на ЦП с тем же
//! вершинным шейдером, тестом глубины (16 бит) и билинейной выборкой; без MSAA.
//! Холст Canvas 2D — tiny-skia и [`super::raster`]; текст — [`super::canvas_text`].
//! Сверка с Electron — с допуском (решение 19).
//!
//! Отличия от 0.4.7: слой декодируется сразу, без подмены ближайшим загруженным; поля за
//! кадром при ширине > 1600 — размытие с повтором края (в 0.4.7 прозрачный край размытия
//! ложится на прошлый кадр холста и после нескольких кадров непрозрачен).

use super::canvas_text::{self, Align, Baseline, Font, Ink, Paint, Place};
use super::hl1_overlay::{number, rgb_of, truthy};
use super::preview_assets as assets;
use super::raster::{self, Area, Blend, RasterError};
use hlsr_core::js;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::f64::consts::PI;
use std::sync::OnceLock;
use tiny_skia::{
    Color, Paint as SkPaint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
};

pub const FRAME_WIDTH: u32 = 1600;
pub const FRAME_HEIGHT: u32 = 900;
const FW: f64 = FRAME_WIDTH as f64;
const FH: f64 = FRAME_HEIGHT as f64;
const UNIT: f64 = FH / 480.0;
const CX: f64 = FW / 2.0;
const CY: f64 = FH / 2.0;
/// Шаг сетки по цветному ракурсу, px.
const MESH: usize = 6;
/// Цвет HUD без своего (`gh_hud_color_enable 0`), как его измерили маски.
const STOCK: [u8; 3] = [255, 220, 0];
/// Прицел оружия; `gh_hud_color_affects_crosshair` перекрашивает его.
const DOTS: [i32; 4] = [787, 437, 812, 464];
const DOT_STOCK: [u8; 3] = [255, 221, 66];
/// Дуги быстрой информации вокруг прицела всегда цвета HUD.
const QUICKINFO: [i32; 4] = [758, 418, 840, 482];
/// `gh_hud_color_alpha` против своей прозрачности схемы (около 100).
const STOCK_ALPHA: f64 = 99.6;

/// HUD мода Condensed Animated HUD (`custom-packs`, `condensed-hud`), каким его рисует
/// игра: раскладка — из его `scripts/hudlayout.res` (единицы 640×480), цифры — шрифтом
/// HudNumbers его `resource/clientscheme.res` (Alte DIN 1451 Mittelschrift 2, tall 36,
/// scanlines 2; HudNumbersSmall — tall 16, weight 1000). FgColor мода — 255 220 0 220, у
/// стандартного HUD альфа 100. Подписи и значки — пиксели стандартных масок на местах мода.
const CONDENSED_ALPHA: f64 = 220.0;

/// Цифры мода: левый край, верх ячейки и её высота (`tall`), px кадра.
#[derive(Clone, Copy)]
struct Digits {
    x: f64,
    top: f64,
    tall: f64,
}

/// Элемент HUD мода: `text` — начало подписи у мода, `stock_text` — у стандартного HUD.
struct CondensedDef {
    id: &'static str,
    plate: Option<[i32; 4]>,
    stock_plate: bool,
    anchor: Option<[i32; 4]>,
    text: [f64; 2],
    stock_text: [f64; 2],
    digits: Digits,
    reserve: Option<Digits>,
}

fn at480(v: f64) -> f64 {
    v * UNIT
}

fn from_right(v: f64) -> f64 {
    FW - at480(v)
}

fn from_centre(v: f64) -> f64 {
    CX + at480(v)
}

fn panel(x: f64, y: f64, w: f64, h: f64) -> [i32; 4] {
    [
        js::math_round(x) as i32,
        js::math_round(at480(y)) as i32,
        js::math_round(x + at480(w)) as i32,
        js::math_round(at480(y + h)) as i32,
    ]
}

fn condensed_defs() -> [CondensedDef; 5] {
    let cell = |x: f64| Digits {
        x,
        top: at480(434.0),
        tall: at480(36.0),
    };
    [
        CondensedDef {
            id: "health",
            plate: Some(panel(at480(16.0), 426.0, 48.0, 42.0)),
            stock_plate: false,
            anchor: None,
            text: [at480(20.0), at480(428.0)],
            stock_text: [at480(24.0), at480(452.0)],
            digits: cell(at480(18.0)),
            reserve: None,
        },
        CondensedDef {
            id: "suit",
            plate: Some(panel(at480(68.0), 426.0, 48.0, 42.0)),
            stock_plate: false,
            anchor: None,
            text: [at480(72.0), at480(428.0)],
            stock_text: [at480(148.0), at480(452.0)],
            digits: cell(at480(70.0)),
            reserve: None,
        },
        CondensedDef {
            id: "speed",
            plate: None,
            stock_plate: true,
            anchor: None,
            text: [from_centre(-49.0), at480(452.0)],
            stock_text: [from_centre(-49.0), at480(452.0)],
            digits: cell(from_centre(-7.0)),
            reserve: None,
        },
        // HudAmmo мода остаётся на r170 и со вторичными патронами.
        CondensedDef {
            id: "ammo",
            plate: Some(panel(from_right(170.0), 432.0, 50.0, 36.0)),
            stock_plate: false,
            anchor: None,
            text: [from_right(162.0), at480(456.0)],
            stock_text: [from_right(214.0), at480(452.0)],
            digits: cell(from_right(134.0)),
            reserve: Some(Digits {
                x: from_right(104.0),
                top: at480(450.0),
                tall: at480(16.0),
            }),
        },
        // У HudAmmoSecondary мода BgColor 0 0 0 0: плашки нет, место — размер панели.
        CondensedDef {
            id: "ammo2",
            plate: None,
            stock_plate: false,
            anchor: Some(panel(from_right(76.0), 432.0, 60.0, 36.0)),
            text: [from_right(64.0), at480(456.0)],
            stock_text: [from_right(68.0), at480(454.0)],
            digits: cell(from_right(40.0)),
            reserve: None,
        },
    ]
}
const SHADE_STEPS: [(f64, f64); 6] = [
    (0.0, 0.81),
    (32.0, 1.037),
    (64.0, 1.262),
    (128.0, 1.714),
    (192.0, 2.169),
    (255.0, 2.612),
];
/// Сколько ракурсов оружия держать декодированными (по 6–7 МБ).
const VIEWS_KEPT: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Center,
    Right,
}

/// Элемент HUD: `box` — всё, что он рисует; `plate` — что ставят `gh_*_align` (у HUD мода
/// без плашки — `anchor`, размер его панели).
#[derive(Clone)]
struct Element {
    id: &'static str,
    key: &'static str,
    side: Side,
    moved: f64,
    bbox: [i32; 4],
    plate: [i32; 4],
    anchor: Option<[i32; 4]>,
    labels: Cow<'static, [[i32; 4]]>,
    icons: Cow<'static, [[i32; 4]]>,
    caption: Option<(&'static str, f64, f64)>,
    value: &'static str,
    digits: (f64, f64, f64, Align),
    reserve: Option<(&'static str, f64, f64, f64)>,
}

const ELEMENTS: [Element; 6] = [
    Element {
        id: "timer",
        key: "gh_timer",
        side: Side::Left,
        moved: 0.0,
        bbox: [78, 246, 261, 291],
        plate: [84, 252, 255, 285],
        anchor: None,
        labels: Cow::Borrowed(&[]),
        icons: Cow::Borrowed(&[]),
        caption: None,
        value: "00:00:00.000",
        digits: (169.0, 276.0, 19.0, Align::Center),
        reserve: None,
    },
    Element {
        id: "health",
        key: "gh_health",
        side: Side::Left,
        moved: 0.0,
        bbox: [24, 804, 227, 883],
        plate: [30, 810, 221, 877],
        anchor: None,
        labels: Cow::Borrowed(&[[42, 846, 101, 863]]),
        icons: Cow::Borrowed(&[]),
        caption: Some(("HEALTH", 44.0, 859.0)),
        value: "100",
        digits: (131.0, 865.0, 51.0, Align::Left),
        reserve: None,
    },
    Element {
        id: "suit",
        key: "gh_suit",
        side: Side::Left,
        moved: 0.0,
        bbox: [256, 804, 470, 883],
        plate: [262, 810, 464, 877],
        anchor: None,
        labels: Cow::Borrowed(&[[274, 846, 314, 864]]),
        icons: Cow::Borrowed(&[]),
        caption: Some(("SUIT", 276.0, 859.0)),
        value: "60",
        digits: (358.0, 865.0, 51.0, Align::Left),
        reserve: None,
    },
    Element {
        id: "speed",
        key: "gh_speedmeter",
        side: Side::Center,
        moved: 0.0,
        bbox: [688, 804, 913, 883],
        plate: [694, 810, 907, 877],
        anchor: None,
        labels: Cow::Borrowed(&[[706, 846, 740, 864]]),
        icons: Cow::Borrowed(&[]),
        caption: Some(("UPS", 708.0, 859.0)),
        value: "0",
        digits: (790.0, 865.0, 51.0, Align::Left),
        reserve: None,
    },
    Element {
        id: "ammo",
        key: "gh_ammo",
        side: Side::Right,
        moved: (222.0 - 150.0) * UNIT,
        bbox: [1178, 804, 1437, 883],
        plate: [1184, 810, 1431, 877],
        anchor: None,
        labels: Cow::Borrowed(&[[1195, 846, 1244, 863]]),
        icons: Cow::Borrowed(&[[1193, 825, 1243, 842]]),
        caption: Some(("AMMO", 1197.0, 859.0)),
        value: "45",
        digits: (1268.0, 865.0, 51.0, Align::Left),
        reserve: Some(("162", 1371.0, 866.0, 27.0)),
    },
    Element {
        id: "ammo2",
        key: "gh_ammo2",
        side: Side::Right,
        moved: 0.0,
        bbox: [1452, 804, 1576, 883],
        plate: [1458, 810, 1570, 877],
        anchor: None,
        labels: Cow::Borrowed(&[[1469, 850, 1502, 867]]),
        icons: Cow::Borrowed(&[[1456, 827, 1511, 849]]),
        caption: Some(("ALT", 1471.0, 863.0)),
        value: "3",
        digits: (1527.0, 865.0, 51.0, Align::Left),
        reserve: None,
    },
];

#[derive(Debug, Deserialize)]
struct ViewInfo {
    x: f64,
    at: [f64; 2],
}

#[derive(Debug, Deserialize)]
struct ViewmodelMeta {
    fov: f64,
    inv: [f64; 2],
}

#[derive(Debug, Deserialize)]
struct Meta {
    scenes: Vec<f64>,
    viewmodel: ViewmodelMeta,
    viewmodels: Vec<ViewInfo>,
}

struct Tables {
    meta: Meta,
    scenes: Vec<f64>,
    defaults: Map<String, Value>,
    /// `LINEAR` 0.4.7: sRGB → линейный свет, `Float32Array`.
    linear: [f32; 256],
    /// `SRGB` 0.4.7: 4096 шагов линейного света → sRGB.
    srgb: Vec<u8>,
}

const SRGB_STEPS: usize = 4096;

fn to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let meta: Meta =
            serde_json::from_str(assets::GHOSTING_META).expect("ghosting-preview/meta.json");
        let mut scenes = meta.scenes.clone();
        scenes.sort_by(f64::total_cmp);
        let defaults = match hlsr_delivery::ghosting_settings::defaults().to_value() {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let mut linear = [0f32; 256];
        for (i, v) in linear.iter_mut().enumerate() {
            *v = to_linear(i as f64 / 255.0) as f32;
        }
        let srgb = (0..=SRGB_STEPS)
            .map(|i| {
                let v = i as f64 / SRGB_STEPS as f64;
                let s = if v <= 0.0031308 {
                    v * 12.92
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055
                };
                js::math_round(255.0 * s).clamp(0.0, 255.0) as u8
            })
            .collect();
        Tables {
            meta,
            scenes,
            defaults,
            linear,
            srgb,
        }
    })
}

fn tan(fov: f64) -> f64 {
    (fov * PI / 360.0).tan()
}

/// Фокус движка: поле зрения — для экрана 4:3 той же высоты.
fn focal(fov: f64) -> f64 {
    FH * 2.0 / 3.0 / tan(fov)
}

fn frame_for(list: &[f64], fov: f64) -> f64 {
    list.iter()
        .copied()
        .find(|&step| step >= fov - 0.01)
        .unwrap_or_else(|| list.last().copied().unwrap_or(110.0))
}

fn shade_gain(alpha: f64) -> f64 {
    let Some(i) = SHADE_STEPS.iter().position(|&(step, _)| alpha <= step) else {
        return SHADE_STEPS[0].1;
    };
    if i == 0 {
        return SHADE_STEPS[0].1;
    }
    let (a0, k0) = SHADE_STEPS[i - 1];
    let (a1, k1) = SHADE_STEPS[i];
    k0 + (k1 - k0) * (alpha - a0) / (a1 - a0)
}

/// Насколько элемент у этого края сдвигается в кадре ширины `width`.
fn shift_of(side: Side, width: f64) -> f64 {
    match side {
        Side::Right => width - FW,
        Side::Center => js::math_round((width - FW) / 2.0),
        Side::Left => 0.0,
    }
}

/// `fontName(name)`: имя системного шрифта или `""` (путь к файлу, `.ttf`, `.otf`).
pub fn font_name(name: Option<&Value>) -> String {
    let Some(name) = name.and_then(Value::as_str).filter(|n| !n.is_empty()) else {
        return String::new();
    };
    let lower = name.to_ascii_lowercase();
    if name.contains(['\\', '/']) || lower.ends_with(".ttf") || lower.ends_with(".otf") {
        return String::new();
    }
    name.chars()
        .filter(|c| !matches!(c, '"' | ';' | '{' | '}'))
        .collect()
}

/// Куда `gh_<элемент>_align` и `_x`/`_y` ставят элемент (сдвиг от его места в кадре).
/// `None` — якорь вне таблицы (у 0.4.7 — `NaN`, элемент не рисуется).
fn offset(
    s: &Map<String, Value>,
    prefix: &str,
    plate: [f64; 4],
    width: f64,
    side: Side,
    moved: f64,
) -> Option<(f64, f64)> {
    let anchor = number(s.get(&format!("{prefix}_align")), 0.0);
    let dx = number(s.get(&format!("{prefix}_x")), 0.0);
    let dy = number(s.get(&format!("{prefix}_y")), 0.0);
    let w = plate[2] - plate[0];
    let h = plate[3] - plate[1];
    let mut x = plate[0] + shift_of(side, width);
    let mut y = plate[1];
    if (1.0..=9.0).contains(&anchor) {
        if anchor.fract() != 0.0 {
            return None;
        }
        let a = anchor as usize - 1;
        x = [0.0, (width - w) / 2.0, width - w][a % 3];
        y = [0.0, (FH - h) / 2.0, FH - h][a / 3];
    } else if dx != 0.0 || dy != 0.0 {
        x += moved;
    }
    Some((
        js::math_round(x + dx * UNIT - plate[0]),
        js::math_round(y + dy * UNIT - plate[1]),
    ))
}

fn inside(rects: &[[i32; 4]], x: i32, y: i32) -> bool {
    rects
        .iter()
        .any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3])
}

/// Часть HUD, посчитанная для одного вида (`u · A + B` в линейном свете по пикселям).
struct Part {
    x0: i32,
    y0: i32,
    width: i32,
    where_: Vec<usize>,
    mul: Vec<f32>,
    add: Vec<f32>,
}

/// Квадрат канала 0..255 для масок HUD (`hud.png`): таблица экономит 4× памяти без потери точности.
static SQ_LUT: std::sync::LazyLock<[f32; 256]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|v| ((v as f64 / 255.0).powi(2)) as f32)
});

/// Маски HUD (`hud.png`): квадрат канала, `Float32Array`.
/// Для стокового `hud.png` хранятся исходные байты `[r, g, b]` (4,1 МиБ вместо 16,5 МиБ),
/// а для `condensedMasks` — произвольные веса `f32`.
enum HudMasks {
    Stock(Vec<[u8; 3]>),
    Custom {
        glow: Vec<f32>,
        shade: Vec<f32>,
        plate: Vec<f32>,
    },
}

impl HudMasks {
    #[inline]
    fn glow(&self, i: usize) -> f32 {
        match self {
            HudMasks::Stock(data) => SQ_LUT[usize::from(data[i][0])],
            HudMasks::Custom { glow, .. } => glow[i],
        }
    }

    #[inline]
    fn shade(&self, i: usize) -> f32 {
        match self {
            HudMasks::Stock(data) => SQ_LUT[usize::from(data[i][1])],
            HudMasks::Custom { shade, .. } => shade[i],
        }
    }

    #[inline]
    fn plate(&self, i: usize) -> f32 {
        match self {
            HudMasks::Stock(data) => SQ_LUT[usize::from(data[i][2])],
            HudMasks::Custom { plate, .. } => plate[i],
        }
    }
}

/// Ракурс оружия: вершины `[x, глубина, z, u, v]` (f32), треугольники и текстура.
struct ViewMesh {
    cols: usize,
    rows: usize,
    points: Vec<[f32; 5]>,
    texture: Pixmap,
    /// Клетка сетки даёт фрагменты (в её билинейном окне есть альфа ≥ 0.02).
    live: Vec<bool>,
}

/// Что рендер берёт из файлов игрока (`assets` 0.4.7): семейства шрифтов и спрей.
#[derive(Default, Clone)]
pub struct Assets {
    /// `gh_hud_font` из файла.
    pub font_family: Option<String>,
    pub showfps: Option<String>,
    pub showpos: Option<String>,
    pub spray: Option<std::sync::Arc<Pixmap>>,
    /// Файлы шрифтов игрока (ключ, байты): регистрируются в потоке рисования.
    pub fonts: Vec<(String, std::sync::Arc<Vec<u8>>)>,
    /// Включён HUD-мод Condensed HUD (`hudMod` 0.4.9): его раскладка и цифры.
    pub hud_mod: bool,
    /// Семейство шрифта цифр мода (`resource/din1451alt2.ttf` из его папки); нет — DIN из ОС.
    pub hud_mod_font: Option<String>,
    /// Своя модель призрака (`gh_model`) для сцены «Призраки и запись».
    pub ghost_model: Option<std::sync::Arc<super::ghost_mesh::MeshSprite>>,
}

/// Мир кадра (сцена, поле за кадром) и то, из чего он нарисован.
struct Base {
    key: String,
    pixels: Pixmap,
}

/// Маски HUD мода (`condensedMasks` 0.4.9) для шрифта цифр `family` и его элементы.
struct Condensed {
    family: String,
    masks: HudMasks,
    elements: Vec<Element>,
}

/// Рендер превью Ghosting.
#[derive(Default)]
pub struct OverlayRenderer {
    scenes: HashMap<String, Pixmap>,
    scene_order: VecDeque<String>,
    hud: Option<HudMasks>,
    views: HashMap<usize, ViewMesh>,
    view_order: VecDeque<usize>,
    parts: HashMap<String, Part>,
    /// Мир под оружием и HUD (сцена, поле за кадром) последнего вида: при «движении»
    /// меняются только оружие, клавиши и HUD (кэш слоёв, задача 34).
    base: Option<Base>,
    condensed: Option<Condensed>,
}

fn err(name: &str, reason: &str) -> RasterError {
    RasterError {
        name: name.to_owned(),
        reason: reason.to_owned(),
    }
}

fn decode(name: &str) -> Result<Pixmap, RasterError> {
    let bytes = assets::ghosting(name).ok_or_else(|| err(name, "нет в сборке"))?;
    raster::decode(name, bytes)
}

/// Цвет CSS `rgb()` → цвет tiny-skia с прозрачностью.
fn sk(rgb: [u8; 3], alpha: f64) -> Color {
    Color::from_rgba8(
        rgb[0],
        rgb[1],
        rgb[2],
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn fill_rect(frame: &mut Pixmap, x: f64, y: f64, w: f64, h: f64, color: Color) {
    let Some(rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) else {
        return;
    };
    let mut paint = SkPaint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    frame.fill_rect(rect, &paint, Transform::identity(), None);
}

impl OverlayRenderer {
    pub fn new() -> OverlayRenderer {
        OverlayRenderer::default()
    }

    fn scene(&mut self, step: f64) -> Result<&Pixmap, RasterError> {
        let name = format!("scene-{step}");
        if !self.scenes.contains_key(&name) {
            let pixmap = decode(&name)?;
            self.scene_order.push_back(name.clone());
            while self.scene_order.len() > 2 {
                if let Some(old) = self.scene_order.pop_front() {
                    self.scenes.remove(&old);
                }
            }
            self.scenes.insert(name.clone(), pixmap);
        }
        Ok(&self.scenes[&name])
    }

    fn hud(&mut self) -> Result<&HudMasks, RasterError> {
        if self.hud.is_none() {
            let sheet = decode("hud")?;
            let data: Vec<[u8; 3]> = sheet
                .pixels()
                .iter()
                .map(|p| [p.red(), p.green(), p.blue()])
                .collect();
            self.hud = Some(HudMasks::Stock(data));
        }
        Ok(self.hud.as_ref().expect("маски HUD"))
    }

    /// Сетка ракурса `i` (`viewmodelMesh` 0.4.7) и его текстура.
    fn view(&mut self, i: usize) -> Result<&ViewMesh, RasterError> {
        match self.views.entry(i) {
            std::collections::hash_map::Entry::Occupied(_) => {
                if let Some(at) = self.view_order.iter().position(|&v| v == i) {
                    self.view_order.remove(at);
                    self.view_order.push_back(i);
                }
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(view_mesh(i)?);
                self.view_order.push_back(i);
            }
        }
        Ok(&self.views[&i])
    }

    fn trim_views(&mut self, keep: &[usize]) {
        while self.view_order.len() > VIEWS_KEPT {
            let Some(at) = self.view_order.iter().position(|v| !keep.contains(v)) else {
                break;
            };
            if let Some(old) = self.view_order.remove(at) {
                self.views.remove(&old);
            }
        }
    }

    /// Кадр превью ширины `width` (высота 900) для персонализации `value`, раздела
    /// `section` (`hud`, `crosshair`, `weapon`, `keys`, `misc`…) в момент `time`, с.
    pub fn render(
        &mut self,
        width: u32,
        value: &Value,
        section: &str,
        assets: &Assets,
        time: f64,
    ) -> Result<Pixmap, RasterError> {
        for (key, bytes) in &assets.fonts {
            canvas_text::register_font(key, bytes.clone());
        }
        let t = tables();
        let mut s = t.defaults.clone();
        if let Some(Value::Object(extra)) = value.get("extra") {
            for (k, v) in extra {
                s.insert(k.clone(), v.clone());
            }
        }
        let w = f64::from(width);
        let centre = shift_of(Side::Center, w);
        let world = number(s.get("fov_desired"), 75.0);
        let step = frame_for(&t.scenes, world);
        let k = js::math::min(4.0, tan(step) / tan(world));
        // Спрей на земле в 0.4.9 убран: у «Эффектов и спреев» своя сцена (ghost_scene).
        let _ = section;
        let key = format!("{width}|{step}|{world}");
        if !self.base.as_ref().is_some_and(|base| base.key == key) {
            self.base = None;
            let mut base =
                Pixmap::new(width.max(1), FRAME_HEIGHT).ok_or_else(|| err("frame", "холст"))?;
            // Кадры сняты в 16:9; на более широком экране то, чего в кадре нет, — размытая
            // растянутая сцена.
            if width > FRAME_WIDTH {
                let scene = self.scene(step)?.clone();
                fill_wide(&mut base, &scene);
            }
            {
                let scene = self.scene(step)?;
                raster::draw_image(
                    &mut base,
                    scene,
                    centre + CX - CX * k,
                    CY - CY * k,
                    FW * k,
                    FH * k,
                    Blend::Sprite,
                    1.0,
                );
            }
            self.base = Some(Base { key, pixels: base });
        }
        let mut frame = self
            .base
            .as_ref()
            .map(|base| base.pixels.clone())
            .ok_or_else(|| err("frame", "холст"))?;
        self.draw_weapon(&mut frame, &s, time, centre)?;

        let hud_enabled = truthy(value.get("hudEnabled"));
        let hud_color = if hud_enabled {
            rgb_of(value.get("hudColor")).unwrap_or([0, 0, 0])
        } else {
            STOCK
        };
        let hud_alpha = number(value.get("hudAlpha"), -1.0);
        let alpha = if hud_enabled && hud_alpha >= 0.0 {
            hud_alpha
        } else {
            -1.0
        };
        // Включён HUD-мод: его раскладка и маски, настройки персонализации — поверх, как в игре.
        let condensed = assets.hud_mod;
        let elements = if condensed {
            self.condensed_masks(assets.hud_mod_font.as_deref().unwrap_or(""))?
                .elements
                .clone()
        } else {
            ELEMENTS.to_vec()
        };
        for e in &elements {
            self.draw_element(&mut frame, e, &s, hud_color, alpha, assets, condensed)?;
        }
        self.draw_reticle(&mut frame, value, hud_color, centre)?;
        if truthy(s.get("gh_keyshud")) {
            draw_keys(&mut frame, &s, time);
        }
        draw_meters(&mut frame, &s, assets);
        Ok(frame)
    }

    /// Смешать часть HUD с кадром, как игра (`blend` 0.4.7): `source` — её прямоугольник
    /// в масках, `at` — сдвиг; `glyph(x, y)` — какие пиксели маски оставляют глифы.
    #[allow(clippy::too_many_arguments)]
    fn blend(
        &mut self,
        frame: &mut Pixmap,
        source: [i32; 4],
        at: (i32, i32),
        plate: bool,
        glyph: &dyn Fn(i32, i32) -> bool,
        key: &str,
        colour: [f32; 3],
        gain: f64,
        shade: f64,
        condensed: bool,
    ) -> Result<(), RasterError> {
        let fw = frame.width() as i32;
        let x0 = (source[0] + at.0).max(0);
        let y0 = (source[1] + at.1).max(0);
        let x1 = (source[2] + at.0).min(fw);
        let y1 = (source[3] + at.1).min(FRAME_HEIGHT as i32);
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        let set = match (&self.condensed, condensed) {
            (Some(c), true) => format!("condensed:{}", c.family),
            _ => "stock".to_owned(),
        };
        let id = format!(
            "{set}|{key}|{:?}|{}|{}|{plate}|{:?}|{gain}|{shade}",
            source, at.0, at.1, colour
        );
        if !self.parts.contains_key(&id) {
            let masks = match (&self.condensed, condensed) {
                (Some(c), true) => &c.masks,
                _ => self.hud()?,
            };
            let width = x1 - x0;
            let mut part = Part {
                x0,
                y0,
                width,
                where_: Vec::new(),
                mul: Vec::new(),
                add: Vec::new(),
            };
            for y in y0..y1 {
                for x in x0..x1 {
                    let (sx, sy) = (x - at.0, y - at.1);
                    if sx < 0 || sy < 0 || sx >= FRAME_WIDTH as i32 {
                        continue;
                    }
                    let i = (sy * FRAME_WIDTH as i32 + sx) as usize;
                    let p = if plate {
                        f64::from(masks.plate(i))
                    } else {
                        0.0
                    };
                    let glow_v = masks.glow(i);
                    let keep = glow_v > 0.0 && glyph(sx, sy);
                    if p == 0.0 && !keep {
                        continue;
                    }
                    let g = if keep {
                        f64::from(glow_v) * gain
                    } else {
                        0.0
                    };
                    part.where_.push(((y - y0) * width + (x - x0)) as usize);
                    let m = (1.0 - p)
                        * if keep {
                            1.0 - js::math::min(1.0, f64::from(masks.shade(i)) * shade)
                        } else {
                            1.0
                        };
                    part.mul.push(m as f32);
                    for c in colour {
                        part.add.push((f64::from(c) * g) as f32);
                    }
                }
            }
            if self.parts.len() > 96 {
                self.parts.clear();
            }
            self.parts.insert(id.clone(), part);
        }
        let t = tables();
        let part = &self.parts[&id];
        let px = frame.pixels_mut();
        for (n, &o) in part.where_.iter().enumerate() {
            let (dx, dy) = (
                (o % part.width as usize) as i32,
                (o / part.width as usize) as i32,
            );
            let at = ((part.y0 + dy) * fw + part.x0 + dx) as usize;
            let p = px[at];
            let m = f64::from(part.mul[n]);
            let ch = |v: u8, c: usize| {
                let lin = f64::from(t.linear[usize::from(v)]) * m + f64::from(part.add[n * 3 + c]);
                let step = js::math_round(lin * SRGB_STEPS as f64).clamp(0.0, SRGB_STEPS as f64);
                t.srgb[step as usize]
            };
            // Кадр непрозрачный: каналы и есть значения sRGB.
            let (r, g, b) = (ch(p.red(), 0), ch(p.green(), 1), ch(p.blue(), 2));
            if let Some(out) = tiny_skia::PremultipliedColorU8::from_rgba(r, g, b, 255) {
                px[at] = out;
            }
        }
        Ok(())
    }

    /// Оружие: своё поле зрения (`viewmodel_fov`), `viewmodel_offset_x/y/z` — вправо,
    /// вперёд и вверх в пространстве камеры; `cl_righthand 0` отражает картинку.
    fn draw_weapon(
        &mut self,
        frame: &mut Pixmap,
        s: &Map<String, Value>,
        time: f64,
        centre: f64,
    ) -> Result<(), RasterError> {
        let f = focal(number(s.get("viewmodel_fov"), 54.0));
        let right = if truthy(s.get("cl_righthand")) {
            1.0
        } else {
            -1.0
        };
        let dx = js::math::max(
            -5.0,
            js::math::min(5.0, number(s.get("viewmodel_offset_x"), 0.0)),
        );
        let dy = number(s.get("viewmodel_offset_y"), 0.0);
        let dz = number(s.get("viewmodel_offset_z"), 0.0);
        let bob = if time > 0.0 && truthy(s.get("viewmodel_bobbing")) {
            (time * 7.0).sin() * 5.0
        } else {
            0.0
        };
        let position = (dx + 5.0) * 2.0;
        let lo = position.floor() as usize;
        let hi = (lo + 1).min(20);
        let mix = position - lo as f64;
        let mut chosen: Vec<(usize, f64)> = [(lo, 1.0 - mix), (hi, mix)]
            .into_iter()
            .filter(|&(_, w)| w > 0.001)
            .collect();
        if chosen.is_empty() {
            chosen.push((lo, 1.0));
        }
        let keep: Vec<usize> = chosen.iter().map(|c| c.0).collect();
        let sum: f64 = chosen.iter().map(|c| c.1).sum();
        let composite = chosen.len() > 1;
        let uniforms = Uniforms {
            offset: [dx as f32, dy as f32, dz as f32],
            f: f as f32,
            hand: right as f32,
            bob: bob as f32,
        };
        for &(i, _) in &chosen {
            self.view(i)?;
        }
        // Рамка оружия — по всем вершинам ракурсов (0.4.7 читает из WebGL только её):
        // слой и буфер глубины — только на неё, а не на весь кадр 1600×900.
        let (mut x0, mut y0, mut x1, mut y1) = (FW, FH, 0.0f64, 0.0f64);
        for &(i, _) in &chosen {
            for p in &self.views[&i].points {
                let depth = js::math::max(1.0, f64::from(p[1]) + dy);
                let mut x = CX + f * (f64::from(p[0]) + dx) / depth;
                if right < 0.0 {
                    x = FW - x;
                }
                let y = CY - f * (f64::from(p[2]) + dz) / depth + bob;
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
        let x0 = js::math::max(0.0, x0.floor() - 2.0) as i32;
        let y0 = js::math::max(0.0, y0.floor() - 2.0) as i32;
        let x1 = js::math::min(FW, x1.ceil() + 2.0) as i32;
        let y1 = js::math::min(FH, y1.ceil() + 2.0) as i32;
        if x1 <= x0 || y1 <= y0 {
            self.trim_views(&keep);
            return Ok(());
        }
        let region = Region {
            x0,
            y0,
            width: x1 - x0,
            height: y1 - y0,
        };
        // Слой WebGL в рамке оружия: предумноженный RGBA.
        let size = (region.width * region.height) as usize;
        let mut layer = vec![[0u8; 4]; size];
        if composite {
            // Каждый ракурс проходит свой тест глубины в своём слое (оба — сразу), затем
            // слои складываются с весами, как кадровый буфер 0.4.7 (blendFunc ONE, ONE).
            let mut own: Vec<Vec<[u8; 4]>> = chosen.iter().map(|_| vec![[0u8; 4]; size]).collect();
            let views: Vec<&ViewMesh> = chosen.iter().map(|(i, _)| &self.views[i]).collect();
            std::thread::scope(|scope| {
                for (view, own) in views.iter().zip(own.iter_mut()) {
                    let (uniforms, region) = (&uniforms, &region);
                    scope.spawn(move || rasterize(view, uniforms, region, own));
                }
            });
            for (&(_, weight), own) in chosen.iter().zip(&own) {
                let k = weight / sum;
                for (dst, src) in layer.iter_mut().zip(own) {
                    if src[3] == 0 && src[0] == 0 {
                        continue;
                    }
                    for c in 0..4 {
                        let v = f64::from(src[c]) * k + f64::from(dst[c]);
                        dst[c] = v.round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        } else if let Some(&(i, _)) = chosen.first() {
            rasterize(&self.views[&i], &uniforms, &region, &mut layer);
        }
        self.trim_views(&keep);
        let Some(mut part) = Pixmap::new(region.width as u32, region.height as u32) else {
            return Ok(());
        };
        for (out, &[r, g, b, a]) in part.pixels_mut().iter_mut().zip(&layer) {
            if let Some(c) =
                tiny_skia::PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a)
            {
                *out = c;
            }
        }
        let at = centre.round() as i32;
        raster::blit(frame, &part, Area::of(&part), x0 + at, y0, Blend::Over);
        Ok(())
    }

    /// Маски HUD мода в том же виде, что стандартные (`condensedMasks` 0.4.9): таймер
    /// целиком, подписи и значки — пиксели стандартных масок на местах мода и ярче в
    /// `CONDENSED_ALPHA / STOCK_ALPHA` раз, плашки — стандартная плашка по 9 частям, цифры —
    /// шрифтом мода (`family`; пусто — похожий DIN из ОС). Готовятся один раз на шрифт.
    fn condensed_masks(&mut self, family: &str) -> Result<&Condensed, RasterError> {
        if self.condensed.as_ref().is_some_and(|c| c.family == family) {
            return Ok(self.condensed.as_ref().expect("маски мода"));
        }
        self.condensed = None;
        let stock = self.hud()?;
        let size = FRAME_WIDTH as usize * FRAME_HEIGHT as usize;
        let k = CONDENSED_ALPHA / STOCK_ALPHA;
        struct CustomMasks {
            glow: Vec<f32>,
            shade: Vec<f32>,
            plate: Vec<f32>,
        }
        let mut m = CustomMasks {
            glow: vec![0.0; size],
            shade: vec![0.0; size],
            plate: vec![0.0; size],
        };
        let (fw, fh) = (FRAME_WIDTH as i32, FRAME_HEIGHT as i32);
        let visit = |rect: [i32; 4], f: &mut dyn FnMut(i32, i32, usize)| {
            for y in rect[1].max(0)..rect[3].min(fh) {
                for x in rect[0].max(0)..rect[2].min(fw) {
                    f(x, y, (y * fw + x) as usize);
                }
            }
        };
        let stock_of = |id: &str| ELEMENTS.iter().find(|e| e.id == id).expect("элемент HUD");
        let copy_glyphs = |m: &mut CustomMasks, rect: [i32; 4], dx: i32, dy: i32| {
            visit(rect, &mut |x, y, i| {
                let (tx, ty) = (x + dx, y + dy);
                if tx < 0 || ty < 0 || tx >= fw || ty >= fh {
                    return;
                }
                let j = (ty * fw + tx) as usize;
                m.glow[j] = m.glow[j].max((f64::from(stock.glow(i)) * k).min(1.0) as f32);
                m.shade[j] = m.shade[j].max((f64::from(stock.shade(i)) * k).min(1.0) as f32);
            });
        };
        // Плашка здоровья стандартного HUD: углы как есть, середина тянется.
        let source = stock_of("health").plate;
        let corner = 10;
        let slice = |m: &mut CustomMasks, target: [i32; 4]| {
            let map = |v: i32, t0: i32, t1: i32, p0: i32, p1: i32| {
                if v < t0 + corner {
                    p0 + (v - t0)
                } else if v >= t1 - corner {
                    p1 - (t1 - v)
                } else {
                    p0 + corner
                }
            };
            visit(target, &mut |x, y, i| {
                let sx = map(x, target[0], target[2], source[0], source[2]);
                let sy = map(y, target[1], target[3], source[1], source[3]);
                m.plate[i] = m.plate[i].max(stock.plate((sy * fw + sx) as usize));
            });
        };
        // Яркость цифр — самая яркая точка цифр здоровья у стандартного HUD.
        let (mut peak, mut peak_shade) = (0f32, 0f32);
        visit(
            [source[0] + 90, source[1], source[2], source[3]],
            &mut |_, _, i| {
                let g = stock.glow(i);
                if g > peak {
                    peak = g;
                    peak_shade = stock.shade(i);
                }
            },
        );
        // tall в Source — высота ячейки GDI: winAscent + winDescent шрифта мода (1919 + 468
        // при 2048 на кегль). Числа шрифта, а не замеры: запасной шрифт встаёт в ту же ячейку.
        let metrics = |tall: f64, bold: bool| {
            let em = tall * 2048.0 / 2387.0;
            let mut families = Vec::new();
            if !family.is_empty() {
                families.push(family.to_owned());
            }
            // shortcut: «Bahnschrift SemiCondensed» — именованный вариант переменного шрифта, его
            // видит DirectWrite Chromium, но не fontique: без файла мода здесь встаёт Bahnschrift
            // обычной ширины (цифры чуть шире); с файлом мода (как в игре) — его DIN.
            families.extend(
                [
                    "Bahnschrift SemiCondensed",
                    "Arial Narrow",
                    "Bahnschrift",
                    "sans-serif",
                ]
                .map(String::from),
            );
            // `em.toFixed(2)`: кегль округлён до сотых.
            let size = (em * 100.0).round() / 100.0;
            let font = Font {
                weight: if bold { 700 } else { 400 },
                size: size as f32,
                families,
            };
            (em, tall * 1919.0 / 2387.0, font)
        };
        // Цифры в маску: scanlines 2 у HudNumbers мода — каждая вторая строка ячейки тусклее.
        let digits_mask =
            |m: &mut CustomMasks, text: &str, at: Digits, bold: bool, scanlines: bool| {
                let (_, baseline, font) = metrics(at.tall, bold);
                let pad = 4i32;
                let width = canvas_text::layout(text, &font).width;
                let w = f64::from(width).ceil() as i32 + pad * 2;
                let h = at.tall.ceil() as i32 + pad * 2;
                let ox = js::math_round(at.x) as i32 - pad;
                let oy = js::math_round(at.top) as i32 - pad;
                if let Some(mut cell) = Pixmap::new(w.max(1) as u32, h.max(1) as u32) {
                    canvas_text::text(
                        &mut cell,
                        text,
                        &font,
                        Place {
                            x: pad as f32,
                            y: (f64::from(pad) + baseline) as f32,
                            align: Align::Left,
                            baseline: Baseline::Alphabetic,
                            max_width: None,
                        },
                        Paint::Fill,
                        &Ink::Solid([255, 255, 255, 255]),
                        1.0,
                    );
                    for r in 0..h {
                        for q in 0..w {
                            let mut a =
                                f64::from(cell.pixels()[(r * w + q) as usize].alpha()) / 255.0;
                            if a == 0.0 {
                                continue;
                            }
                            // `(r - pad) % 2` у JS: -1 тоже нечётная.
                            if scanlines && (r - pad) % 2 != 0 {
                                a *= 0.5;
                            }
                            let (tx, ty) = (ox + q, oy + r);
                            if tx < 0 || ty < 0 || tx >= fw || ty >= fh {
                                continue;
                            }
                            let j = (ty * fw + tx) as usize;
                            m.glow[j] = m.glow[j].max((a * f64::from(peak) * k).min(1.0) as f32);
                            m.shade[j] =
                                m.shade[j].max((a * f64::from(peak_shade) * k).min(1.0) as f32);
                        }
                    }
                }
                [ox, oy, ox + w, oy + h]
            };
        // Для gh_hud_font: где шрифт игрока пишет цифры (базовая линия и кегль ячейки мода).
        let font_place = |at: Digits| {
            let (em, baseline, _) = metrics(at.tall, false);
            (
                js::math_round(at.x),
                js::math_round(at.top + baseline),
                js::math_round(em),
            )
        };
        let union = |rects: &[[i32; 4]]| {
            [
                rects.iter().map(|r| r[0]).min().unwrap_or(0) - 2,
                rects.iter().map(|r| r[1]).min().unwrap_or(0) - 2,
                rects.iter().map(|r| r[2]).max().unwrap_or(0) + 2,
                rects.iter().map(|r| r[3]).max().unwrap_or(0) + 2,
            ]
        };

        let timer = stock_of("timer");
        visit(timer.bbox, &mut |_, _, i| {
            m.glow[i] = stock.glow(i);
            m.shade[i] = stock.shade(i);
            m.plate[i] = stock.plate(i);
        });
        let mut elements = vec![timer.clone()];
        for def in condensed_defs() {
            let e = stock_of(def.id);
            let dx = js::math_round(def.text[0] - def.stock_text[0]) as i32;
            let dy = js::math_round(def.text[1] - def.stock_text[1]) as i32;
            let shift = |r: &[i32; 4]| [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy];
            for r in e.labels.iter().chain(e.icons.iter()) {
                copy_glyphs(&mut m, *r, dx, dy);
            }
            let labels: Vec<[i32; 4]> = e.labels.iter().map(shift).collect();
            let icons: Vec<[i32; 4]> = e.icons.iter().map(shift).collect();
            let mut plate = def.plate;
            if def.stock_plate {
                plate = Some(e.plate);
                visit(e.bbox, &mut |_, _, i| m.plate[i] = stock.plate(i));
            } else if let Some(p) = plate {
                slice(&mut m, p);
            }
            let place = plate.or(def.anchor).unwrap_or(e.plate);
            let mut parts = vec![place];
            parts.extend(labels.iter().copied());
            parts.extend(icons.iter().copied());
            parts.push(digits_mask(&mut m, e.value, def.digits, false, true));
            let reserve = match (def.reserve, e.reserve) {
                (Some(at), Some((value, ..))) => {
                    parts.push(digits_mask(&mut m, value, at, true, false));
                    let (x, y, size) = font_place(at);
                    Some((value, x, y, size))
                }
                _ => None,
            };
            let (x, y, size) = font_place(def.digits);
            elements.push(Element {
                id: e.id,
                key: e.key,
                side: e.side,
                moved: 0.0,
                bbox: union(&parts),
                plate: place,
                anchor: def.anchor,
                labels: Cow::Owned(labels),
                icons: Cow::Owned(icons),
                caption: e
                    .caption
                    .map(|(text, x, y)| (text, x + f64::from(dx), y + f64::from(dy))),
                value: e.value,
                digits: (x, y, size, Align::Left),
                reserve,
            });
        }
        self.condensed = Some(Condensed {
            family: family.to_owned(),
            masks: HudMasks::Custom {
                glow: m.glow,
                shade: m.shade,
                plate: m.plate,
            },
            elements,
        });
        Ok(self.condensed.as_ref().expect("маски мода"))
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_element(
        &mut self,
        frame: &mut Pixmap,
        e: &Element,
        s: &Map<String, Value>,
        c: [u8; 3],
        alpha: f64,
        assets: &Assets,
        condensed: bool,
    ) -> Result<(), RasterError> {
        if (e.id == "timer" && !truthy(s.get("gh_timer")))
            || (e.id == "speed" && !truthy(s.get("gh_speedmeter")))
        {
            return Ok(());
        }
        let width = f64::from(frame.width());
        let plate_f = e.plate.map(f64::from);
        let anchor = e.anchor.unwrap_or(e.plate).map(f64::from);
        let Some(at) = offset(s, e.key, anchor, width, e.side, e.moved) else {
            return Ok(());
        };
        let at = (at.0 as i32, at.1 as i32);
        let plate = match e.id {
            "timer" => truthy(s.get("gh_timer_bg")),
            "speed" => truthy(s.get("gh_speedmeter_bg")) && truthy(s.get("gh_hud_bg")),
            _ => truthy(s.get("gh_hud_bg")),
        };
        let labels = truthy(s.get("gh_hud_labels"))
            && (e.id != "speed" || truthy(s.get("gh_speedmeter_label")));
        let font = assets
            .font_family
            .clone()
            .unwrap_or_else(|| font_name(s.get("gh_hud_font")));
        // Маски сняты при альфе схемы: у HUD мода она своя (CONDENSED_ALPHA).
        let base = if condensed {
            CONDENSED_ALPHA
        } else {
            STOCK_ALPHA
        };
        let gain = if alpha >= 0.0 { alpha / base } else { 1.0 };
        let shade = if alpha >= 0.0 {
            shade_gain(alpha) * STOCK_ALPHA / base
        } else {
            1.0
        };
        let show_icons = truthy(s.get("gh_hud_icons"));
        let icons: &[[i32; 4]] = if show_icons { &e.icons } else { &[] };
        let mut hidden: Vec<[i32; 4]> = Vec::new();
        if !labels {
            hidden.extend_from_slice(&e.labels);
        }
        if !show_icons {
            hidden.extend_from_slice(&e.icons);
        }
        let has_font = !font.is_empty();
        let glyph = |x: i32, y: i32| {
            if has_font {
                inside(icons, x, y)
            } else if !hidden.is_empty() {
                !inside(&hidden, x, y)
            } else {
                true
            }
        };
        let key = format!(
            "{},{},{},{}",
            e.id,
            if has_font { "font" } else { "" },
            u8::from(labels),
            u8::from(show_icons)
        );
        let t = tables();
        let colour = c.map(|v| t.linear[usize::from(v)]);
        self.blend(
            frame, e.bbox, at, plate, &glyph, &key, colour, gain, shade, condensed,
        )?;
        if has_font {
            draw_hud_text(frame, e, s, c, js::math::min(1.0, gain), labels, &font, at);
        }
        if e.id == "timer" && truthy(s.get("gh_timer_mirror")) {
            let (ax, ay) = (f64::from(at.0), f64::from(at.1));
            let left = Place {
                x: (ax + plate_f[0] + 6.0) as f32,
                y: (ay + plate_f[1] - 5.0) as f32,
                align: Align::Left,
                baseline: Baseline::Alphabetic,
                max_width: None,
            };
            canvas_text::text(
                frame,
                "LIVESPLIT",
                &Font::parse("bold 10px Arial"),
                left,
                Paint::Fill,
                &Ink::Solid([c[0], c[1], c[2], 255]),
                1.0,
            );
            if truthy(s.get("gh_timer_mirror_splits")) {
                let weight = if truthy(s.get("gh_timer_mirror_splits_bold")) {
                    "bold"
                } else {
                    "normal"
                };
                canvas_text::text(
                    frame,
                    "\u{2212}00:01.247",
                    &Font::parse(&format!("{weight} 14px Arial")),
                    Place {
                        y: (ay + plate_f[3] + 17.0) as f32,
                        ..left
                    },
                    Paint::Fill,
                    &Ink::Solid([0x78, 0xd8, 0x8c, 255]),
                    1.0,
                );
            }
        }
        Ok(())
    }

    fn draw_reticle(
        &mut self,
        frame: &mut Pixmap,
        v: &Value,
        hud_color: [u8; 3],
        centre: f64,
    ) -> Result<(), RasterError> {
        let t = tables();
        let none = (centre as i32, 0);
        let lin = |c: [u8; 3]| c.map(|v| t.linear[usize::from(v)]);
        let dots = |x: i32, y: i32| inside(&[DOTS], x, y);
        if !truthy(v.get("hideQuickInfo")) {
            self.blend(
                frame,
                QUICKINFO,
                none,
                false,
                &|x, y| !dots(x, y),
                "quickinfo",
                lin(hud_color),
                1.0,
                1.0,
                false,
            )?;
        }
        let cross_enabled = truthy(v.get("crossEnabled"));
        let affects = truthy(v.get("hudEnabled")) && truthy(v.get("hudAffectsCrosshair"));
        if !cross_enabled || !truthy(v.get("crossHideDefault")) {
            let dot = if affects {
                rgb_of(v.get("hudColor")).unwrap_or([0, 0, 0])
            } else {
                DOT_STOCK
            };
            self.blend(
                frame,
                DOTS,
                none,
                false,
                &|_, _| true,
                "dots",
                lin(dot),
                1.0,
                1.0,
                false,
            )?;
        }
        if !cross_enabled {
            return Ok(());
        }
        let c = rgb_of(if affects {
            v.get("hudColor")
        } else {
            v.get("crossColor")
        })
        .unwrap_or([0, 0, 0]);
        let x = f64::from(frame.width()) / 2.0;
        let y = FH / 2.0;
        let alpha = number(v.get("crossAlpha"), 255.0) / 255.0;
        let thickness = number(v.get("crossThickness"), 2.0);
        let length = number(v.get("crossSize"), 10.0);
        let gap = js::math::max(0.0, number(v.get("crossGap"), 3.0));
        let outline = if truthy(v.get("crossOutline")) {
            number(v.get("crossOutlineThickness"), 1.0)
        } else {
            0.0
        };
        let rect = |frame: &mut Pixmap, xx: f64, yy: f64, w: f64, h: f64, rgb: [u8; 3]| {
            if outline != 0.0 {
                fill_rect(
                    frame,
                    xx - outline,
                    yy - outline,
                    w + outline * 2.0,
                    h + outline * 2.0,
                    sk([0, 0, 0], alpha),
                );
            }
            fill_rect(frame, xx, yy, w, h, sk(rgb, alpha));
        };
        if truthy(v.get("crossTopLine")) && length != 0.0 {
            rect(
                frame,
                x - thickness / 2.0,
                y - gap - length,
                thickness,
                length,
                c,
            );
        }
        if truthy(v.get("crossBottomLine")) && length != 0.0 {
            rect(frame, x - thickness / 2.0, y + gap, thickness, length, c);
        }
        if truthy(v.get("crossLeftLine")) && length != 0.0 {
            rect(
                frame,
                x - gap - length,
                y - thickness / 2.0,
                length,
                thickness,
                c,
            );
        }
        if truthy(v.get("crossRightLine")) && length != 0.0 {
            rect(frame, x + gap, y - thickness / 2.0, length, thickness, c);
        }
        let radius = js::value::to_number(v.get("crossCircleRadius"));
        if radius > 0.0 {
            let mut pb = PathBuilder::new();
            pb.push_circle(x as f32, y as f32, radius as f32);
            if let Some(path) = pb.finish() {
                let mut stroke_with = |width: f64, color: Color| {
                    let mut paint = SkPaint::default();
                    paint.set_color(color);
                    paint.anti_alias = true;
                    let stroke = Stroke {
                        width: width as f32,
                        ..Stroke::default()
                    };
                    frame.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                };
                if outline != 0.0 {
                    stroke_with(thickness + outline * 2.0, sk([0, 0, 0], alpha));
                }
                stroke_with(thickness, sk(c, alpha));
            }
        }
        let dot = js::value::to_number(v.get("crossDotSize"));
        if dot > 0.0 {
            let dot_color = rgb_of(v.get("crossDotColor").filter(|c| js::truthy(c))).unwrap_or(c);
            rect(frame, x - dot / 2.0, y - dot / 2.0, dot, dot, dot_color);
        }
        Ok(())
    }
}

/// Сетка ракурса `i` (`viewmodelMesh` 0.4.7): вершины по цветному ракурсу и карте глубины.
fn view_mesh(i: usize) -> Result<ViewMesh, RasterError> {
    let t = tables();
    let info = t
        .meta
        .viewmodels
        .get(i)
        .ok_or_else(|| err("meta.json", "нет ракурса"))?;
    let texture = decode(&format!("view-{i}"))?;
    let map = decode(&format!("depth-{i}"))?;
    let f = focal(t.meta.viewmodel.fov);
    let [inv0, inv1] = t.meta.viewmodel.inv;
    let (w, h) = (texture.width() as usize, texture.height() as usize);
    let (cw, ch) = (map.width() as usize, map.height() as usize);
    let cols = (w - 1).div_ceil(MESH) + 1;
    let rows = (h - 1).div_ceil(MESH) + 1;
    let mut points = Vec::with_capacity(cols * rows);
    let depth = map.pixels();
    for r in 0..rows {
        for q in 0..cols {
            let x = (q * MESH).min(w - 1) as f64;
            let y = (r * MESH).min(h - 1) as f64;
            let mx = (js::math_round(x / (w - 1) as f64 * (cw - 1) as f64) as usize).min(cw - 1);
            let my = (js::math_round(y / (h - 1) as f64 * (ch - 1) as f64) as usize).min(ch - 1);
            let p = depth[my * cw + mx];
            let raw = (f64::from(p.red()) * 256.0 + f64::from(p.green())) / 65535.0;
            let d = 1.0 / (inv0 + raw * (inv1 - inv0));
            points.push([
                ((x + info.at[0] - CX) / f * d - info.x) as f32,
                d as f32,
                ((CY - y - info.at[1]) / f * d) as f32,
                ((x + 0.5) / w as f64) as f32,
                ((y + 0.5) / h as f64) as f32,
            ]);
        }
    }
    // Клетка живая, если в окне её текселей (с запасом билинейной выборки) есть
    // альфа не меньше 0.02: иначе все её фрагменты отброшены шейдером.
    let alpha = texture.pixels();
    let threshold = (0.02f32 * 255.0).floor() as u8;
    let mut live = vec![false; (cols - 1) * (rows - 1)];
    for r in 0..rows - 1 {
        for q in 0..cols - 1 {
            let x0 = (q * MESH).saturating_sub(1);
            let x1 = ((q + 1) * MESH + 1).min(w - 1);
            let y0 = (r * MESH).saturating_sub(1);
            let y1 = ((r + 1) * MESH + 1).min(h - 1);
            live[r * (cols - 1) + q] =
                (y0..=y1).any(|y| (x0..=x1).any(|x| alpha[y * w + x].alpha() > threshold));
        }
    }
    Ok(ViewMesh {
        cols,
        rows,
        points,
        texture,
        live,
    })
}

/// Поле за кадром на широком экране: сцена, растянутая на ширину кадра, `brightness(0.55)`.
/// `blur(18px)` 0.4.7 не повторяется: без размытия, только затемнение (вопрос 1 плана GPUI).
fn fill_wide(frame: &mut Pixmap, scene: &Pixmap) {
    let (w, h) = (frame.width(), frame.height());
    let Some(mut stretched) = Pixmap::new(w, h) else {
        return;
    };
    let paint = PixmapPaint {
        quality: tiny_skia::FilterQuality::Bicubic,
        ..PixmapPaint::default()
    };
    let transform = Transform::from_scale(
        w as f32 / scene.width() as f32,
        h as f32 / scene.height() as f32,
    );
    stretched.draw_pixmap(0, 0, scene.as_ref(), &paint, transform, None);
    for (out, p) in frame.pixels_mut().iter_mut().zip(stretched.pixels()) {
        let dim = |c: u8| (f32::from(c) * 0.55).round().clamp(0.0, 255.0) as u8;
        if let Some(c) = tiny_skia::PremultipliedColorU8::from_rgba(
            dim(p.red()),
            dim(p.green()),
            dim(p.blue()),
            255,
        ) {
            *out = c;
        }
    }
}

/// Униформы вершинного шейдера оружия.
struct Uniforms {
    offset: [f32; 3],
    f: f32,
    hand: f32,
    bob: f32,
}

/// Часть кадра, в которой растеризуется оружие (px кадра 1600×900).
struct Region {
    x0: i32,
    y0: i32,
    width: i32,
    height: i32,
}

/// Растеризовать ракурс в слой части `region` кадра (предумноженный RGBA): вершинный
/// шейдер 0.4.7, тест глубины `LESS` (16 бит), билинейная выборка с `CLAMP_TO_EDGE`,
/// `discard` при альфе < 0.02.
fn rasterize(view: &ViewMesh, u: &Uniforms, region: &Region, layer: &mut [[u8; 4]]) {
    let (fw, fh) = (FRAME_WIDTH as f32, FRAME_HEIGHT as f32);
    let screen: Vec<[f32; 5]> = view
        .points
        .iter()
        .map(|p| {
            let px = p[0] + u.offset[0];
            let py = (p[1] + u.offset[1]).max(1.0);
            let pz = p[2] + u.offset[2];
            let mut sx = fw / 2.0 + u.f * px / py;
            let sy = fh / 2.0 - u.f * pz / py + u.bob;
            if u.hand <= 0.0 {
                sx = fw - sx;
            }
            [sx, sy, (py / 200.0).clamp(0.0, 1.0), p[3], p[4]]
        })
        .collect();
    let mut depth = vec![u16::MAX; layer.len()];
    let tex = &view.texture;
    let (tw, th) = (tex.width() as i32, tex.height() as i32);
    let texels = tex.pixels();
    let sample = |u: f32, v: f32| -> [f32; 4] {
        let x = u * tw as f32 - 0.5;
        let y = v * th as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: i32, yi: i32| {
            let p = texels[(yi.clamp(0, th - 1) * tw + xi.clamp(0, tw - 1)) as usize];
            [
                f32::from(p.red()),
                f32::from(p.green()),
                f32::from(p.blue()),
                f32::from(p.alpha()),
            ]
        };
        let (xi, yi) = (x0 as i32, y0 as i32);
        let (a, b, c, d) = (
            at(xi, yi),
            at(xi + 1, yi),
            at(xi, yi + 1),
            at(xi + 1, yi + 1),
        );
        let mut out = [0f32; 4];
        for k in 0..4 {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            out[k] = (top + (bottom - top) * fy) / 255.0;
        }
        out
    };
    let cols = view.cols;
    let mut triangles = Vec::new();
    for r in 0..view.rows - 1 {
        for q in 0..cols - 1 {
            if !view.live[r * (cols - 1) + q] {
                continue;
            }
            let n = r * cols + q;
            for tri in [[n, n + 1, n + cols], [n + 1, n + cols + 1, n + cols]] {
                triangles.push(tri.map(|i| screen[i]));
            }
        }
    }
    // Полосы строк — по потокам: у каждого пикселя те же треугольники в том же порядке,
    // поэтому кадр от числа потоков не зависит.
    let width = region.width.max(1) as usize;
    let bands = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 4)
        .min((region.height.max(1) as usize).div_ceil(64));
    let rows_per_band = (region.height.max(1) as usize).div_ceil(bands);
    let chunk = rows_per_band * width;
    // Треугольники полосы — в исходном порядке.
    let mut per_band: Vec<Vec<usize>> = vec![Vec::new(); bands];
    for (i, [a, b, c]) in triangles.iter().enumerate() {
        let top = (a[1].min(b[1]).min(c[1]).floor() as i32 - region.y0).max(0) as usize;
        let bottom = (a[1].max(b[1]).max(c[1]).ceil() as i32 - region.y0).max(0) as usize;
        let first = (top / rows_per_band).min(bands - 1);
        let last = (bottom / rows_per_band).min(bands - 1);
        for list in &mut per_band[first..=last] {
            list.push(i);
        }
    }
    let run = |band: usize, layer: &mut [[u8; 4]], depth: &mut [u16]| {
        let y0 = region.y0 + (band * rows_per_band) as i32;
        let rows = (layer.len() / width) as i32;
        let part = Region {
            x0: region.x0,
            y0,
            width: region.width,
            height: rows,
        };
        for &i in &per_band[band] {
            let [a, b, c] = triangles[i];
            fill_triangle(a, b, c, &part, depth, layer, &sample);
        }
    };
    if bands <= 1 {
        run(0, layer, &mut depth);
        return;
    }
    std::thread::scope(|scope| {
        for (band, (layer, depth)) in layer
            .chunks_mut(chunk)
            .zip(depth.chunks_mut(chunk))
            .enumerate()
        {
            let run = &run;
            scope.spawn(move || run(band, layer, depth));
        }
    });
}

fn fill_triangle(
    a: [f32; 5],
    b: [f32; 5],
    c: [f32; 5],
    region: &Region,
    depth: &mut [u16],
    layer: &mut [[u8; 4]],
    sample: &impl Fn(f32, f32) -> [f32; 4],
) {
    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if area == 0.0 || !area.is_finite() {
        return;
    }
    let min_x = (a[0].min(b[0]).min(c[0]).floor().max(0.0) as i32).max(region.x0);
    let max_x = (a[0].max(b[0]).max(c[0]).ceil() as i32)
        .min(FRAME_WIDTH as i32 - 1)
        .min(region.x0 + region.width - 1);
    let min_y = (a[1].min(b[1]).min(c[1]).floor().max(0.0) as i32).max(region.y0);
    let max_y = (a[1].max(b[1]).max(c[1]).ceil() as i32)
        .min(FRAME_HEIGHT as i32 - 1)
        .min(region.y0 + region.height - 1);
    if max_x < min_x || max_y < min_y {
        return;
    }
    let inv = 1.0 / area;
    for y in min_y..=max_y {
        let py = y as f32 + 0.5;
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let w0 = ((b[0] - px) * (c[1] - py) - (b[1] - py) * (c[0] - px)) * inv;
            let w1 = ((c[0] - px) * (a[1] - py) - (c[1] - py) * (a[0] - px)) * inv;
            let w2 = 1.0 - w0 - w1;
            // Пиксель на ребре принадлежит одному из двух треугольников.
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 || (w2 == 0.0 && w1 == 0.0) {
                continue;
            }
            let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
            let z16 = (z * 65535.0).round() as u16;
            let at = ((y - region.y0) * region.width + x - region.x0) as usize;
            if z16 >= depth[at] {
                continue;
            }
            let u = w0 * a[3] + w1 * b[3] + w2 * c[3];
            let v = w0 * a[4] + w1 * b[4] + w2 * c[4];
            let col = sample(u, v);
            if col[3] < 0.02 {
                continue;
            }
            depth[at] = z16;
            layer[at] = col.map(|k| (k * 255.0).round().clamp(0.0, 255.0) as u8);
        }
    }
}

/// Шрифт HUD игрока (`gh_hud_font`): только тогда масштаб, насыщенность и обводка
/// шрифта что-то меняют.
#[allow(clippy::too_many_arguments)]
fn draw_hud_text(
    frame: &mut Pixmap,
    e: &Element,
    s: &Map<String, Value>,
    c: [u8; 3],
    opacity: f64,
    labels: bool,
    font: &str,
    at: (i32, i32),
) {
    let scale = number(s.get("gh_hud_fontscale"), 1.0);
    let weight = js::value::to_number(s.get("gh_hud_fontweight"));
    let weight = if weight > 0.0 { weight } else { 400.0 };
    let outline = truthy(s.get("gh_hud_fontoutline"));
    let (ax, ay) = (f64::from(at.0), f64::from(at.1));
    let mut text = |value: &str, x: f64, y: f64, size: f64, align: Align| {
        let font = Font::parse(&format!(
            "{weight} {}px \"{font}\", sans-serif",
            js::math_round(size * scale)
        ));
        let place = Place {
            x: (ax + x) as f32,
            y: (ay + y) as f32,
            align,
            baseline: Baseline::Alphabetic,
            max_width: None,
        };
        if outline {
            let width = js::math::max(2.0, size * scale / 12.0);
            canvas_text::text(
                frame,
                value,
                &font,
                place,
                Paint::Stroke(width as f32),
                &Ink::Solid([0, 0, 0, (0.85f32 * 255.0).round() as u8]),
                opacity as f32,
            );
        }
        canvas_text::text(
            frame,
            value,
            &font,
            place,
            Paint::Fill,
            &Ink::Solid([c[0], c[1], c[2], 255]),
            opacity as f32,
        );
    };
    if labels && let Some((caption, x, y)) = e.caption {
        text(caption, x, y, 12.0, Align::Left);
    }
    let (x, y, size, align) = e.digits;
    text(e.value, x, y, size, align);
    if let Some((value, x, y, size)) = e.reserve {
        text(value, x, y, size, Align::Left);
    }
}

const FULL_KEYS: [&str; 6] = [
    "ESC  F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12  Prt Scr Pau",
    "` 1 2 3 4 5 6 7 8 9 0 - = BKSP  INS HOME PGUP  NUM / * -",
    "TAB Q W E R T Y U I O P [ ] \\  DEL END PGDN  7 8 9 +",
    "CAPS A S D F G H J K L ; ' ENTER       4 5 6 +",
    "SHIFT Z X C V B N M , . / SHIFT     \u{2191}    1 2 3 ENTER",
    "CTRL WIN ALT SPACE SPACE SPACE SPACE ALT WIN MENU CTRL    \u{2190} \u{2193} \u{2192}   0 0 . ENTER",
];
const SHORT_KEYS: [&str; 3] = [" W +u", "A S D", "Duck Jump LM RM"];

/// Панель клавиш — свой серый вид, цвет HUD её не касается; без подложки видны только
/// нажатые клавиши.
fn draw_keys(frame: &mut Pixmap, s: &Map<String, Value>, time: f64) {
    let full = truthy(s.get("gh_keyshud_full"));
    let size = js::value::to_number(s.get(if full {
        "gh_keyshud_full_size"
    } else {
        "gh_keyshud_size"
    })) * UNIT;
    let gap = js::value::to_number(s.get("gh_keyshud_padding")) * UNIT;
    let rows: &[&str] = if full { &FULL_KEYS } else { &SHORT_KEYS };
    let columns = if full {
        rows.iter().map(|r| r.split(' ').count()).max().unwrap_or(0) as f64
    } else {
        4.0
    };
    let mouse = truthy(s.get("gh_keyshud_mouse"));
    let wheel = truthy(s.get("gh_keyshud_wheel"));
    let bg = truthy(s.get("gh_keyshud_bg"));
    let bbox = [
        0.0,
        0.0,
        columns * (size + gap) + if mouse || wheel { size * 4.0 } else { 0.0 },
        rows.len() as f64 * (size + gap),
    ];
    let width = f64::from(frame.width());
    let Some((px, py)) = offset(s, "gh_keyshud", bbox, width, Side::Left, 0.0) else {
        return;
    };
    let font = Font::parse(&format!("bold {}px Arial", js::math::max(8.0, size * 0.42)));
    let key = |frame: &mut Pixmap, label: &str, x: f64, y: f64, w: f64, h: f64, pressed: bool| {
        if !bg && !pressed {
            return;
        }
        let color = if pressed {
            sk([235, 235, 235], 0.85)
        } else {
            sk([205, 205, 205], 0.6)
        };
        fill_rect(frame, px + x, py + y, w, h, color);
        if !label.is_empty() {
            canvas_text::text(
                frame,
                label,
                &font,
                Place {
                    x: (px + x + w / 2.0) as f32,
                    y: (py + y + h / 2.0) as f32,
                    align: Align::Center,
                    baseline: Baseline::Middle,
                    max_width: Some((w - 3.0) as f32),
                },
                Paint::Fill,
                &Ink::Solid([0x2c, 0x2c, 0x2c, 255]),
                1.0,
            );
        }
    };
    for (yy, row) in rows.iter().enumerate() {
        for (xx, label) in row.split(' ').enumerate() {
            if label.is_empty() {
                continue;
            }
            let pressed = time != 0.0 && matches!(label, "W" | "Jump" | "SPACE");
            key(
                frame,
                label,
                xx as f64 * (size + gap),
                yy as f64 * (size + gap),
                size,
                size,
                pressed,
            );
        }
    }
    let mx = columns * (size + gap) + 4.0;
    if wheel {
        for (i, label) in ["M\u{2191}", "M\u{2193}", "M"].iter().enumerate() {
            key(frame, label, mx, i as f64 * (size + gap), size, size, false);
        }
    }
    if mouse {
        let xx = mx + size + gap;
        let side = size * 2.5;
        key(frame, "", xx, 0.0, side, side, false);
        let (cx, cy) = (px + xx + side / 2.0, py + side / 2.0);
        fill_rect(
            frame,
            cx - 2.0,
            cy - 2.0,
            4.0,
            4.0,
            sk([255, 255, 255], 1.0),
        );
        if time != 0.0 {
            let length = js::math::min(
                size,
                js::value::to_number(s.get("gh_keyshud_mouse_scale")) * 3.0,
            );
            let mut pb = PathBuilder::new();
            pb.move_to(cx as f32, cy as f32);
            pb.line_to(
                (cx + (time * 3.0).sin() * length) as f32,
                (cy + (time * 3.0).cos() * length) as f32,
            );
            if let Some(path) = pb.finish() {
                let mut paint = SkPaint::default();
                paint.set_color(sk([255, 255, 255], 1.0));
                paint.anti_alias = true;
                frame.stroke_path(
                    &path,
                    &paint,
                    &Stroke::default(),
                    Transform::identity(),
                    None,
                );
            }
        }
    }
}

/// `cl_showfps` / `cl_showpos`, как их печатает движок в правом верхнем углу.
fn draw_meters(frame: &mut Pixmap, s: &Map<String, Value>, assets: &Assets) {
    let left = 1301.0 + shift_of(Side::Right, f64::from(frame.width()));
    let draw =
        |frame: &mut Pixmap, prefix: &str, lines: &[&str], y: f64, step: f64, color: [u8; 3]| {
            let own = match prefix {
                "gh_showfps" => assets.showfps.clone(),
                _ => assets.showpos.clone(),
            };
            let family = own
                .filter(|f| !f.is_empty())
                .or_else(|| {
                    Some(font_name(s.get(&format!("{prefix}_font")))).filter(|f| !f.is_empty())
                })
                .unwrap_or_else(|| "Verdana".to_owned());
            let size = js::value::to_number(s.get(&format!("{prefix}_fontsize")));
            let size = if size == 0.0 || size.is_nan() {
                14.0
            } else {
                size
            };
            let weight = s
                .get(&format!("{prefix}_fontweight"))
                .filter(|w| js::truthy(w))
                .map_or_else(|| "700".to_owned(), js::to_string);
            let font = Font::parse(&format!(
                "{weight} {size}px \"{family}\", Verdana, sans-serif"
            ));
            let outline = truthy(s.get(&format!("{prefix}_fontoutline")));
            for (i, line) in lines.iter().enumerate() {
                let yy = y + i as f64 * js::math::max(step, size + 6.0);
                let place = Place {
                    x: left as f32,
                    y: yy as f32,
                    align: Align::Left,
                    baseline: Baseline::Alphabetic,
                    max_width: None,
                };
                if outline {
                    canvas_text::text(
                        frame,
                        line,
                        &font,
                        place,
                        Paint::Stroke(2.0),
                        &Ink::Solid([0, 0, 0, (0.9f32 * 255.0).round() as u8]),
                        1.0,
                    );
                }
                canvas_text::text(
                    frame,
                    line,
                    &font,
                    place,
                    Paint::Fill,
                    &Ink::Solid([color[0], color[1], color[2], 255]),
                    1.0,
                );
            }
        };
    let fps = number(s.get("cl_showfps"), 0.0);
    if fps > 0.0 {
        let line = if fps == 2.0 {
            "292 fps [49, 303] 3.4 ms on maps/d2_coast_09.bsp"
        } else {
            "297 fps on maps/d2_coast_09.bsp"
        };
        draw(frame, "gh_showfps", &[line], 19.0, 0.0, [0x3c, 0xff, 0x2c]);
    }
    if truthy(s.get("cl_showpos")) {
        draw(
            frame,
            "gh_showpos",
            &[
                "pos:  -182.08 8441.09 -703.97",
                "ang:  8.98 -93.21 0.00",
                "vel:  0.00",
            ],
            50.0,
            25.0,
            [255, 255, 255],
        );
    }
}
