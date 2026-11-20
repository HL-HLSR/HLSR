//! Герой «Игр» (`HeroPanel.vue` 0.4.9): «HALF-LIFE» общий у всех игр и при смене игры стоит на
//! месте; окончание названия, подзаголовок и значения статистики ([`super::stats`]) перетекают
//! на месте (`l-value`, старое и новое в одной ячейке). Строка подзаголовка есть всегда, иначе у
//! игр без него статистика подпрыгивала бы вверх.
//!
//! Данные — `currentGame`, `currentStats`, `currentDelivery` оболочки ([`ShellContext`]), тексты —
//! [`hero_panel::view`]. Заголовок с мягкой тенью `text-shadow` — [`Title`] (тень размыта на ЦП,
//! решение владельца 2); им же пишет заголовок инструмента ([`super::tool_panel`]).

use super::stats::game_stats;
use crate::image::Raster;
use crate::motion::{VALUE, swap};
use crate::scale::{u, zoom, zpx};
use crate::shell::alpha;
use crate::shell::root::ShellContext;
use crate::text::{RUBIK, spaced};
use crate::theme::Theme;
use gpui::{
    App, Context, Div, ElementId, Font, FontWeight, Hsla, IntoElement, ObjectFit, Render,
    RenderOnce, SharedString, Window, div, font, img, prelude::*, white,
};
use hlsr_core::js::{self, object::JsObject};
use hlsr_core::language::Language;
use hlsr_ui::Rgba8Image;
use hlsr_ui::controllers::game_availability::column_games;
use hlsr_ui::controllers::game_demo::{GameHeader, header};
use hlsr_ui::controllers::hero_panel;
use hlsr_ui::render::canvas_text::{self, Align, Baseline, Paint, Place};
use hlsr_ui::render::shadow_blur::gaussian_blur_alpha;
use hlsr_ui::services::{Services, StoreRead};
use serde_json::Value;

/// `text-shadow` заголовка: `0 5px 14px rgba(0, 0, 0, .75)` (dy, размытие, прозрачность).
pub(crate) const TITLE_SHADOW: (f32, f32, f32) = (5., 14., 0.75);
/// `text-shadow` подзаголовка: `0 4px 10px rgba(0, 0, 0, .7)`.
pub(crate) const SUBTITLE_SHADOW: (f32, f32, f32) = (4., 10., 0.7);

/// Размытая тень строки: картинка (чёрный, прозрачность — маска) и её левый верхний угол от
/// начала базовой линии, физические px.
pub(crate) struct ShadowArt {
    pub image: Rgba8Image,
    pub x: f32,
    pub y: f32,
}

/// Тень строки Rubik 700 кеглем `size` с разрядкой `spacing` (физические px): маска глифов,
/// размытая гауссом σ = `blur` / 2 (как `text-shadow` у Skia), прозрачность × `alpha`.
pub(crate) fn shadow_art(
    text: &str,
    size: f32,
    spacing: f32,
    blur: f32,
    alpha: f32,
) -> Option<ShadowArt> {
    let font = canvas_text::Font {
        weight: 700,
        size,
        families: vec![RUBIK.into()],
    };
    let line = canvas_text::layout_spaced(text, &font, spacing);
    let place = Place {
        x: 0.,
        y: 0.,
        align: Align::Left,
        baseline: Baseline::Alphabetic,
        max_width: None,
    };
    let cover = canvas_text::coverage(&line, place, Paint::Fill)?;
    let sigma = blur / 2.;
    let pad = (sigma * 3.).ceil() as u32;
    let (width, height) = (cover.width + 2 * pad, cover.height + 2 * pad);
    let mut mask = vec![0u8; (width * height) as usize];
    for row in 0..cover.height {
        let from = (row * cover.width) as usize;
        let to = ((row + pad) * width + pad) as usize;
        mask[to..to + cover.width as usize]
            .copy_from_slice(&cover.data[from..from + cover.width as usize]);
    }
    gaussian_blur_alpha(&mut mask, width, height, sigma);
    let mut image = Rgba8Image::new(width, height);
    for (pixel, cover) in image.make_mut_slice().iter_mut().zip(&mask) {
        pixel.a = (f32::from(*cover) * alpha).round() as u8;
    }
    Some(ShadowArt {
        image,
        x: (cover.x - pad as i32) as f32,
        y: (cover.y - pad as i32) as f32,
    })
}

