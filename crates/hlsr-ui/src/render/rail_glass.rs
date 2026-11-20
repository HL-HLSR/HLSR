//! Стекло колонки значков (`backdrop-filter: blur(var(--l-rail-blur))` у `.game-rail`,
//! GameSidebar.vue 0.4.9). Перенесено из slint (`render/rail_glass.rs`) без типов Slint; кэш
//! кусков — у вида (`hlsr_gpui::glass`).
//!
//! Chromium размывает не весь кадр, а то, что лежит под коробкой колонки: пиксели за краем
//! коробки в размытие не входят, за краем берётся их зеркало (`SkTileMode::kMirror`, замер по
//! эталону с контрастными полосами у краёв коробки: повтор крайнего пикселя расходится с
//! Chromium вдвое больше). Копия всего кадра, размытая один раз, подмешивала бы к колонке
//! соседний кусок кадра — у узкой колонки (80 px при σ = 28) это почти всё, что видно. Здесь
//! размывается только кусок под коробкой: его размер — размер коробки в CSS px, кадр лежит в
//! окне со своим сдвигом (cover и параллакс учитывает хозяин фона). Картинка маленькая (σ ≈ 4 px
//! картинки), вид растягивает её на коробку.

use super::shadow_blur::gaussian_blur_rgba;
use crate::{Rgba, Rgba8Image};

/// σ размытой копии в её собственных пикселях.
const SIGMA: f32 = 4.0;

/// Где кадр лежит в окне: левый верхний угол и размер в CSS px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Placement {
    /// `background-size: cover; background-position: center top` (`.launcher-bg__image`) для
    /// картинки `image` в окне `view` (CSS px), без сдвига и увеличения параллакса.
    pub fn cover_top(image: (u32, u32), view: (f32, f32)) -> Placement {
        let (iw, ih) = (image.0.max(1) as f32, image.1.max(1) as f32);
        let k = (view.0 / iw).max(view.1 / ih);
        let (width, height) = (iw * k, ih * k);
        Placement {
            x: (view.0 - width) / 2.0,
            y: 0.0,
            width,
            height,
        }
    }
}

/// Кусок кадра под коробкой `box_width × box_height` с левым верхним углом в углу окна,
/// размытый `blur(css_blur px)`. Размер результата — коробка, уменьшенная примерно до
/// σ = 4 px картинки. Кадр с предумноженной альфой (так их кладёт `ImageLoader`) читается как
/// есть, без копии; результат — прямая альфа. `scrim(x, y)` — что лежит над кадром в точке окна
/// (CSS px, прямая альфа): `backdrop-filter` размывает кадр вместе с затемнением.
pub fn box_backdrop(
    source: &Rgba8Image,
    frame: Placement,
    box_width: f32,
    box_height: f32,
    css_blur: f32,
    scrim: impl Fn(f32, f32) -> Rgba,
) -> Rgba8Image {
    box_backdrop_at(source, frame, box_width, box_height, css_blur, SIGMA, scrim)
}

/// [`box_backdrop`] на сетке, где σ размытия — `cells` клеток (грубее — быстрее).
pub fn box_backdrop_at(
    source: &Rgba8Image,
    frame: Placement,
    box_width: f32,
    box_height: f32,
    css_blur: f32,
    cells: f32,
    scrim: impl Fn(f32, f32) -> Rgba,
) -> Rgba8Image {
    let (src_w, src_h) = (source.width() as usize, source.height() as usize);
    if src_w == 0 || src_h == 0 || frame.width <= 0.0 || frame.height <= 0.0 {
        return Rgba8Image::new(1, 1);
    }
    if box_width < 1.0 || box_height < 1.0 {
        return Rgba8Image::new(1, 1);
    }
    // CSS px на пиксель результата: σ картинки — `cells`.
    let step = (css_blur / cells).max(1.0);
    let out_w = (box_width / step).ceil().max(1.0) as usize;
    let out_h = (box_height / step).ceil().max(1.0) as usize;
    let (step_x, step_y) = (box_width / out_w as f32, box_height / out_h as f32);
    let mut small = Rgba8Image::new(out_w as u32, out_h as u32);
    for (index, out) in small.make_mut_slice().iter_mut().enumerate() {
        let (i, j) = ((index % out_w) as f32, (index / out_w) as f32);
        let under = area(
            source,
            frame,
            (i * step_x, j * step_y),
            ((i + 1.0) * step_x, (j + 1.0) * step_y),
        );
        *out = over(scrim((i + 0.5) * step_x, (j + 0.5) * step_y), under);
    }
    // Усреднение клетки уже размыло на 1/12 квадрата клетки.
    let sigma = css_blur / step;
    let blur = (sigma * sigma - 1.0 / 12.0).max(0.0).sqrt();
    // За краем коробки — зеркало: клетка −1 равна клетке 0, −2 — клетке 1 (период 2·размер).
    // Поля шире 3σ, поэтому собственный повтор края у `gaussian_blur_rgba` до результата не
    // дотягивается.
    let pad = (blur * 3.0).ceil() as usize + 2;
    let (padded_w, padded_h) = (out_w + 2 * pad, out_h + 2 * pad);
    let mirror = |i: isize, len: usize| -> usize {
        let period = 2 * len as isize;
        let m = i.rem_euclid(period);
        (if m < len as isize { m } else { period - 1 - m }) as usize
    };
    let mut padded = Rgba8Image::new(padded_w as u32, padded_h as u32);
    for (index, out) in padded.make_mut_slice().iter_mut().enumerate() {
        let i = (index % padded_w) as isize - pad as isize;
        let j = (index / padded_w) as isize - pad as isize;
        *out = small.as_slice()[mirror(j, out_h) * out_w + mirror(i, out_w)];
    }
    gaussian_blur_rgba(&mut padded, blur);
    for (index, out) in small.make_mut_slice().iter_mut().enumerate() {
        let (i, j) = (index % out_w, index / out_w);
        *out = padded.as_slice()[(j + pad) * padded_w + i + pad];
    }
    small
}

