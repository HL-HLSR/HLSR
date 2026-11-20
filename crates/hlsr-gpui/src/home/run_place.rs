//! Места 1–3 в «Рекордах» (`.home-record.is-podium` HomePanel 0.4.9, просьба владельца 14.11):
//! золото, серебро, бронза — цвет места, не темы. Фон карточки — `radial` и `linear` поверх
//! `--l-surface-solid`, кольцо .5 и свечение 22 px; сзади пульсирует свечение (`::after`,
//! `home-podium-glow` 2,8 с), раз в 4,5 с по карточке проходит блик (`::before`); справа внизу
//! кубок с номером (`home-podium-trophy`, масштаб до 1.06), у времени мягкая тень.
//!
//! Градиенты — растры SVG (resvg) в долях карточки: растянутые на неё (`ObjectFit::Fill`), они
//! совпадают с `radial-gradient` в процентах. Кубок — `fa-trophy` FA 7.3.1 с градиентом
//! (`background-clip: text`) и двумя `drop-shadow` одним растром. Движение стоит под `AnimGate`
//! (`.no-anim`/reduced-motion: свечение полное, кубок без масштаба, блика нет).

use crate::games::hero::shadow_art;
use crate::image::{Raster, device_size, render_image};
use crate::kit::button::child_id;
use crate::motion::{AnimGate, Frame, keyframes, registry};
use crate::paint::{ring, shadow};
use crate::scale::{u, zoom, zpx};
use crate::text::{RUBIK, TextExt};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, BoxShadow, Div, ElementId, Font, FontWeight, Hsla, ObjectFit, RenderImage,
    SharedString, Window, div, font, img, linear_color_stop, linear_gradient, relative, rgb,
};
use hlsr_ui::motion::CubicBezier;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

/// `@keyframes home-podium-glow` (и кадры карточки): 2,8 с, `alternate`, бесконечно.
const GLOW: &str = "home-podium-glow";
/// `@keyframes home-podium-trophy`: масштаб 1 → 1.06 в такт свечению.
const TROPHY_PULSE: &str = "home-podium-trophy";
/// `home-podium-shine 4.5s cubic-bezier(.45, 0, .25, 1) infinite`: до 55 % стоит слева
/// (−120 %), потом уходит направо (320 % своей ширины, 40 % карточки).
const SHINE_MS: f64 = 4500.;
const SHINE_CURVE: [f64; 4] = [0.45, 0., 0.25, 1.];

/// Цвета места (`--pod-rgb`, `--pod`, `--pod-light`, `--pod-deep`, `--pod-ink`).
#[derive(Debug, PartialEq, Eq)]
pub struct Pod {
    rgb: u32,
    pod: u32,
    light: u32,
    deep: u32,
    ink: u32,
}

const GOLD: Pod = Pod {
    rgb: 0xf5c542,
    pod: 0xf5c542,
    light: 0xffe08a,
    deep: 0xb8860b,
    ink: 0x4a3200,
};
const SILVER: Pod = Pod {
    rgb: 0xc8d4e2,
    pod: 0xd3dce7,
    light: 0xf4f7fb,
    deep: 0x7d8a99,
    ink: 0x262e38,
};
const BRONZE: Pod = Pod {
    rgb: 0xd68740,
    pod: 0xd98a45,
    light: 0xf6c08e,
    deep: 0x8a4a1a,
    ink: 0x3a1c05,
};

/// Цвета места 1–3; другое место — обычная карточка.
pub fn pod(place: u8) -> Option<&'static Pod> {
    match place {
        1 => Some(&GOLD),
        2 => Some(&SILVER),
        3 => Some(&BRONZE),
        _ => None,
    }
}

fn hex(color: u32, alpha: f32) -> Hsla {
    let mut c = rgb(color);
    c.a = alpha;
    c.into()
}

impl Pod {
    /// `rgba(var(--pod-rgb), alpha)`.
    pub fn rgba(&self, alpha: f32) -> Hsla {
        hex(self.rgb, alpha)
    }

    /// `--pod`: время забега.
    pub fn color(&self) -> Hsla {
        hex(self.pod, 1.)
    }

