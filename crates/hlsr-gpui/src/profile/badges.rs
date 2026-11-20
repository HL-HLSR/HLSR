//! Значки под ником (`.profile__badges` 0.4.9): плашка с полями 4 px, значки 32 × 32 со
//! скруглением 6 и подсказкой. Картинки разработчика, баг-хантера, бустера и годовщин светятся
//! своим цветом (`filter: drop-shadow(0 0 3px rgba(--glow, .55))`) и «дышат» 2,4 с
//! (`profile-badge-glow`: от 2 px .35 до 6 px .95, туда и обратно). Свечение — размытие маски
//! картинки на ЦП (решение владельца 2, ядро `render::shadow_blur`): растр на каждую из
//! [`LEVELS`] ступеней дыхания, один раз на размер. Без анимаций свечение стоит (3 px, .55).
//! «Подтверждён» — глиф акцентом; свои значки из админки — SVG (`themed` — силуэт акцентом).

use crate::image::{Pictures, device_size};
use crate::kit::button::white;
use crate::layer::TipExt;
use crate::motion::{AnimGate, Timing, hover, keyframes};
use crate::paint::ring;
use crate::scale::u;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, IntoElement, ObjectFit, RenderImage, Window, div, img, prelude::*, svg,
};
use hlsr_ui::controllers::profile::BadgeView;
use hlsr_ui::images::{Fit, ImageRequest, ImageSource, data_uri_bytes};
use hlsr_ui::render::shadow_blur::gaussian_blur_alpha;
use hlsr_ui::{Rgba, Rgba8Image};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// Ступеней дыхания свечения (кадр берёт ближайшую).
pub const LEVELS: u32 = 16;

/// Область загрузчика картинок окна профиля.
pub const SCOPE: &str = "profile";

/// Картинки встроенных значков (`assets/launcher/badges`).
fn asset_bytes(asset: &str) -> Option<&'static [u8]> {
    macro_rules! png {
        ($name:literal) => {
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../hlsr-ui/assets/launcher/badges/",
                $name,
                ".png"
            ))
        };
    }
    Some(match asset {
        "developer" => png!("developer"),
        "bughunter" => png!("bughunter"),
        "booster" => png!("booster"),
        "anniversary-1" => png!("anniversary-1"),
        "anniversary-2" => png!("anniversary-2"),
        "anniversary-3" => png!("anniversary-3"),
        "anniversary-4" => png!("anniversary-4"),
        "anniversary-5" => png!("anniversary-5"),
        "anniversary-6" => png!("anniversary-6"),
        "anniversary-7" => png!("anniversary-7"),
        "anniversary-8" => png!("anniversary-8"),
        "anniversary-9" => png!("anniversary-9"),
        "anniversary-10" => png!("anniversary-10"),
        _ => return None,
    })
}

/// Свечение ступени `level` (0…[`LEVELS`]; `None` — неподвижное): размытие CSS px и прозрачность.
pub fn glow_of(level: Option<u32>) -> (f32, f32) {
    match level {
        None => (3., 0.55),
        Some(level) => {
            let mix = level.min(LEVELS) as f32 / LEVELS as f32;
            (2. + 4. * mix, 0.35 + 0.6 * mix)
        }
    }
}

/// Самое сильное свечение (6 px): поле растра под него, чтобы размер от кадра не менялся.
const REACH: f32 = 6.;

