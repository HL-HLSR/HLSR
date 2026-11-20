//! Превью разделов «Призраки и запись» (призрак на пляже) и «Эффекты и спреи» (спрей на
//! стене) Ghosting 0.4.9: расчёты `shared/ghosting-scene-preview.js` (числа — из исходников
//! мода gh2013: GhostEntity.cpp, gameinterface.cpp, GhostHud.cpp, SpriteTrail.cpp,
//! gh_spray.cpp) и рисование `ghosting-scene-renderer.js`.
//!
//! `ghost.webp` — программный рендер `models/player.mdl` (поза Idle01) без цвета и
//! прозрачности той же камерой, что у сцены; `ghost.json` — где он в кадре 1920×1080.
//! Фон призрака — участок пляжа, размытый как `filter: blur()` холста (ядро
//! [`super::shadow_blur`], решение владельца 17.11: эффекты как в 0.4.9) и затемнённый.

use super::canvas_text::{self, Align, Baseline, Font, Ink, Paint as TextPaint, Place};
use super::ghost_mesh::{MeshSprite, PLAYER_HEIGHT};
use super::ghosting_overlay::Assets;
use super::hl1_overlay::{number, rgb_of, truthy};
use super::preview_assets as assets;
use super::raster::{self, Blend, RasterError};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tiny_skia::{
    BlendMode, Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap,
    PixmapPaint, Point, SpreadMode, Transform,
};

/// Кадр игры, в котором показан призрак: 1920×1080, `fov_desired 90` (фокус 720 px).
pub const FRAME: (f64, f64) = (1920.0, 1080.0);
const FOCAL: f64 = FRAME.1 * 2.0 / 3.0;
const EYE_HEIGHT: f64 = 64.0;
/// Призрак стоит в 220 юнитах (дальше `gh_opacity_range` по умолчанию), бежит к игроку и
/// вправо. Игрок в начале координат смотрит вдоль +x; y — влево, z — вверх.
const GHOST_ORIGIN: [f64; 3] = [212.0, -60.0, 0.0];
const GHOST_YAW: f64 = 215.0;
/// Вертикальный участок кадра: призрак с именем по центру, x — середина модели.
const GHOST_VIEW: (f64, f64, f64) = (1171.5, 627.0, 380.0);
/// Фон сцены призрака: размытие в px кадра превью (высота 900) и затемнение чёрным.
const BACKDROP_BLUR: f64 = 10.0;
const BACKDROP_DIM: f64 = 0.2;
/// `env_spritetrail` (GhostEntity::CreateTrail): сложение, renderamt 75, ширина 9.5 → 1.05.
const TRAIL_START: f64 = 9.5;
const TRAIL_END: f64 = 1.05;
const TRAIL_BRIGHTNESS: f64 = 75.0;
/// Яркость `laser.vtf` поперёк ленты, от середины к краю.
const LASER_HALF: [f64; 16] = [
    246.0, 69.0, 66.0, 62.0, 56.0, 49.0, 40.0, 32.0, 24.0, 15.0, 5.0, 0.0, 0.0, 0.0, 0.0, 0.0,
];
/// shortcut: путь следа придуман (бежит издалека и у камеры сворачивает вправо), а не взят
/// из настоящего забега; заменить записью `.run`, если понадобится (как в 0.4.9).
const TRAIL_SPEED: f64 = 300.0;
const TRAIL_FAR_YAW: f64 = 172.0;
const TRAIL_STEP: f64 = 1.0 / 30.0;
/// Имена (CHudGhostNames): Verdana 700, 6 единиц 480 строк, альфа 180, тень, +12 юнитов.
const NAMES_SIZE: f64 = 6.0;
const NAMES_ALPHA: f64 = 180.0;
const NAMES_FADE_START: f64 = 250.0;
const NAMES_RAISE: f64 = 12.0;
const NAMES_FALLBACK: [u8; 3] = [255, 220, 70];
/// Спрей: квадрат со стороной — степень двойки от 8 до 256; стена фона — 192 юнита.
/// shortcut: ширина стены подобрана на глаз (как в 0.4.9), а не измерена в игре.
const SPRAY_MIN: u32 = 8;
const SPRAY_MAX: u32 = 256;
const SPRAY_WALL_UNITS: f64 = 192.0;

