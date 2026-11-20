//! Превью персонализации HL1 (`hl1-overlay-renderer.js` и `hl1-crosshair.js` 0.4.7,
//! задача 33): слои — кадры настоящей игры (Half-Life WON 2005 + Bunnymod XT, c2a5,
//! 1600×900), процесс игры не запускается.
//!
//! Игра рисует HUD и панели BXT, прибавляя цвет × спрайт к картинке в значениях sRGB:
//! `hud.png` — спрайты HUD белым, `panels.png` — цифры BXT, поэтому кадр — картинка +
//! цвет × маска. `hud-stock.png` — то, что прибавляет стандартный HUD, `crosshair.png` —
//! прицел оружия (он сохраняет свой цвет и ложится поверх всего). Картинка снята при
//! нескольких полях зрения: вид — ближайший более широкий кадр, увеличенный от центра.
//! Оружие снято в игре при тех же полях зрения и пяти шагах `bxt_viewmodel_ofs_forward`
//! (−20, −10, 0, 10, 20; 0.4.9): сдвиг вперёд показывает руки и остальную модель, как игра.
//! Полупрозрачное (`bxt_viewmodel_semitransparent`) прибавляется.
//!
//! Отличие от 0.4.7: слой другого поля зрения декодируется сразу (в потоке превью), а не
//! подменяется ближайшим загруженным до загрузки — виден сразу конечный кадр.

use super::preview_assets as assets;
use super::raster::{self, Area, Blend, RasterError};
use hlsr_core::js::{self, value::to_number};
use hlsr_delivery::hl1_pack_preview::{self, Command, PackMeta};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::f64::consts::PI;
use std::sync::OnceLock;
use tiny_skia::Pixmap;

pub const FRAME_WIDTH: u32 = 1600;
pub const FRAME_HEIGHT: u32 = 900;
const CX: f64 = FRAME_WIDTH as f64 / 2.0;
const CY: f64 = FRAME_HEIGHT as f64 / 2.0;
/// Панели BXT и HUD без своего цвета.
const BXT_STOCK: [u8; 3] = [255, 160, 0];
/// Цвет HUD пака AG без своего цвета.
const PACK_STOCK: [u8; 3] = [100, 63, 0];

/// `meta-weapons.json` 0.4.9: шаги сдвига, поля зрения и где лежит каждый слой в кадре
/// (`weapon`/`semi` → поле зрения → шаг). Нет записи — оружия при этом сдвиге не видно.
#[derive(Debug, Deserialize)]
struct WeaponMeta {
    forward: Vec<f64>,
    fovs: Vec<f64>,
    weapon: BTreeMap<String, BTreeMap<String, [f64; 2]>>,
    semi: BTreeMap<String, BTreeMap<String, [f64; 2]>>,
}

#[derive(Debug, Deserialize)]
struct Panels {
    timer: [i32; 4],
    speedometer: [i32; 4],
    jumpspeed: [i32; 4],
}

#[derive(Debug, Deserialize)]
struct AnglesLine {
    top: i32,
    number: i32,
    value: String,
}

#[derive(Debug, Deserialize)]
struct AnglesText {
    height: i32,
    digit: i32,
    point: i32,
    lines: Vec<AnglesLine>,
    glyphs: BTreeMap<String, [i32; 2]>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    scenes: Vec<f64>,
    viewangles: [i32; 2],
    crosshair: [i32; 2],
    panels: Panels,
    angles_text: AnglesText,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Packs {
    hud: hl1_pack_preview::Rects,
    shapes: Vec<String>,
    colors: Vec<String>,
    changer_cell: f64,
    changer_inset: f64,
    changer_size: f64,
}

struct Tables {
    meta: Meta,
    weapons: WeaponMeta,
    packs: Packs,
    pack_meta: PackMeta,
    /// `defaults()` из `shared/goldsrc-settings.js`.
    defaults: Map<String, Value>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let meta: Meta = serde_json::from_str(assets::HL1_META).expect("hl1-preview/meta.json");
        let packs: Packs = serde_json::from_str(assets::HL1_PACKS).expect("hl1-preview/packs.json");
        let weapons: WeaponMeta =
            serde_json::from_str(assets::HL1_WEAPONS).expect("hl1-preview/meta-weapons.json");
        let pack_meta = PackMeta {
            shapes: packs.shapes.clone(),
            colors: packs.colors.clone(),
            changer_cell: packs.changer_cell,
            changer_inset: packs.changer_inset,
            changer_size: packs.changer_size,
        };
        let defaults = match hlsr_delivery::goldsrc_settings::defaults().to_value() {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        Tables {
            meta,
            weapons,
            packs,
            pack_meta,
            defaults,
        }
    })
}