/// Растр значка `side` CSS px с тенью `glow` при масштабе `scale` (физические px на CSS px):
/// картинка вписана в квадрат, под ней размытая маска её прозрачности цветом свечения. Возвращает
/// растр и поле вокруг картинки в CSS px.
pub fn glow_raster(
    png: &[u8],
    side: f32,
    rgb: [u8; 3],
    level: Option<u32>,
    scale: f32,
) -> Option<(Rgba8Image, f32)> {
    let icon = ::image::load_from_memory(png).ok()?.into_rgba8();
    let s = scale.max(0.01);
    let px_side = (side * s).round().max(1.) as u32;
    // CSS: drop-shadow blur — радиус, σ = blur / 2; поле — 3σ самого сильного кадра.
    let pad = (REACH / 2. * 3. * s).ceil() as u32 + 1;
    let total = px_side + pad * 2;
    let small = ::image::imageops::resize(
        &icon,
        px_side,
        px_side,
        ::image::imageops::FilterType::Triangle,
    );
    let (blur, alpha) = glow_of(level);
    let mut mask = vec![0u8; (total * total) as usize];
    for (x, y, p) in small.enumerate_pixels() {
        mask[((y + pad) * total + x + pad) as usize] = p.0[3];
    }
    gaussian_blur_alpha(&mut mask, total, total, blur / 2. * s);
    let mut out = Rgba8Image::new(total, total).premultiplied();
    let pixels = out.make_mut_slice();
    for (index, m) in mask.iter().enumerate() {
        // Тень предумноженным цветом.
        let a = f32::from(*m) / 255. * alpha;
        let pm = |c: u8| (f32::from(c) * a).round() as u8;
        pixels[index] = Rgba::new(pm(rgb[0]), pm(rgb[1]), pm(rgb[2]), (a * 255.).round() as u8);
    }
    for (x, y, p) in small.enumerate_pixels() {
        let [r, g, b, a] = p.0;
        let index = ((y + pad) * total + x + pad) as usize;
        let under = pixels[index];
        let fa = f32::from(a) / 255.;
        // Картинка поверх тени (source-over, предумноженные).
        let over = |c: u8, u: u8| (f32::from(c) * fa + f32::from(u) * (1. - fa)).round() as u8;
        pixels[index] = Rgba::new(
            over(r, under.r),
            over(g, under.g),
            over(b, under.b),
            (f32::from(a) + f32::from(under.a) * (1. - fa)).round() as u8,
        );
    }
    Some((out, pad as f32 / s))
}