/// Что показывает сцена.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ghost,
    Spray,
}

/// Каким квадратом и какого размера спрей лёг на стену (подпись под кадром).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SprayInfo {
    pub units: f64,
    pub texels: u32,
}

/// Точка кадра 1920×1080.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projected {
    pub x: f64,
    pub y: f64,
}

/// Точка мира → пиксели кадра 1920×1080.
pub fn project(p: [f64; 3]) -> Projected {
    let depth = p[0];
    Projected {
        x: FRAME.0 / 2.0 - FOCAL * p[1] / depth,
        y: FRAME.1 / 2.0 - FOCAL * (p[2] - EYE_HEIGHT) / depth,
    }
}

/// Участок кадра игры (x, y, ширина, высота) для превью с соотношением сторон `aspect`.
pub fn ghost_view(aspect: f64) -> (f64, f64, f64, f64) {
    let width = GHOST_VIEW.2 * aspect;
    (
        GHOST_VIEW.0 - width / 2.0,
        GHOST_VIEW.1 - GHOST_VIEW.2 / 2.0,
        width,
        GHOST_VIEW.2,
    )
}

/// Расстояние до призрака, юниты (подпись `#L_GS_X_SCENE_GHOST`).
pub fn ghost_distance() -> f64 {
    hypot3(GHOST_ORIGIN)
}

fn hypot3(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// `ghostAlpha`: ближе `gh_opacity_range` призрак прозрачнее пропорционально расстоянию.
pub fn ghost_alpha(opacity: Option<&Value>, range: Option<&Value>) -> f64 {
    let base = number(opacity, 75.0).trunc().clamp(0.0, 255.0);
    let limit = number(range, 200.0);
    let distance = ghost_distance();
    if limit > 0.0 && distance <= limit {
        return (distance * (base / limit)).trunc().clamp(0.0, 255.0);
    }
    base
}

/// Имя над призраком (`ghostName`).
#[derive(Debug, Clone, PartialEq)]
pub struct GhostName {
    pub text: String,
    pub alpha: f64,
    pub color: [u8; 3],
    pub size: f64,
    pub x: f64,
    pub y: f64,
}

/// Имя или `None`, если игра его не рисует. `hud_color` — цвет HUD лаунчера, если включён.
pub fn ghost_name(s: &Map<String, Value>, hud_color: Option<[u8; 3]>) -> Option<GhostName> {
    let name = match s.get("gh_name") {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(true)) => "true".to_owned(),
        _ => String::new(),
    };
    if !truthy(s.get("gh_ghost_names")) || name.is_empty() {
        return None;
    }
    let o = GHOST_ORIGIN;
    let distance = hypot3([o[0], o[1], o[2] - EYE_HEIGHT]);
    let max = number(s.get("gh_ghost_names_dist"), 2000.0);
    if max > 0.0 && distance > max {
        return None;
    }
    let mut fraction = 1.0;
    if max > 0.0 && distance > NAMES_FADE_START {
        fraction = 1.0 - (distance - NAMES_FADE_START) / (max - NAMES_FADE_START).max(1.0);
    }
    let alpha = (NAMES_ALPHA * fraction.clamp(0.0, 1.0) + 0.5).trunc();
    if alpha <= 0.0 {
        return None;
    }
    let color = if truthy(s.get("gh_ghost_names_own_color")) {
        rgb_of(s.get("gh_ghost_color")).unwrap_or([0, 0, 0])
    } else {
        hud_color.unwrap_or(NAMES_FALLBACK)
    };
    let anchor = project([o[0], o[1], o[2] + EYE_HEIGHT + NAMES_RAISE]);
    // `name.slice(0, 63)`: единицы UTF-16.
    let units: Vec<u16> = name.encode_utf16().take(63).collect();
    Some(GhostName {
        text: String::from_utf16_lossy(&units),
        alpha,
        color,
        size: (NAMES_SIZE * FRAME.1 / 480.0).trunc().max(1.0),
        x: anchor.x,
        y: anchor.y,
    })
}

