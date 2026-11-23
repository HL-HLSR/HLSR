//! Вкладка «Достижения» окна профиля (`ProfileModal.vue` 0.4.9, t328–395): заголовок со
//! счётчиком «Получено X/Y», полоса (золотая при 100 %), сетка карточек в две колонки, ряды одной
//! высоты (`grid-auto-rows: 1fr`). Карточка: значок 44 (замок у неполученного), название,
//! описание, полоса прогресса у неполученного с целью, справа «Получено» и дата (у своего) или
//! «31/50». Открытое секретное — розовое; редкость полученного — золото и фиолет (оттенок
//! `radial-gradient`, кольцо, свечение `BoxShadow`) и радуга (переливающиеся кольцо и значок,
//! `profile-rainbow` 6 с). Данные — [`super::ProfileContext`].
//!
//! Градиенты без масок GPUI: фон карточки и полоса радуги значка — картинки, запечённые на ЦП
//! в физических px и нарисованные со скруглением (`paint_image`); радуга значка — сдвиг этой
//! картинки внутри значка; радужное кольцо — куски вдоль края, как неон окна ([`super::neon`]).

use super::ProfileContext;
use super::neon::{paint_pieces, pieces};
use crate::icons::fa_glyph;
use crate::kit::button::white;
use crate::motion::{Timing, hover, keyframes, mix, tween};
use crate::paint::{ring, shadow, strips};
use crate::rows::{Rows, Slot};
use crate::scale::{u, zoom, zpx};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::text::{TextExt, spaced};
use crate::theme::{Gradient, Stop, Theme};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, Context, Corners, FontWeight, Hsla, IntoElement, Pixels, Render,
    RenderImage, Rgba, Role, Window, canvas, div, point, prelude::*, px, relative, rgb, size,
};
use hlsr_ui::controllers::profile::AchievementView;
use hlsr_ui::services::Services;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// Угол радуги (`linear-gradient(115deg, …)`).
const ANGLE: f32 = 115.;
/// Кольцо радужного (`padding: 1.5px` маски).
const RAINBOW_RING: f32 = 1.5;

const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a,
    }
}

/// Кольцо `::before`: `#ff5f6d, #ffc371, #f9f871, #5ff59a, #4fc3f7, #a78bfa, #ff5f6d`.
const RING: [Rgba; 7] = [
    rgba(0xff, 0x5f, 0x6d, 1.),
    rgba(0xff, 0xc3, 0x71, 1.),
    rgba(0xf9, 0xf8, 0x71, 1.),
    rgba(0x5f, 0xf5, 0x9a, 1.),
    rgba(0x4f, 0xc3, 0xf7, 1.),
    rgba(0xa7, 0x8b, 0xfa, 1.),
    rgba(0xff, 0x5f, 0x6d, 1.),
];
/// Значок радужного: те же цвета без жёлтого.
const BADGE: [Rgba; 6] = [
    rgba(0xff, 0x5f, 0x6d, 1.),
    rgba(0xff, 0xc3, 0x71, 1.),
    rgba(0x5f, 0xf5, 0x9a, 1.),
    rgba(0x4f, 0xc3, 0xf7, 1.),
    rgba(0xa7, 0x8b, 0xfa, 1.),
    rgba(0xff, 0x5f, 0x6d, 1.),
];
/// Фон радужной карточки.
const WASH: [Rgba; 5] = [
    rgba(255, 95, 109, 0.12),
    rgba(255, 195, 113, 0.1),
    rgba(95, 245, 154, 0.1),
    rgba(79, 195, 247, 0.1),
    rgba(167, 139, 250, 0.12),
];
/// `--tier` золота и фиолета.
const GOLD: (u8, u8, u8) = (245, 197, 66);
const PURPLE: (u8, u8, u8) = (168, 115, 255);
/// Открытое секретное: `rgba(219, 97, 162, …)`.
const PINK: (u8, u8, u8) = (219, 97, 162);

fn tone((r, g, b): (u8, u8, u8), a: f32) -> Hsla {
    rgba(r, g, b, a).into()
}

