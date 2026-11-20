//! Бегущая неоновая рамка окна профиля (`neon.scss` 0.4.9): кольцо 2 px у края окна, цвет —
//! `conic-gradient` из середины окна, который крутится по часовой (оборот за 6 с, анимация
//! `profile-neon` реестра). Подписчик — голубая, 100 % достижений — золотая.
//!
//! В GPUI нет конического градиента и маски кольца: кольцо рисуется кусками вдоль периметра,
//! у каждого куска свой цвет по его углу от середины окна. Прямые края — полосы по
//! [`STEP`] px, углы — рамка скруглённого квадрата угла, разрезанная маской на [`CORNER_CUTS`]
//! частей. Кадры просит элемент движения F4: под окном-слоем, в спрятанном окне и вне экрана
//! рамка стоит ([`crate::motion::AnimGate`]); без анимаций — неподвижна на угле 0.

use crate::motion::keyframes;
use crate::scale::zpx;
use gpui::{
    Bounds, ContentMask, Corners, Edges, ElementId, Hsla, IntoElement, Pixels, Rgba, Styled,
    Window, canvas, point, quad, size, transparent_black,
};

/// Длина куска прямого края, CSS px.
const STEP: f32 = 4.;
/// На сколько частей режется каждый угол.
const CORNER_CUTS: usize = 6;
/// Толщина кольца, CSS px (`padding: 2px`).
const WIDTH: f32 = 2.;

/// Цвета `conic-gradient(from 0deg, dim, dim 30%, b 55%, a 75%, dim)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub a: Rgba,
    pub b: Rgba,
    pub dim: Rgba,
}

const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a,
    }
}

/// Подписчик: `neon(profile-neon, #38bdf8, #60a5fa, rgba(56, 189, 248, .2))`.
pub const SUBSCRIBER: Palette = Palette {
    a: rgba(0x38, 0xbd, 0xf8, 1.),
    b: rgba(0x60, 0xa5, 0xfa, 1.),
    dim: rgba(56, 189, 248, 0.2),
};

/// 100 %: `.is-complete .neon::before` — `#f5a623` (75 %), `#ffd86b` (55 %).
pub const COMPLETE: Palette = Palette {
    a: rgba(0xf5, 0xa6, 0x23, 1.),
    b: rgba(0xff, 0xd8, 0x6b, 1.),
    dim: rgba(245, 197, 66, 0.2),
};

impl Palette {
    /// Цвет в доле `t` ∈ [0, 1) оборота (смешение с предумноженной альфой, как у Chromium).
    pub fn at(&self, t: f32) -> Rgba {
        let stops = [
            (0., self.dim),
            (0.3, self.dim),
            (0.55, self.b),
            (0.75, self.a),
            (1., self.dim),
        ];
        let t = t.rem_euclid(1.);
        let pair = stops
            .windows(2)
            .find(|pair| t <= pair[1].0)
            .unwrap_or(&stops[3..5]);
        let (from, to) = (pair[0], pair[1]);
        let k = if to.0 > from.0 {
            (t - from.0) / (to.0 - from.0)
        } else {
            0.
        };
        let alpha = from.1.a + (to.1.a - from.1.a) * k;
        let channel = |x: f32, y: f32| {
            if alpha > 0. {
                (x * from.1.a + (y * to.1.a - x * from.1.a) * k) / alpha
            } else {
                0.
            }
        };
        Rgba {
            r: channel(from.1.r, to.1.r),
            g: channel(from.1.g, to.1.g),
            b: channel(from.1.b, to.1.b),
            a: alpha,
        }
    }
}

/// Доля оборота точки `(x, y)` от середины `(cx, cy)` при повороте слоя на `rotation` градусов:
/// 0 — вверх, по часовой.
pub fn turn(x: f32, y: f32, cx: f32, cy: f32, rotation: f32) -> f32 {
    let angle = (x - cx).atan2(cy - y).to_degrees();
    (angle - rotation).rem_euclid(360.) / 360.
}

/// Слой рамки поверх окна (`absolute`, `inset: 0`, щелчки проходят): `radius` — CSS px
/// скругления окна, `zoom` — зум страницы.
pub fn neon(
    id: impl Into<ElementId>,
    palette: Palette,
    radius: f32,
    zoom: f32,
) -> impl IntoElement {
    keyframes(id, "profile-neon", move |frame| {
        let rotation = frame.rotation as f32;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| paint_ring(bounds, palette, radius, zoom, rotation, window),
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    })
}