/// Средний цвет кадра `source` (место в окне `frame`) в прямоугольнике окна `from`–`to` (CSS px),
/// прямая альфа; за краем кадра — крайние пиксели. Кадр с предумноженной альфой читается как есть.
pub fn area(source: &Rgba8Image, frame: Placement, from: (f32, f32), to: (f32, f32)) -> Rgba {
    let (src_w, src_h) = (source.width() as usize, source.height() as usize);
    if src_w == 0 || src_h == 0 || frame.width <= 0.0 || frame.height <= 0.0 {
        return Rgba::TRANSPARENT;
    }
    let premultiplied = source.is_premultiplied();
    // Пикселей кадра на CSS px окна.
    let (kx, ky) = (src_w as f32 / frame.width, src_h as f32 / frame.height);
    let range = |a: f32, b: f32, len: usize| {
        let last = len as i64 - 1;
        let from = (a.floor() as i64).clamp(0, last);
        let to = ((b.ceil() as i64) - 1).clamp(from, last);
        (from as usize, to as usize)
    };
    let (cx0, cx1) = range((from.0 - frame.x) * kx, (to.0 - frame.x) * kx, src_w);
    let (cy0, cy1) = range((from.1 - frame.y) * ky, (to.1 - frame.y) * ky, src_h);
    let pixels = source.as_slice();
    // Суммы целые: точнее f32 и в разы быстрее (стекло Liquid Glass зовёт это на каждую клетку).
    let mut acc = [0u64; 4];
    let mut count = 0u64;
    for y in cy0..=cy1 {
        for p in &pixels[y * src_w + cx0..=y * src_w + cx1] {
            let a = u64::from(p.a);
            // Цвет с весом альфы: у предумноженного он уже умножен на a / 255.
            let weight = if premultiplied { 255 } else { a };
            acc[0] += u64::from(p.r) * weight;
            acc[1] += u64::from(p.g) * weight;
            acc[2] += u64::from(p.b) * weight;
            acc[3] += a;
        }
        count += (cx1 - cx0 + 1) as u64;
    }
    // Частное с округлением половины вверх, как `f32::round` у положительных.
    let ratio = |v: u64, d: u64| ((2 * v + d) / (2 * d)).min(255) as u8;
    let channel = |v: u64| if acc[3] > 0 { ratio(v, acc[3]) } else { 0 };
    Rgba::new(
        channel(acc[0]),
        channel(acc[1]),
        channel(acc[2]),
        ratio(acc[3], count),
    )
}