/// Смесь двух цветов с предумноженной альфой (как градиент Chromium).
fn lerp(from: Rgba, to: Rgba, k: f32) -> Rgba {
    let a = from.a + (to.a - from.a) * k;
    let channel = |x: f32, y: f32| {
        if a > 0. {
            (x * from.a + (y * to.a - x * from.a) * k) / a
        } else {
            0.
        }
    };
    Rgba {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a,
    }
}

/// Цвет `linear-gradient(<angle>deg, стопы поровну)` в точке (x, y) блока w × h.
fn linear_at(stops: &[Rgba], w: f32, h: f32, x: f32, y: f32) -> Rgba {
    let (sin, cos) = ANGLE.to_radians().sin_cos();
    let length = (w * sin).abs() + (h * cos).abs();
    let t = ((x - w / 2.) * sin - (y - h / 2.) * cos) / length + 0.5;
    let last = stops.len() - 1;
    let pos = t.clamp(0., 1.) * last as f32;
    let index = (pos.floor() as usize).min(last - 1);
    lerp(stops[index], stops[index + 1], pos - index as f32)
}

/// `radial-gradient(120% 140% at 0% 0%, color 0%, transparent 60%)` в точке (x, y) блока w × h.
fn glow_at(color: Rgba, w: f32, h: f32, x: f32, y: f32) -> Rgba {
    let d = ((x / (1.2 * w)).powi(2) + (y / (1.4 * h)).powi(2)).sqrt();
    Rgba {
        a: color.a * (1. - d / 0.6).max(0.),
        ..color
    }
}

/// Что запекается (гладкие градиенты в компактных текстурах, масштабируемых GPU).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Bake {
    /// Плитка радуги значка: 300 % × 100 %.
    Tile,
    /// Фон радужной карточки.
    Wash,
    /// Оттенок полученного (секретное, золото, фиолет) цветом с прозрачностью.
    Glow(u8, u8, u8, u8),
}

impl Bake {
    fn color(self, w: f32, h: f32, x: f32, y: f32) -> Rgba {
        match self {
            Bake::Tile => linear_at(&BADGE, w, h, x, y),
            Bake::Wash => linear_at(&WASH, w, h, x, y),
            Bake::Glow(r, g, b, a) => glow_at(rgba(r, g, b, f32::from(a) / 100.), w, h, x, y),
        }
    }

    /// Оттенок `(r, g, b)` с прозрачностью в сотых.
    fn glow((r, g, b): (u8, u8, u8), alpha: u8) -> Bake {
        Bake::Glow(r, g, b, alpha)
    }

    /// Компактный размер текстуры: гладкие градиенты билинейно растягиваются видеокартой без
    /// потерь, а карточки разной высоты не выбивают друг друга из кэша.
    fn tex_size(self, (w, h): (u32, u32)) -> (u32, u32) {
        match self {
            Bake::Glow(..) => (64, 64),
            Bake::Tile => (192, 64),
            Bake::Wash => {
                // Сохраняем пропорции карточки для точного угла 115°.
                let tw = 96u32;
                let th = ((h as f32 / w.max(1) as f32) * tw as f32)
                    .round()
                    .clamp(16., 96.) as u32;
                (tw, th)
            }
        }
    }
}

thread_local! {
    /// Запечённые текстуры градиентов карточек (по ключу вида и компактного размера).
    static BAKED: RefCell<std::collections::HashMap<(Bake, (u32, u32)), Arc<RenderImage>>> =
        RefCell::default();
}

