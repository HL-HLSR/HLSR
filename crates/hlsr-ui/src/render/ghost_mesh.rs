//! Своя модель призрака (`gh_model`) в превью «Призраки и запись» (`ghost-mesh-sprite.js`
//! 0.4.9): треугольники из `ghost_model` доставки рисуются серым силуэтом со светом
//! сверху-спереди, а сцена красит его цветом призрака так же, как готовый `ghost.webp`
//! (multiply). Поза — привязки (как в `.vvd`), без анимации и текстур: игра тоже заливает
//! призрака цветом.

use tiny_skia::{Color, FillRule, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// Рост `models/player.mdl` в единицах игры: столько занимает `ghost.webp` в кадре.
pub const PLAYER_HEIGHT: f64 = 72.0;
/// Модель чуть повёрнута к камере, как призрак на готовом кадре.
const YAW: f64 = -0.45;

/// Силуэт модели: картинка, пикселей на единицу игры, где в ней ось модели и земля.
pub struct MeshSprite {
    pub image: Pixmap,
    pub ppu: f64,
    pub origin_x: f64,
    pub ground_y: f64,
}

/// Направление на свет в осях вида (глубина к камере, вправо, вверх).
fn light() -> [f64; 3] {
    let v: [f64; 3] = [0.65, -0.35, 0.68];
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|x| x / n)
}

/// `meshSprite(mesh, ppu = 4)`: `positions` — x, y, z × 3 на треугольник. Меньше одного
/// треугольника — `None`.
pub fn mesh_sprite(positions: &[f64]) -> Option<MeshSprite> {
    if positions.len() < 9 {
        return None;
    }
    let (cos, sin) = (YAW.cos(), YAW.sin());
    let n = positions.len() / 3;
    let (mut depth, mut sx, mut sy) = (vec![0f32; n], vec![0f32; n], vec![0f32; n]);
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    );
    for i in 0..n {
        let (x, y, z) = (positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]);
        // Модель Source смотрит вдоль +X; камера стоит на +X и смотрит на неё: вправо — +Y.
        // Float32Array 0.4.9: значения хранятся в f32.
        depth[i] = (x * cos - y * sin) as f32;
        sx[i] = (x * sin + y * cos) as f32;
        sy[i] = (-z) as f32;
        min_x = min_x.min(sx[i]);
        max_x = max_x.max(sx[i]);
        min_y = min_y.min(sy[i]);
        max_y = max_y.max(sy[i]);
    }
    let (min_x, max_x, min_y, max_y) = (
        f64::from(min_x),
        f64::from(max_x),
        f64::from(min_y),
        f64::from(max_y),
    );
    // Огромная модель не даёт огромный холст: масштаб уменьшается до стороны 2048.
    let span = (max_x - min_x).max(max_y - min_y).max(1e-3);
    let ppu = 4f64.min(2040.0 / span);
    let pad = 2.0;
    let width = ((max_x - min_x) * ppu).ceil() + pad * 2.0;
    let height = ((max_y - min_y) * ppu).ceil() + pad * 2.0;
    let mut image = Pixmap::new(width as u32, height as u32)?;
    let px = |i: usize| ((f64::from(sx[i]) - min_x) * ppu + pad) as f32;
    let py = |i: usize| ((f64::from(sy[i]) - min_y) * ppu + pad) as f32;
    // Дальние треугольники — первыми (художник); сортировка устойчивая, как у V8.
    let count = n / 3;
    let far: Vec<f32> = (0..count)
        .map(|t| depth[t * 3] + depth[t * 3 + 1] + depth[t * 3 + 2])
        .collect();
    let mut tris: Vec<usize> = (0..count).collect();
    tris.sort_by(|&a, &b| far[a].total_cmp(&far[b]));
    let l = light();
    let stroke = Stroke {
        width: 0.6,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    for t in tris {
        let (a, b, c) = (t * 3, t * 3 + 1, t * 3 + 2);
        // Нормаль в осях вида; свет с двух сторон: порядок обхода у моделей бывает разный.
        let d = |i: usize, j: usize| f64::from(depth[i]) - f64::from(depth[j]);
        let h = |i: usize, j: usize| f64::from(sx[i]) - f64::from(sx[j]);
        let v = |i: usize, j: usize| -(f64::from(sy[i]) - f64::from(sy[j]));
        let (ux, uy, uz) = (d(b, a), h(b, a), v(b, a));
        let (vx, vy, vz) = (d(c, a), h(c, a), v(c, a));
        let (nx, ny, nz) = (uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx);
        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        let len = if len > 0.0 { len } else { 1.0 };
        let lit = ((nx * l[0] + ny * l[1] + nz * l[2]) / len).abs();
        let g = hlsr_core::js::math_round(255.0 * (0.28 + 0.62 * lit)).clamp(0.0, 255.0) as u8;
        let mut path = PathBuilder::new();
        path.move_to(px(a), py(a));
        path.line_to(px(b), py(b));
        path.line_to(px(c), py(c));
        path.close();
        let Some(path) = path.finish() else {
            continue;
        };
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(g, g, g, 255));
        paint.anti_alias = true;
        image.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        // Обводка закрывает щели сглаживания между соседними треугольниками.
        image.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    Some(MeshSprite {
        image,
        ppu,
        origin_x: -min_x * ppu + pad,
        ground_y: (max_y - min_y) * ppu + pad,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_stands_on_the_ground_with_its_axis_inside() {
        let (x0, y0, z0, x1, y1, z1) = (-8.0, -8.0, 0.0, 8.0, 8.0, 72.0);
        let v = [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y1, z0],
            [x0, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
        ];
        let faces = [
            [0, 1, 2, 3],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [1, 2, 6, 5],
            [2, 3, 7, 6],
            [3, 0, 4, 7],
        ];
        let mut positions = Vec::new();
        for [a, b, c, d] in faces {
            for i in [a, b, c, a, c, d] {
                positions.extend_from_slice(&v[i]);
            }
        }
        let sprite = mesh_sprite(&positions).unwrap();
        assert_eq!(sprite.ppu, 4.0);
        // 72 юнита × 4 px + поля по 2.
        assert_eq!(sprite.image.height(), 292);
        assert_eq!(sprite.ground_y, 290.0);
        assert!(sprite.origin_x > 2.0 && sprite.origin_x < f64::from(sprite.image.width()) - 2.0);
        // Середина силуэта закрашена серым, угол холста пуст.
        let mid = sprite.image.pixel(sprite.origin_x as u32, 146).unwrap();
        assert_eq!(mid.alpha(), 255);
        assert!(mid.red() == mid.green() && mid.red() >= 71);
        assert_eq!(sprite.image.pixel(0, 0).unwrap().alpha(), 0);
        assert!(mesh_sprite(&positions[..6]).is_none());
    }
}