/// Куски кольца: (место, цвет, скругление угла и стороны рамки) — общие для рисования и теста.
pub(crate) struct Piece {
    pub bounds: Bounds<Pixels>,
    pub mask: Bounds<Pixels>,
    pub color: Rgba,
    pub corners: Corners<Pixels>,
    pub borders: Edges<Pixels>,
}

/// Куски кольца толщиной `ring` CSS px у края `bounds` со скруглением `radius`: цвет куска —
/// `paint(x, y)` в точке края (px окна). Его же рисует радужная рамка достижения (S10).
pub(crate) fn pieces(
    bounds: Bounds<Pixels>,
    ring: f32,
    radius: f32,
    zoom: f32,
    paint: impl Fn(f32, f32) -> Rgba,
) -> Vec<Piece> {
    let w = zpx(zoom, ring);
    let r = zpx(zoom, radius).max(w);
    let (left, top) = (bounds.origin.x, bounds.origin.y);
    let (width, height) = (bounds.size.width, bounds.size.height);
    let color = |x: Pixels, y: Pixels| paint(f32::from(x), f32::from(y));
    let mut out = Vec::new();
    let mut strip =
        |origin: gpui::Point<Pixels>, extent: gpui::Size<Pixels>, at: gpui::Point<Pixels>| {
            let piece = Bounds::new(origin, extent);
            out.push(Piece {
                bounds: piece,
                mask: piece,
                color: color(at.x, at.y),
                corners: Corners::default(),
                borders: Edges::default(),
            });
        };
    // Прямые края между углами.
    let mut x = left + r;
    while x < left + width - r {
        let len = (left + width - r - x).min(zpx(zoom, STEP));
        strip(point(x, top), size(len, w), point(x + len / 2., top));
        strip(
            point(x, top + height - w),
            size(len, w),
            point(x + len / 2., top + height),
        );
        x += len;
    }
    let mut y = top + r;
    while y < top + height - r {
        let len = (top + height - r - y).min(zpx(zoom, STEP));
        strip(point(left, y), size(w, len), point(left, y + len / 2.));
        strip(
            point(left + width - w, y),
            size(w, len),
            point(left + width, y + len / 2.),
        );
        y += len;
    }
    // Углы: угол r × r рамки квадрата 2r × 2r со скруглением r, разрезанный по x. Шейдер GPUI
    // берёт скругление по четверти квада от его середины: у квадрата r × r дуга оставалась
    // только в его четверти — угол срезался коротким скосом.
    let corners = [
        (left, top, true, true),
        (left + width - r, top, false, true),
        (left, top + height - r, true, false),
        (left + width - r, top + height - r, false, false),
    ];
    for (x0, y0, on_left, on_top) in corners {
        let square = Bounds::new(
            point(
                if on_left { x0 } else { x0 - r },
                if on_top { y0 } else { y0 - r },
            ),
            size(r * 2., r * 2.),
        );
        let mut corner_radii = Corners::default();
        let mut borders = Edges::default();
        match (on_left, on_top) {
            (true, true) => corner_radii.top_left = r,
            (false, true) => corner_radii.top_right = r,
            (true, false) => corner_radii.bottom_left = r,
            (false, false) => corner_radii.bottom_right = r,
        }
        if on_left {
            borders.left = w;
        } else {
            borders.right = w;
        }
        if on_top {
            borders.top = w;
        } else {
            borders.bottom = w;
        }
        let cut = r / CORNER_CUTS as f32;
        for index in 0..CORNER_CUTS {
            let mask = Bounds::new(point(x0 + cut * index as f32, y0), size(cut, r));
            // Точка дуги над серединой куска.
            let mid = f32::from(cut) * (index as f32 + 0.5);
            let rr = f32::from(r);
            let from_center = if on_left { rr - mid } else { mid };
            let rise = rr - (rr * rr - from_center * from_center).max(0.).sqrt();
            let ax = f32::from(x0) + mid;
            let ay = if on_top {
                f32::from(y0) + rise
            } else {
                f32::from(y0) + rr - rise
            };
            out.push(Piece {
                bounds: square,
                mask,
                color: paint(ax, ay),
                corners: corner_radii,
                borders,
            });
        }
    }
    out
}