/// Картинка `kind` под рамку `(w, h)` физических px: берётся из кэша, без пересчёта на ЦП.
fn baked(kind: Bake, size_px: (u32, u32), window: &mut Window, cx: &mut App) -> Arc<RenderImage> {
    let (w, h) = kind.tex_size(size_px);
    let key = (kind, (w, h));
    BAKED.with(|cell| {
        let mut map = cell.borrow_mut();
        if let Some(image) = map.get(&key) {
            return image.clone();
        }
        if map.len() >= 32 {
            for (_, image) in map.drain() {
                cx.drop_image(image, Some(&mut *window));
            }
        }
        let mut bytes = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let c = kind.color(w as f32, h as f32, x as f32 + 0.5, y as f32 + 0.5);
                let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
                bytes.extend_from_slice(&[byte(c.b), byte(c.g), byte(c.r), byte(c.a)]);
            }
        }
        let buffer = ::image::RgbaImage::from_raw(w, h, bytes).expect("размер совпадает с байтами");
        let image = Arc::new(RenderImage::new([::image::Frame::new(buffer)]));
        map.insert(key, image.clone());
        image
    })
}

/// Физические px рамки `bounds`.
fn device(bounds: Bounds<Pixels>, window: &Window) -> (u32, u32) {
    let scale = window.scale_factor();
    (
        (f32::from(bounds.size.width) * scale).round().max(1.) as u32,
        (f32::from(bounds.size.height) * scale).round().max(1.) as u32,
    )
}

/// Слой поверх места (`absolute`, `inset: 0`): запечённая картинка со скруглением `radius`.
fn backdrop(kind: Bake, radius: Pixels) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let image = baked(kind, device(bounds, window), window, cx);
            window
                .paint_image(bounds, bounds, Corners::all(radius), image, 0, false)
                .ok();
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Сдвиг полосы 300 % при `background-position: <shift × 100>% 0`: (W − 3W) × shift.
fn offset(width: f32, shift: f32) -> f32 {
    -2. * width * shift
}