fn tan(fov: f64) -> f64 {
    (fov * PI / 360.0).tan()
}

/// `number(v, fallback)` 0.4.7: `Number(v)`, если конечное, иначе `fallback`.
pub(crate) fn number(value: Option<&Value>, fallback: f64) -> f64 {
    let n = to_number(value);
    if n.is_finite() { n } else { fallback }
}

pub(crate) fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(js::truthy)
}

/// Цвет `{r, g, b}` из значения так, как его прочтёт CSS `rgb(r,g,b)`.
pub(crate) fn rgb_of(value: Option<&Value>) -> Option<[u8; 3]> {
    let object = value?.as_object()?;
    let channel = |key: &str| {
        let n = to_number(object.get(key));
        if n.is_finite() {
            js::math_round(n).clamp(0.0, 255.0) as u8
        } else {
            0
        }
    };
    Some([channel("r"), channel("g"), channel("b")])
}

/// Что показывает игра: `default_fov` — по горизонтали настоящего экрана;
/// `bxt_fix_widescreen_fov` считает его видом 4:3, расширенным до экрана.
pub fn world_fov(s: &Map<String, Value>) -> f64 {
    let fov = number(s.get("fov"), 90.0);
    if !truthy(s.get("fixWidescreenFov")) {
        return fov;
    }
    let wide =
        (tan(fov) * 0.75 * f64::from(FRAME_WIDTH) / f64::from(FRAME_HEIGHT)).atan() * 360.0 / PI;
    js::math::max(10.0, js::math::min(150.0, wide))
}

/// `bxt_viewmodel_fov 0` рисует оружие с полем зрения вида.
pub fn weapon_fov(s: &Map<String, Value>) -> f64 {
    let own = number(s.get("viewmodelFov"), 0.0);
    if own > 0.0 { own } else { world_fov(s) }
}

/// Самый узкий кадр, который ещё вмещает весь вид.
fn frame_for(list: &[f64], fov: f64) -> f64 {
    list.iter()
        .copied()
        .find(|&step| step >= fov - 0.01)
        .unwrap_or_else(|| list.last().copied().unwrap_or(90.0))
}

#[derive(Debug, Clone, Copy)]
struct Offset {
    scale: f64,
    x: f64,
    y: f64,
}

const NO_OFFSET: Offset = Offset {
    scale: 1.0,
    x: 0.0,
    y: 0.0,
};

/// `forwardStep`: ближайший кадр, снятый при том же или более дальнем шаге
/// `bxt_viewmodel_ofs_forward`. В дальнем кадре больше оружия (руки): увеличенный до нужной
/// глубины, он только обрезается краем экрана, как в игре.
pub fn forward_step(forward: f64) -> f64 {
    let steps = &tables().weapons.forward;
    steps
        .iter()
        .copied()
        .find(|&step| step >= forward)
        .unwrap_or_else(|| steps.last().copied().unwrap_or(0.0))
}

/// Что осталось после кадра (между шагами) и `bxt_viewmodel_ofs_right/up`: слой оружия
/// плоский, поэтому сдвигается так, будто весь стоит на глубине около 20 единиц (середина
/// MP5 в кадре), не ближе ближней плоскости (4).
const WEAPON_DEPTH: f64 = 20.0;
const NEAR_PLANE: f64 = 4.0;

