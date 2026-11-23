//! Оформление: панель (в Electron 0.4.9 стеклянная только колонка — она в `glass.rs`, тема
//! Liquid Glass удалена; остальные панели — заливка `--l-surface` и рамка), тени и кольцо `inset` в CSS px макета,
//! многостоповый линейный градиент полосами по 2 стопа (в GPUI градиент только из двух),
//! `radial-gradient` — маской SVG с цветом.

use crate::scale::{u, zoom, zpx};
use crate::theme::{Gradient, Stop, Theme};
use gpui::{
    BoxShadow, Canvas, Corners, Div, Hsla, ParentElement, Pixels, RenderImage, Rgba, Styled,
    canvas, div, linear_color_stop, linear_gradient, point, px,
};
use std::cell::RefCell;
use std::collections::HashMap;
#[cfg(test)]
use std::fmt::Write as _;
use std::sync::Arc;

/// `box-shadow: x y blur spread color` в CSS px макета.
pub fn shadow(zoom: f32, x: f32, y: f32, blur: f32, spread: f32, color: Hsla) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(zpx(zoom, x), zpx(zoom, y)),
        // У GPUI `blur_radius` — σ гаусса (шейдер тени, поле 3σ), у CSS σ = blur / 2.
        blur_radius: zpx(zoom, blur / 2.),
        spread_radius: zpx(zoom, spread),
        inset: false,
    }
}

/// `box-shadow: inset 0 0 0 <width> color` — рамка, которая не занимает места в раскладке.
/// Линия 1 px остаётся 1 px при любом зуме, как у Chromium.
pub fn ring(zoom: f32, width: f32, color: Hsla) -> BoxShadow {
    let spread = if width == 1. {
        px(1.)
    } else {
        zpx(zoom, width)
    };
    BoxShadow {
        color,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: spread,
        inset: true,
    }
}

/// Панель: заливка `--l-surface`, рамка `--l-border`, радиус `--l-radius`. Тени — у места.
pub fn panel(theme: &Theme) -> Div {
    div()
        .bg(theme.color("l-surface"))
        .border_1()
        .border_color(theme.color("l-border"))
        .rounded(theme.rem("l-radius"))
}

/// `--l-bg-scrim` (launcher.css 0.4.9) цветом `--l-shade-rgb`: угол CSS и стопы (доля,
/// прозрачность), нижний слой первым.
pub const SCRIM: [(f32, &[(f32, f32)]); 4] = [
    (270., &[(0., 0.3), (0.12, 0.07), (0.24, 0.)]),
    (180., &[(0., 0.42), (0.07, 0.1), (0.14, 0.)]),
    (0., &[(0., 0.88), (0.13, 0.45), (0.28, 0.06), (0.42, 0.)]),
    (
        90.,
        &[
            (0., 0.86),
            (0.18, 0.66),
            (0.34, 0.36),
            (0.48, 0.1),
            (0.62, 0.),
        ],
    ),
];

/// Прозрачность [`SCRIM`] в точке (x, y) окна `view` (CSS px). Слои одного цвета, поэтому
/// вместе — тот же цвет с прозрачностью 1 − Π(1 − a).
pub fn scrim_alpha(x: f32, y: f32, (width, height): (f32, f32)) -> f32 {
    let along = |stops: &[(f32, f32)], t: f32| {
        let mut prev = stops[0];
        for &(pos, a) in stops {
            if t <= pos {
                return if pos > prev.0 {
                    prev.1 + (a - prev.1) * (t - prev.0) / (pos - prev.0)
                } else {
                    a
                };
            }
            prev = (pos, a);
        }
        prev.1
    };
    let clear: f32 = SCRIM
        .iter()
        .map(|&(angle, stops)| {
            let t = match angle as u32 {
                0 => 1. - y / height,
                90 => x / width,
                180 => y / height,
                _ => 1. - x / width,
            };
            1. - along(stops, t)
        })
        .product();
    1. - clear
}

