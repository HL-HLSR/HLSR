//! HLSR: снимок подложки стекла (тема Liquid Glass лаунчера HLSR, `HLSR-PATCH.md`).
//!
//! Блок стекла ставит первой своей тенью метку [`MARKER`] — прозрачную тень без размытия во всю
//! коробку блока. Отрисовка кадра, дойдя до метки, копирует место блока из цели отрисовки: там
//! ровно то, что окно нарисовало до блока (его тени, заливка и дети — после метки). Снимки
//! читаются на следующих кадрах без ожидания видеокарты и выкладываются в [`latest`].

use gpui::{Bounds, Hsla, PrimitiveBatch, ScaledPixels, Scene};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

/// Метка подложки: цвет первой тени блока. Прозрачная и чёрная — не рисуется; тон и
/// насыщенность — опознавательные (у чёрного цвета они ни на что не влияют).
pub const MARKER: Hsla = Hsla {
    h: 0.618_034,
    s: 0.314_159,
    l: 0.,
    a: 0.,
};

/// Не чаще раза в столько — новые снимки окна.
pub const PACE: Duration = Duration::from_millis(50);

/// Снимок подложки одного блока: пиксели цели отрисовки (BGRA, предумноженная альфа, строки без
/// зазоров) под коробкой блока, как они были перед его меткой.
#[derive(Clone)]
pub struct Capture {
    /// Коробка блока (`shape_bounds` метки), пиксели устройства: x, y, ширина, высота.
    pub shape: [f32; 4],
    /// Что снято: коробка, обрезанная маской блока и кадром, — x, y, ширина, высота.
    pub rect: [u32; 4],
    pub bgra: Arc<[u8]>,
}

/// Последние снимки окна `hwnd` (по одному на метку кадра).
pub fn latest(hwnd: isize) -> Option<Arc<[Capture]>> {
    STORE.lock().ok()?.get(&hwnd).cloned()
}

static STORE: LazyLock<Mutex<HashMap<isize, Arc<[Capture]>>>> = LazyLock::new(Default::default);

/// Выложить снимки окна; одинаковые с прежними (место и пиксели) берут прежние байты, чтобы
/// потребитель узнавал их по указателю. `true` — что-то изменилось.
pub(crate) fn publish(hwnd: isize, captures: Vec<Capture>) -> bool {
    let Ok(mut store) = STORE.lock() else {
        return false;
    };
    let previous = store.get(&hwnd).cloned().unwrap_or_else(|| Arc::from([]));
    let mut changed = captures.len() != previous.len();
    let captures: Arc<[Capture]> = captures
        .into_iter()
        .map(|mut capture| {
            let same = previous.iter().find(|old| {
                old.rect == capture.rect && old.shape == capture.shape && old.bgra == capture.bgra
            });
            match same {
                Some(old) => capture.bgra = old.bgra.clone(),
                None => changed = true,
            }
            capture
        })
        .collect();
    store.insert(hwnd, captures);
    changed
}

/// Окно закрылось: его снимки больше не нужны.
pub(crate) fn forget(hwnd: isize) {
    if let Ok(mut store) = STORE.lock() {
        store.remove(&hwnd);
    }
}

fn is_marker(color: &Hsla) -> bool {
    color.a == 0. && color.l == 0. && color.h == MARKER.h && color.s == MARKER.s
}

/// Где в кадре снимать: метка — тень `shadow`, до неё нарисовано `quads` прямоугольников
/// (непрозрачные из них — до снимка, остальные — после).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cut {
    pub shadow: usize,
    pub quads: usize,
    pub shape: [f32; 4],
    /// Левый, верхний, правый, нижний край снимка в цели `width × height`.
    pub area: [u32; 4],
}

/// Метки кадра в порядке отрисовки (`Scene::batches`). Порядок GPUI: всё, что нарисовано раньше
/// метки и легло на её коробку, идёт раньше неё; всё, что позже и легло на коробку, — позже
/// (тень может делить порядок с более ранними только тенями и встаёт после них, `BoundsTree`).
pub(crate) fn cuts(scene: &Scene, width: u32, height: u32) -> Vec<Cut> {
    let marks: Vec<usize> = scene
        .shadows
        .iter()
        .enumerate()
        .filter(|(_, shadow)| !shadow.inset.as_bool() && is_marker(&shadow.color))
        .map(|(index, _)| index)
        .collect();
    if marks.is_empty() {
        return Vec::new();
    }
    let mut marks = marks.into_iter().peekable();
    let mut quads = 0;
    let mut out = Vec::new();
    for batch in scene.batches() {
        match batch {
            PrimitiveBatch::Shadows(range) => {
                while let Some(shadow) = marks.next_if(|index| range.contains(index)) {
                    let mark = &scene.shadows[shadow];
                    let clip = mark.shape_bounds.intersect(&mark.content_mask.bounds);
                    if let Some(area) = area(clip, width, height) {
                        out.push(Cut {
                            shadow,
                            quads,
                            shape: rect(mark.shape_bounds),
                            area,
                        });
                    }
                }
            }
            PrimitiveBatch::Quads(range) => quads += range.len(),
            _ => {}
        }
        if marks.peek().is_none() {
            break;
        }
    }
    out
}