/// Направление движения (yaw, градусы) за `t` секунд до кадра.
fn heading(t: f64) -> f64 {
    TRAIL_FAR_YAW + (GHOST_YAW - TRAIL_FAR_YAW) * (-t / 0.12).exp()
        - 6.0 * (2.2 * t).sin() * (-t / 2.5).exp()
}

/// Точка следа: середина ленты (`x`, `y`), её видимый край (`ex`, `ey`) и доля яркости.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailPoint {
    pub x: f64,
    pub y: f64,
    pub ex: f64,
    pub ey: f64,
    pub alpha: f64,
}

/// `trailPoints`: точки следа в px кадра от призрака к хвосту. Лента повёрнута к камере
/// (CBeamSegDraw::ComputeNormal), её половина ниже земли не видна.
pub fn trail_points(s: &Map<String, Value>) -> Vec<TrailPoint> {
    let life = number(s.get("gh_trail_length"), 5.0);
    if !truthy(s.get("gh_draw_trails")) || life.is_nan() || life <= 0.0 {
        return Vec::new();
    }
    let mut path: Vec<[f64; 4]> = Vec::new();
    let (mut x, mut y) = (GHOST_ORIGIN[0], GHOST_ORIGIN[1]);
    let mut t = 0.0;
    while t <= life + 1e-9 {
        path.push([x, y, GHOST_ORIGIN[2], t]);
        let a = heading(t) * std::f64::consts::PI / 180.0;
        x -= a.cos() * TRAIL_SPEED * TRAIL_STEP;
        y -= a.sin() * TRAIL_SPEED * TRAIL_STEP;
        t += TRAIL_STEP;
    }
    let mut points = Vec::new();
    for i in 0..path.len() {
        let p = path[i];
        let a = path[i.saturating_sub(1)];
        let b = path[(i + 1).min(path.len() - 1)];
        let tangent = [a[0] - b[0], a[1] - b[1], 0.0];
        let view = [p[0], p[1], p[2] - EYE_HEIGHT];
        let n = [
            tangent[1] * view[2] - tangent[2] * view[1],
            tangent[2] * view[0] - tangent[0] * view[2],
            tangent[0] * view[1] - tangent[1] * view[0],
        ];
        let length = hypot3(n);
        let length = if length > 0.0 { length } else { 1.0 };
        let side = if n[2] < 0.0 { -1.0 } else { 1.0 };
        let left = 1.0 - p[3] / life;
        let half = (TRAIL_END + (TRAIL_START - TRAIL_END) * left) / 2.0 * side / length;
        let c = project([p[0], p[1], p[2]]);
        let e = project([p[0] + n[0] * half, p[1] + n[1] * half, p[2] + n[2] * half]);
        if (e.x - c.x).hypot(e.y - c.y) < 0.05 {
            break;
        }
        points.push(TrailPoint {
            x: c.x,
            y: c.y,
            ex: e.x,
            ey: e.y,
            alpha: TRAIL_BRIGHTNESS / 255.0 * left,
        });
    }
    points
}

/// Как игра делает спрей из картинки `width × height` и какого он размера на стене
/// шириной `wall_width` px: (texels, units, px).
pub fn spray_layout(
    scale: Option<&Value>,
    width: u32,
    height: u32,
    wall_width: f64,
) -> (u32, f64, f64) {
    let longest = width.max(height).clamp(1, SPRAY_MAX);
    let mut texels = SPRAY_MIN;
    while texels * 2 <= longest && texels * 2 <= SPRAY_MAX {
        texels *= 2;
    }
    let units = f64::from(texels) * number(scale, 0.25).clamp(0.05, 2.0);
    (texels, units, units * wall_width / SPRAY_WALL_UNITS)
}

#[derive(Debug, Deserialize)]
struct Sprite {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn sprite() -> &'static Sprite {
    static SPRITE: OnceLock<Sprite> = OnceLock::new();
    SPRITE.get_or_init(|| serde_json::from_str(assets::GHOST_SPRITE).expect("scene/ghost.json"))
}