/// Строка заголовка (Rubik 700, разрядка в em) с `text-shadow: 0 dy blur rgba(0,0,0,a)`: тень
/// — размытая на ЦП маска строки под [`spaced`].
#[derive(IntoElement)]
pub(crate) struct Title {
    pub id: ElementId,
    pub text: SharedString,
    pub size: f32,
    pub line_height: f32,
    pub spacing_em: f32,
    pub color: Hsla,
    /// (dy, размытие, прозрачность), CSS px.
    pub shadow: (f32, f32, f32),
}

/// Растр тени [`Title`]: пересчитывается при смене текста, кегля в физических px или тени.
#[derive(Default)]
struct ShadowState {
    key: Option<(SharedString, [u32; 4])>,
    /// x, y от начала базовой линии, ширина, высота — физические px.
    rect: Option<[f32; 4]>,
    raster: Raster,
}

impl RenderOnce for Title {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let zoom = zoom(window);
        let scale = zoom * window.scale_factor();
        let (dy, blur, shade) = self.shadow;
        let size = self.size * scale;
        let spacing = self.spacing_em * size;
        let key = (
            self.text.clone(),
            [size, spacing, blur * scale, shade].map(f32::to_bits),
        );
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| {
            cx.on_release(|state: &mut ShadowState, cx| state.raster.release(cx))
                .detach();
            ShadowState::default()
        });
        let text = self.text.clone();
        let rect = state.update(cx, |state, _| {
            if state.key.as_ref() != Some(&key) {
                let art = shadow_art(&text, size, spacing, blur * scale, shade);
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
        // Линия букв — как у [`spaced`]: половинный интерлиньяж + подъём шрифта GPUI.
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&Font {
            weight: FontWeight::BOLD,
            ..font(RUBIK)
        });
        let font_px = zpx(zoom, self.size);
        let ascent = f32::from(text_system.ascent(font_id, font_px));
        let descent = f32::from(text_system.descent(font_id, font_px)).abs();
        let baseline = ((self.line_height * zoom - ascent - descent) / 2. + ascent) / zoom;
        div()
            .relative()
            .flex_none()
            .children(image.zip(rect).map(|(image, [x, y, width, height])| {
                img(image)
                    .absolute()
                    .left(u(x / scale))
                    .top(u(baseline + dy + y / scale))
                    .w(u(width / scale))
                    .h(u(height / scale))
                    .object_fit(ObjectFit::Fill)
            }))
            .child(spaced(
                self.text,
                self.size,
                self.line_height,
                FontWeight::BOLD,
                self.spacing_em,
                self.color,
            ))
    }
}

/// Цвет заголовков в «Бумаге» — тёплым белым палитры (`l-paper-*`), в прочих темах — белый.
pub(crate) fn paper(theme: &Theme, key: &str, fallback: Hsla) -> Hsla {
    theme.try_color(key).unwrap_or(fallback)
}

/// Подзаголовок (`.hero__subtitle`, `.tool-panel__subtitle`): 0,4 em заголовка, 600 (у Rubik —
/// 700), разрядка .3em, отступ 8 сверху, тень мягче.
pub(crate) fn subtitle(id: ElementId, text: &str, theme: &Theme) -> Div {
    if text.is_empty() {
        return div();
    }
    div().mt(u(8.)).child(Title {
        id,
        text: text.to_uppercase().into(),
        size: theme.len("l-hero-title") * 0.4,
        line_height: theme.len("l-hero-subtitle-lh"),
        spacing_em: 0.3,
        color: paper(theme, "l-paper-subtitle", alpha(white(), 0.85)),
        shadow: SUBTITLE_SHADOW,
    })
}

/// Заголовок (`.hero__title`, `.tool-panel__title`): кегль и строка по ступени сетки, разрядка
/// .14em, заглавные.
pub(crate) fn title(id: ElementId, text: &str, theme: &Theme) -> Title {
    Title {
        id,
        text: text.to_uppercase().into(),
        size: theme.len("l-hero-title"),
        line_height: theme.len("l-hero-title-lh"),
        spacing_em: 0.14,
        color: paper(theme, "l-paper-title", white()),
        shadow: TITLE_SHADOW,
    }
}