/// Радужный значок: плитка 300 % × 100 %, сдвинутая на `shift` по кругу, скругление `radius`.
fn rainbow_badge(index: usize, radius: Pixels) -> impl IntoElement {
    keyframes(
        ("ach-rainbow-badge", index),
        "profile-rainbow-badge",
        move |frame| {
            let shift = frame.shift as f32;
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    if !bounds.intersects(&window.content_mask().bounds) {
                        return;
                    }
                    let (w, h) = device(bounds, window);
                    let tile = baked(Bake::Tile, (w * 3, h), window, cx);
                    let width = bounds.size.width;
                    let start = bounds.origin.x + width * offset(1., shift);
                    for k in 0..2 {
                        let at = point(start + width * (3. * k as f32), bounds.origin.y);
                        let place = Bounds::new(at, size(width * 3., bounds.size.height));
                        window
                            .paint_image(
                                bounds,
                                place,
                                Corners::all(radius),
                                tile.clone(),
                                0,
                                false,
                            )
                            .ok();
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        },
    )
}

/// Радужное кольцо 1,5 px у края карточки: полоса 300 % × 100 % со сдвигом, куски по краю.
fn rainbow_ring(index: usize, radius: f32, zoom: f32) -> impl IntoElement {
    keyframes(
        ("ach-rainbow-ring", index),
        "profile-rainbow-ring",
        move |frame| {
            let shift = frame.shift as f32;
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    if !bounds.intersects(&window.content_mask().bounds) {
                        return;
                    }
                    let (left, top) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                    let start = offset(w, shift);
                    let list = pieces(bounds, RAINBOW_RING, radius, zoom, |x, y| {
                        let along = (x - left - start).rem_euclid(3. * w);
                        linear_at(&RING, 3. * w, h, along, y - top)
                    });
                    paint_pieces(list, window);
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        },
    )
}

/// Цвет редкости `gold`/`purple`.
fn tier_of(tier: &str) -> Option<(u8, u8, u8)> {
    match tier {
        "gold" => Some(GOLD),
        "purple" => Some(PURPLE),
        _ => None,
    }
}

/// Карточка `.profile__achievement`.
fn card(item: &AchievementView, index: usize, theme: &Theme, zoom: f32) -> AnyElement {
    let locked = !item.unlocked;
    let rainbow = item.tier == "rainbow";
    let tier = tier_of(item.tier);
    let secret = item.secret && !locked;
    let (text, dim, faint, border, accent) = (
        theme.color("l-text"),
        theme.color("l-text-dim"),
        theme.color("l-text-faint"),
        theme.color("l-border"),
        theme.color("l-accent"),
    );
    let (accent_soft, modal_bg) = (theme.color("l-accent-soft"), theme.color("l-modal-bg"));
    let (radius, radius_sm) = (theme.len("l-radius"), theme.len("l-radius-sm"));
    let border_hover = mix(border, accent, 0.45);
    let wash = if rainbow {
        Some(Bake::Wash)
    } else if let Some(color) = tier {
        Some(Bake::glow(color, 16))
    } else if secret {
        Some(Bake::glow(PINK, 14))
    } else {
        None
    };
    let (badge_bg, badge_color) = if locked {
        (white(0.05), faint)
    } else if rainbow {
        (gpui::transparent_black(), gpui::white())
    } else if let Some(color) = tier {
        (tone(color, 0.18), tone(color, 1.))
    } else if secret {
        (tone(PINK, 0.16), rgb(0xec7fbb).into())
    } else {
        (accent_soft, accent)
    };
    let badge = div()
        .relative()
        .flex_none()
        .size(u(44.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(u(radius_sm))
        .bg(badge_bg)
        .text_color(badge_color)
        .when(rainbow, |el| {
            el.child(rainbow_badge(index, zpx(zoom, radius_sm)))
        })
        .child(fa_glyph(&item.icon, 18.))
        .when(locked, |el| {
            el.child(
                div()
                    .absolute()
                    .right(u(-4.))
                    .bottom(u(-4.))
                    .size(u(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(modal_bg)
                    .border_1()
                    .border_color(border)
                    .text_color(faint)
                    .child(fa_glyph("fas fa-lock", 9.)),
            )
        });
    let content = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(u(4.))
        .child(
            div()
                .text_px(14., 20.)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if locked { dim } else { text })
                .child(item.title.clone()),
        )
        .child(
            div()
                .text_px(12.5, 18.)
                .text_color(dim)
                .child(item.text.clone()),
        )
        .when(item.meter, |el| {
            el.child(
                div()
                    .mt(u(8.))
                    .h(u(4.))
                    .rounded(u(2.))
                    .bg(white(0.08))
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(relative(item.percent.clamp(0, 100) as f32 / 100.))
                            .bg(accent),
                    ),
            )
        });
    let state = |done: bool| {
        div()
            .flex()
            .items_center()
            .gap(u(6.))
            .text_px(11.5, 20.)
            .font_weight(FontWeight::SEMIBOLD)
            .whitespace_nowrap()
            .tnum()
            .text_color(if done { rgb(0x4ad07a).into() } else { faint })
    };
    let side = if item.unlocked {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .items_end()
            .child(
                state(true)
                    .child(fa_glyph("fas fa-check", 11.5))
                    .child(item.state.clone()),
            )
            .when(!item.at.is_empty(), |el| {
                el.child(
                    div()
                        .text_px(11., 16.)
                        .text_color(faint)
                        .whitespace_nowrap()
                        .tnum()
                        .child(item.at.clone()),
                )
            })
    } else {
        state(false).flex_none().child(item.state.clone())
    };
    let title = item.title.clone();
    hover(("ach-hover", index), Timing::fast(), move |t| {
        let t = if locked { 0. } else { t };
        // Наведение у рамки: обычная — к акценту, золото/фиолет — ярче; у радуги рамки нет.
        let shadows = if rainbow {
            vec![shadow(zoom, 0., 0., 18., 0., tone((167, 139, 250), 0.18))]
        } else if let Some(color) = tier {
            vec![
                ring(zoom, 1., tone(color, 0.5 + 0.3 * t)),
                shadow(zoom, 0., 0., 16. + 2. * t, 0., tone(color, 0.14 + 0.08 * t)),
            ]
        } else {
            vec![ring(zoom, 1., mix(border, border_hover, t))]
        };
        div()
            .id(("ach", index))
            .debug_selector(|| format!("ach-{index}"))
            .role(Role::ListItem)
            .aria_label(title)
            .relative()
            .flex_1()
            .flex()
            .items_center()
            .gap(u(16.))
            .p(u(16.))
            .rounded(u(radius))
            // Заливка сразу поверх фона окна (он непрозрачный): наружное свечение GPUI рисует
            // и под блоком, сквозь полупрозрачную заливку оно красило бы всю карточку.
            .bg(mix(modal_bg, gpui::white(), 0.03 + 0.02 * t))
            .shadow(shadows)
            .when_some(wash, |el, kind| el.child(backdrop(kind, zpx(zoom, radius))))
            .child(badge)
            .child(content)
            .child(side)
            .when(rainbow, |el| el.child(rainbow_ring(index, radius, zoom)))
    })
    .into_any_element()
}

/// Вкладка «Достижения» (со своей прокруткой — кэшируется окном как единый вид).
pub struct AchievementsTab {
    scroll: Scroll,
    rows: Rows,
    /// Общая высота ряда (`grid-auto-rows: 1fr`): `(ширина, зум, макс. высота)`.
    row_h: Rc<Cell<(Pixels, f32, Pixels)>>,
}

impl AchievementsTab {
    pub fn new(_services: &Services, _cx: &mut Context<AchievementsTab>) -> AchievementsTab {
        AchievementsTab {
            scroll: Scroll::new(),
            rows: Rows::default(),
            row_h: Rc::new(Cell::new((Pixels::ZERO, 0., Pixels::ZERO))),
        }
    }

    pub fn reset_scroll(&mut self) {
        self.scroll = Scroll::new();
        self.rows = Rows::default();
        self.row_h.set((Pixels::ZERO, 0., Pixels::ZERO));
    }
}

impl Render for AchievementsTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let Some(context) = ProfileContext::get(cx) else {
            return div().into_any_element();
        };
        let state = context.state.clone();
        let Some(modal) = context.modal.upgrade() else {
            return div().into_any_element();
        };
        let (view, tr) = {
            let modal = modal.read(cx);
            let view = modal.view_snapshot().unwrap_or_else(|| {
                std::rc::Rc::new(state.borrow().view(modal.language, &modal.wall, &modal.tr))
            });
            (view, modal.tr.clone())
        };
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let faint = theme.color("l-text-faint");
        let accent = theme.color("l-accent");
        let complete = view.complete;
        let tokens = &hlsr_ui::motion::registry().tokens;
        let slow = Timing::new(tokens.l_dur_slow, tokens.l_ease);
        let head = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(u(16.))
            .child(spaced(
                tr("#L_PROFILE_ACHIEVEMENTS", &[]).to_uppercase(),
                11.,
                16.,
                FontWeight::BOLD,
                0.12,
                faint,
            ))
            .child(
                div()
                    .text_px(12., 20.)
                    .text_color(faint)
                    .tnum()
                    .child(view.counter.clone()),
            );
        // `.profile__meter`: 4 px, поля 12/20; при 100 % — золотая полоса.
        let meter = div()
            .mt(u(12.))
            .mb(u(20.))
            .h(u(4.))
            .rounded(u(2.))
            .bg(white(0.08))
            .overflow_hidden()
            .child(tween(
                "ach-meter",
                view.percent as f32 / 100.,
                slow,
                move |fill| {
                    let bar = div().relative().h_full().w(relative(fill));
                    if complete {
                        bar.children(strips(
                            &Gradient {
                                angle: 90.,
                                stops: [(0xf5a623, 0.), (0xffd86b, 0.5), (0xf5c542, 1.)]
                                    .map(|(color, pos)| Stop {
                                        color: rgb(color),
                                        pos,
                                    })
                                    .to_vec(),
                            },
                            px(0.),
                        ))
                    } else {
                        bar.bg(accent)
                    }
                },
            ));
        let row_count = view.achievements.len().div_ceil(2);
        let keys: Vec<String> = (0..row_count)
            .map(|r| {
                let a = &view.achievements[r * 2];
                match view.achievements.get(r * 2 + 1) {
                    Some(b) => format!("{}:{}", a.title, b.title),
                    None => a.title.clone(),
                }
            })
            .collect();
        let slots = self.rows.plan(&self.scroll, keys, 12., window, cx);
        let (_, saved_zoom, min_row_h) = self.row_h.get();
        let min_row_h = (saved_zoom == zoom && min_row_h > Pixels::ZERO).then_some(min_row_h);
        let grid_rows: Vec<AnyElement> = slots
            .into_iter()
            .map(|slot| match slot {
                Slot::Row(r) => {
                    let left = r * 2;
                    let right = left + 1;
                    let probe_h = self.row_h.clone();
                    let probe = canvas(
                        move |bounds, window, _| {
                            let (lw, lz, lh) = probe_h.get();
                            let z = crate::scale::zoom(window);
                            if (f32::from(bounds.size.width) - f32::from(lw)).abs() > 0.5
                                || z != lz
                            {
                                probe_h.set((bounds.size.width, z, bounds.size.height));
                                crate::motion::monitor_frame(window);
                            } else if f32::from(bounds.size.height) > f32::from(lh) + 0.5 {
                                probe_h.set((lw, lz, bounds.size.height));
                                crate::motion::monitor_frame(window);
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0();
                    div()
                        .relative()
                        .flex()
                        .items_stretch()
                        .gap(u(12.))
                        .when_some(min_row_h, |el, h| el.min_h(h))
                        .child(probe)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(card(&view.achievements[left], left, &theme, zoom)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .when_some(view.achievements.get(right), |el, item| {
                                    el.child(card(item, right, &theme, zoom))
                                }),
                        )
                        .into_any_element()
                }
                Slot::Space(height) => div().flex_none().h(height).into_any_element(),
            })
            .collect();
        let grid = self
            .rows
            .measure(
                &self.scroll,
                div().flex().flex_col().gap(u(12.)).children(grid_rows),
            )
            .id("ach-list")
            .role(Role::List);
        let body = div()
            .flex()
            .flex_col()
            .pt(u(28.))
            .px(u(32.))
            .pb(u(32.))
            .child(head)
            .child(meter)
            .child(grid);
        crate::snap::target(
            "profile-body",
            ScrollArea::new(("profile-scroll", 1usize), &self.scroll)
                .bar(Bar::Thin)
                .size_full()
                .child(body),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Rgba, b: Rgba) -> bool {
        [(a.r, b.r), (a.g, b.g), (a.b, b.b), (a.a, b.a)]
            .iter()
            .all(|(x, y)| (x - y).abs() < 0.01)
    }

    #[test]
    fn the_rainbow_runs_along_115_degrees_and_wraps_after_one_tile() {
        // Углы блока — края градиентной линии: левый верхний — первый стоп, правый нижний — последний.
        assert!(near(linear_at(&RING, 300., 100., 0., 0.), RING[0]));
        assert!(near(linear_at(&RING, 300., 100., 300., 100.), RING[6]));
        // Середина — середина линии (4-й стоп из 7).
        assert!(near(linear_at(&RING, 300., 100., 150., 50.), RING[3]));
        // Сдвиг на полторы ширины полосы (`150%`) — ровно одна плитка 3W: тот же вид.
        assert_eq!(offset(100., 1.5), -300.);
        assert_eq!(offset(100., 0.), 0.);
    }

    #[test]
    fn the_tier_glow_fades_out_at_60_percent_of_the_ellipse() {
        let gold = rgba(245, 197, 66, 0.16);
        assert!((glow_at(gold, 400., 100., 0., 0.).a - 0.16).abs() < 1e-6);
        // По горизонтали 60 % от 120 % ширины — 0,72 W.
        assert!(glow_at(gold, 400., 100., 288., 0.).a.abs() < 1e-6);
        assert!((glow_at(gold, 400., 100., 144., 0.).a - 0.08).abs() < 1e-4);
        // Прозрачный край смешивается без серого: цвет тот же.
        let half = lerp(gold, Rgba { a: 0., ..gold }, 0.5);
        assert!(near(half, Rgba { a: 0.08, ..gold }));
    }
}