    /// `--pod-light`: метка «Мировой рекорд».
    pub fn light(&self) -> Hsla {
        hex(self.light, 1.)
    }

    /// Кольцо `inset 1px rgba(pod, .5)` и свечение `0 0 22px rgba(pod, .14)`; наведение `t` —
    /// .75 и 26 px .22.
    pub fn shadows(&self, t: f32, zoom: f32) -> Vec<BoxShadow> {
        vec![
            ring(zoom, 1., self.rgba(0.5 + 0.25 * t)),
            shadow(zoom, 0., 0., 22. + 4. * t, 0., self.rgba(0.14 + 0.08 * t)),
        ]
    }

    /// Метка места `el` (`.home-record__sub--wr`): `linear-gradient(90deg, rgba(pod, .22),
    /// rgba(pod, .1))`, кольцо .55, цвет `--pod-light`.
    pub fn chip(&self, el: Div, zoom: f32) -> Div {
        el.bg(linear_gradient(
            90.,
            linear_color_stop(self.rgba(0.22), 0.),
            linear_color_stop(self.rgba(0.1), 1.),
        ))
        .shadow(vec![ring(zoom, 1., self.rgba(0.55))])
        .text_color(self.light())
    }
}

/// Растры места: градиенты в долях блока и кубок в физических px.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Art {
    Back,
    Glow,
    Shine,
    Trophy(u32),
}

/// Готовые растры: место и вид → картинка (`None` — не отрисовалась).
type Arts = HashMap<(u8, Art), Option<Arc<RenderImage>>>;

thread_local! {
    // shortcut: растры мест живут в атласе до конца процесса — их 9 градиентов и по кубку на
    // размер в физических px; станет больше (размер на каждый кадр) — `Raster` на карточку.
    static ARTS: RefCell<Arts> = RefCell::default();
}

fn art(place: u8, kind: Art) -> Option<Arc<RenderImage>> {
    let pod = pod(place)?;
    ARTS.with(|arts| {
        arts.borrow_mut()
            .entry((place, kind))
            .or_insert_with(|| {
                let (svg, width, height) = match kind {
                    // radial-gradient(140% 160% at 100% 50%, rgba(pod, A), transparent 55%).
                    Art::Back => (radial_svg(pod.rgb, (1., 0.5), (1.4, 1.6), 0.55), 256, 64),
                    // ::after: radial-gradient(60% 120% at 92% 50%, rgba(pod, .22), transparent 70%).
                    Art::Glow => (radial_svg(pod.rgb, (0.92, 0.5), (0.6, 1.2), 0.7), 256, 64),
                    // ::before: linear-gradient(105deg, transparent 20%, rgba(pod, .16) 50%,
                    // transparent 80%) на 40 % карточки (около 5,3 : 1 при 1600 × 900).
                    Art::Shine => (
                        linear_svg(pod.rgb, 512., 96., 105., &[(0.2, 0.), (0.5, 1.), (0.8, 0.)]),
                        512,
                        96,
                    ),
                    Art::Trophy(side) => (trophy_svg(pod), side, side),
                };
                let image = hlsr_ui::safe_svg::render(svg.as_bytes(), width, height);
                image.map(|image| render_image(&image))
            })
            .clone()
    })
}

/// Градиент одного цвета: альфа 1 в центре и 0 на `fade` луча; эллипс в долях блока.
fn radial_svg(color: u32, (cx, cy): (f32, f32), (rx, ry): (f32, f32), fade: f32) -> String {
    format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='256' height='64' viewBox='0 0 1 1' \
         preserveAspectRatio='none'><radialGradient id='g' gradientUnits='userSpaceOnUse' \
         cx='0' cy='0' r='1' gradientTransform='matrix({rx} 0 0 {ry} {cx} {cy})'>\
         <stop offset='0' stop-color='#{color:06x}'/>\
         <stop offset='{fade}' stop-color='#{color:06x}' stop-opacity='0'/></radialGradient>\
         <rect width='1' height='1' fill='url(#g)'/></svg>"
    )
}