pub struct HeroPanel {
    /// `library.config`: часы `inGameTime` для запасных данных (`games` 0.4.9 читает `library`
    /// при расчёте, не следит за ним).
    config: Value,
}

impl HeroPanel {
    pub fn new(services: &Services, _cx: &mut Context<HeroPanel>) -> HeroPanel {
        let config = match services.store_read("library") {
            StoreRead::Found(library) => library.get("config").cloned().unwrap_or_default(),
            StoreRead::Missing | StoreRead::Invalid => Value::Null,
        };
        HeroPanel { config }
    }

    /// `currentGame`: выбранная игра колонки, иначе первая.
    fn game(&self, id: &str) -> Option<GameHeader> {
        let games = column_games();
        let entry = games
            .iter()
            .find(|entry| entry.game.id == id)
            .or(games.first())?;
        let id = entry.game.id;
        header(
            id,
            self.config.get(id).and_then(|game| game.get("inGameTime")),
        )
    }
}

/// Игра, статистика и статус доставки выбранной игры (`|| null`).
fn selection(cx: &App) -> (Language, String, Option<Value>, Option<Value>) {
    let Some(context) = ShellContext::get(cx) else {
        return (Language::En, String::new(), None, None);
    };
    let language = context.shell.language();
    context.flows.with_state(|state| {
        let id = state.selected_game_id.clone();
        let current = |map: &JsObject<Value>| map.get(&id).filter(|v| js::truthy(v)).cloned();
        let (stats, delivery) = (current(&state.stats), current(&state.delivery));
        (language, id, stats, delivery)
    })
}

impl Render for HeroPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let (language, id, stats, delivery) = selection(cx);
        let hero = div().size_full().relative().overflow_hidden();
        let Some(game) = self.game(&id) else {
            return hero;
        };
        let view = hero_panel::view_local(language, &game, stats.as_ref(), delivery.as_ref());
        let theme = cx.global::<Theme>().clone();
        let sub_lh = theme.len("l-hero-subtitle-lh");
        let rest_theme = theme.clone();
        let sub_theme = theme.clone();
        hero.child(
            div()
                .absolute()
                .left(theme.rem("l-hero-pad"))
                .top(theme.rem("l-hero-top"))
                .bottom_0()
                .flex()
                .flex_col()
                .gap(theme.rem("l-hero-gap"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .child(title("hero-title".into(), &view.title.base, &theme))
                                .child(swap(
                                    "hero-title-rest",
                                    view.title.rest.clone(),
                                    VALUE,
                                    move |rest: &String| {
                                        let id =
                                            ElementId::Name(format!("hero-rest:{rest}").into());
                                        title(id, rest, &rest_theme)
                                    },
                                )),
                        )
                        .child(
                            swap(
                                "hero-subtitle",
                                view.subtitle.clone(),
                                VALUE,
                                move |text: &String| {
                                    let id = ElementId::Name(format!("hero-sub:{text}").into());
                                    subtitle(id, text, &sub_theme)
                                },
                            )
                            .min_h(u(sub_lh + 8.)),
                        ),
                )
                .child(game_stats(&view.rows, &theme)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_shadow_is_a_blurred_glyph_mask_around_the_baseline() {
        let art = shadow_art("HALF-LIFE", 68., 0.14 * 68., 14., 0.75).unwrap();
        // Поле 3σ = 21 px со всех сторон (левый край «H» — около 5 px), заглавные над линией букв.
        assert!(art.x < 0. && art.x > -21. && art.y < -21. - 40.);
        assert!(art.image.width() > 400 && art.image.height() > 42 + 40);
        let alpha: Vec<u8> = art.image.as_slice().iter().map(|p| p.a).collect();
        // Не больше 0,75 непрозрачности, края поля почти прозрачны, середина — тень.
        assert!(alpha.iter().all(|&a| a <= 192));
        assert!(alpha[0] < 4);
        assert!(alpha.iter().any(|&a| a > 100));
        assert!(shadow_art("", 68., 0., 14., 0.75).is_none());
    }
}