/// Многостоповый `linear-gradient` вдоль оси (CSS 0/90/180/270deg) полосами по 2 стопа.
/// Полосы — `absolute` в родителе с `relative`. GPUI режет детей прямоугольником, поэтому
/// полосы у краёв скругляют свои углы на `radius` (у окна — радиус окна, иначе 0).
pub fn strips(gradient: &Gradient, radius: Pixels) -> Vec<Div> {
    strips_with_corners(gradient, gpui::Corners::all(radius))
}

pub(crate) fn strips_with_corners(gradient: &Gradient, corners: gpui::Corners<Pixels>) -> Vec<Div> {
    let mut stops = gradient.stops.clone();
    let (Some(first), Some(last)) = (stops.first().copied(), stops.last().copied()) else {
        return Vec::new();
    };
    // До первого и после последнего стопа CSS тянет крайний цвет.
    if first.pos > 0. {
        stops.insert(0, Stop { pos: 0., ..first });
    }
    if last.pos < 1. {
        stops.push(Stop { pos: 1., ..last });
    }
    let stops = premultiplied(stops);
    debug_assert!(
        gradient.angle % 90. == 0.,
        "полосы только вдоль оси, угол {}",
        gradient.angle
    );
    // Другой угол (не из themes.json) — к ближайшей оси, а не молча снизу вверх.
    let angle = (gradient.angle / 90.).round().rem_euclid(4.) * 90.;
    stops
        .windows(2)
        .filter(|pair| pair[1].pos > pair[0].pos)
        .map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let fill = linear_gradient(
                angle,
                linear_color_stop(a.color, 0.),
                linear_color_stop(b.color, 1.),
            );
            div().absolute().size_full().child(
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let horizontal = angle == 90. || angle == 270.;
                        let (start, end) = if angle == 90. || angle == 180. {
                            (a.pos, b.pos)
                        } else {
                            (1. - b.pos, 1. - a.pos)
                        };
                        let (origin, length) = if horizontal {
                            (bounds.left(), bounds.size.width)
                        } else {
                            (bounds.top(), bounds.size.height)
                        };
                        // Соседние полосы используют одну координату стыка. Независимо
                        // округлённые inset и width при смене ширины оставляли щель.
                        let edge = |t| {
                            let px = origin + length * t;
                            if t == 0. || t == 1. {
                                px
                            } else {
                                window.pixel_snap(px)
                            }
                        };
                        let (left, top, right, bottom) = if horizontal {
                            (edge(start), bounds.top(), edge(end), bounds.bottom())
                        } else {
                            (bounds.left(), edge(start), bounds.right(), edge(end))
                        };
                        if right <= left || bottom <= top {
                            return;
                        }
                        let radius = |show, radius| if show { radius } else { px(0.) };
                        let at_left = !horizontal || start == 0.;
                        let at_right = !horizontal || end == 1.;
                        let at_top = horizontal || start == 0.;
                        let at_bottom = horizontal || end == 1.;
                        window.paint_quad(
                            gpui::fill(
                                gpui::Bounds::from_corners(point(left, top), point(right, bottom)),
                                fill,
                            )
                            .corner_radii(gpui::Corners {
                                top_left: radius(at_left && at_top, corners.top_left),
                                top_right: radius(at_right && at_top, corners.top_right),
                                bottom_left: radius(at_left && at_bottom, corners.bottom_left),
                                bottom_right: radius(at_right && at_bottom, corners.bottom_right),
                            }),
                        );
                    },
                )
                .size_full(),
            )
        })
        .collect()
}

/// CSS смешивает стопы градиента в предумноженных цветах, GPUI — в прямых: между стопами разного
/// цвета и разной прозрачности (`--l-bg-fallback` Liquid Glass: белый .07 → непрозрачный тёмный)
/// прямое смешивание даёт светлую дымку. Там пара делится на 8 полос с цветами, смешанными по CSS.
fn premultiplied(stops: Vec<Stop>) -> Vec<Stop> {
    const STEPS: usize = 8;
    let mut out = Vec::with_capacity(stops.len());
    for pair in stops.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        out.push(a);
        let (ca, cb) = (a.color, b.color);
        let tinted = (ca.r, ca.g, ca.b) != (cb.r, cb.g, cb.b);
        if tinted && ca.a != cb.a && b.pos > a.pos {
            for k in 1..STEPS {
                let t = k as f32 / STEPS as f32;
                out.push(Stop {
                    color: crate::motion::mix(ca.into(), cb.into(), t).into(),
                    pos: a.pos + (b.pos - a.pos) * t,
                });
            }
        }
    }
    out.extend(stops.last().copied());
    out
}