/// Конец и начало линии `linear-gradient(<angle>deg, …)` в блоке `width × height` (как CSS:
/// 0deg — вверх, линия через центр, длина — проекция блока на неё).
fn gradient_line(width: f32, height: f32, angle: f32) -> [f32; 4] {
    let (sin, cos) = angle.to_radians().sin_cos();
    let (dx, dy) = (sin, -cos);
    let half = (width * dx.abs() + height * dy.abs()) / 2.;
    let (cx, cy) = (width / 2., height / 2.);
    [
        cx - dx * half,
        cy - dy * half,
        cx + dx * half,
        cy + dy * half,
    ]
}

/// Линейный градиент одного цвета: стопы (позиция, альфа).
fn linear_svg(color: u32, width: f32, height: f32, angle: f32, stops: &[(f32, f32)]) -> String {
    let [x1, y1, x2, y2] = gradient_line(width, height, angle);
    let mut out = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='{height}' \
         viewBox='0 0 {width} {height}'><linearGradient id='g' gradientUnits='userSpaceOnUse' \
         x1='{x1}' y1='{y1}' x2='{x2}' y2='{y2}'>"
    );
    for (at, alpha) in stops {
        let _ = write!(
            out,
            "<stop offset='{at}' stop-color='#{color:06x}' stop-opacity='{alpha}'/>"
        );
    }
    let _ = write!(
        out,
        "</linearGradient><rect width='{width}' height='{height}' fill='url(#g)'/></svg>"
    );
    out
}

/// `fa-trophy` (Font Awesome Free 7.3.1, CC BY 4.0), квадрат 512 = 1em.
const TROPHY_PATH: &str = "M144.3 0l224 0c26.5 0 48.1 21.8 47.1 48.2-.2 5.3-.4 10.6-.7 15.8l49.6 0c26.1 0 49.1 21.6 47.1 49.8-7.5 103.7-60.5 160.7-118 190.5-15.8 8.2-31.9 14.3-47.2 18.8-20.2 28.6-41.2 43.7-57.9 51.8l0 73.1 64 0c17.7 0 32 14.3 32 32s-14.3 32-32 32l-192 0c-17.7 0-32-14.3-32-32s14.3-32 32-32l64 0 0-73.1c-16-7.7-35.9-22-55.3-48.3-18.4-4.8-38.4-12.1-57.9-23.1-54.1-30.3-102.9-87.4-109.9-189.9-1.9-28.1 21-49.7 47.1-49.7l49.6 0c-.3-5.2-.5-10.4-.7-15.8-1-26.5 20.6-48.2 47.1-48.2zM101.5 112l-52.4 0c6.2 84.7 45.1 127.1 85.2 149.6-14.4-37.3-26.3-86-32.8-149.6zM380 256.8c40.5-23.8 77.1-66.1 83.3-144.8L411 112c-6.2 60.9-17.4 108.2-31 144.8z";
/// Глиф 40 px CSS; вокруг поле 14 px под свечение (3σ = 12 px).
const GLYPH: f32 = 40.;
const TROPHY_PAD: f32 = 14.;

/// Кубок 40 px: `linear-gradient(160deg, light 10%, pod 45%, deep 95%)` по глифу,
/// `drop-shadow(0 0 8px rgba(pod, .45)) drop-shadow(0 2px 2px rgba(0, 0, 0, .45))`.
fn trophy_svg(pod: &Pod) -> String {
    let unit = 512. / GLYPH;
    let pad = TROPHY_PAD * unit;
    let side = 512. + 2. * pad;
    let [x1, y1, x2, y2] = gradient_line(512., 512., 160.);
    format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{side}' height='{side}' \
         viewBox='{} {} {side} {side}'><defs><linearGradient id='g' \
         gradientUnits='userSpaceOnUse' x1='{x1}' y1='{y1}' x2='{x2}' y2='{y2}'>\
         <stop offset='.1' stop-color='#{:06x}'/><stop offset='.45' stop-color='#{:06x}'/>\
         <stop offset='.95' stop-color='#{:06x}'/></linearGradient>\
         <filter id='f' x='-50%' y='-50%' width='200%' height='200%' \
         color-interpolation-filters='sRGB'>\
         <feDropShadow dx='0' dy='0' stdDeviation='{}' flood-color='#{:06x}' flood-opacity='.45'/>\
         <feDropShadow dx='0' dy='{}' stdDeviation='{}' flood-color='#000' flood-opacity='.45'/>\
         </filter></defs><path d='{TROPHY_PATH}' fill='url(#g)' filter='url(#f)'/></svg>",
        -pad,
        -pad,
        pod.light,
        pod.pod,
        pod.deep,
        4. * unit,
        pod.rgb,
        2. * unit,
        unit,
    )
}