/// `top` поверх `bottom`, прямая альфа.
pub fn over(top: Rgba, bottom: Rgba) -> Rgba {
    if top.a == 0 {
        return bottom;
    }
    let (ta, ba) = (f32::from(top.a) / 255.0, f32::from(bottom.a) / 255.0);
    let a = ta + ba * (1.0 - ta);
    let mix = |t: u8, b: u8| {
        ((f32::from(t) * ta + f32::from(b) * ba * (1.0 - ta)) / a)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgba::new(
        mix(top.r, bottom.r),
        mix(top.g, bottom.g),
        mix(top.b, bottom.b),
        (a * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Rgba8Image {
        let mut buffer = Rgba8Image::new(width, height);
        for (index, p) in buffer.make_mut_slice().iter_mut().enumerate() {
            let [r, g, b] = pixel(index as u32 % width, index as u32 / width);
            *p = Rgba::rgb(r, g, b);
        }
        buffer
    }

    fn none(_: f32, _: f32) -> Rgba {
        Rgba::TRANSPARENT
    }

    const WINDOW: Placement = Placement {
        x: 0.0,
        y: 0.0,
        width: 1600.0,
        height: 900.0,
    };

    #[test]
    fn size_is_the_box_over_the_step() {
        let source = buffer(1600, 900, |_, _| [10, 20, 30]);
        let out = box_backdrop(&source, WINDOW, 80.0, 900.0, 28.0, none);
        // 28 / 4 = 7 CSS px на пиксель.
        assert_eq!((out.width(), out.height()), (12, 129));
        let out = box_backdrop(&source, WINDOW, 240.0, 900.0, 28.0, none);
        assert_eq!((out.width(), out.height()), (35, 129));
        // Не размытие (blur(0)) — клетка 1 px.
        assert_eq!(
            box_backdrop(&source, WINDOW, 8.0, 4.0, 0.0, none).width(),
            8
        );
        // Пустой кадр — прозрачный пиксель.
        let empty = box_backdrop(&Rgba8Image::new(0, 0), WINDOW, 80.0, 900.0, 28.0, none);
        assert_eq!(empty.as_slice(), [Rgba::TRANSPARENT]);
    }

    #[test]
    fn scrim_is_blurred_with_the_frame() {
        // Ровное затемнение наполовину — ровная половина яркости.
        let source = buffer(1600, 900, |_, _| [200, 200, 200]);
        let half = |_: f32, _: f32| Rgba::new(0, 0, 0, 128);
        let out = box_backdrop(&source, WINDOW, 240.0, 900.0, 28.0, half);
        assert!(
            out.as_slice()
                .iter()
                .all(|p| p.r.abs_diff(100) <= 1 && p.a == 255)
        );
        // Резкая граница затемнения размывается вместе с кадром.
        let top = |_: f32, y: f32| {
            if y < 450.0 {
                Rgba::new(0, 0, 0, 255)
            } else {
                Rgba::TRANSPARENT
            }
        };
        let out = box_backdrop(&source, WINDOW, 240.0, 900.0, 28.0, top);
        let width = out.width() as usize;
        let column: Vec<u8> = (0..out.height() as usize)
            .map(|j| out.as_slice()[j * width].r)
            .collect();
        assert!(column[0] <= 1 && column[column.len() - 1] >= 199);
        assert!(column.iter().any(|&r| (20..180).contains(&r)));
    }

    #[test]
    fn flat_colour_stays_flat() {
        let source = buffer(1600, 900, |_, _| [10, 20, 30]);
        let out = box_backdrop(&source, WINDOW, 240.0, 900.0, 28.0, none);
        assert!(out.as_slice().iter().all(|p| *p == Rgba::rgb(10, 20, 30)));
    }

    #[test]
    fn premultiplied_frame_gives_the_same_glass() {
        // Кадры ImageLoader — с предумноженной альфой: стекло то же, что у того же кадра с
        // прямой альфой, и у полупрозрачных мест тоже.
        let mut straight = Rgba8Image::new(320, 180);
        for (index, p) in straight.make_mut_slice().iter_mut().enumerate() {
            let (x, y) = (index % 320, index / 320);
            *p = Rgba::new(
                (x * 255 / 319) as u8,
                90,
                (y * 255 / 179) as u8,
                if (x / 16 + y / 16) % 2 == 0 { 255 } else { 120 },
            );
        }
        let mut premultiplied = straight.clone().premultiplied();
        for p in premultiplied.make_mut_slice() {
            let k = |c: u8| ((u32::from(c) * u32::from(p.a) + 127) / 255) as u8;
            *p = Rgba::new(k(p.r), k(p.g), k(p.b), p.a);
        }
        let frame = Placement { x: -20.0, ..WINDOW };
        let want = box_backdrop(&straight, frame, 240.0, 900.0, 28.0, none);
        let got = box_backdrop(&premultiplied, frame, 240.0, 900.0, 28.0, none);
        assert_eq!((got.width(), got.height()), (want.width(), want.height()));
        for (got, want) in got.as_slice().iter().zip(want.as_slice()) {
            let close = got.r.abs_diff(want.r) <= 1
                && got.g.abs_diff(want.g) <= 1
                && got.b.abs_diff(want.b) <= 1
                && got.a == want.a;
            assert!(close, "{got:?} против {want:?}");
        }
    }

    #[test]
    fn nothing_from_beyond_the_box_gets_in() {
        // Слева тёмный кадр до x = 100, справа светлый: коробка 80 px видит только тёмное,
        // хотя размытие всего кадра добавило бы светлого (σ = 28 дотягивается на 84 px).
        let source = buffer(
            1600,
            900,
            |x, _| if x < 100 { [0, 0, 0] } else { [255, 255, 255] },
        );
        let out = box_backdrop(&source, WINDOW, 80.0, 900.0, 28.0, none);
        assert!(out.as_slice().iter().all(|p| p.r == 0));
    }

    #[test]
    fn edge_is_mirrored_beyond_the_box() {
        // Горизонтальный градиент: у правого края коробки значения не проседают к чёрному
        // (за краем — зеркало, а не пустота).
        let source = buffer(1600, 900, |x, _| {
            let v = (x * 255 / 1599) as u8;
            [v, v, v]
        });
        let out = box_backdrop(&source, WINDOW, 240.0, 900.0, 28.0, none);
        let row = &out.as_slice()[60 * out.width() as usize..61 * out.width() as usize];
        assert!(row.windows(2).all(|pair| pair[1].r >= pair[0].r));
        assert!(row[row.len() - 1].r > row[0].r);
    }

    #[test]
    fn blur_at_the_edge_follows_a_mirrored_gaussian() {
        // Полоса у правого края коробки 96 px: светлая 24 px, тёмная 12 px. Независимый расчёт:
        // гауссиана σ = 28 по строке в 1-px шагах с зеркалом (−1 ↔ 0) за краем коробки; повтор
        // крайнего пикселя дал бы у края на 45–60 уровней темнее.
        let source = buffer(1600, 900, |x, _| match x {
            60..=83 => [255, 255, 255],
            84..=95 => [0, 0, 0],
            _ => [100, 100, 100],
        });
        let out = box_backdrop(&source, WINDOW, 96.0, 900.0, 28.0, none);
        let width = out.width() as usize;
        let row: Vec<f32> = (0..96)
            .map(|x| match x {
                60..=83 => 255.0,
                84..=95 => 0.0,
                _ => 100.0,
            })
            .collect();
        let at = |i: isize| -> f32 {
            let m = i.rem_euclid(192);
            row[(if m < 96 { m } else { 191 - m }) as usize]
        };
        let blurred = |centre: f32| -> f32 {
            let (mut sum, mut weights) = (0.0f32, 0.0f32);
            for i in -160..256isize {
                let d = (i as f32 + 0.5 - centre) / 28.0;
                let w = (-0.5 * d * d).exp();
                sum += at(i) * w;
                weights += w;
            }
            sum / weights
        };
        let step = 96.0 / width as f32;
        for column in [0, width / 2, width - 2, width - 1] {
            let centre = (column as f32 + 0.5) * step;
            let (got, want) = (
                f32::from(out.as_slice()[60 * width + column].r),
                blurred(centre),
            );
            assert!(
                (got - want).abs() < 4.0,
                "столбец {column} (x = {centre:.1}): {got} против {want:.1}"
            );
        }
    }

    #[test]
    fn the_frame_offset_moves_what_is_under_the_box() {
        // Кадр сдвинут вправо на 700 px: под коробкой 0…80 — левый край кадра (повтор).
        let source = buffer(
            1600,
            900,
            |x, _| if x < 800 { [200, 0, 0] } else { [0, 0, 200] },
        );
        let moved = Placement { x: 700.0, ..WINDOW };
        let out = box_backdrop(&source, moved, 80.0, 900.0, 28.0, none);
        assert!(out.as_slice().iter().all(|p| p.r == 200 && p.b == 0));
        // Сдвиг влево: под коробкой уже правая половина (x = 800 кадра — окно 100).
        let moved = Placement {
            x: -900.0,
            ..WINDOW
        };
        let out = box_backdrop(&source, moved, 80.0, 900.0, 28.0, none);
        assert!(out.as_slice().iter().all(|p| p.b == 200 && p.r == 0));
    }

    #[test]
    fn cover_top_fills_the_view_and_centres_across() {
        // Кадр 16:9 в окне 16:9 — ровно окно.
        assert_eq!(Placement::cover_top((2560, 1440), (1600.0, 900.0)), WINDOW);
        // Узкое окно: высота по окну, лишнее по ширине поровну с боков.
        let p = Placement::cover_top((1920, 1080), (800.0, 900.0));
        assert_eq!((p.width, p.height, p.x, p.y), (1600.0, 900.0, -400.0, 0.0));
        // Высокий кадр: ширина по окну, верх прижат (center top).
        let p = Placement::cover_top((1000, 1000), (1600.0, 900.0));
        assert_eq!((p.width, p.height, p.x, p.y), (1600.0, 1600.0, 0.0, 0.0));
    }
}
