//! Оверлей LiveSplit в окне игры: `overlay.ini`, геометрия и маски фона
//! (`src/shared/livesplit-overlay.js`, часть подпроекта 3; настройки и их проверка —
//! `hlsr_delivery::livesplit_overlay`). Общий для запуска (`overlay.ini` для прокси в папке игры)
//! и окна персонализации (предпросмотр). Маски повторяют фильтры OBS «Цветовой ключ» и
//! «Яркостный ключ» и код DLL (`scripts/livesplit-overlay/common.h`, `key_alpha`): предпросмотр
//! вырезает те же пиксели, что игра.
//!
//! Функции берут уже проверенные настройки ([`Overlay`], результат `sanitize_overlay`): в 0.4.7
//! каждая из них проверяет вход заново, но результат проверки идемпотентен.

use hlsr_core::js::math_round;
use hlsr_core::js::number::number_to_string;
use hlsr_delivery::livesplit_overlay::Overlay;

/// Цвет (B, G, R, X), которым DLL заливает холст перед `PrintWindow`; что им осталось, окно
/// LiveSplit не нарисовало (`SENTINEL` в `common.h`).
pub const SENTINEL: [u8; 4] = [0x5a, 0xff, 0x00, 0x5a];

/// `KEY_CODES[key]`: число, которое читает DLL.
fn key_code(key: &str) -> u8 {
    match key {
        "color" => 1,
        "luma" => 2,
        _ => 0,
    }
}

/// `toIni(value)`: `overlay.ini` для прокси (разбор — `config_parse` в `common.h`).
/// Строки через `\r\n`, в конце тоже `\r\n`.
pub fn to_ini(overlay: &Overlay) -> String {
    let color = overlay.key_color;
    let number = number_to_string;
    [
        format!("enabled={}", u8::from(overlay.enabled)),
        format!("x={}", number(overlay.x)),
        format!("y={}", number(overlay.y)),
        format!("height={}", number(overlay.height)),
        format!("opacity={}", overlay.opacity),
        format!("key={}", key_code(overlay.key)),
        format!("key_color={:02x}{:02x}{:02x}", color.r, color.g, color.b),
        format!("similarity={}", overlay.similarity),
        format!("smoothness={}", overlay.smoothness),
        format!("luma_min={}", number(overlay.luma_min)),
        format!("luma_min_smooth={}", number(overlay.luma_min_smooth)),
        format!("luma_max={}", number(overlay.luma_max)),
        format!("luma_max_smooth={}", number(overlay.luma_max_smooth)),
        format!("crop_top={}", overlay.crop.top),
        format!("crop_right={}", overlay.crop.right),
        format!("crop_bottom={}", overlay.crop.bottom),
        format!("crop_left={}", overlay.crop.left),
        String::new(),
    ]
    .join("\r\n")
}

/// `smoothstep(edge0, edge1, x)`.
pub fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    if edge1 <= edge0 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = 1.0_f64.min(0.0_f64.max((x - edge0) / (edge1 - edge0)));
    t * t * (3.0 - 2.0 * t)
}

/// `keyAlpha(value, r, g, b)`: непрозрачность пикселя 0..1 после маски; `r`, `g`, `b` — 0..1.
pub fn key_alpha(value: &Overlay, r: f64, g: f64, b: f64) -> f64 {
    match value.key {
        "color" => {
            let color = value.key_color;
            let dr = r - f64::from(color.r) / 255.0;
            let dg = g - f64::from(color.g) / 255.0;
            let db = b - f64::from(color.b) / 255.0;
            let base = (dr * dr + dg * dg + db * db).sqrt() - f64::from(value.similarity) / 1000.0;
            if base <= 0.0 {
                return 0.0;
            }
            1.0_f64.min(base / (f64::from(value.smoothness) / 1000.0))
        }
        "luma" => {
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let low = smoothstep(value.luma_min, value.luma_min + value.luma_min_smooth, luma);
            let high = if value.luma_max_smooth <= 0.0 {
                if luma > value.luma_max { 0.0 } else { 1.0 }
            } else {
                1.0 - smoothstep(value.luma_max - value.luma_max_smooth, value.luma_max, luma)
            };
            low * high
        }
        _ => 1.0,
    }
}

/// Результат [`crop_rect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CropRect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

/// `cropRect(value, width, height)`: обрезка снимка; если от картинки остаётся меньше 8
/// пикселей, обрезка по этой оси не применяется (так же делает DLL).
pub fn crop_rect(value: &Overlay, width: i64, height: i64) -> CropRect {
    let crop = value.crop;
    let (mut left, mut right) = (i64::from(crop.left), i64::from(crop.right));
    let (mut top, mut bottom) = (i64::from(crop.top), i64::from(crop.bottom));
    if left + right > width - 8 {
        left = 0;
        right = 0;
    }
    if top + bottom > height - 8 {
        top = 0;
        bottom = 0;
    }
    CropRect {
        x: left,
        y: top,
        width: width - left - right,
        height: height - top - bottom,
    }
}