fn rect(bounds: Bounds<ScaledPixels>) -> [f32; 4] {
    [
        bounds.origin.x.as_f32(),
        bounds.origin.y.as_f32(),
        bounds.size.width.as_f32(),
        bounds.size.height.as_f32(),
    ]
}

/// Целые пиксели, накрывающие `bounds`, в пределах цели; пусто — `None`.
fn area(bounds: Bounds<ScaledPixels>, width: u32, height: u32) -> Option<[u32; 4]> {
    let [x, y, w, h] = rect(bounds);
    let clamp = |v: f32, max: u32| (v.max(0.) as u32).min(max);
    let (left, top) = (clamp(x.floor(), width), clamp(y.floor(), height));
    let (right, bottom) = (clamp((x + w).ceil(), width), clamp((y + h).ceil(), height));
    (right > left && bottom > top).then_some([left, top, right, bottom])
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{ContentMask, Quad, Shadow, bounds, point, size};

    fn scaled(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
        bounds(
            point(ScaledPixels(x), ScaledPixels(y)),
            size(ScaledPixels(w), ScaledPixels(h)),
        )
    }

    fn mask() -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: scaled(0., 0., 1000., 1000.),
        }
    }

    fn shadow(at: Bounds<ScaledPixels>, color: Hsla) -> Shadow {
        Shadow {
            order: 0,
            blur_radius: ScaledPixels(0.),
            bounds: at,
            corner_radii: Default::default(),
            content_mask: mask(),
            color,
            shape_bounds: at,
            shape_corner_radii: Default::default(),
            inset: false.into(),
            pad: 0,
        }
    }

    fn quad(at: Bounds<ScaledPixels>) -> Quad {
        Quad {
            order: 0,
            border_style: Default::default(),
            bounds: at,
            content_mask: mask(),
            background: gpui::red().into(),
            border_color: Default::default(),
            corner_radii: Default::default(),
            border_widths: Default::default(),
        }
    }

    #[test]
    fn the_cut_falls_between_what_lies_under_and_over_the_block() {
        let block = scaled(100., 100., 200.5, 100.);
        let mut scene = Scene::default();
        // Под блоком: фон, тень соседа; блок: метка, своя тень, заливка; поверх — кнопка.
        scene.insert_primitive(quad(scaled(0., 0., 1000., 1000.)));
        scene.insert_primitive(shadow(scaled(50., 50., 100., 100.), gpui::black()));
        scene.insert_primitive(shadow(block, MARKER));
        scene.insert_primitive(shadow(block, gpui::black().opacity(0.3)));
        scene.insert_primitive(quad(block));
        scene.insert_primitive(quad(scaled(120., 120., 20., 20.)));
        scene.finish();
        let cuts = cuts(&scene, 250, 1000);
        assert_eq!(cuts.len(), 1);
        let cut = &cuts[0];
        // Тени по порядку: сосед, метка, своя тень блока.
        assert_eq!(scene.shadows[cut.shadow].color, MARKER);
        assert_eq!(cut.shadow, 1);
        // До метки нарисован только фон; заливка и кнопка — после.
        assert_eq!(cut.quads, 1);
        assert_eq!(cut.shape, [100., 100., 200.5, 100.]);
        // Дробный край — до целого наружу, справа обрезано целью.
        assert_eq!(cut.area, [100, 100, 250, 200]);
    }

    #[test]
    fn no_marks_no_cuts_and_empty_areas_are_skipped() {
        let mut scene = Scene::default();
        scene.insert_primitive(quad(scaled(0., 0., 100., 100.)));
        scene.insert_primitive(shadow(scaled(10., 10., 10., 10.), gpui::black()));
        scene.finish();
        assert!(cuts(&scene, 100, 100).is_empty());
        let mut scene = Scene::default();
        scene.insert_primitive(shadow(scaled(500., 10., 10., 10.), MARKER));
        scene.finish();
        assert!(cuts(&scene, 100, 100).is_empty());
    }

    #[test]
    fn unchanged_captures_keep_their_bytes() {
        let capture = |byte: u8| Capture {
            shape: [0., 0., 1., 1.],
            rect: [0, 0, 1, 1],
            bgra: Arc::from([byte; 4]),
        };
        let hwnd = -7;
        assert!(publish(hwnd, vec![capture(1)]));
        let first = latest(hwnd).unwrap()[0].bgra.clone();
        assert!(!publish(hwnd, vec![capture(1)]));
        assert!(Arc::ptr_eq(&first, &latest(hwnd).unwrap()[0].bgra));
        assert!(publish(hwnd, vec![capture(2)]));
        assert!(publish(hwnd, Vec::new()));
        forget(hwnd);
        assert!(latest(hwnd).is_none());
    }
}
