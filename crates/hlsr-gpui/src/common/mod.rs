//! Общее для экранов оболочки (S8): ник с оформлением и значком подписчика, рамка аватара.
//! Берут шапка (поиск, аккаунт), «Главная» (лента, рекорды, игроки), профиль, спонсоры.

pub mod avatar_frame;
pub mod nick;

pub use avatar_frame::{AvatarFrame, avatar_frame};
pub use nick::{NickName, nick_name};

use crate::image::Raster;
use gpui::{App, Hsla, RenderImage, Window};
use hlsr_ui::{Rgba, Rgba8Image};
use std::sync::Arc;
use std::time::Instant;

/// Заглушка сборки без онлайна: текст `key` («… доступна в официальном лаунчере HLSR») и под ним
/// ссылка на страницу выпусков (`open-external-link` с ключом `releases` из `links.rs`).
/// Колонка без выравнивания: место ставит его само.
pub fn official_stub(
    id: &'static str,
    key: &str,
    size: f32,
    window: &mut gpui::Window,
    cx: &mut App,
) -> gpui::Div {
    use crate::text::TextExt;
    use gpui::{AppContext as _, ParentElement, Styled, div};
    let shell = crate::shell::root::ShellContext::get(cx);
    let language = shell.map_or(hlsr_core::language::Language::Ru, |s| s.shell.language());
    let services = shell.map(|shell| shell.services.clone());
    let dim = cx.global::<crate::theme::Theme>().color("l-text-dim");
    let link = crate::kit::link::text_link(
        id,
        hlsr_ui::tr(language, "#L_OFFICIAL_RELEASES", &[]),
        move |_, cx| {
            if let Some(services) = &services {
                let reply =
                    services.invoke("open-external-link", vec![serde_json::json!("releases")]);
                cx.background_spawn(async move {
                    let _ = reply.await;
                })
                .detach();
            }
        },
        window,
        cx,
    );
    div()
        .flex()
        .flex_col()
        .gap(crate::scale::u(size * 0.3))
        .text_px(size, size * 1.35)
        .text_color(dim)
        .child(hlsr_ui::tr(language, key, &[]))
        .child(link)
}

/// Растр, который рисует логика (`hlsr_ui::render::profile_art`): новый кадр уходит в атлас,
/// только когда логика отдала другую картинку (неподвижная берётся из её кеша).
#[derive(Default)]
struct Art {
    raster: Raster,
    shown: Option<Arc<Rgba8Image>>,
}

impl Art {
    /// Кадр `image`; `None` — оформления больше нет: прежний кадр уходит из атласа.
    fn image(
        &mut self,
        image: Option<&Arc<Rgba8Image>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<RenderImage>> {
        match image {
            Some(image)
                if !self
                    .shown
                    .as_ref()
                    .is_some_and(|old| Arc::ptr_eq(old, image)) =>
            {
                self.raster.set(image);
                self.shown = Some(image.clone());
            }
            Some(_) => {}
            None => {
                self.raster.clear();
                self.shown = None;
            }
        }
        self.raster.image(window, cx)
    }

    fn release(&mut self, cx: &mut App) {
        self.raster.release(cx);
        self.shown = None;
    }
}

/// Цвет GPUI → 8 бит на канал без предумножения (вход растров логики).
fn rgba8(color: Hsla) -> Rgba {
    let c = gpui::Rgba::from(color);
    let channel = |v: f32| (v * 255.).round().clamp(0., 255.) as u8;
    Rgba::new(channel(c.r), channel(c.g), channel(c.b), channel(c.a))
}

/// Мс с `since` до `now`.
fn elapsed(now: Instant, since: Instant) -> f64 {
    now.saturating_duration_since(since).as_secs_f64() * 1000.
}

/// Кадры просит бесконечная анимация реестра с правилом `still: none` (как у стилей ника):
/// её элемент движения ставит их на паузу под [`crate::motion::AnimGate`] (окно спрятано,
/// сверху окно-слой, вне видимой области). Свой кадр ник и рамка считают по своим часам.
const TICK: &str = "nick-badge-glow";