/// `radial-gradient(ellipse rx ry at cx cy, …)` одного цвета: запечённая компактная 64×64
/// текстура с билинейным масштабированием и скруглением на GPU (`window.paint_image`) — без
/// полноразмерной растеризации SVG через `resvg` в потоке интерфейса.
#[derive(Debug, Clone, PartialEq)]
pub struct Radial {
    pub center: (f32, f32),
    pub radius: (f32, f32),
    /// (позиция 0…1, альфа маски 0…1).
    pub stops: Vec<(f32, f32)>,
}

const RADIAL_SIDE: u32 = 64;
const RADIAL_CACHE_LIMIT: usize = 64;

#[derive(Hash, PartialEq, Eq)]
struct RadialKey {
    cx: u32,
    cy: u32,
    rx: u32,
    ry: u32,
    stops: Vec<(u32, u32)>,
    rgba: [u8; 4],
}

fn eval_stops(stops: &[(f32, f32)], t: f32) -> f32 {
    if stops.is_empty() {
        return 0.;
    }
    if t <= stops[0].0 {
        return stops[0].1;
    }
    for pair in stops.windows(2) {
        let (p0, a0) = pair[0];
        let (p1, a1) = pair[1];
        if t <= p1 {
            let k = if p1 > p0 { (t - p0) / (p1 - p0) } else { 0. };
            return a0 + (a1 - a0) * k;
        }
    }
    stops.last().map_or(0., |&(_, a)| a)
}