fn weapon_offset(s: &Map<String, Value>, fov: f64, step: f64) -> Offset {
    let depth = js::math::max(
        NEAR_PLANE,
        WEAPON_DEPTH + number(s.get("viewmodelForward"), 0.0),
    );
    let focal = CX / tan(fov);
    Offset {
        scale: (WEAPON_DEPTH + step) / depth,
        x: number(s.get("viewmodelRight"), 0.0) * focal / depth,
        y: -number(s.get("viewmodelUp"), 0.0) * focal / depth,
    }
}

/// BXT ставит панель в якорь × экран плюс свой сдвиг (`GetPosition`), якорь читается как
/// float, произведение отбрасывает дробь. В кадре якоря по умолчанию — панель сдвигается
/// на разницу.
fn anchor_at(percent: f64, size: f64) -> i32 {
    let product = f64::from((percent / 100.0) as f32) * size;
    (product as f32).trunc() as i32
}

fn panel_shift(s: &Map<String, Value>, panel: &str) -> (i32, i32) {
    let base = &tables().defaults;
    let (x, y) = (format!("{panel}X"), format!("{panel}Y"));
    let base_x = number(base.get(&x), 0.0);
    let base_y = number(base.get(&y), 0.0);
    (
        anchor_at(number(s.get(&x), base_x), f64::from(FRAME_WIDTH))
            - anchor_at(base_x, f64::from(FRAME_WIDTH)),
        anchor_at(number(s.get(&y), base_y), f64::from(FRAME_HEIGHT))
            - anchor_at(base_y, f64::from(FRAME_HEIGHT)),
    )
}

/// Углы в кадре — точные float: pitch 89 и yaw 257.51953125. `std::fixed` пишет их с
/// `bxt_hud_precision` знаками, точная середина округляется к чётной цифре.
pub fn fixed(value: &str, digits: usize) -> String {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let mut all: Vec<u8> = whole.bytes().collect();
    all.extend(fraction.bytes());
    while all.len() < whole.len() + digits {
        all.push(b'0');
    }
    let cut = whole.len() + digits;
    let mut kept: Vec<u8> = all[..cut].to_vec();
    let rest = &all[cut..];
    let first = rest.first().copied();
    let odd = kept.last().is_some_and(|d| (d - b'0') % 2 == 1);
    let round_up = match first {
        Some(c) if c > b'5' => true,
        Some(b'5') => rest[1..].iter().any(|&c| (b'1'..=b'9').contains(&c)) || odd,
        _ => false,
    };
    if round_up {
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, b'1');
                break;
            }
            i -= 1;
            if kept[i] == b'9' {
                kept[i] = b'0';
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    let point = kept.len() - digits;
    let mut out = String::from_utf8_lossy(&kept[..point]).into_owned();
    if digits > 0 {
        out.push('.');
        out.push_str(&String::from_utf8_lossy(&kept[point..]));
    }
    out
}

/// Цвет HUD так, как его пишет лаунчер: прозрачность входит в цвет. `None` — HUD
/// без своего цвета.
pub fn hud_color(value: &Value) -> Option<[u8; 3]> {
    if !truthy(value.get("hudEnabled")) {
        return None;
    }
    let c = rgb_of(value.get("hudColor").filter(|v| js::truthy(v))).unwrap_or(BXT_STOCK);
    let a = number(value.get("hudAlpha"), -1.0);
    // `!(a > 0)`: и NaN тоже.
    if a.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return Some(c);
    }
    Some(c.map(|v| js::math_round(f64::from(v) * a / 255.0).clamp(0.0, 255.0) as u8))
}

// ---- Прицел BXT (hl1-crosshair.js) ----------------------------------------------------

/// Прямоугольник целых пикселей прицела и его цвет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossPixel {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub color: [u8; 3],
}

const BLACK: [u8; 3] = [0, 0, 0];

/// Пиксели, которые линия покрывает вдоль: горизонтальная кончается до последней точки,
/// вертикальная начинается после первой.
fn span(from: f64, to: f64, vertical: bool) -> (i64, i64) {
    let round = |v: f64| {
        if vertical {
            (v + 0.5).floor()
        } else {
            (v - 0.5).ceil()
        }
    };
    let a = round(js::math::min(from, to));
    let b = round(js::math::max(from, to));
    (a as i64, (b - a) as i64)
}