/// Доля пути блика 0…1 через `elapsed` мс (`translateX(-120% → 320%)` после паузы 55 %).
fn shine_progress(elapsed: f64) -> f64 {
    let phase = elapsed.rem_euclid(SHINE_MS) / SHINE_MS;
    if phase <= 0.55 {
        0.
    } else {
        CubicBezier::new(SHINE_CURVE).solve((phase - 0.55) / 0.45)
    }
}

/// Слой картинки места поверх всей карточки (радиус `radius` px CSS).
fn cover(image: Arc<RenderImage>, radius: f32, opacity: f32) -> impl IntoElement {
    img(image)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(u(radius))
        .object_fit(ObjectFit::Fill)
        .opacity(opacity)
}

/// Фон места под содержимым карточки радиуса `radius`: `linear` и `radial` (наведение `t`),
/// блик и пульсирующее свечение. Слой `absolute` во всю карточку, с обрезкой (её
/// `overflow: hidden`); строит его на каждый кадр [`Podium::render`].
fn backdrop(place: u8, pod: &Pod, t: f32, glow: f32, shine: Option<f64>, radius: f32) -> Div {
    let linear = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(u(radius))
        .bg(linear_gradient(
            90.,
            linear_color_stop(pod.rgba(0.07 + 0.02 * t), 0.),
            linear_color_stop(pod.rgba(0.), 0.45),
        ));
    // shortcut: обрезка GPUI прямоугольная — у скруглённых углов блик на миг выходит за угол
    // (альфа ≤ .16 на нескольких px); нужен точный угол — маска блика по карточке.
    let shine = shine.and_then(|progress| {
        let x = -1.2 + 4.4 * progress as f32;
        art(place, Art::Shine).map(|image| {
            img(image)
                .absolute()
                .top_0()
                .h_full()
                .w(relative(0.4))
                .left(relative(0.4 * x))
                .object_fit(ObjectFit::Fill)
                .opacity(0.16)
        })
    });
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .overflow_hidden()
        .child(linear)
        .children(art(place, Art::Back).map(|image| cover(image, radius, 0.2 + 0.06 * t)))
        .children(shine)
        .children(art(place, Art::Glow).map(|image| cover(image, radius, 0.22 * glow)))
}

/// Место карточки, готовое к кадру: цвета, движение и кубок (их собирают до наведения — там
/// нет окна).
pub struct Podium {
    id: ElementId,
    place: u8,
    pod: &'static Pod,
    moving: bool,
    /// Путь блика этого кадра; `None` — движения нет.
    shine: Option<f64>,
    trophy: AnyElement,
}

/// Место 1–3 карточки `id`; другое — `None`. Блик идёт по своим часам с монтирования.
pub fn podium(id: &ElementId, place: u8, window: &mut Window, cx: &mut App) -> Option<Podium> {
    let pod = pod(place)?;
    let moving = registry().keyframes_moving(GLOW, AnimGate::still(cx));
    let now = cx.background_executor().now();
    let start = window.use_keyed_state(child_id(id, "start"), cx, |_, _| now);
    let elapsed = now.saturating_duration_since(*start.read(cx)).as_secs_f64() * 1000.;
    Some(Podium {
        id: child_id(id, "glow"),
        place,
        pod,
        moving,
        shine: moving.then(|| shine_progress(elapsed)),
        trophy: trophy(child_id(id, "trophy"), place, pod, window),
    })
}