fn radial_image(
    radial: &Radial,
    color: Hsla,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> Arc<RenderImage> {
    thread_local! {
        static CACHE: RefCell<HashMap<RadialKey, Arc<RenderImage>>> = RefCell::new(HashMap::new());
    }
    let c = Rgba::from(color);
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    let rgba = [byte(c.r), byte(c.g), byte(c.b), byte(c.a)];
    let key = RadialKey {
        cx: radial.center.0.to_bits(),
        cy: radial.center.1.to_bits(),
        rx: radial.radius.0.to_bits(),
        ry: radial.radius.1.to_bits(),
        stops: radial
            .stops
            .iter()
            .map(|&(p, a)| (p.to_bits(), a.to_bits()))
            .collect(),
        rgba,
    };
    CACHE.with(|cell| {
        let mut map = cell.borrow_mut();
        if let Some(hit) = map.get(&key) {
            return hit.clone();
        }
        if map.len() >= RADIAL_CACHE_LIMIT {
            for (_, image) in map.drain() {
                cx.drop_image(image, Some(window));
            }
        }
        let side = RADIAL_SIDE;
        let inv = 1. / side as f32;
        let (cx0, cy0) = radial.center;
        let (rx, ry) = (
            radial.radius.0.max(f32::EPSILON),
            radial.radius.1.max(f32::EPSILON),
        );
        let base_a = c.a.clamp(0., 1.);
        let [r, g, b, _] = rgba;
        let mut bytes = Vec::with_capacity((side * side * 4) as usize);
        for y in 0..side {
            let ny = ((y as f32 + 0.5) * inv - cy0) / ry;
            let ny2 = ny * ny;
            for x in 0..side {
                let nx = ((x as f32 + 0.5) * inv - cx0) / rx;
                let d = (nx * nx + ny2).sqrt();
                let a = (eval_stops(&radial.stops, d) * base_a).clamp(0., 1.);
                bytes.extend_from_slice(&[b, g, r, (a * 255.).round() as u8]);
            }
        }
        let buffer =
            ::image::RgbaImage::from_raw(side, side, bytes).expect("размер совпадает с байтами");
        let image = Arc::new(RenderImage::new([::image::Frame::new(buffer)]));
        map.insert(key, image.clone());
        image
    })
}

impl Radial {
    /// Блок `width`×`height` CSS px с этим градиентом (обычно `absolute` поверх места).
    pub fn paint(&self, width: f32, height: f32, color: Hsla) -> Canvas<()> {
        self.paint_corners(width, height, (0., 0.), color)
    }

    /// Блок `width`×`height` CSS px со скруглением `radius` верхних углов.
    pub fn paint_rounded_top(
        &self,
        width: f32,
        height: f32,
        radius: f32,
        color: Hsla,
    ) -> Canvas<()> {
        self.paint_corners(width, height, (radius, 0.), color)
    }

    /// Блок `width`×`height` CSS px со скруглением `radius` всех четырёх углов.
    pub fn paint_rounded(&self, width: f32, height: f32, radius: f32, color: Hsla) -> Canvas<()> {
        self.paint_corners(width, height, (radius, radius), color)
    }

    fn paint_corners(
        &self,
        width: f32,
        height: f32,
        (top_r, bottom_r): (f32, f32),
        color: Hsla,
    ) -> Canvas<()> {
        let radial = self.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
                    return;
                }
                let z = zoom(window);
                let top = zpx(z, top_r);
                let bottom = zpx(z, bottom_r);
                let corners = Corners {
                    top_left: top,
                    top_right: top,
                    bottom_right: bottom,
                    bottom_left: bottom,
                };
                let image = radial_image(&radial, color, window, cx);
                window
                    .paint_image(bounds, bounds, corners, image, 0, false)
                    .ok();
            },
        )
        .w(u(width))
        .h(u(height))
    }

    /// SVG маски (оставлено для проверки геометрии градиента в тестах).
    #[cfg(test)]
    fn svg(&self, width: f32, height: f32, radius: f32) -> String {
        let (cx, cy) = self.center;
        let (rx, ry) = self.radius;
        let mut out = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='{height}' \
             viewBox='0 0 1 1' preserveAspectRatio='none'><radialGradient id='g' \
             gradientUnits='userSpaceOnUse' cx='0' cy='0' r='1' \
             gradientTransform='matrix({rx} 0 0 {ry} {cx} {cy})'>"
        );
        for (pos, alpha) in &self.stops {
            let _ = write!(
                out,
                "<stop offset='{pos}' stop-color='#fff' stop-opacity='{alpha}'/>"
            );
        }
        out.push_str("</radialGradient>");
        if radius > 0. && width > 0. && height > 0. {
            let x = (radius / width).min(0.5);
            let y = (radius / height).min(1.);
            let right = 1. - x;
            let _ = write!(
                out,
                "<path d='M0 {y} A{x} {y} 0 0 1 {x} 0 H{right} A{x} {y} 0 0 1 1 {y} V1 H0 Z' fill='url(#g)'/>"
            );
        } else {
            out.push_str("<rect width='1' height='1' fill='url(#g)'/>");
        }
        out.push_str("</svg>");
        out
    }
}

