//! Вид превью с зумом и панорамой (`GhostingPreviewViewport.vue` 0.4.7, задача 33).
//! Кадр хранится в пикселях источника; вид держит зум относительно «вписать» и точку
//! источника в центре окна вида, поэтому смена размера не сдвигает то, на что смотрят.
//! У каждого жеста мышью есть кнопка или клавиша.
//!
//! Все координаты — логические px окна (CSS px 0.4.7).

/// Шаг кнопок и клавиш `+`/`−`.
pub const STEP: f64 = 1.25;
/// Наибольший масштаб: пикселей экрана на пиксель источника.
pub const MAX_SCALE: f64 = 3.0;

/// Состояние вида: зум относительно «вписать» и точка источника в центре.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub zoom: f64,
    pub cx: f64,
    pub cy: f64,
}

/// Размер окна вида (`box`) и кадра источника.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub box_width: f64,
    pub box_height: f64,
    pub frame_width: f64,
    pub frame_height: f64,
}

impl Geometry {
    /// Пикселей экрана на пиксель источника, когда виден весь кадр.
    pub fn fit(&self) -> f64 {
        if self.box_width > 0.0 && self.box_height > 0.0 {
            (self.box_width / self.frame_width).min(self.box_height / self.frame_height)
        } else {
            1.0
        }
    }

    pub fn max_zoom(&self) -> f64 {
        (MAX_SCALE / self.fit()).max(1.0)
    }

    fn measured(&self) -> bool {
        self.box_width > 0.0
    }
}

/// Вид с начала: весь кадр (`data()` и `fitView()` 0.4.7).
pub fn initial(g: &Geometry) -> View {
    View {
        zoom: 1.0,
        cx: g.frame_width / 2.0,
        cy: g.frame_height / 2.0,
    }
}

/// Что показывает вид: масштаб, подпись, состояния кнопок и где лежит кадр.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paint {
    pub scale: f64,
    /// `{{ percent }}%`.
    pub percent: i64,
    pub zoomed: bool,
    pub at_fit: bool,
    pub actual: bool,
    pub can_zoom_in: bool,
    pub can_zoom_out: bool,
    /// Левый верхний угол и размер кадра в окне вида.
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

pub fn paint(view: &View, g: &Geometry) -> Paint {
    let scale = g.fit() * view.zoom;
    let zoomed = view.zoom > 1.001;
    Paint {
        scale,
        percent: hlsr_core::js::math_round(scale * 100.0) as i64,
        zoomed,
        at_fit: !zoomed,
        actual: (scale - 1.0).abs() < 0.005,
        can_zoom_in: view.zoom < g.max_zoom() - 0.001,
        can_zoom_out: zoomed,
        left: g.box_width / 2.0 - view.cx * scale,
        top: g.box_height / 2.0 - view.cy * scale,
        width: g.frame_width * scale,
        height: g.frame_height * scale,
    }
}

/// Кадр закрывает окно вида; по оси, которую он не заполняет, — по центру.
pub fn clamp(view: &View, g: &Geometry) -> View {
    if !g.measured() {
        return *view;
    }
    let scale = g.fit() * view.zoom;
    let visible_w = g.box_width / scale;
    let visible_h = g.box_height / scale;
    let axis = |c: f64, visible: f64, size: f64| {
        if visible >= size {
            size / 2.0
        } else {
            (size - visible / 2.0).min((visible / 2.0).max(c))
        }
    };
    View {
        zoom: view.zoom,
        cx: axis(view.cx, visible_w, g.frame_width),
        cy: axis(view.cy, visible_h, g.frame_height),
    }
}