impl Podium {
    pub fn pod(&self) -> &'static Pod {
        self.pod
    }

    /// Карточка этого кадра: `build(фон, кубок)` (наведение `t`, радиус карточки `radius`).
    /// Кадры просит `home-podium-glow` — пауза под `AnimGate`.
    pub fn render(
        self,
        t: f32,
        radius: f32,
        build: impl FnOnce(Div, AnyElement) -> AnyElement + 'static,
    ) -> AnyElement {
        let Podium {
            id,
            place,
            pod,
            moving,
            shine,
            trophy,
        } = self;
        keyframes(id, GLOW, move |frame: &Frame| {
            let glow = if moving { frame.opacity as f32 } else { 1. };
            build(backdrop(place, pod, t, glow, shine, radius), trophy)
        })
        .into_any_element()
    }
}

/// `.home-record__trophy`: 48 × 48 справа внизу (20, 12), кубок пульсирует масштабом, номер
/// места в чаше (14/800 цветом `--pod-ink`).
fn trophy(id: ElementId, place: u8, pod: &Pod, window: &mut Window) -> AnyElement {
    let side = GLYPH + 2. * TROPHY_PAD;
    // Растр на наибольший масштаб: в такте он только уменьшается.
    let (device, _) = device_size(window, side * 1.06, side * 1.06);
    let image = art(place, Art::Trophy(device));
    let rank = div()
        .absolute()
        .left_0()
        .w(u(48.))
        .top(u(12.5))
        .h(u(14.))
        .flex()
        .justify_center()
        .text_px(14., 14.)
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(hex(pod.ink, 1.))
        .child(place.to_string());
    let cup = keyframes(id, TROPHY_PULSE, move |frame: &Frame| {
        let size = side * frame.scale as f32;
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(image.map(|image| {
                img(image)
                    .absolute()
                    .left(u(24. - size / 2.))
                    .top(u(24. - size / 2.))
                    .size(u(size))
                    .object_fit(ObjectFit::Fill)
            }))
    });
    // shortcut: номер не растёт вместе с кубком (×1.06): кегль на каждом кадре — новые глифы в
    // атласе; разница — доля пикселя.
    div()
        .absolute()
        .right(u(20.))
        .bottom(u(12.))
        .size(u(48.))
        .child(cup)
        .child(rank)
        .into_any_element()
}

/// Растр тени времени: ключ (текст, размеры в физических px, цвет места) и место картинки.
#[derive(Default)]
struct Glow {
    key: Option<(SharedString, [u32; 2], u32)>,
    rect: Option<[f32; 4]>,
    raster: Raster,
}