/// Ключ картинки свечения: значок, сторона в физ. px, ступень.
type GlowKey = (&'static str, u32, Option<u32>);

thread_local! {
    /// Готовые картинки свечения и поле вокруг картинки (CSS px).
    // shortcut: кеш не чистится — 13 значков × 17 ступеней × масштабы, до сотен КиБ; чистить, если масштабов станет много.
    static GLOW: RefCell<HashMap<GlowKey, (Arc<RenderImage>, f32)>> = RefCell::new(HashMap::new());
}

fn glow_image(
    asset: &'static str,
    side: f32,
    rgb: [u8; 3],
    level: Option<u32>,
    scale: f32,
) -> Option<(Arc<RenderImage>, f32)> {
    let key = (asset, (side * scale).round() as u32, level);
    if let Some(hit) = GLOW.with(|c| c.borrow().get(&key).cloned()) {
        return Some(hit);
    }
    let (raster, pad) = glow_raster(asset_bytes(asset)?, side, rgb, level, scale)?;
    let made = (crate::image::render_image(&raster), pad);
    GLOW.with(|c| c.borrow_mut().insert(key, made.clone()));
    Some(made)
}

/// Картинка без свечения (годовщина 1): сам PNG, вписанный в квадрат.
fn plain_image(asset: &'static str, side: f32, scale: f32) -> Option<Arc<RenderImage>> {
    thread_local! {
        static PLAIN: RefCell<HashMap<(&'static str, u32), Arc<RenderImage>>> = RefCell::new(HashMap::new());
    }
    let px_side = (side * scale).round().max(1.) as u32;
    if let Some(hit) = PLAIN.with(|c| c.borrow().get(&(asset, px_side)).cloned()) {
        return Some(hit);
    }
    let icon = ::image::load_from_memory(asset_bytes(asset)?)
        .ok()?
        .into_rgba8();
    let small = ::image::imageops::resize(
        &icon,
        px_side,
        px_side,
        ::image::imageops::FilterType::Triangle,
    );
    let image =
        crate::image::render_image(&Rgba8Image::from_bytes(px_side, px_side, small.as_raw()));
    PLAIN.with(|c| c.borrow_mut().insert((asset, px_side), image.clone()));
    Some(image)
}

/// Картинка встроенного значка `side` CSS px; светящаяся «дышит» по кадрам реестра.
fn builtin(
    badge: &BadgeView,
    index: usize,
    side: f32,
    window: &Window,
    cx: &gpui::App,
) -> AnyElement {
    let asset = badge.asset;
    let scale = crate::scale::zoom(window) * window.scale_factor();
    let Some(rgb) = badge.glow else {
        return plain_image(asset, side, scale)
            .map(|image| {
                img(image)
                    .size(u(side))
                    .object_fit(ObjectFit::Contain)
                    .into_any_element()
            })
            .unwrap_or_else(|| div().size(u(side)).into_any_element());
    };
    let draw = move |level: Option<u32>| {
        let shown = glow_image(asset, side, rgb, level, scale);
        let el = div().relative().size(u(side));
        match shown {
            Some((image, pad)) => el.child(
                img(image)
                    .absolute()
                    .left(u(-pad))
                    .top(u(-pad))
                    .size(u(side + pad * 2.))
                    .object_fit(ObjectFit::Fill),
            ),
            None => el,
        }
    };
    if AnimGate::still(cx).any() {
        return draw(None).into_any_element();
    }
    keyframes(
        ("profile-badge-glow", index),
        "profile-badge-glow",
        move |frame| {
            let level = (frame.mix.clamp(0., 1.) * f64::from(LEVELS)).round() as u32;
            draw(Some(level))
        },
    )
    .into_any_element()
}

/// Плашка значков; `badges` не пустой.
pub fn badge_row<V: 'static>(
    badges: &[BadgeView],
    pictures: &Pictures,
    window: &mut Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let theme = cx.global::<Theme>().clone();
    let zoom = crate::scale::zoom(window);
    let accent = theme.color("l-accent");
    let mut row = div()
        .flex()
        .items_center()
        .p(u(4.))
        .rounded(theme.rem("l-radius-sm"))
        .bg(white(0.05))
        .shadow(vec![ring(zoom, 1., theme.color("l-border"))]);
    for (index, badge) in badges.iter().enumerate() {
        let content: AnyElement = if !badge.asset.is_empty() {
            builtin(
                badge,
                index,
                if badge.anniversary { 30. } else { 24. },
                window,
                cx,
            )
        } else if !badge.image.is_empty() && badge.themed {
            // Силуэт SVG цветом акцента (`mask`), 16 px.
            match data_uri_bytes(&badge.image) {
                Ok(bytes) => svg()
                    .data(&bytes)
                    .size(u(16.))
                    .text_color(accent)
                    .into_any_element(),
                Err(_) => div().size(u(16.)).into_any_element(),
            }
        } else if !badge.image.is_empty() {
            let (w, h) = device_size(window, 24., 24.);
            let picture = ImageSource::parse(&badge.image).and_then(|source| {
                let request = ImageRequest::new(source, SCOPE).with_target(w, h, Fit::Contain);
                pictures.get(request, window, cx).and_then(Result::ok)
            });
            match picture {
                Some(image) => img(image)
                    .size(u(24.))
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                None => div().size(u(24.)).into_any_element(),
            }
        } else {
            crate::icons::icon(&badge.icon, 16., accent)
        };
        let label = badge.label.clone();
        row = row.child(hover(("profile-badge", index), Timing::fast(), move |t| {
            div()
                .id(("profile-badge-chip", index))
                .size(u(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(u(6.))
                .bg(white(0.07 * t))
                .when(!label.is_empty(), |el| el.tip(label))
                .child(content)
        }));
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glow_breathes_between_the_0_4_9_keyframes() {
        assert_eq!(glow_of(None), (3., 0.55));
        assert_eq!(glow_of(Some(0)), (2., 0.35));
        let (blur, alpha) = glow_of(Some(LEVELS));
        assert!((blur - 6.).abs() < 1e-6 && (alpha - 0.95).abs() < 1e-6);
    }

    #[test]
    fn the_glow_spreads_around_the_picture_in_its_colour() {
        let png = asset_bytes("developer").expect("значок разработчика");
        let (raster, pad) = glow_raster(png, 24., [40, 140, 255], None, 1.).expect("растр");
        let pad = pad as u32;
        assert_eq!(raster.width(), 24 + 2 * pad);
        // В поле вокруг картинки — только свечение, и оно синее.
        let (mut alpha, mut red, mut blue) = (0u32, 0u32, 0u32);
        for (index, p) in raster.as_slice().iter().enumerate() {
            let (x, y) = (index as u32 % raster.width(), index as u32 / raster.width());
            let inside = (pad..pad + 24).contains(&x) && (pad..pad + 24).contains(&y);
            if !inside {
                alpha += u32::from(p.a);
                red += u32::from(p.r);
                blue += u32::from(p.b);
            }
        }
        assert!(
            alpha > 0 && blue > red,
            "свечение в поле: a {alpha}, r {red}, b {blue}"
        );
        for asset in ["bughunter", "booster", "anniversary-1", "anniversary-10"] {
            assert!(asset_bytes(asset).is_some(), "{asset}");
        }
    }
}