fn defaults() -> &'static Map<String, Value> {
    static DEFAULTS: OnceLock<Map<String, Value>> = OnceLock::new();
    DEFAULTS.get_or_init(
        || match hlsr_delivery::ghosting_settings::defaults().to_value() {
            Value::Object(map) => map,
            _ => Map::new(),
        },
    )
}

fn err(name: &str, reason: &str) -> RasterError {
    RasterError {
        name: name.to_owned(),
        reason: reason.to_owned(),
    }
}

/// `drawImage(img, sx, sy, sw, sh, dx, dy, dw, dh)` с `imageSmoothingQuality: 'high'`.
#[allow(clippy::too_many_arguments)]
fn draw_part(
    dst: &mut Pixmap,
    img: &Pixmap,
    sx: f64,
    sy: f64,
    sw: f64,
    sh: f64,
    dx: f64,
    dy: f64,
    dw: f64,
    dh: f64,
) {
    let (kx, ky) = (dw / sw, dh / sh);
    let quality = if kx >= 1.0 && ky >= 1.0 {
        tiny_skia::FilterQuality::Bicubic
    } else {
        tiny_skia::FilterQuality::Bilinear
    };
    let paint = PixmapPaint {
        quality,
        ..PixmapPaint::default()
    };
    let transform = Transform::from_row(
        kx as f32,
        0.0,
        0.0,
        ky as f32,
        (dx - sx * kx) as f32,
        (dy - sy * ky) as f32,
    );
    dst.draw_pixmap(0, 0, img.as_ref(), &paint, transform, None);
}

/// Картинка, уменьшенная до стороны не меньше `w × h` половинками (мип-уровни Skia при
/// `kHigh`): иначе билинейное уменьшение больше чем вдвое теряет пиксели.
fn mip(img: &Pixmap, w: f64, h: f64) -> Pixmap {
    let mut out = img.clone();
    while f64::from(out.width()) >= 2.0 * w && f64::from(out.height()) >= 2.0 * h {
        let Some(next) = super::preview::halve(&out) else {
            break;
        };
        out = next;
    }
    out
}

/// Сколько размытых фонов разных размеров держит [`SceneRenderer`].
const BACKDROPS: usize = 3;

/// Рендер сцен превью: декодированные картинки и подготовленные слои.
#[derive(Default)]
pub struct SceneRenderer {
    images: HashMap<&'static str, Pixmap>,
    /// Размытый фон по размерам кадра, последний — первым: портрет сначала рисуется по прежнему
    /// окну вида, потом по своему, и один фон заново считался бы при каждом возврате в раздел.
    backdrops: Vec<Pixmap>,
    /// Окраски призрака: ключ — цвет и картинка (готовый призрак или своя модель).
    tints: HashMap<([u8; 3], usize), Pixmap>,
    sprays: HashMap<(usize, u32), Pixmap>,
    /// Своя модель и спрей игрока, чьи адреса — ключи окрасок и квадратов: пока они здесь,
    /// адрес не достанется другой картинке.
    model: Option<Arc<MeshSprite>>,
    spray: Option<Arc<Pixmap>>,
}

impl SceneRenderer {
    pub fn new() -> SceneRenderer {
        SceneRenderer::default()
    }

    fn image(&mut self, name: &'static str) -> Result<&Pixmap, RasterError> {
        if !self.images.contains_key(name) {
            let bytes = assets::ghosting_scene(name).ok_or_else(|| err(name, "нет в сборке"))?;
            self.images.insert(name, raster::decode(name, bytes)?);
        }
        Ok(&self.images[name])
    }

    /// Кадр сцены `kind` шириной `width` (высота 900); для спрея — его подпись.
    pub fn render(
        &mut self,
        width: u32,
        value: &Value,
        kind: Kind,
        assets: &Assets,
    ) -> Result<(Pixmap, Option<SprayInfo>), RasterError> {
        let mut s = defaults().clone();
        if let Some(Value::Object(extra)) = value.get("extra") {
            for (k, v) in extra {
                s.insert(k.clone(), v.clone());
            }
        }
        let mut frame = Pixmap::new(width.max(1), super::preview::FRAME_HEIGHT)
            .ok_or_else(|| err("frame", "холст"))?;
        let info = match kind {
            Kind::Ghost => {
                self.draw_ghost(&mut frame, &s, value, assets.ghost_model.as_ref())?;
                None
            }
            Kind::Spray => self.draw_spray(&mut frame, &s, assets.spray.as_ref())?,
        };
        Ok((frame, info))
    }