/// Толщина — округлённая, не меньше пикселя; на границе пикселей горизонтальная линия
/// берёт ряд ниже, вертикальная — столбец левее.
fn across(centre: f64, width: f64, vertical: bool) -> (i64, i64) {
    let w = js::math::max(1.0, js::math_round(width));
    let start = if vertical {
        (centre - w / 2.0 - 0.5).ceil()
    } else {
        (centre - w / 2.0 + 0.5).floor()
    };
    (start as i64, w as i64)
}

#[allow(clippy::too_many_arguments)]
fn line(out: &mut Vec<CrossPixel>, x0: f64, y0: f64, x1: f64, y1: f64, width: f64, color: [u8; 3]) {
    if y0 == y1 {
        let (x, w) = span(x0, x1, false);
        let (y, h) = across(y0, width, false);
        if w > 0 {
            out.push(CrossPixel { x, y, w, h, color });
        }
    } else if x0 == x1 {
        let (y, h) = span(y0, y1, true);
        let (x, w) = across(x0, width, true);
        if h > 0 {
            out.push(CrossPixel { x, y, w, h, color });
        }
    }
}

/// Замкнутая ломаная в один пиксель через точки: по пикселю на шаг вдоль главной оси.
fn strip(out: &mut Vec<CrossPixel>, points: &[(f64, f64)], color: [u8; 3]) {
    let mut seen = std::collections::HashSet::new();
    let mut put = |out: &mut Vec<CrossPixel>, x: i64, y: i64| {
        if seen.insert((x, y)) {
            out.push(CrossPixel {
                x,
                y,
                w: 1,
                h: 1,
                color,
            });
        }
    };
    for i in 0..points.len() {
        let (ax, ay) = points[i];
        let (bx, by) = points[(i + 1) % points.len()];
        let (dx, dy) = (bx - ax, by - ay);
        let or_one = |v: f64| if v == 0.0 || v.is_nan() { 1.0 } else { v };
        if dx.abs() >= dy.abs() {
            let (x0, n) = span(ax, bx, false);
            for x in x0..x0 + n {
                let y = (ay + dy * ((x as f64 + 0.5 - ax) / or_one(dx))).floor() as i64;
                put(out, x, y);
            }
        } else {
            let (y0, n) = span(ay, by, true);
            for y in y0..y0 + n {
                let x = (ax + dx * ((y as f64 + 0.5 - ay) / or_one(dy))).floor() as i64;
                put(out, x, y);
            }
        }
    }
}

fn circle_points(cx: f64, cy: f64, radius: f64) -> Vec<(f64, f64)> {
    let count = (PI / ((radius - 0.1) / radius).acos()).ceil();
    let count = if count.is_finite() && count > 0.0 {
        count as usize
    } else {
        0
    };
    (0..count)
        .map(|i| {
            let angle = PI * 2.0 * i as f64 / count as f64;
            (cx + radius * angle.cos(), cy + radius * angle.sin())
        })
        .collect()
}

/// `Number(v) || 0`.
fn or_zero(value: Option<&Value>) -> f64 {
    let n = to_number(value);
    if n.is_nan() { 0.0 } else { n }
}