/// Точка источника под `(x, y)` окна вида остаётся под ней (`setScale`).
pub fn set_scale(view: &View, g: &Geometry, next: f64, x: f64, y: f64) -> View {
    if !g.measured() {
        return *view;
    }
    let fit = g.fit();
    let before = fit * view.zoom;
    let px = view.cx + (x - g.box_width / 2.0) / before;
    let py = view.cy + (y - g.box_height / 2.0) / before;
    let zoom = g.max_zoom().min((next / fit).max(1.0));
    let after = fit * zoom;
    clamp(
        &View {
            zoom,
            cx: px - (x - g.box_width / 2.0) / after,
            cy: py - (y - g.box_height / 2.0) / after,
        },
        g,
    )
}

/// `zoomBy(factor, x, y)`; без точки — центр окна.
pub fn zoom_by(view: &View, g: &Geometry, factor: f64, at: Option<(f64, f64)>) -> View {
    let (x, y) = at.unwrap_or((g.box_width / 2.0, g.box_height / 2.0));
    set_scale(view, g, g.fit() * view.zoom * factor, x, y)
}

/// `actualSize(x, y)`: 100 %.
pub fn actual_size(view: &View, g: &Geometry, at: Option<(f64, f64)>) -> View {
    let (x, y) = at.unwrap_or((g.box_width / 2.0, g.box_height / 2.0));
    set_scale(view, g, 1.0, x, y)
}

/// `pan(dx, dy)` в пикселях источника.
pub fn pan(view: &View, g: &Geometry, dx: f64, dy: f64) -> View {
    clamp(
        &View {
            zoom: view.zoom,
            cx: view.cx + dx,
            cy: view.cy + dy,
        },
        g,
    )
}

/// Колесо: `deltaY` события Chromium (вниз — больше нуля, px).
pub fn wheel(view: &View, g: &Geometry, delta_y: f64, x: f64, y: f64) -> View {
    if !g.measured() {
        return *view;
    }
    zoom_by(view, g, (-delta_y * 0.0018).exp(), Some((x, y)))
}

/// Точка источника под `(x, y)` окна вида.
pub fn source_point(view: &View, g: &Geometry, x: f64, y: f64) -> (f64, f64) {
    let scale = g.fit() * view.zoom;
    (
        view.cx + (x - g.box_width / 2.0) / scale,
        view.cy + (y - g.box_height / 2.0) / scale,
    )
}

/// Перетаскивание кадра: от вида в начале на сдвиг указателя.
pub fn drag(start: &View, g: &Geometry, dx: f64, dy: f64) -> View {
    let scale = g.fit() * start.zoom;
    clamp(
        &View {
            zoom: start.zoom,
            cx: start.cx - dx / scale,
            cy: start.cy - dy / scale,
        },
        g,
    )
}

/// Двойной щелчок вне ручки: от 100 % и крупнее — весь кадр, иначе 100 % у указателя.
/// Зум хранится в разметке как f32: 100 % после «1» — 0,99999994, поэтому допуск 1e-6.
pub fn double_click(view: &View, g: &Geometry, x: f64, y: f64) -> View {
    if g.fit() * view.zoom >= 1.0 - 1e-6 {
        initial(g)
    } else {
        actual_size(view, g, Some((x, y)))
    }
}

/// Клавиша вида (`onKey`): `key` — `KeyboardEvent.key` (`+`, `=`, `-`, `_`, `0`, `1`,
/// `ArrowLeft`…). Не своя клавиша — `None` (событие уходит дальше).
pub fn key(view: &View, g: &Geometry, key: &str) -> Option<View> {
    let scale = g.fit() * view.zoom;
    let step = (g.box_width / scale * 0.12).max(24.0);
    Some(match key {
        "+" | "=" => zoom_by(view, g, STEP, None),
        "-" | "_" | "\u{2212}" => zoom_by(view, g, 1.0 / STEP, None),
        "0" => initial(g),
        "1" => actual_size(view, g, None),
        "ArrowLeft" => pan(view, g, -step, 0.0),
        "ArrowRight" => pan(view, g, step, 0.0),
        "ArrowUp" => pan(view, g, 0.0, -step),
        "ArrowDown" => pan(view, g, 0.0, step),
        _ => return None,
    })
}