/// `.home-record__time` у места: 20/24 700 цветом места с `text-shadow: 0 0 14px rgba(pod,
/// .35)` — маска строки, размытая на ЦП (решение владельца 2).
pub fn time(
    id: ElementId,
    text: SharedString,
    pod: &Pod,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let (size, line) = (20., 24.);
    let zoom = zoom(window);
    let scale = zoom * window.scale_factor();
    let key = (
        text.clone(),
        [size * scale, 14. * scale].map(f32::to_bits),
        pod.rgb,
    );
    let state = window.use_keyed_state(id, cx, |_, cx| {
        cx.on_release(|state: &mut Glow, cx| state.raster.release(cx))
            .detach();
        Glow::default()
    });
    let rgb = pod.rgb;
    let rect = state.update(cx, |state, _| {
        if state.key.as_ref() != Some(&key) {
            let art = shadow_art(&text, size * scale, 0., 14. * scale, 0.35).map(|mut art| {
                for pixel in art.image.make_mut_slice() {
                    [pixel.r, pixel.g, pixel.b] = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
                }
                art
            });
            match &art {
                Some(art) => state.raster.set(&art.image),
                None => state.raster.clear(),
            }
            state.rect = art.map(|art| {
                let image = &art.image;
                [art.x, art.y, image.width() as f32, image.height() as f32]
            });
            state.key = Some(key);
        }
        state.rect
    });
    let image = state.update(cx, |state, cx| state.raster.image(window, cx));
    // Линия букв GPUI: половинный интерлиньяж + подъём (спуск в метриках отрицательный).
    let text_system = window.text_system();
    let font_id = text_system.resolve_font(&Font {
        weight: FontWeight::BOLD,
        ..font(RUBIK)
    });
    let font_px = zpx(zoom, size);
    let ascent = f32::from(text_system.ascent(font_id, font_px));
    let descent = f32::from(text_system.descent(font_id, font_px)).abs();
    let baseline = ((line * zoom - ascent - descent) / 2. + ascent) / zoom;
    div()
        .relative()
        .flex_none()
        .children(image.zip(rect).map(|(image, [x, y, width, height])| {
            img(image)
                .absolute()
                .left(u(x / scale))
                .top(u(baseline + y / scale))
                .w(u(width / scale))
                .h(u(height / scale))
                .object_fit(ObjectFit::Fill)
        }))
        .child(
            div()
                .relative()
                .text_px(size, line)
                .font_weight(FontWeight::BOLD)
                .tnum()
                .text_color(pod.color())
                .child(text),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_one_to_three_have_their_colours_only() {
        assert_eq!(pod(1), Some(&GOLD));
        assert_eq!(pod(3), Some(&BRONZE));
        assert!(pod(0).is_none() && pod(4).is_none());
    }

    #[test]
    fn the_shine_waits_then_crosses_the_card() {
        // До 55 % цикла блик стоит за левым краем, к концу цикла — за правым.
        assert_eq!(shine_progress(0.), 0.);
        assert_eq!(shine_progress(0.55 * SHINE_MS), 0.);
        assert!((shine_progress(SHINE_MS - 1.) - 1.).abs() < 0.01);
        let mid = shine_progress(0.775 * SHINE_MS);
        assert!(mid > 0.3 && mid < 0.9, "{mid}");
        // Следующий цикл начинается заново.
        assert_eq!(shine_progress(SHINE_MS + 10.), 0.);
    }

    #[test]
    fn gradient_lines_follow_css_angles() {
        // 90deg — слева направо по середине, 180deg — сверху вниз.
        assert_eq!(
            gradient_line(100., 50., 90.).map(f32::round),
            [0., 25., 100., 25.]
        );
        assert_eq!(
            gradient_line(100., 50., 180.).map(f32::round),
            [50., 0., 50., 50.]
        );
    }

    struct Card(u8);

    impl Render for Card {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let id = ElementId::Name("card".into());
            match podium(&id, self.0, window, cx) {
                Some(podium) => podium.render(0., 12., |backdrop, trophy| {
                    div()
                        .relative()
                        .w(u(400.))
                        .h(u(106.))
                        .child(backdrop)
                        .child(trophy)
                        .into_any_element()
                }),
                None => div().into_any_element(),
            }
        }
    }

    fn open(cx: &mut gpui::TestAppContext, place: u8) -> gpui::WindowHandle<Card> {
        let window = cx.open_window(gpui::size(zpx(1., 500.), zpx(1., 200.)), move |_, _| {
            Card(place)
        });
        cx.run_until_parked();
        window
    }

    /// Кадр и число тех, кто его просил.
    fn frame(window: &gpui::WindowHandle<Card>, cx: &mut gpui::TestAppContext) -> usize {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        let count = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        cx.run_until_parked();
        count
    }

    #[gpui::test]
    fn podium_motion_runs_only_under_an_open_gate(cx: &mut gpui::TestAppContext) {
        assert_eq!(frame(&open(cx, 4), cx), 0);
        let gold = open(cx, 1);
        assert!(frame(&gold, cx) > 0);
        assert!(frame(&gold, cx) > 0);
        // Окно спрятано: свечение, кубок и блик стоят.
        cx.update(|cx| AnimGate::set_window_hidden(cx, true));
        cx.run_until_parked();
        frame(&gold, cx);
        assert_eq!(frame(&gold, cx), 0);
        cx.update(|cx| AnimGate::set_window_hidden(cx, false));
        cx.run_until_parked();
        assert!(frame(&gold, cx) > 0);
        // Без анимаций место стоит совсем.
        cx.update(|cx| AnimGate::set_no_anim(cx, true));
        cx.run_until_parked();
        frame(&gold, cx);
        assert_eq!(frame(&gold, cx), 0);
    }

    #[test]
    fn podium_rasters_render() {
        for place in 1..=3 {
            for kind in [Art::Back, Art::Glow, Art::Shine, Art::Trophy(72)] {
                assert!(art(place, kind).is_some(), "{place} {kind:?}");
            }
        }
        assert!(art(4, Art::Back).is_none());
    }
}