/// `crosshairPixels(v, cx, cy, color)`: прямоугольники в порядке рисования — сначала
/// обводка рамкой линий вокруг каждой палки, затем палки, круг и точка (как
/// `DrawCrosshair` в `hud_custom.cpp` BXT без сглаживания).
pub fn crosshair_pixels(v: &Value, cx: f64, cy: f64, color: [u8; 3]) -> Vec<CrossPixel> {
    let mut out = Vec::new();
    let size = or_zero(v.get("crossSize"));
    let gap = or_zero(v.get("crossGap"));
    let thickness = or_zero(v.get("crossThickness"));
    let top = truthy(v.get("crossTopLine"));
    let bottom = truthy(v.get("crossBottomLine"));
    let left = truthy(v.get("crossLeftLine"));
    let right = truthy(v.get("crossRightLine"));
    let outline = if truthy(v.get("crossOutline")) {
        or_zero(v.get("crossOutlineThickness"))
    } else {
        0.0
    };
    let dot = or_zero(v.get("crossDotSize"));

    if outline != 0.0 {
        let (ht, hw) = (thickness / 2.0, outline / 2.0);
        let o = ht + hw;
        let mut frame = |segments: [[f64; 4]; 4]| {
            for s in segments {
                line(&mut out, s[0], s[1], s[2], s[3], outline, BLACK);
            }
        };
        if top {
            frame([
                [cx - o, cy - gap - size, cx + o, cy - gap - size],
                [cx + ht, cy - gap - size + hw, cx + ht, cy - gap - hw],
                [cx + o, cy - gap, cx - o, cy - gap],
                [cx - ht, cy - gap - hw, cx - ht, cy - gap - size + hw],
            ]);
        }
        if bottom {
            frame([
                [cx - o, cy + gap + size, cx + o, cy + gap + size],
                [cx + ht, cy + gap + size - hw, cx + ht, cy + gap + hw],
                [cx + o, cy + gap, cx - o, cy + gap],
                [cx - ht, cy + gap + hw, cx - ht, cy + gap + size - hw],
            ]);
        }
        if left {
            frame([
                [cx - gap - size, cy - o, cx - gap - size, cy + o],
                [cx - gap - size + hw, cy + ht, cx - gap - hw, cy + ht],
                [cx - gap, cy + o, cx - gap, cy - o],
                [cx - gap - hw, cy - ht, cx - gap - size + hw, cy - ht],
            ]);
        }
        if right {
            frame([
                [cx + gap + size, cy - o, cx + gap + size, cy + o],
                [cx + gap + size - hw, cy + ht, cx + gap + hw, cy + ht],
                [cx + gap, cy + o, cx + gap, cy - o],
                [cx + gap + hw, cy - ht, cx + gap + size - hw, cy - ht],
            ]);
        }
        if dot != 0.0 {
            let d = dot / 2.0;
            frame([
                [cx - d - hw, cy - d, cx + d + hw, cy - d],
                [cx + d, cy - d + hw, cx + d, cy + d - hw],
                [cx - d, cy - d + hw, cx - d, cy + d - hw],
                [cx - d - hw, cy + d, cx + d + hw, cy + d],
            ]);
        }
    }

    if thickness != 0.0 {
        if top {
            line(
                &mut out,
                cx,
                cy - gap - size,
                cx,
                cy - gap,
                thickness,
                color,
            );
        }
        if bottom {
            line(
                &mut out,
                cx,
                cy + gap + size,
                cx,
                cy + gap,
                thickness,
                color,
            );
        }
        if left {
            line(
                &mut out,
                cx - gap - size,
                cy,
                cx - gap,
                cy,
                thickness,
                color,
            );
        }
        if right {
            line(
                &mut out,
                cx + gap + size,
                cy,
                cx + gap,
                cy,
                thickness,
                color,
            );
        }
    }

    if to_number(v.get("crossCircleRadius")) > 0.0 {
        let points = circle_points(cx, cy, to_number(v.get("crossCircleRadius")));
        strip(&mut out, &points, color);
    }

    if dot != 0.0 {
        let d = dot / 2.0;
        let (x, w) = span(cx - d, cx + d, false);
        let (y, h) = span(cy - d, cy + d, true);
        if w > 0 && h > 0 {
            let dot_color =
                rgb_of(v.get("crossDotColor").filter(|c| js::truthy(c))).unwrap_or(color);
            out.push(CrossPixel {
                x,
                y,
                w,
                h,
                color: dot_color,
            });
        }
    }
    out
}

// ---- Рендер ---------------------------------------------------------------------------

/// Сколько кадров сцены и слоёв оружия (`-f<сдвиг>`) держать декодированными (кадр — 5,8 МБ):
/// кадр и его оружие, прежние кадр и оружие.
const SCENES_KEPT: usize = 4;