/// Перетаскиваемый прямоугольник (оверлей LiveSplit) в пикселях кадра.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handle {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl Handle {
    pub fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.left
            && x <= self.left + self.width
            && y >= self.top
            && y <= self.top + self.height
    }
}

/// Рамка ручки в физических px окна (`paint` 0.4.7: линия 1 физ. px по центрам
/// пикселей): левый верхний угол и размер прямоугольника линии.
pub fn handle_rect(paint: &Paint, handle: &Handle, ratio: f64) -> (f64, f64, f64, f64) {
    let scale = paint.scale * ratio;
    let left = paint.left * ratio;
    let top = paint.top * ratio;
    let x = hlsr_core::js::math_round(left + handle.left * scale) + 0.5;
    let y = hlsr_core::js::math_round(top + handle.top * scale) + 0.5;
    let w = hlsr_core::js::math_round(handle.width * scale) - 1.0;
    let h = hlsr_core::js::math_round(handle.height * scale) - 1.0;
    (x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: Geometry = Geometry {
        box_width: 480.0,
        box_height: 270.0,
        frame_width: 1600.0,
        frame_height: 900.0,
    };

    #[test]
    fn fit_max_zoom_and_percent_follow_the_box() {
        assert!((G.fit() - 0.3).abs() < 1e-12);
        // 300 % — 3 / 0.3 = 10 от «вписать».
        assert!((G.max_zoom() - 10.0).abs() < 1e-12);
        let p = paint(&initial(&G), &G);
        assert_eq!(p.percent, 30);
        assert!(p.at_fit && !p.can_zoom_out && p.can_zoom_in);
        // Вид больше кадра в 3 раза: «вписать» уже 300 % — увеличивать нельзя.
        let big = Geometry {
            box_width: 4800.0,
            box_height: 2700.0,
            ..G
        };
        assert_eq!(big.max_zoom(), 1.0);
        assert!(!paint(&initial(&big), &big).can_zoom_in);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer_and_clamps_to_the_frame() {
        let v = zoom_by(&initial(&G), &G, 2.0, Some((0.0, 0.0)));
        // Левый верхний угол вида остался углом кадра.
        assert_eq!(source_point(&v, &G, 0.0, 0.0), (0.0, 0.0));
        // Панорама за край — до края.
        let v = pan(&v, &G, -10_000.0, -10_000.0);
        assert_eq!((v.cx, v.cy), (400.0, 225.0));
        // Меньше «вписать» нельзя.
        let v = zoom_by(&v, &G, 0.01, None);
        assert_eq!(v, initial(&G));
    }

    #[test]
    fn keys_step_by_twelve_percent_of_the_visible_width_but_not_less_than_24_px() {
        let v = zoom_by(&initial(&G), &G, 10.0, None);
        // 300 %: видно 160 px кадра, 12 % — 19,2 → 24 px.
        let moved = key(&v, &G, "ArrowRight").unwrap();
        assert!((moved.cx - v.cx - 24.0).abs() < 1e-9);
        assert!(key(&v, &G, "x").is_none());
        assert_eq!(key(&v, &G, "0"), Some(initial(&G)));
        let actual = key(&v, &G, "1").unwrap();
        assert_eq!(paint(&actual, &G).percent, 100);
    }

    #[test]
    fn handle_frame_is_one_device_pixel_on_pixel_centres() {
        let p = paint(&initial(&G), &G);
        let handle = Handle {
            left: 100.0,
            top: 50.0,
            width: 300.0,
            height: 200.0,
        };
        assert!(handle.contains((100.0, 250.0)) && !handle.contains((99.0, 50.0)));
        // 30 % × 1,25: (100 × 0,375) = 37,5 → 38 + 0,5.
        let (x, y, w, h) = handle_rect(&p, &handle, 1.25);
        assert_eq!((x, y, w, h), (38.5, 19.5, 112.0, 74.0));
    }
}