/// Знак HLSR — кольцо с точкой (`.launcher-topbar__logo-mark` 0.4.9 и `Logo` заставки): кольцо
/// `inset 0 0 0 <stroke>px currentColor` во всю коробку, по центру точка `dot` CSS px с
/// `transform: scale(<scale>)`. Рисуется путями от центра коробки, а не рамкой и блоком: GPUI
/// округляет рамки и коробки до пикселя экрана, и точка под курсором росла ступенями по 1 px
/// со сдвигом от центра, а кольцо 2 px на масштабе 125 % выходило в 2 пикселя вместо 2,5.
/// Пути GPUI сглаживает по точной геометрии, как Chromium — тень и `scale()`.
pub fn logo_mark(stroke: f32, dot: f32, scale: f32, color: Hsla) -> gpui::Canvas<()> {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let zoom = crate::scale::zoom(window);
            let center = bounds.center();
            let outer = f32::from(bounds.size.width.min(bounds.size.height)) / 2.;
            let circle = |path: &mut gpui::PathBuilder, r: f32| {
                let (cx, cy) = (f32::from(center.x), f32::from(center.y));
                let radii = point(px(r), px(r));
                path.move_to(point(px(cx + r), px(cy)));
                path.arc_to(radii, px(0.), false, true, point(px(cx - r), px(cy)));
                path.arc_to(radii, px(0.), false, true, point(px(cx + r), px(cy)));
                path.close();
            };
            // Кольцо: два круга, заливка even-odd оставляет полосу между ними.
            let mut ring = gpui::PathBuilder::fill();
            circle(&mut ring, outer);
            circle(&mut ring, (outer - stroke * zoom).max(0.));
            if let Ok(path) = ring.build() {
                window.paint_path(path, color);
            }
            let mut point_path = gpui::PathBuilder::fill();
            circle(&mut point_path, dot * scale * zoom / 2.);
            if let Ok(path) = point_path.build() {
                window.paint_path(path, color);
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Rgba;

    #[test]
    fn strips_cover_every_stop_pair_and_pad_ends() {
        let color = |a| Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a,
        };
        let gradient = Gradient {
            angle: 0.,
            stops: vec![
                Stop {
                    color: color(0.96),
                    pos: 0.,
                },
                Stop {
                    color: color(0.86),
                    pos: 0.34,
                },
                Stop {
                    color: color(0.),
                    pos: 0.84,
                },
            ],
        };
        // 0–0.34, 0.34–0.84 и сплошная 0.84–1.
        assert_eq!(strips(&gradient, px(0.)).len(), 3);
        let empty = Gradient {
            angle: 0.,
            stops: Vec::new(),
        };
        assert!(strips(&empty, px(0.)).is_empty());
    }

    #[test]
    fn stops_of_different_colour_and_alpha_mix_premultiplied() {
        let stop = |r: f32, a: f32, pos: f32| Stop {
            color: Rgba { r, g: r, b: r, a },
            pos,
        };
        // Белый .07 → тёмный непрозрачный: 8 полос, середина тёмная (по CSS ~0,05, а не 0,5).
        let mixed = premultiplied(vec![stop(1., 0.07, 0.), stop(0.05, 1., 1.)]);
        assert_eq!(mixed.len(), 9);
        let mid = mixed[4].color;
        assert!((mid.a - 0.535).abs() < 1e-3 && mid.r < 0.12, "{mid:?}");
        // Один цвет с разной прозрачностью (затемнение) и ровные стопы не делятся.
        assert_eq!(
            premultiplied(vec![stop(0., 0.9, 0.), stop(0., 0., 1.)]).len(),
            2
        );
    }

    #[test]
    fn scrim_alpha_stacks_the_four_layers() {
        let view = (1600., 900.);
        // Левый верхний угол: сверху 0.42 и слева 0.86 → 1 − 0.58 · 0.14.
        assert!((scrim_alpha(0., 0., view) - 0.9188).abs() < 1e-4);
        // Середина: только левый слой, между 0.48 (0.1) и 0.62 (0).
        assert!((scrim_alpha(800., 450., view) - 0.1 * 0.12 / 0.14).abs() < 1e-4);
        // Правый край посередине высоты: только правый слой, 0.3.
        assert!((scrim_alpha(1600., 450., view) - 0.3).abs() < 1e-4);
    }

    #[test]
    fn radial_svg_maps_css_ellipse_to_box_fractions() {
        let radial = Radial {
            center: (0.7, 0.4),
            radius: (0.46, 0.52),
            stops: vec![(0., 0.05), (0.7, 0.)],
        };
        let text = radial.svg(1600., 900., 0.);
        assert!(text.contains("width='1600' height='900'"), "{text}");
        assert!(text.contains("matrix(0.46 0 0 0.52 0.7 0.4)"), "{text}");
        assert!(text.contains("offset='0.7' stop-color='#fff' stop-opacity='0'"));
    }
}