/// Рендер превью HL1: декодированные слои и рабочие холсты.
#[derive(Default)]
pub struct Hl1Renderer {
    images: HashMap<String, Pixmap>,
    /// Порядок использования кадров сцены (вытесняется самый давний).
    scenes: VecDeque<String>,
}

impl Hl1Renderer {
    pub fn new() -> Hl1Renderer {
        Hl1Renderer::default()
    }

    /// Декодированные слои (имена `FILES` 0.4.7).
    pub fn loaded(&self) -> impl Iterator<Item = &str> {
        self.images.keys().map(String::as_str)
    }

    fn image(&mut self, name: &str) -> Result<&Pixmap, RasterError> {
        if !self.images.contains_key(name) {
            let bytes = assets::hl1(name).or_else(|| assets::hl1_weapon(name));
            let bytes = bytes.ok_or_else(|| RasterError {
                name: name.to_owned(),
                reason: "нет в сборке".to_owned(),
            })?;
            let pixmap = raster::decode(name, bytes)?;
            if name.starts_with("scene-") || name.contains("-f") {
                self.scenes.push_back(name.to_owned());
                while self.scenes.len() > SCENES_KEPT {
                    if let Some(old) = self.scenes.pop_front() {
                        self.images.remove(&old);
                    }
                }
            }
            self.images.insert(name.to_owned(), pixmap);
        } else if (name.starts_with("scene-") || name.contains("-f"))
            && let Some(at) = self.scenes.iter().position(|s| s == name)
        {
            let used = self.scenes.remove(at).unwrap_or_default();
            self.scenes.push_back(used);
        }
        Ok(&self.images[name])
    }

