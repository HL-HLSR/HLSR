//! Оформление: панель (в Electron 0.4.9 стеклянная только колонка — она в `glass.rs`, тема
//! Liquid Glass удалена; остальные панели — заливка `--l-surface` и рамка), тени и кольцо `inset` в CSS px макета,
//! многостоповый линейный градиент полосами по 2 стопа (в GPUI градиент только из двух),
//! `radial-gradient` — маской SVG с цветом.

use crate::scale::{u, zpx};
use crate::theme::{Gradient, Stop, Theme};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    BoxShadow, Div, Hsla, Pixels, Styled, Svg, div, linear_color_stop, linear_gradient, point, px,
    relative, svg,
};
use std::fmt::Write as _;

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
            let (start, end) = (a.pos == 0., b.pos == 1.);
            let strip = div().absolute().bg(fill);
            match angle as u32 {
                90 => strip
                    .top_0()
                    .bottom_0()
                    .left(relative(a.pos))
                    .w(relative(b.pos - a.pos))
                    .when(start, |s| s.rounded_tl(radius).rounded_bl(radius))
                    .when(end, |s| s.rounded_tr(radius).rounded_br(radius)),
                270 => strip
                    .top_0()
                    .bottom_0()
                    .right(relative(a.pos))
                    .w(relative(b.pos - a.pos))
                    .when(start, |s| s.rounded_tr(radius).rounded_br(radius))
                    .when(end, |s| s.rounded_tl(radius).rounded_bl(radius)),
                180 => strip
                    .left_0()
                    .right_0()
                    .top(relative(a.pos))
                    .h(relative(b.pos - a.pos))
                    .when(start, |s| s.rounded_tl(radius).rounded_tr(radius))
                    .when(end, |s| s.rounded_bl(radius).rounded_br(radius)),
                _ => strip
                    .left_0()
                    .right_0()
                    .bottom(relative(a.pos))
                    .h(relative(b.pos - a.pos))
                    .when(start, |s| s.rounded_bl(radius).rounded_br(radius))
                    .when(end, |s| s.rounded_tl(radius).rounded_tr(radius)),
            }
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

/// `radial-gradient(ellipse rx ry at cx cy, …)` одного цвета: маска SVG (альфа по стопам)
/// и цвет `color`, как у одноцветного значка. Доли — от размера блока (`46%` → 0.46).
/// Маска не зависит от темы, поэтому при перетекании темы новых растров нет.
#[derive(Debug, Clone, PartialEq)]
pub struct Radial {
    pub center: (f32, f32),
    pub radius: (f32, f32),
    /// (позиция 0…1, альфа маски 0…1).
    pub stops: Vec<(f32, f32)>,
}

impl Radial {
    /// Блок `width`×`height` CSS px с этим градиентом (обычно `absolute` поверх места).
    pub fn paint(&self, width: f32, height: f32, color: Hsla) -> Svg {
        svg()
            .data(self.svg(width, height).as_bytes())
            .w(u(width))
            .h(u(height))
            .text_color(color)
    }

    /// SVG маски: стороны как у блока (растр GPUI сохраняет пропорции SVG), градиент в долях.
    fn svg(&self, width: f32, height: f32) -> String {
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
        out.push_str("</radialGradient><rect width='1' height='1' fill='url(#g)'/></svg>");
        out
    }
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
        let text = radial.svg(1600., 900.);
        assert!(text.contains("width='1600' height='900'"), "{text}");
        assert!(text.contains("matrix(0.46 0 0 0.52 0.7 0.4)"), "{text}");
        assert!(text.contains("offset='0.7' stop-color='#fff' stop-opacity='0'"));
    }
}