/// Результат [`placement`]: где картинка встанет в кадре.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub left: i64,
    pub top: i64,
    pub width: i64,
    pub height: i64,
}

/// `placement(value, screenWidth, screenHeight, width, height)`: где картинка `width`×`height`
/// встанет в кадре `screenWidth`×`screenHeight` (как `place` в DLL); `None` — кадр или картинка
/// слишком малы.
pub fn placement(
    value: &Overlay,
    screen_width: f64,
    screen_height: f64,
    width: f64,
    height: f64,
) -> Option<Placement> {
    if screen_width < 16.0 || screen_height < 16.0 || width < 1.0 || height < 1.0 {
        return None;
    }
    let mut h = screen_height * value.height / 100.0;
    let mut w = h * width / height;
    if w > screen_width {
        w = screen_width;
        h = w * height / width;
    }
    let out_width = math_round(w);
    let out_height = math_round(h);
    Some(Placement {
        left: math_round((screen_width - out_width) * value.x / 100.0) as i64,
        top: math_round((screen_height - out_height) * value.y / 100.0) as i64,
        width: out_width as i64,
        height: out_height as i64,
    })
}

/// Результат [`position_of`]: `x` и `y` в процентах.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// `positionOf(left, top, screenWidth, screenHeight, width, height)`: свободная точка (x, y в
/// процентах) для картинки с левым верхним углом в (`left`, `top`) — обратное к [`placement`],
/// для перетаскивания в предпросмотре.
pub fn position_of(
    left: f64,
    top: f64,
    screen_width: f64,
    screen_height: f64,
    width: f64,
    height: f64,
) -> Position {
    let axis = |start: f64, free: f64| {
        if free > 0.0 {
            math_round(100.0_f64.min(0.0_f64.max(start / free * 100.0)) * 10.0) / 10.0
        } else {
            50.0
        }
    };
    Position {
        x: axis(left, screen_width - width),
        y: axis(top, screen_height - height),
    }
}

/// `isSentinel(bgrx, at)`: пиксель с номером байта `at` не закрашен окном.
pub fn is_sentinel(bgrx: &[u8], at: usize) -> bool {
    bgrx.get(at..at + 4) == Some(&SENTINEL[..])
}

/// Цвет фона LiveSplit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// `backgroundColor(bgrx, width, height)`: цвет фона LiveSplit для цветового ключа — левый
/// нижний угол снимка, где у стандартного макета нет текста. Незакрашенные полосы пропускаются.
pub fn background_color(bgrx: &[u8], width: usize, height: usize) -> Option<Rgb> {
    let mut step = 2;
    while step < width.min(height) {
        let at = ((height - step) * width + step) * 4;
        if !is_sentinel(bgrx, at) {
            let byte = |offset: usize| bgrx.get(at + offset).copied().unwrap_or(0);
            return Some(Rgb {
                r: byte(2),
                g: byte(1),
                b: byte(0),
            });
        }
        step += 2;
    }
    None
}

/// Порядок байтов пикселя снимка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelOrder {
    /// Снимок окна LiveSplit (`livesplit_window_capture`, под Wine — помощник): B, G, R, X.
    Bgrx,
    /// Примерный LiveSplit, нарисованный в canvas: R, G, B, A.
    Rgba,
}

/// Результат [`key_image`]: RGBA, альфа не предумножена.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// `keyImage(value, bgrx, width, height, order)`: снимок окна LiveSplit (сверху вниз) → RGBA
/// для canvas, с обрезкой и маской. Незакрашенное место прозрачно при любой маске, как в DLL.
pub fn key_image(
    value: &Overlay,
    bgrx: &[u8],
    width: usize,
    height: usize,
    order: PixelOrder,
) -> KeyedImage {
    let (red, blue) = match order {
        PixelOrder::Rgba => (0, 2),
        PixelOrder::Bgrx => (2, 0),
    };
    let rect = crop_rect(value, width as i64, height as i64);
    let (rect_x, rect_y) = (rect.x as usize, rect.y as usize);
    let (rect_width, rect_height) = (rect.width as usize, rect.height as usize);
    let mut data = vec![0_u8; rect_width * rect_height * 4];
    let byte = |at: usize| bgrx.get(at).copied().unwrap_or(0);
    for y in 0..rect_height {
        let mut source = ((y + rect_y) * width + rect_x) * 4;
        let mut target = y * rect_width * 4;
        for _ in 0..rect_width {
            let (r, g, b) = (byte(source + red), byte(source + 1), byte(source + blue));
            if order == PixelOrder::Bgrx && is_sentinel(bgrx, source) {
                source += 4;
                target += 4;
                continue;
            }
            data[target] = r;
            data[target + 1] = g;
            data[target + 2] = b;
            data[target + 3] = if value.key == "none" {
                255
            } else {
                let alpha = key_alpha(
                    value,
                    f64::from(r) / 255.0,
                    f64::from(g) / 255.0,
                    f64::from(b) / 255.0,
                );
                math_round(alpha * 255.0) as u8
            };
            source += 4;
            target += 4;
        }
    }
    KeyedImage {
        width: rect_width,
        height: rect_height,
        data,
    }
}