    /// Кадр, снятый с полем зрения `from`, показанный при `to`: увеличенный от центра и
    /// сдвинутый на `offset`.
    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        frame: &mut Pixmap,
        name: &str,
        at: [f64; 2],
        from: f64,
        to: f64,
        offset: Offset,
        blend: Blend,
    ) -> Result<(), RasterError> {
        let k = js::math::min(4.0, tan(from) / tan(to)) * offset.scale;
        let img = self.image(name)?;
        let (w, h) = (f64::from(img.width()), f64::from(img.height()));
        raster::draw_image(
            frame,
            img,
            CX + (at[0] - CX) * k + offset.x,
            CY + (at[1] - CY) * k + offset.y,
            w * k,
            h * k,
            blend,
            1.0,
        );
        Ok(())
    }

    /// Прибавляет цвет × маска (её часть `area`) к кадру в точке `at` от её места.
    fn add(
        &mut self,
        frame: &mut Pixmap,
        mask: &str,
        colour: Option<[u8; 3]>,
        area: Option<Area>,
        at: (i32, i32),
    ) -> Result<(), RasterError> {
        let mask = self.image(mask)?;
        let area = area.unwrap_or_else(|| Area::of(mask));
        let Some(mut work) = Pixmap::new(area.width.max(1) as u32, area.height.max(1) as u32)
        else {
            return Ok(());
        };
        raster::blit(&mut work, mask, area, 0, 0, Blend::Copy);
        if let Some(c) = colour {
            let whole = Area::of(&work);
            raster::multiply_rgb(&mut work, whole, c);
        }
        raster::blit(
            frame,
            &work,
            Area::new(0, 0, area.width, area.height),
            at.0 + area.x,
            at.1 + area.y,
            Blend::Plus,
        );
        Ok(())
    }

    /// Кадр превью 1600×900 для персонализации `value` (`render(target, value)` 0.4.7).
    pub fn render(&mut self, value: &Value) -> Result<Pixmap, RasterError> {
        let t = tables();
        let mut s = t.defaults.clone();
        if let Some(Value::Object(extra)) = value.get("extra") {
            for (k, v) in extra {
                s.insert(k.clone(), v.clone());
            }
        }
        let mut frame =
            raster::solid(FRAME_WIDTH, FRAME_HEIGHT, [0, 0, 0, 255]).ok_or_else(|| {
                RasterError {
                    name: "frame".to_owned(),
                    reason: "холст".to_owned(),
                }
            })?;

        let world = world_fov(&s);
        let step = frame_for(&t.meta.scenes, world);
        self.place(
            &mut frame,
            &format!("scene-{step}"),
            [0.0, 0.0],
            step,
            world,
            NO_OFFSET,
            Blend::Sprite,
        )?;

        // bxt_remove_viewmodel — оружия нет совсем.
        if !truthy(s.get("removeViewmodel")) {
            let vm = weapon_fov(&s);
            let semi = truthy(s.get("viewmodelSemitransparent"));
            let kind = if semi { "semi" } else { "weapon" };
            let step = frame_for(&t.weapons.fovs, vm);
            let forward = forward_step(number(s.get("viewmodelForward"), 0.0));
            let layers = if semi {
                &t.weapons.semi
            } else {
                &t.weapons.weapon
            };
            // Нет места слоя — в игре оружия при этом сдвиге не видно (за ближней плоскостью).
            if let Some(&at) = layers
                .get(&step.to_string())
                .and_then(|by_step| by_step.get(&forward.to_string()))
            {
                let offset = weapon_offset(&s, vm, forward);
                let name = format!("{kind}-{step}-f{forward}");
                let blend = if semi { Blend::Plus } else { Blend::Sprite };
                self.place(&mut frame, &name, at, step, vm, offset, blend)?;
            }
        }

        let colour = hud_color(value);
        if s.get("packHud").and_then(Value::as_str) == Some("ag-hud") {
            self.draw_pack_hud(&mut frame, colour)?;
        } else if colour.is_some() {
            self.add(&mut frame, "hud", colour, None, (0, 0))?;
        } else {
            self.add(&mut frame, "hud-stock", None, None, (0, 0))?;
        }

        let tint = colour.unwrap_or(BXT_STOCK);
        let panel_area = |b: [i32; 4]| Area::new(b[0], b[1], b[2] - b[0], b[3] - b[1]);
        for (key, b) in [
            ("timer", t.meta.panels.timer),
            ("speedometer", t.meta.panels.speedometer),
            ("jumpspeed", t.meta.panels.jumpspeed),
        ] {
            if truthy(s.get(key)) {
                let shift = panel_shift(&s, key);
                self.add(&mut frame, "panels", Some(tint), Some(panel_area(b)), shift)?;
            }
        }
        if truthy(s.get("jumpdistance")) {
            // Цифры HUD на ряд выше скорости прыжка; до прыжка — 0.
            let number_height = t.meta.panels.speedometer[1] - t.meta.panels.jumpspeed[1];
            self.add(
                &mut frame,
                "panels",
                Some(tint),
                Some(panel_area(t.meta.panels.jumpspeed)),
                (0, -number_height),
            )?;
        }
        if truthy(s.get("viewangles")) {
            self.draw_angles(&mut frame, &s)?;
        }

        if truthy(value.get("crossEnabled")) {
            draw_crosshair(&mut frame, value);
        }
        // Прицел оружия остаётся, пока его не прячут прицел BXT или hideCrosshair.
        if !truthy(s.get("hideCrosshair"))
            && !(truthy(value.get("crossEnabled")) && truthy(value.get("crossHideDefault")))
        {
            let values = Value::Object(s.clone());
            if let Some(pack) = hl1_pack_preview::crosshair_layer(&values, &t.pack_meta) {
                let [x, y, w, h] = pack.rect.map(|v| v as i32);
                let img = self.image(pack.image)?;
                raster::blit(
                    &mut frame,
                    img,
                    Area::new(x, y, w, h),
                    (CX - f64::from(w) / 2.0) as i32,
                    (CY - f64::from(h) / 2.0) as i32,
                    Blend::Over,
                );
            } else {
                let [x, y] = t.meta.crosshair;
                let img = self.image("crosshair")?;
                raster::blit(&mut frame, img, Area::of(img), x, y, Blend::Sprite);
            }
        }
        Ok(frame)
    }

    /// Строка «Angles:» как есть, затем в каждой строке значения — подпись и число,
    /// собранное из глифов кадра.
    fn draw_angles(
        &mut self,
        frame: &mut Pixmap,
        s: &Map<String, Value>,
    ) -> Result<(), RasterError> {
        let t = tables();
        let text = &t.meta.angles_text;
        let shift = panel_shift(s, "viewangles");
        let left = t.meta.viewangles[0] + shift.0;
        let top = t.meta.viewangles[1] + shift.1;
        let digits = js::math::max(
            0.0,
            js::math::min(16.0, number(s.get("precision"), 6.0).trunc()),
        ) as usize;
        let img = self.image("viewangles")?;
        let width = img.width() as i32;
        let first = text.lines.first().map_or(0, |l| l.top);
        raster::blit(
            frame,
            img,
            Area::new(0, 0, width, first),
            left,
            top,
            Blend::Over,
        );
        for line in &text.lines {
            raster::blit(
                frame,
                img,
                Area::new(0, line.top, line.number, text.height),
                left,
                top + line.top,
                Blend::Over,
            );
            let mut x = line.number;
            for ch in fixed(&line.value, digits).chars() {
                // Глифа нет — у 0.4.7 исключение и кадр без остатка строки.
                let Some(&[sy, sx]) = text.glyphs.get(&ch.to_string()) else {
                    return Ok(());
                };
                let w = if ch == '.' { text.point } else { text.digit };
                raster::blit(
                    frame,
                    img,
                    Area::new(sx, sy, w, text.height),
                    left + x,
                    top + line.top,
                    Blend::Over,
                );
                x += w;
            }
        }
        Ok(())
    }

    fn draw_pack_hud(
        &mut self,
        frame: &mut Pixmap,
        colour: Option<[u8; 3]>,
    ) -> Result<(), RasterError> {
        let tint = colour.unwrap_or(PACK_STOCK);
        let commands = hl1_pack_preview::hud_commands(
            &tables().packs.hud,
            f64::from(FRAME_WIDTH),
            f64::from(FRAME_HEIGHT),
        )
        .map_err(|e| RasterError {
            name: "packs.json".to_owned(),
            reason: format!("{e:?}"),
        })?;
        for item in commands {
            match item {
                Command::Fill { at, size } => {
                    let (w, h) = (size[0] as i32, size[1] as i32);
                    if let Some(fill) = raster::solid(
                        w.max(1) as u32,
                        h.max(1) as u32,
                        [tint[0], tint[1], tint[2], 255],
                    ) {
                        let area = Area::new(0, 0, w, h);
                        raster::blit(frame, &fill, area, at[0] as i32, at[1] as i32, Blend::Plus);
                    }
                }
                Command::Sprite {
                    rect: Some(rect),
                    at,
                    ..
                } => {
                    let [x, y, w, h] = rect.map(|v| v as i32);
                    self.add(
                        frame,
                        "hud-ag",
                        Some(tint),
                        Some(Area::new(x, y, w, h)),
                        (at[0] as i32 - x, at[1] as i32 - y),
                    )?;
                }
                // Нет прямоугольника — у 0.4.7 исключение и кадр без остатка HUD.
                Command::Sprite { rect: None, .. } => return Ok(()),
            }
        }
        Ok(())
    }
}

/// Прицел BXT пикселями игры: каждый прямоугольник — `fillRect` с `globalAlpha`.
fn draw_crosshair(frame: &mut Pixmap, v: &Value) {
    let alpha = number(v.get("crossAlpha"), 255.0) / 255.0;
    if alpha == 0.0 || alpha.is_nan() {
        return;
    }
    let colour = if truthy(v.get("hudEnabled")) && truthy(v.get("hudAffectsCrosshair")) {
        v.get("hudColor")
    } else {
        v.get("crossColor")
    };
    let colour = rgb_of(colour.filter(|c| js::truthy(c))).unwrap_or([0, 255, 0]);
    let alpha8 = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    for p in crosshair_pixels(v, CX, CY, colour) {
        raster::fill_rect(
            frame,
            Area::new(p.x as i32, p.y as i32, p.w as i32, p.h as i32),
            p.color,
            alpha8,
        );
    }
}