fn paint_ring(
    bounds: Bounds<Pixels>,
    palette: Palette,
    radius: f32,
    zoom: f32,
    rotation: f32,
    window: &mut Window,
) {
    let center = bounds.center();
    let (cx, cy) = (f32::from(center.x), f32::from(center.y));
    let list = pieces(bounds, WIDTH, radius, zoom, |x, y| {
        palette.at(turn(x, y, cx, cy, rotation))
    });
    paint_pieces(list, window);
}

/// Нарисовать куски [`pieces`].
pub(crate) fn paint_pieces(list: Vec<Piece>, window: &mut Window) {
    for piece in list {
        let color: Hsla = piece.color.into();
        window.with_content_mask(Some(ContentMask { bounds: piece.mask }), |window| {
            if piece.borders == Edges::default() {
                window.paint_quad(gpui::fill(piece.bounds, color));
            } else {
                window.paint_quad(quad(
                    piece.bounds,
                    piece.corners,
                    transparent_black(),
                    piece.borders,
                    color,
                    gpui::BorderStyle::Solid,
                ));
            }
        });
    }
}

/// Движется ли рамка при нынешних воротах (тест паузы).
#[cfg(test)]
pub(crate) fn moving(cx: &gpui::App) -> bool {
    hlsr_ui::motion::registry().keyframes_moving("profile-neon", crate::motion::AnimGate::still(cx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn the_gradient_runs_clockwise_from_the_top() {
        // Вверх от середины — начало оборота, справа — четверть.
        assert!((turn(0., -10., 0., 0., 0.) - 0.).abs() < 1e-4);
        assert!((turn(10., 0., 0., 0., 0.) - 0.25).abs() < 1e-4);
        assert!((turn(0., 10., 0., 0., 0.) - 0.5).abs() < 1e-4);
        // Слой повернули на 90° — справа снова начало.
        assert!(turn(10., 0., 0., 0., 90.).abs() < 1e-4);
        let near = |a: Rgba, b: Rgba| {
            (a.r - b.r).abs() < 1e-4
                && (a.g - b.g).abs() < 1e-4
                && (a.b - b.b).abs() < 1e-4
                && (a.a - b.a).abs() < 1e-4
        };
        assert!(near(SUBSCRIBER.at(0.1), SUBSCRIBER.dim));
        assert!(near(SUBSCRIBER.at(0.75), SUBSCRIBER.a));
        assert!(near(COMPLETE.at(0.55), COMPLETE.b));
        // Между стопами — смесь: на 65 % альфа 1, цвет между b и a.
        let mid = SUBSCRIBER.at(0.65);
        assert!((mid.a - 1.).abs() < 1e-4 && mid.g > SUBSCRIBER.b.g && mid.g < SUBSCRIBER.a.g);
    }

    #[test]
    fn pieces_cover_the_edges_and_the_corners() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(zpx(1., 200.), zpx(1., 100.)));
        let list = pieces(bounds, WIDTH, 16., 1., |x, y| {
            SUBSCRIBER.at(turn(x, y, 100., 50., 0.))
        });
        let corners = list
            .iter()
            .filter(|p| p.borders != Edges::default())
            .count();
        assert_eq!(corners, 4 * CORNER_CUTS);
        // Прямые края: (200 − 32) / 4 × 2 + (100 − 32) / 4 × 2.
        assert_eq!(list.len() - corners, 42 * 2 + 17 * 2);
        // Угол куска — четверть квада (по ней шейдер берёт скругление), маска — внутри неё.
        for piece in list.iter().filter(|p| p.borders != Edges::default()) {
            let r = zpx(1., 16.);
            assert_eq!(piece.bounds.size, size(r * 2., r * 2.));
            let quarter = Bounds::new(
                point(
                    if piece.corners.top_left > px(0.) || piece.corners.bottom_left > px(0.) {
                        piece.bounds.left()
                    } else {
                        piece.bounds.left() + r
                    },
                    if piece.corners.top_left > px(0.) || piece.corners.top_right > px(0.) {
                        piece.bounds.top()
                    } else {
                        piece.bounds.top() + r
                    },
                ),
                size(r, r),
            );
            let eps = 1e-3;
            assert!(f32::from(piece.mask.left() - quarter.left()) > -eps);
            assert!(f32::from(piece.mask.top() - quarter.top()) > -eps);
            assert!(f32::from(quarter.right() - piece.mask.right()) > -eps);
            assert!(f32::from(quarter.bottom() - piece.mask.bottom()) > -eps);
            assert!(bounds.contains(&quarter.origin));
        }
    }
}