    /// Тот же участок пляжа, размытый и затемнённый; один раз на размер кадра (хранятся
    /// [`BACKDROPS`] последних).
    fn backdrop_for(
        &mut self,
        width: u32,
        height: u32,
        crop: (f64, f64, f64, f64),
    ) -> Result<&Pixmap, RasterError> {
        if let Some(i) = self
            .backdrops
            .iter()
            .position(|b| b.width() == width && b.height() == height)
        {
            let hit = self.backdrops.remove(i);
            self.backdrops.insert(0, hit);
            return Ok(&self.backdrops[0]);
        }
        let img = self.image("beach")?.clone();
        let k = f64::from(img.width()) / FRAME.0;
        let blur = BACKDROP_BLUR * f64::from(height) / 900.0;
        let pad = blur * 3.0;
        // Поля с запасом, чтобы края не темнели от размытия: размывается холст с полями.
        let p = pad.ceil();
        let (pw, ph) = (width + 2 * p as u32, height + 2 * p as u32);
        let mut padded = Pixmap::new(pw, ph).ok_or_else(|| err("beach", "холст"))?;
        let s = f64::from(width) / crop.2;
        draw_part(
            &mut padded,
            &img,
            (crop.0 - pad / s) * k,
            (crop.1 - pad / s) * k,
            (crop.2 + 2.0 * pad / s) * k,
            (crop.3 + 2.0 * pad / s) * k,
            p - pad,
            p - pad,
            f64::from(width) + 2.0 * pad,
            f64::from(height) + 2.0 * pad,
        );
        let mut image = raster::to_image(&padded);
        super::shadow_blur::gaussian_blur_rgba(&mut image, blur as f32);
        let blurred = image.as_slice();
        let mut out = Pixmap::new(width, height).ok_or_else(|| err("beach", "холст"))?;
        let (w, pad_px) = (width as usize, p as usize);
        for (i, px) in out.pixels_mut().iter_mut().enumerate() {
            let c = blurred[(i / w + pad_px) * pw as usize + i % w + pad_px];
            let m = |v: u8| raster::premultiply(v, c.a);
            *px = tiny_skia::PremultipliedColorU8::from_rgba(m(c.r), m(c.g), m(c.b), c.a)
                .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT);
        }
        let dim = (BACKDROP_DIM * 255.0).round() as u8;
        let whole = raster::Area::of(&out);
        raster::fill_rect(&mut out, whole, [0, 0, 0], dim);
        self.backdrops.truncate(BACKDROPS - 1);
        self.backdrops.insert(0, out);
        Ok(&self.backdrops[0])
    }

    /// Кадр любой ширины: высота участка постоянна, призрак всегда по центру. Своя модель
    /// стоит ногами там же, где готовый призрак, масштаб — по росту модели игрока.
    fn draw_ghost(
        &mut self,
        frame: &mut Pixmap,
        s: &Map<String, Value>,
        value: &Value,
        model: Option<&Arc<MeshSprite>>,
    ) -> Result<(), RasterError> {
        if let Some(model) = model
            && !self.model.as_ref().is_some_and(|m| Arc::ptr_eq(m, model))
        {
            self.model = Some(model.clone());
        }
        let (width, height) = (frame.width(), frame.height());
        let crop = ghost_view(f64::from(width) / f64::from(height));
        let k = f64::from(height) / crop.3;
        let at = |x: f64, y: f64| ((x - crop.0) * k, (y - crop.1) * k);
        let backdrop = self.backdrop_for(width, height, crop)?;
        raster::blit(
            frame,
            backdrop,
            raster::Area::of(backdrop),
            0,
            0,
            Blend::Copy,
        );
        let points: Vec<TrailPoint> = trail_points(s)
            .into_iter()
            .map(|p| {
                let (x, y) = at(p.x, p.y);
                let (ex, ey) = at(p.ex, p.ey);
                TrailPoint {
                    x,
                    y,
                    ex,
                    ey,
                    alpha: p.alpha,
                }
            })
            .collect();
        draw_trail(
            frame,
            &points,
            rgb_of(s.get("gh_trail_color")).unwrap_or([0, 0, 0]),
        );
        // kRenderTransColor: цвет умножает модель, gh_opacity — её альфа.
        let alpha = ghost_alpha(s.get("gh_opacity"), s.get("gh_opacity_range")) / 255.0;
        let color = rgb_of(s.get("gh_ghost_color")).unwrap_or([0, 0, 0]);
        let sp = sprite();
        if alpha > 0.0 {
            let (tint, x, y, w, h) = if let Some(model) = model {
                let unit = sp.height / PLAYER_HEIGHT * k / model.ppu;
                let (fx, fy) = at(sp.x + sp.width / 2.0, sp.y + sp.height);
                let tint = self.tinted(color, &model.image, Arc::as_ptr(model) as usize);
                (
                    tint,
                    fx - model.origin_x * unit,
                    fy - model.ground_y * unit,
                    f64::from(model.image.width()) * unit,
                    f64::from(model.image.height()) * unit,
                )
            } else {
                let img = self.image("ghost")?.clone();
                let (x, y) = at(sp.x, sp.y);
                (
                    self.tinted(color, &img, 0),
                    x,
                    y,
                    sp.width * k,
                    sp.height * k,
                )
            };
            raster::draw_image(frame, tint, x, y, w, h, Blend::Over, alpha as f32);
        }
        let hud = truthy(value.get("hudEnabled"))
            .then(|| rgb_of(value.get("hudColor")))
            .flatten();
        if let Some(name) = ghost_name(s, hud) {
            let (x, y) = at(name.x, name.y);
            let size = name.size * k;
            let font = Font {
                weight: 700,
                size: size as f32,
                families: vec!["Verdana".into(), "DejaVu Sans".into(), "sans-serif".into()],
            };
            // textBaseline 'bottom' — низ квадрата em.
            let descent = canvas_text::layout(&name.text, &font).metrics.em_descent;
            let place = |x: f64, y: f64| Place {
                x: x as f32,
                y: y as f32 - descent,
                align: Align::Center,
                baseline: Baseline::Alphabetic,
                max_width: None,
            };
            let shadow = (name.alpha * 180.0 / 255.0).round().clamp(0.0, 255.0) as u8;
            canvas_text::text(
                frame,
                &name.text,
                &font,
                place(x + k, y + k),
                TextPaint::Fill,
                &Ink::Solid([0, 0, 0, shadow]),
                1.0,
            );
            let c = name.color;
            canvas_text::text(
                frame,
                &name.text,
                &font,
                place(x, y),
                TextPaint::Fill,
                &Ink::Solid([c[0], c[1], c[2], name.alpha as u8]),
                1.0,
            );
        }
        Ok(())
    }

    /// Призрак цвета `color`: `multiply` цветом, затем `destination-in` картинкой.
    fn tinted(&mut self, color: [u8; 3], img: &Pixmap, source: usize) -> &Pixmap {
        let key = (color, source);
        if !self.tints.contains_key(&key) {
            // Своя модель меняется — её окраски отдельно; у неё холст большой: хранится одна
            // окраска, у готового призрака — до 16.
            let limit = if source == 0 { 16 } else { 0 };
            if self.tints.keys().any(|k| k.1 != source) || self.tints.len() > limit {
                self.tints.clear();
            }
            let mut out = img.clone();
            let c = color.map(f64::from);
            for p in out.pixels_mut() {
                let a = f64::from(p.alpha()) / 255.0;
                if a == 0.0 {
                    continue;
                }
                // Непрозрачная заливка multiply над пикселем картинки, потом × её альфа.
                let ch = |cs: f64, pd: u8| {
                    let cb = f64::from(pd) / 255.0 / a;
                    let co = (1.0 - a) * cs + a * cs * cb;
                    (co * a).round().clamp(0.0, 255.0) as u8
                };
                let (r, g, b) = (ch(c[0], p.red()), ch(c[1], p.green()), ch(c[2], p.blue()));
                let alpha = p.alpha();
                *p = tiny_skia::PremultipliedColorU8::from_rgba(
                    r.min(alpha),
                    g.min(alpha),
                    b.min(alpha),
                    alpha,
                )
                .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT);
            }
            self.tints.insert(key, out);
        }
        &self.tints[&key]
    }

    /// Спрей там, куда смотрит игрок, — в середине кадра. Игра сжимает картинку в квадрат
    /// и кладёт его декалью на стену.
    fn draw_spray(
        &mut self,
        frame: &mut Pixmap,
        s: &Map<String, Value>,
        picked: Option<&Arc<Pixmap>>,
    ) -> Result<Option<SprayInfo>, RasterError> {
        if let Some(picked) = picked
            && !self.spray.as_ref().is_some_and(|p| Arc::ptr_eq(p, picked))
        {
            self.spray = Some(picked.clone());
            self.sprays.clear();
        }
        let (width, height) = (f64::from(frame.width()), f64::from(frame.height()));
        let wall = self.image("wall")?.clone();
        raster::draw_image(frame, &wall, 0.0, 0.0, width, height, Blend::Sprite, 1.0);
        if !truthy(s.get("gh_sprays")) {
            return Ok(None);
        }
        let img = match picked {
            Some(img) => img.as_ref().clone(),
            None => self.image("spray")?.clone(),
        };
        let (texels, units, pixels) =
            spray_layout(s.get("gh_spray_scale"), img.width(), img.height(), width);
        let key = (picked.map_or(0, |p| Arc::as_ptr(p) as usize), texels);
        if !self.sprays.contains_key(&key) {
            let mut square = Pixmap::new(texels, texels).ok_or_else(|| err("spray", "холст"))?;
            let t = f64::from(texels);
            let small = mip(&img, t, t);
            raster::draw_image(&mut square, &small, 0.0, 0.0, t, t, Blend::Over, 1.0);
            // Декаль LightmappedGeneric берёт освещение стены: в тени фона она темнее.
            for p in square.pixels_mut() {
                let dim = |v: u8| (f64::from(v) * 0.88).round() as u8;
                *p = tiny_skia::PremultipliedColorU8::from_rgba(
                    dim(p.red()),
                    dim(p.green()),
                    dim(p.blue()),
                    p.alpha(),
                )
                .unwrap_or(*p);
            }
            if self.sprays.len() > 4 {
                self.sprays.clear();
            }
            self.sprays.insert(key, square);
        }
        let square = &self.sprays[&key];
        raster::draw_image(
            frame,
            square,
            (width - pixels) / 2.0,
            (height - pixels) / 2.0,
            pixels,
            pixels,
            Blend::Over,
            1.0,
        );
        Ok(Some(SprayInfo { units, texels }))
    }
}

/// Лента `laser.vtf` сложением: поперёк — яркость текстуры от середины к краю, вдоль —
/// renderamt × оставшаяся жизнь точки.
fn draw_trail(frame: &mut Pixmap, points: &[TrailPoint], color: [u8; 3]) {
    if points.len() < 2 {
        return;
    }
    let (width, height) = (f64::from(frame.width()), f64::from(frame.height()));
    let last = (LASER_HALF.len() - 1) as f32;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let xs = [a.x, b.x, a.ex, b.ex];
        let ys = [a.y, b.y, a.ey, b.ey];
        let max = |v: [f64; 4]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let min = |v: [f64; 4]| v.iter().copied().fold(f64::INFINITY, f64::min);
        if max(xs) < 0.0 || min(xs) > width || max(ys) < 0.0 || min(ys) > height {
            continue;
        }
        let alpha = (a.alpha + b.alpha) / 2.0;
        let stops: Vec<GradientStop> = LASER_HALF
            .iter()
            .enumerate()
            .map(|(j, v)| {
                GradientStop::new(
                    j as f32 / last,
                    Color::from_rgba8(
                        color[0],
                        color[1],
                        color[2],
                        ((v / 255.0 * alpha) * 255.0).round().clamp(0.0, 255.0) as u8,
                    ),
                )
            })
            .collect();
        let start = Point::from_xy(((a.x + b.x) / 2.0) as f32, ((a.y + b.y) / 2.0) as f32);
        let end = Point::from_xy(((a.ex + b.ex) / 2.0) as f32, ((a.ey + b.ey) / 2.0) as f32);
        let Some(shader) =
            LinearGradient::new(start, end, stops, SpreadMode::Pad, Transform::identity())
        else {
            continue;
        };
        let mut path = PathBuilder::new();
        path.move_to(a.x as f32, a.y as f32);
        path.line_to(b.x as f32, b.y as f32);
        path.line_to(b.ex as f32, b.ey as f32);
        path.line_to(a.ex as f32, a.ey as f32);
        path.close();
        let Some(path) = path.finish() else {
            continue;
        };
        let paint = Paint {
            shader,
            blend_mode: BlendMode::Plus,
            anti_alias: true,
            ..Paint::default()
        };
        frame.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Своя модель для превью из ответа `game-personalization-preview-asset` (`kind: model`):
/// `{status: "ok", mesh: {positions: [...]}}`.
pub fn model_of(reply: &Value) -> Option<Arc<MeshSprite>> {
    let positions: Vec<f64> = reply
        .get("mesh")?
        .get("positions")?
        .as_array()?
        .iter()
        .map(|v| v.as_f64().unwrap_or(f64::NAN))
        .collect();
    super::ghost_mesh::mesh_sprite(&positions).map(Arc::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings(extra: Value) -> Map<String, Value> {
        let mut s = defaults().clone();
        if let Value::Object(e) = extra {
            s.extend(e);
        }
        s
    }

    #[test]
    fn numbers_follow_the_0_4_9_scene_preview() {
        assert_eq!(hlsr_core::js::math_round(ghost_distance()), 220.0);
        // 220 > gh_opacity_range 200: непрозрачность как есть; ближе — пропорционально.
        assert_eq!(ghost_alpha(Some(&json!(75)), Some(&json!(200))), 75.0);
        assert_eq!(ghost_alpha(Some(&json!(255)), Some(&json!(400))), 140.0);
        let view = ghost_view(0.75);
        assert_eq!(view, (1171.5 - 142.5, 437.0, 285.0, 380.0));
        // Спрей: 300×200 → 256 текселей, 0.25 → 64 юнита; 100×40 → 64.
        assert_eq!(spray_layout(Some(&json!(0.25)), 300, 200, 1600.0).0, 256);
        assert_eq!(
            spray_layout(Some(&json!(0.5)), 100, 40, 1600.0),
            (64, 32.0, 32.0 * 1600.0 / 192.0)
        );
        assert_eq!(spray_layout(Some(&json!(9)), 1, 1, 192.0), (8, 16.0, 16.0));
    }

    #[test]
    fn name_and_trail_follow_the_settings() {
        let s = settings(json!({}));
        let name = ghost_name(&s, None).unwrap();
        assert_eq!((name.text.as_str(), name.size), ("Ghost", 13.0));
        assert_eq!(name.color, [237, 133, 60]);
        let hud = settings(json!({"gh_ghost_names_own_color": false}));
        assert_eq!(ghost_name(&hud, None).unwrap().color, NAMES_FALLBACK);
        assert_eq!(ghost_name(&hud, Some([1, 2, 3])).unwrap().color, [1, 2, 3]);
        assert!(ghost_name(&settings(json!({"gh_ghost_names_dist": 100})), None).is_none());
        let trail = trail_points(&s);
        assert!(trail.len() > 100);
        assert!((trail[0].alpha - 75.0 / 255.0).abs() < 1e-12);
        assert!(trail.windows(2).all(|w| w[1].alpha < w[0].alpha));
        assert!(trail_points(&settings(json!({"gh_draw_trails": false}))).is_empty());
        assert!(trail_points(&settings(json!({"gh_trail_length": 0}))).is_empty());
    }
}
