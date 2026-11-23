//! Окно «Спонсорство» (`SponsorsModal.vue` 0.4.9). Открывает оболочка (`sponsorsOpen`):
//! сердце в шапке (S4) и «Поддержать проект» в настройках профиля ([`open_sponsors`]). Список
//! запрашивается один раз при открытии (`public-sponsors-get`): `sponsor-open-link` открывает
//! запись по номеру в последнем списке main, и окно не должно разойтись с ним, пока открыто.
//!
//! Пока нет ответа — только загрузка. Старый сервер (`version: 1`) — окно 0.4.7: розовый баннер
//! и «царь горы» в сетке из шести дорожек. Новое окно — прокрутка целиком: шапка с розовым
//! отсветом, «О проекте», кнопки «Подписка» и «Донат» (Telegram-бот через `support-open`),
//! разделы «Активные подписчики», «Донаты» (трон, пьедестал, по три в ряд), «Поддерживали
//! проект»; внизу вне прокрутки — подсказка про «анонимно» в Tribute. Логика —
//! [`hlsr_ui::controllers::sponsors`].
//!
//! Окно в бегущей розовой рамке (`neon.scss`, как у профиля подписчика) и розовом свечении;
//! z-index 50, Esc и подложка закрывают, фокус при открытии — на крестике (первая кнопка).

use crate::common::{avatar_frame, nick_name};
use crate::icons::{fa_glyph, icon};
use crate::image::{Pictures, device_size};
use crate::kit::button::{child_id, focus_handle, focus_visible, outline, white};
use crate::layer::{Modal, TipExt, listen};
use crate::motion::{Push, Timing, hover, mix, press};
use crate::paint::{ring, shadow};
use crate::profile::neon::{Palette, neon};
use crate::scale::{u, viewport, zoom, zpx};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::shell::root::ShellContext;
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, BoxShadow, ContentMask, Context, Corners, ElementId, FocusHandle,
    FontWeight, Global, Hsla, IntoElement, ObjectFit, Pixels, Radians, Render, Rgba, Role,
    SharedString, Transformation, Window, canvas, div, img, linear_color_stop, linear_gradient,
    point, prelude::*, px, relative, rgb, svg,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::bridge::Tr;
use hlsr_ui::controllers::sponsors::{Action, Entry, Section, SponsorCard, SponsorsState, View};
use hlsr_ui::images::{Fit, ImageRequest, ImageSource};
use hlsr_ui::services::Services;
use std::rc::Rc;

/// `--sponsor`: розовый GitHub Sponsors, цвет сердечка в шапке.
const SPONSOR: u32 = 0xdb61a2;
/// `--gold`: корона «царя горы».
const GOLD: u32 = 0xffd166;
/// Область картинок окна (аватары).
const SCOPE: &str = "sponsors";

/// `fas fa-crown` (Font Awesome Free 7, CC BY 4.0) контуром: поворот `rotate(-12deg)` есть
/// только у SVG. Квадрат 1em × 1,125em, как глиф в строке высотой 1em.
const CROWN: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 576 512'><path d='\
M313 87C322 80 328 69 328 56C328 34 310 16 288 16C266 16 248 34 248 56C248 69 254 80 263 87\
L195 195C185 211 163 214 148 203L89 159C93 152 96 144 96 136C96 114 78 96 56 96C34 96 16 114 16 136\
C16 158 34 176 55 176L88 394C93 425 119 448 151 448L425 448C457 448 484 425 488 394L521 176\
C543 176 560 158 560 136C560 114 542 96 520 96C498 96 480 114 480 136C480 144 483 152 487 159\
L428 203C413 215 392 211 382 195Z'/></svg>";

const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a,
    }
}

/// `neon(sponsors-neon, #ff4fd8, #ff9de6, rgba(255, 79, 216, .2))`.
const NEON: Palette = Palette {
    a: rgba(0xff, 0x4f, 0xd8, 1.),
    b: rgba(0xff, 0x9d, 0xe6, 1.),
    dim: rgba(255, 79, 216, 0.2),
};

fn sponsor(alpha: f32) -> Hsla {
    Hsla::from(rgb(SPONSOR)).opacity(alpha)
}

fn gold(alpha: f32) -> Hsla {
    Hsla::from(rgb(GOLD)).opacity(alpha)
}

/// `rgba(255, 79, 216, a)`: свечение окна.
fn pink(alpha: f32) -> Hsla {
    rgba(255, 79, 216, alpha).into()
}

/// `radial-gradient(rx ry at cx cy, color a0, transparent p1)` во весь блок (доли — от его
/// размера): быстрая запечённая 64×64 текстура с масштабированием на GPU (без `resvg`).
fn glow(
    center: (f32, f32),
    radius: (f32, f32),
    (a0, p1): (f32, f32),
    (width, height): (f32, f32),
    color: Hsla,
) -> gpui::Div {
    div().absolute().top_0().left_0().size_full().child(
        crate::paint::Radial {
            center,
            radius,
            stops: vec![(0., a0), (p1, 0.)],
        }
        .paint(width, height, color)
        .size_full(),
    )
}

/// Наружная `box-shadow` полупрозрачного блока: в CSS тень только снаружи рамки, GPUI рисует её
/// и под блоком — сквозь фон .03–.16 она видна. Слой позади блока (родитель — `relative`, без
/// `overflow_hidden`) рисует тени только вне скруглённого блока ([`outside`]).
fn outer_shadow(shadows: Vec<BoxShadow>, radius: f32, zoom: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let corners = Corners::all(zpx(zoom, radius)).clamp_radii_for_quad_size(bounds.size);
            let masks = outside(
                bounds,
                corners.top_left,
                zpx(zoom, 200.),
                window.scale_factor(),
            );
            for mask in masks {
                window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
                    window.paint_drop_shadows(bounds, corners, &shadows)
                });
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Место вне блока `bounds` со скруглением `radius` до `far` от него — прямоугольниками (маска
/// GPUI только прямоугольная): полосы сверху и снизу, по бокам — по строке пикселей устройства
/// (`scale`) до дуги на середине строки. Маску GPUI расширяет до пикселей устройства
/// (`cover_bounds`: ближний край вниз, дальний вверх), поэтому соседние строки с краем в одном
/// пикселе сливаются в одну полосу — вид тот же, полос (и `paint_drop_shadows`) почти вдвое
/// меньше (значок 48 px на 1,25: 72 вместо 122). Границы строк — по сетке пикселей: строка не рисуется дважды и не пропадает.
pub(super) fn outside(
    bounds: Bounds<Pixels>,
    radius: Pixels,
    far: Pixels,
    scale: f32,
) -> Vec<Bounds<Pixels>> {
    let (l, t, r, b) = (bounds.left(), bounds.top(), bounds.right(), bounds.bottom());
    let snap = |y: Pixels| Pixels::from((f32::from(y) * scale).round() / scale);
    let (top, bottom) = (snap(t), snap(b));
    let mut out = vec![
        Bounds::from_corners(point(l - far, t - far), point(r + far, top)),
        Bounds::from_corners(point(l - far, bottom), point(r + far, b + far)),
    ];
    let rr = f32::from(radius);
    // (начало, конец, пиксель края после `cover_bounds`, край первой строки) по каждой стороне.
    type Side = Vec<(Pixels, Pixels, f32, Pixels)>;
    let merge =
        |side: &mut Side, y0: Pixels, y1: Pixels, pixel: f32, x: Pixels| match side.last_mut() {
            Some((_, end, last, _)) if *last == pixel => *end = y1,
            _ => side.push((y0, y1, pixel, x)),
        };
    let (mut left, mut right): (Side, Side) = Default::default();
    let mut row = 0;
    loop {
        let y0 = top + Pixels::from(row as f32 / scale);
        if y0 >= bottom {
            break;
        }
        let y1 = (top + Pixels::from((row + 1) as f32 / scale)).min(bottom);
        let mid = f32::from(y0 + y1) / 2.;
        let dy = (f32::from(t) + rr - mid)
            .max(mid - (f32::from(b) - rr))
            .max(0.);
        let inset = Pixels::from(rr - (rr * rr - dy * dy).max(0.).sqrt());
        let (lx, rx) = (l + inset, r - inset);
        merge(&mut left, y0, y1, (f32::from(lx) * scale).ceil(), lx);
        merge(&mut right, y0, y1, (f32::from(rx) * scale).floor(), rx);
        row += 1;
    }
    for (y0, y1, _, x) in left {
        out.push(Bounds::from_corners(point(l - far, y0), point(x, y1)));
    }
    for (y0, y1, _, x) in right {
        out.push(Bounds::from_corners(point(x, y0), point(r + far, y1)));
    }
    out
}

/// `filter: brightness(k)`.
fn brightness(color: Hsla, k: f32) -> Hsla {
    let c = Rgba::from(color);
    Rgba {
        r: (c.r * k).min(1.),
        g: (c.g * k).min(1.),
        b: (c.b * k).min(1.),
        a: c.a,
    }
    .into()
}

/// `sponsorsOpen` LauncherShell 0.4.9.
#[derive(Default)]
struct SponsorsOpen(bool);

impl Global for SponsorsOpen {}

/// Открыть «Спонсорство» (`onOpenSponsors`): профиль лежит выше и закрыл бы окно, поэтому он
/// закрывается.
pub fn open_sponsors(cx: &mut App) {
    // Спонсорство — только в официальной сборке.
    if !crate::shell::root::official(cx) {
        return;
    }
    if let Some(shell) = ShellContext::get(cx) {
        shell.flows.clone().close_profile();
    }
    cx.set_global(SponsorsOpen(true));
}

/// Сборка перестала быть официальной: «Спонсорство» закрывается.
pub fn close_sponsors(cx: &mut App) {
    if sponsors_open(cx) {
        cx.set_global(SponsorsOpen(false));
    }
}

/// Окно открыто (`sponsorsOpen`): сердце в шапке (S4) — в состоянии `.open`.
pub fn sponsors_open(cx: &App) -> bool {
    cx.try_global::<SponsorsOpen>().is_some_and(|open| open.0)
}

pub struct SponsorsModal {
    services: Services,
    modal: Modal,
    close: FocusHandle,
    state: SponsorsState,
    /// Растёт при каждом открытии: ответ прежнего окна не применяется.
    generation: u64,
    scroll: Scroll,
    pictures: Pictures,
    language: Language,
    tr: Tr,
    /// Разделы и карточки ответа: собираются один раз в `answer`, не на каждом кадре наведения.
    sections: Rc<[Section]>,
    cards: Rc<[SponsorCard]>,
}

impl SponsorsModal {
    pub fn new(services: &Services, cx: &mut Context<SponsorsModal>) -> SponsorsModal {
        let close = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("sponsors", cx);
        modal.focus_initial(close.clone());
        let pictures = cx.try_global::<Pictures>().cloned().unwrap_or_else(|| {
            tracing::warn!("[sponsors] нет Pictures: аватары не загрузятся");
            Pictures::new(hlsr_ui::images::ImageLoader::new(
                Rc::new(hlsr_ui::images::NoRemote),
                &*crate::env::GpuiEnv::new(cx),
            ))
        });
        cx.on_release({
            let pictures = pictures.clone();
            move |_, cx| pictures.release_scope(SCOPE, cx)
        })
        .detach();
        cx.observe_global::<SponsorsOpen>(|_, cx| cx.notify())
            .detach();
        let language = crate::shell::root::language_of(services);
        SponsorsModal {
            services: services.clone(),
            modal,
            close,
            state: SponsorsState::new(),
            generation: 0,
            scroll: Scroll::new(),
            pictures,
            language,
            tr: Rc::new(move |key, args| hlsr_ui::tr(language, key, args)),
            sections: Rc::new([]),
            cards: Rc::new([]),
        }
    }

    fn t(&self, key: &str) -> String {
        (self.tr)(key, &[])
    }

    /// `v-if="sponsorsOpen"`: новое окно — `data()` и `created()` (запрос списка).
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if sponsors_open(cx) {
            if !self.modal.is_open() {
                self.generation += 1;
                self.state = SponsorsState::new();
                self.scroll = Scroll::new();
                self.run(SponsorsState::load(), cx);
                self.modal.open(window, cx);
            }
        } else if self.modal.is_open() {
            self.generation += 1;
            self.modal.close(window, cx);
        }
        cx.notify();
    }

    /// `$emit("close")`.
    fn close(&mut self, cx: &mut Context<Self>) {
        cx.set_global(SponsorsOpen(false));
    }

    fn run(&mut self, action: Action, cx: &mut Context<Self>) {
        match action {
            // `sponsorsOpen = false; openRunnerProfile(steamId)`.
            Action::Profile(steam_id) => {
                cx.set_global(SponsorsOpen(false));
                if let Some(shell) = ShellContext::get(cx) {
                    shell.flows.clone().open_runner_profile(&steam_id);
                }
            }
            Action::Invoke(channel, args) => {
                let reply = self.services.invoke(channel, args);
                if channel != "public-sponsors-get" {
                    // `.catch(() => null)`: ответ окну не нужен.
                    cx.spawn(async move |_, _| {
                        reply.await.ok();
                    })
                    .detach();
                    return;
                }
                let generation = self.generation;
                cx.spawn(async move |this, cx| {
                    let reply = reply.await.ok();
                    this.update(cx, |this, cx| {
                        if this.generation == generation {
                            this.state.answer(reply);
                            this.sections = this.state.sections(this.language, &this.tr).into();
                            this.cards = this.state.legacy_cards(this.language, &this.tr).into();
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    fn activate(&mut self, section: &'static str, index: usize, cx: &mut Context<Self>) {
        if let Some(action) = self.state.activate(section, index) {
            self.run(action, cx);
        }
    }

    fn support(&mut self, key: &'static str, cx: &mut Context<Self>) {
        if let Some(action) = self.state.support(key) {
            self.run(action, cx);
        }
    }

    /// `.sponsors-modal__avatar` в `AvatarFrame` (рамка 2 px): буква под картинкой — видна, пока
    /// аватар грузится или если он битый. У трона — золото и кольцо 2 px.
    #[allow(clippy::too_many_arguments)]
    fn avatar(
        &self,
        id: ElementId,
        url: &str,
        letter: &str,
        frame_css: &str,
        size: f32,
        king: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let photo = ImageSource::parse(url).and_then(|source| {
            let (w, h) = device_size(window, size, size);
            let request = ImageRequest::new(source, SCOPE).with_target(w, h, Fit::Cover);
            self.pictures.get(request, window, cx).and_then(Result::ok)
        });
        let zoom = zoom(window);
        let tone = if king { gold } else { sponsor };
        let face = div()
            .relative()
            .flex_none()
            .size(u(size))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(tone(if king { 0.16 } else { 0.18 }))
            .text_px(size * 0.44, size * 0.44)
            .font_weight(FontWeight::BOLD)
            .text_color(tone(1.))
            .child(letter.to_owned())
            .children(photo.map(|photo| {
                img(photo)
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .rounded_full()
                    .object_fit(ObjectFit::Cover)
            }))
            // `box-shadow: 0 0 0 2px gold 70%` — кольцо снаружи круга.
            .when(king, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(u(-2.))
                        .left(u(-2.))
                        .size(u(size + 4.))
                        .rounded_full()
                        .shadow(vec![ring(zoom, 2., gold(0.7))]),
                )
            });
        avatar_frame(id, size, face)
            .css(frame_css.to_owned())
            .width(2.)
            .into_any_element()
    }

    /// Место аватара с короной над ним (`.sponsors-modal__seat`, корона трона: 24 px, −27,
    /// `rotate(-12deg)`).
    fn seat(avatar: AnyElement, size: f32, crown: bool) -> impl IntoElement {
        let width = 24. * 1.125;
        div()
            .relative()
            .flex_none()
            .child(avatar)
            .when(crown, |el| {
                el.child(
                    svg()
                        .data(CROWN.as_bytes())
                        .absolute()
                        .top(u(-27.))
                        .left(u((size - width) / 2.))
                        .w(u(width))
                        .h(u(24.))
                        .text_color(gold(1.))
                        .with_transformation(Transformation::rotate(Radians(-12f32.to_radians()))),
                )
            })
    }

    /// Номер места (`__rank`, `__place`): серебро и бронза у пьедестала, серый у остальных.
    fn rank(index: usize, label: String, theme: &Theme, zoom: f32) -> gpui::Stateful<gpui::Div> {
        let place: Hsla = match index {
            1 => rgb(0xcfd8e3).into(),
            2 => rgb(0xe0a370).into(),
            _ => theme.color("l-text-faint"),
        };
        div()
            .id(("sponsors-rank", index))
            .flex_none()
            .self_start()
            .min_w(u(22.))
            .px(u(6.))
            .rounded_full()
            .bg(place.opacity(0.14))
            .shadow(vec![ring(zoom, 1., place.opacity(0.4))])
            .text_px(11., 20.)
            .font_weight(FontWeight::BOLD)
            .text_center()
            .tnum()
            .text_color(place)
            .aria_label(label)
            .child((index + 1).to_string())
    }

    /// Ник с подсветкой (`__nick`, `__name`): поля 8/14 под свечение съедены отступом, кегль
    /// `size`, высота строки 1,3; скрытый — курсивом цветом `--l-text-faint`.
    #[allow(clippy::too_many_arguments)]
    fn nick(
        id: ElementId,
        name: &str,
        style: &str,
        css: &str,
        subscriber: bool,
        anonymous: bool,
        size: f32,
        theme: &Theme,
    ) -> gpui::Div {
        div()
            .min_w_0()
            .max_w_full()
            .mx(u(-14.))
            .my(u(-8.))
            .px(u(14.))
            .py(u(8.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_px(size, size * 1.3)
            .map(|el| {
                if anonymous {
                    el.italic()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.color("l-text-faint"))
                } else {
                    el.font_weight(FontWeight::BOLD)
                        .text_color(theme.color("l-text"))
                }
            })
            .child(
                nick_name(id, name.to_owned())
                    .look(style.to_owned(), css.to_owned())
                    .subscriber(subscriber, None),
            )
    }

    /// Кнопка-крестик в углу окна.
    fn close_button(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>();
        let zoom = zoom(window);
        let label = self.t("#L_CLOSE");
        let focus = self.close.clone();
        let ring_on = focus_visible(&self.close, window);
        let (dim, text, border, accent) = (
            theme.color("l-text-dim"),
            theme.color("l-text"),
            theme.color("l-border"),
            theme.color("l-accent"),
        );
        let radius = theme.len("l-radius-sm");
        let click = cx.listener(|this, _, _, cx| this.close(cx));
        hover("sponsors-close", Timing::fast(), move |t| {
            div()
                .id("sponsors-close-button")
                .button_text()
                .on_click(click)
                .role(Role::Button)
                .aria_label(label.clone())
                .tip(label)
                .track_focus(&focus)
                .absolute()
                .top(u(24.))
                .right(u(24.))
                .size(u(40.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(u(radius))
                .bg(mix(gpui::black().opacity(0.32), white(0.1), t))
                .shadow(vec![ring(zoom, 1., border)])
                .text_color(mix(dim, text, t))
                .cursor_pointer()
                .child(fa_glyph("fas fa-times", 15.))
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., accent))
                })
        })
        .into_any_element()
    }

    /// Загрузка: одно сообщение посреди окна, без шапки.
    fn loading(&self, theme: &Theme) -> AnyElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(u(12.))
            .text_px(14., 20.)
            .text_color(theme.color("l-text-dim"))
            .child(icon("fas fa-spinner fa-spin", 24., sponsor(1.)))
            .child(self.t("#L_SPONSORS_LOADING"))
            .into_any_element()
    }

    /// Пусто или сбой: сердце и текст в рамке.
    fn state_box(&self, theme: &Theme, zoom: f32) -> gpui::Div {
        let failed = self.state.failed();
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(u(12.))
            .min_h(u(160.))
            .p(u(24.))
            .rounded(theme.rem("l-radius"))
            .bg(white(0.02))
            .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
            .text_px(14., 20.)
            .text_center()
            .text_color(theme.color("l-text-dim"))
            .child(fa_glyph("far fa-heart", 24.).text_color(mix(
                theme.color("l-text-faint"),
                sponsor(1.),
                0.7,
            )))
            .child(self.t(if failed {
                "#L_SPONSORS_FAILED"
            } else {
                "#L_S_SPONSORS_EMPTY"
            }))
    }

    // ── Новое окно ──────────────────────────────────────────────────────────

    fn window_view(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let keys = self.state.support_keys();
        let sections = self.sections.clone();
        let head = self.head(&keys, width, &theme, window, cx);
        let mut content = div().px(u(48.)).pb(u(12.)).flex().flex_col().child(head);
        if sections.is_empty() {
            content = content.child(self.state_box(&theme, zoom));
        } else {
            for section in sections.iter() {
                let part = self.part(section, &theme, window, cx);
                content = content.child(part);
            }
        }
        let hint = (!keys.is_empty()).then(|| {
            div()
                .flex_none()
                .pt(u(8.))
                .px(u(48.))
                .pb(u(12.))
                .text_px(12., 17.)
                .text_center()
                .text_color(theme.color("l-text-faint"))
                .child(self.t("#L_SPONSORS_HINT"))
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            // Цель стенда `sponsors-body` — прокрутка окна (колесо в сценах).
            .child(crate::snap::target(
                "sponsors-body",
                ScrollArea::new("sponsors-scroll", &self.scroll)
                    .bar(Bar::Thin)
                    .flex_1()
                    .min_h_0()
                    .child(content),
            ))
            .children(hint)
            .into_any_element()
    }

    /// Шапка: розовый отсвет сверху по центру, сердце, заголовок, «О проекте», кнопки оплаты.
    fn head(
        &self,
        keys: &[&'static str],
        width: f32,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = zoom(window);
        // radial-gradient(46% 240px at 50% 0%, sponsor 20%, transparent).
        let haze = div()
            .absolute()
            .top_0()
            .left_0()
            .w(u(width))
            .h(u(240.))
            .child(glow(
                (0.5, 0.),
                (0.46, 1.),
                (0.2, 1.),
                (width, 240.),
                sponsor(1.),
            ));
        let badge = div()
            .relative()
            .child(outer_shadow(
                vec![shadow(zoom, 0., 0., 32., 0., sponsor(0.24))],
                24.,
                zoom,
            ))
            .child(
                div()
                    .size(u(48.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(sponsor(0.16))
                    .shadow(vec![ring(zoom, 1., sponsor(0.4))])
                    .text_color(sponsor(1.))
                    .child(fa_glyph("fas fa-heart", 20.)),
            );
        let buttons = (!keys.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .justify_center()
                .gap(u(10.))
                .mt(u(24.))
                .children(keys.iter().map(|&key| self.cta(key, theme, window, cx)))
        });
        div()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .mx(u(-48.))
            .pt(u(40.))
            .px(u(112.))
            .pb(u(32.))
            .text_center()
            .child(haze)
            .child(badge)
            .child(
                div()
                    .mt(u(14.))
                    .text_px(26., 32.)
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.color("l-text"))
                    .child(self.t("#L_SPONSORS_TITLE")),
            )
            .child(
                div()
                    .max_w(u(620.))
                    .mt(u(10.))
                    .text_px(14., 22.)
                    .text_color(theme.color("l-text-dim"))
                    .child(self.t("#L_SPONSORS_ABOUT")),
            )
            .children(buttons)
            .into_any_element()
    }

    /// `.sponsors-modal__cta`: подписка — розовая, донат — с золотой короной; нажатие
    /// `scale(.97)` `--l-fast`.
    fn cta(
        &self,
        key: &'static str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = zoom(window);
        let id: ElementId = ("sponsors-cta", usize::from(key == "donate")).into();
        let focus = focus_handle(&id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let subscribe = key == "subscribe";
        let (glyph, label) = if subscribe {
            ("fas fa-star", self.t("#L_SPONSORS_SUBSCRIBE"))
        } else {
            ("fas fa-crown", self.t("#L_SPONSORS_DONATE"))
        };
        let (strong, text, accent) = (
            theme.color("l-border-strong"),
            theme.color("l-text"),
            theme.color("l-accent"),
        );
        let radius = theme.len("l-radius-sm");
        let click = cx.listener(move |this, _, _, cx| this.support(key, cx));
        let target = if subscribe {
            "sponsors-subscribe"
        } else {
            "sponsors-donate"
        };
        let press_id = child_id(&id, "press");
        let button = hover(id.clone(), Timing::fast(), move |t| {
            let (bg, border, color) = if subscribe {
                (
                    mix(sponsor(1.), brightness(sponsor(1.), 1.08), t),
                    gpui::transparent_black(),
                    gpui::white(),
                )
            } else {
                (mix(white(0.05), white(0.1), t), strong, text)
            };
            let el = div()
                .id(id)
                .role(Role::Button)
                .button_text()
                .relative()
                .flex()
                .items_center()
                .gap(u(9.))
                .h(u(40.))
                .px(u(18.))
                .border_1()
                .border_color(border)
                .rounded(u(radius))
                .bg(bg)
                .when(subscribe, |el| {
                    el.shadow(vec![shadow(zoom, 0., 6., 22., 0., sponsor(0.32))])
                })
                .text_px(13.5, 16.)
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .text_color(color)
                .track_focus(&focus)
                .cursor_pointer()
                .on_click(click)
                .child(fa_glyph(glyph, 13.).when(!subscribe, |el| el.text_color(gold(1.))))
                .child(label)
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., accent))
                });
            press(press_id, Push::fast(0.97), el)
        });
        crate::snap::target(target, button)
    }

    /// Раздел: подпись с иконкой по центру, под ней записи; сверху — тонкий разделитель,
    /// гаснущий к краям.
    fn part(
        &mut self,
        section: &Section,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (glyph, label, tone) = match section.key {
            "active" => ("fas fa-star", "#L_SPONSORS_ACTIVE", sponsor(1.)),
            "donors" => ("fas fa-crown", "#L_SPONSORS_DONORS", gold(1.)),
            _ => (
                "fas fa-heart",
                "#L_SPONSORS_PAST",
                mix(theme.color("l-text-faint"), sponsor(1.), 0.55),
            ),
        };
        let border = theme.color("l-modal-border");
        let clear = border.opacity(0.);
        let line = div()
            .absolute()
            .top_0()
            .left(relative(0.06))
            .right(relative(0.06))
            .h(px(1.))
            .flex()
            .child(div().h_full().w(relative(0.22)).bg(linear_gradient(
                90.,
                linear_color_stop(clear, 0.),
                linear_color_stop(border, 1.),
            )))
            .child(div().h_full().flex_1().bg(border))
            .child(div().h_full().w(relative(0.22)).bg(linear_gradient(
                90.,
                linear_color_stop(border, 0.),
                linear_color_stop(clear, 1.),
            )));
        let title = div()
            .flex()
            .items_center()
            .gap(u(8.))
            .mb(u(20.))
            .child(fa_glyph(glyph, 12.).text_color(tone))
            .child(spaced(
                self.t(label).to_uppercase(),
                11.5,
                16.,
                FontWeight::BOLD,
                0.14,
                mix(theme.color("l-text-dim"), tone, 0.45),
            ));
        let body = match section.key {
            "active" => self.people(&section.entries, theme, window, cx),
            "donors" => self.donors(&section.entries, theme, window, cx),
            _ => self.names(&section.entries, theme, window, cx),
        };
        div()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .pt(u(28.))
            .pb(u(32.))
            .text_center()
            .child(line)
            .child(title)
            .child(body)
            .into_any_element()
    }

    /// Подписчики: аватар и ник, без плашки — подсветка ника и есть украшение.
    fn people(
        &mut self,
        entries: &[Entry],
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = zoom(window);
        let accent = theme.color("l-accent");
        let mut list = div()
            .flex()
            .flex_wrap()
            .justify_center()
            .gap_x(u(8.))
            .gap_y(u(6.));
        for entry in entries {
            let index = entry.index;
            let id: ElementId = ("sponsors-person", index).into();
            let avatar = self.avatar(
                ("sponsors-person-frame", index).into(),
                &entry.avatar,
                &entry.initial,
                &entry.frame_css,
                34.,
                false,
                window,
                cx,
            );
            let nick = Self::nick(
                ("sponsors-person-nick", index).into(),
                &entry.name,
                &entry.nick_style,
                &entry.nick_css,
                entry.subscriber,
                entry.anonymous,
                15.,
                theme,
            );
            let link = entry.link;
            let title = entry.title.clone();
            let focus = link.then(|| focus_handle(&id, window, cx));
            let ring_on = focus.as_ref().is_some_and(|f| focus_visible(f, window));
            let click = cx.listener(move |this, _, _, cx| this.activate("active", index, cx));
            let person = hover(id.clone(), Timing::fast(), move |t| {
                let t = if link { t } else { 0. };
                div()
                    .id(id)
                    .relative()
                    .flex()
                    .items_center()
                    .gap(u(10.))
                    .max_w(u(280.))
                    .py(u(6.))
                    .pl(u(6.))
                    .pr(u(16.))
                    .rounded_full()
                    .bg(sponsor(0.1 * t))
                    .child(avatar)
                    .child(nick)
                    .when(!title.is_empty(), |el| el.tip(title))
                    .when_some(focus, |el, focus| {
                        el.role(Role::Button)
                            .button_text()
                            .track_focus(&focus)
                            .cursor_pointer()
                            .on_click(click)
                    })
                    .when(ring_on, |el| el.child(outline(zoom, 999., 2., 2., accent)))
            });
            list = list.child(crate::snap::target(
                SharedString::from(format!("sponsors-person-{index}")),
                person,
            ));
        }
        list.into_any_element()
    }

    /// Донаты, «царь горы»: трон на всю ширину, пьедестал (2-е и 3-е) — два в ряд, дальше по три
    /// в ряд; ряды по центру. Пьедестал всегда в своём ряду: между ним и остальными — пустая
    /// строка (`::after` с `order: 2`), она добавляет ещё один зазор 10 px.
    fn donors(
        &mut self,
        entries: &[Entry],
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = || div().flex().justify_center().gap(u(10.));
        let mut lines = div().flex().flex_col().gap(u(10.)).w_full().max_w(u(800.));
        let mut cells: Vec<AnyElement> = entries
            .iter()
            .map(|entry| self.donor(entry, theme, window, cx))
            .collect();
        let rest = cells.split_off(cells.len().min(3));
        let mut top = cells.into_iter();
        if let Some(king) = top.next() {
            lines = lines.child(king);
        }
        let podium: Vec<AnyElement> = top.collect();
        if !podium.is_empty() {
            lines = lines.child(row().children(podium));
        }
        lines = lines.child(div());
        let mut rest = rest.into_iter().peekable();
        while rest.peek().is_some() {
            lines = lines.child(row().children(rest.by_ref().take(3)));
        }
        lines.into_any_element()
    }

    fn donor(
        &mut self,
        entry: &Entry,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = zoom(window);
        let index = entry.index;
        let king = entry.tier == "king";
        let podium = entry.tier == "podium";
        let (avatar_size, nick_size) = match entry.tier {
            "king" => (76., 26.),
            "podium" => (42., 16.),
            _ => (34., 14.),
        };
        let avatar = self.avatar(
            ("sponsors-donor-frame", index).into(),
            &entry.avatar,
            &entry.initial,
            &entry.frame_css,
            avatar_size,
            king,
            window,
            cx,
        );
        let seat = Self::seat(avatar, avatar_size, king);
        let nick = Self::nick(
            ("sponsors-donor-nick", index).into(),
            &entry.name,
            &entry.nick_style,
            &entry.nick_css,
            entry.subscriber,
            entry.anonymous,
            nick_size,
            theme,
        );
        let (dim, faint) = (theme.color("l-text-dim"), theme.color("l-text-faint"));
        let sum = div()
            .id(("sponsors-donor-sum", index))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .font_weight(FontWeight::SEMIBOLD)
            .tnum()
            .map(|el| {
                if king {
                    el.mt(u(4.)).text_px(16., 22.).text_color(gold(1.))
                } else {
                    el.text_px(12., 16.).text_color(dim)
                }
            })
            .tip(entry.amount_full.clone())
            .child(entry.amount.clone());
        let words = (!entry.note.is_empty()).then(|| {
            div()
                .id(("sponsors-donor-words", index))
                .map(|el| {
                    if king {
                        el.clamp(3)
                            .max_w(u(440.))
                            .mt(u(8.))
                            .text_px(13., 19.)
                            .text_color(dim)
                    } else {
                        el.clamp(2).mt(u(2.)).text_px(12., 16.).text_color(faint)
                    }
                })
                .tip(entry.note.clone())
                .child(entry.note.clone())
        });
        let who = div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .when(king, |el| el.items_center().w_full())
            .child(nick)
            .child(sum)
            .children(words);
        let rank = (index > 0).then(|| Self::rank(index, entry.place_label.clone(), theme, zoom));
        let id: ElementId = ("sponsors-donor", index).into();
        let link = entry.link;
        let title = entry.title.clone();
        let focus = link.then(|| focus_handle(&id, window, cx));
        let ring_on = focus.as_ref().is_some_and(|f| focus_visible(f, window));
        let (border, accent) = (theme.color("l-border"), theme.color("l-accent"));
        let radius = theme.len("l-radius-sm");
        let click = cx.listener(move |this, _, _, cx| this.activate("donors", index, cx));
        let plate = hover(id.clone(), Timing::fast(), move |t| {
            let t = if link { t } else { 0. };
            let face = div()
                .relative()
                .flex()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .rounded(u(radius))
                .text_left();
            let face = if king {
                // Трон: столбиком по центру, золотой отсвет сверху, корона над аватаром.
                let wash = glow((0.5, 0.), (0.7, 1.2), (0.22, 0.7), (520., 240.), gold(1.))
                    .opacity((0.16 + 0.06 * t) / 0.22);
                face.flex_col()
                    .items_center()
                    .gap(u(14.))
                    .pt(u(34.))
                    .px(u(28.))
                    .pb(u(22.))
                    .bg(mix(white(0.03), gold(1.), 0.04 + 0.02 * t))
                    .shadow(vec![ring(zoom, 1., gold(0.45 + 0.25 * t))])
                    .text_center()
                    .child(wash)
            } else {
                face.items_center()
                    .gap(u(if podium { 14. } else { 12. }))
                    .py(u(if podium { 12. } else { 10. }))
                    .px(u(if podium { 14. } else { 12. }))
                    .bg(mix(white(0.03), sponsor(0.08), t))
                    .shadow(vec![ring(zoom, 1., mix(border, sponsor(0.4), t))])
            };
            div()
                .id(id)
                .relative()
                .flex()
                .flex_col()
                .min_w_0()
                .map(|el| {
                    if king {
                        el.w(u(520.)).max_w_full().child(outer_shadow(
                            vec![shadow(zoom, 0., 8., 32., 0., gold(0.1 + 0.06 * t))],
                            radius,
                            zoom,
                        ))
                    } else {
                        el.flex_1()
                    }
                })
                .child(face.child(seat).child(who).children(rank))
                .when(!title.is_empty(), |el| el.tip(title))
                .when_some(focus, |el, focus| {
                    el.role(Role::Button)
                        .button_text()
                        .track_focus(&focus)
                        .cursor_pointer()
                        .on_click(click)
                })
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., accent))
                })
        });
        let plate =
            crate::snap::target(SharedString::from(format!("sponsors-donor-{index}")), plate);
        let cell = div().flex().flex_col().min_w_0();
        if king {
            cell.w_full()
                .items_center()
                .mb(u(8.))
                .child(plate)
                .child(
                    div()
                        .mt(u(10.))
                        .text_px(12., 16.)
                        .text_color(faint)
                        .child(entry.dethrone.clone()),
                )
                .into_any_element()
        } else {
            cell.w(u(if podium { 384. } else { 252. }))
                .child(plate)
                .into_any_element()
        }
    }

    /// Поддерживали раньше: обычные ники, без подсветки.
    fn names(
        &mut self,
        entries: &[Entry],
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = zoom(window);
        let (dim, text, faint, accent) = (
            theme.color("l-text-dim"),
            theme.color("l-text"),
            theme.color("l-text-faint"),
            theme.color("l-accent"),
        );
        let mut list = div()
            .flex()
            .flex_wrap()
            .justify_center()
            .gap_x(u(22.))
            .gap_y(u(6.))
            .max_w(u(760.));
        for entry in entries {
            let index = entry.index;
            let id: ElementId = ("sponsors-past", index).into();
            let link = entry.link;
            let anonymous = entry.anonymous;
            let title = if entry.title.is_empty() {
                entry.name.clone()
            } else {
                entry.title.clone()
            };
            let name = entry.name.clone();
            let focus = link.then(|| focus_handle(&id, window, cx));
            let ring_on = focus.as_ref().is_some_and(|f| focus_visible(f, window));
            let click = cx.listener(move |this, _, _, cx| this.activate("past", index, cx));
            list = list.child(hover(id.clone(), Timing::fast(), move |t| {
                let t = if link { t } else { 0. };
                div()
                    .id(id)
                    .relative()
                    .min_w_0()
                    .max_w(u(220.))
                    .py(u(2.))
                    .text_px(13.5, 20.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .map(|el| {
                        if anonymous {
                            el.italic().text_color(faint)
                        } else {
                            el.text_color(mix(dim, text, t))
                        }
                    })
                    .tip(title)
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(name),
                    )
                    .when_some(focus, |el, focus| {
                        el.role(Role::Button)
                            .button_text()
                            .track_focus(&focus)
                            .cursor_pointer()
                            .on_click(click)
                    })
                    .when(ring_on, |el| el.child(outline(zoom, 4., 2., 2., accent)))
            }));
        }
        list.into_any_element()
    }

    // ── Окно 0.4.7 (старый сервер) ──────────────────────────────────────────

    fn legacy_view(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let radius = theme.len("l-radius");
        let bg = theme.color("l-modal-bg");
        // Баннер: radial-gradient(80% 160% at 0 0, sponsor 26%, transparent 70%) поверх
        // linear-gradient(180deg, sponsor 10% в фоне окна → фон окна); снизу линия рамки.
        let wash = glow(
            (0., 0.),
            (0.8, 1.6),
            (0.26, 0.7),
            (width, 136.),
            sponsor(1.),
        );
        let hero = div()
            .flex_none()
            .relative()
            .flex()
            .items_center()
            .gap(u(24.))
            .pt(u(40.))
            .pr(u(88.))
            .pb(u(32.))
            .pl(u(32.))
            .rounded_t(u(radius))
            .bg(linear_gradient(
                180.,
                linear_color_stop(mix(bg, sponsor(1.), 0.1), 0.),
                linear_color_stop(bg, 1.),
            ))
            .child(wash)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(theme.color("l-modal-border")),
            )
            .child(
                div()
                    .flex_none()
                    .size(u(64.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(sponsor(0.16))
                    .shadow(vec![ring(zoom, 1., sponsor(0.4))])
                    .text_color(sponsor(1.))
                    .child(fa_glyph("fas fa-heart", 26.)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_px(24., 32.)
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.color("l-text"))
                            .child(self.t("#L_S_SPONSORS")),
                    )
                    .child(
                        div()
                            .mt(u(8.))
                            .text_px(14., 20.)
                            .text_color(theme.color("l-text-dim"))
                            .child(self.t("#L_SPONSORS_LEAD")),
                    ),
            );
        let cards = self.cards.clone();
        let heading = spaced(
            self.t("#L_SPONSORS_LIST").to_uppercase(),
            11.,
            16.,
            FontWeight::BOLD,
            0.12,
            theme.color("l-text-faint"),
        );
        let body = if cards.is_empty() {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .pt(u(28.))
                .px(u(32.))
                .pb(u(32.))
                .child(div().mb(u(12.)).child(heading))
                .child(self.state_box(&theme, zoom).flex_1())
                .into_any_element()
        } else {
            let hill = self.hill(&cards, &theme, window, cx);
            ScrollArea::new("sponsors-legacy-scroll", &self.scroll)
                .bar(Bar::Thin)
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .pt(u(28.))
                        .px(u(32.))
                        .pb(u(32.))
                        .child(div().mb(u(12.)).child(heading))
                        .child(hill),
                )
                .into_any_element()
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(hero)
            .child(body)
            .into_any_element()
    }

    /// Царь горы: шесть дорожек — трон все шесть, пьедестал по три (один — целиком), остальные
    /// по две; незаполненные клетки последнего ряда пустые.
    fn hill(
        &mut self,
        cards: &[SponsorCard],
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut rows: Vec<Vec<AnyElement>> = Vec::new();
        let mut spans: Vec<i32> = Vec::new();
        for (index, card) in cards.iter().enumerate() {
            let cell = self.legacy_card(index, card, theme, window, cx);
            let count = rows.len();
            match rows.last_mut() {
                Some(row) if card.row as usize + 1 == count => row.push(cell),
                _ => {
                    rows.push(vec![cell]);
                    spans.push(card.colspan);
                }
            }
        }
        let mut hill = div().flex().flex_col().gap(u(8.));
        for (row, span) in rows.into_iter().zip(spans) {
            let slots = (6 / span.max(1)) as usize;
            let missing = slots.saturating_sub(row.len());
            hill = hill.child(
                div()
                    .flex()
                    .gap(u(8.))
                    .children(
                        row.into_iter()
                            .map(|cell| div().flex_1().min_w_0().flex().flex_col().child(cell)),
                    )
                    .children((0..missing).map(|_| div().flex_1())),
            );
        }
        hill.into_any_element()
    }

    fn legacy_card(
        &mut self,
        index: usize,
        card: &SponsorCard,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = zoom(window);
        let king = card.tier == "king";
        let podium = card.tier == "podium";
        let (avatar_size, nick_size) = match card.tier {
            "king" => (76., 28.),
            "podium" => (48., 19.),
            _ => (36., 15.),
        };
        let avatar = self.avatar(
            ("sponsors-card-frame", index).into(),
            &card.avatar,
            &card.initial,
            &card.frame_css,
            avatar_size,
            king,
            window,
            cx,
        );
        let seat = Self::seat(avatar, avatar_size, king);
        let (dim, faint) = (theme.color("l-text-dim"), theme.color("l-text-faint"));
        let king_title = king.then(|| {
            spaced(
                card.king_title.to_uppercase(),
                11.,
                16.,
                FontWeight::BOLD,
                0.12,
                gold(1.),
            )
        });
        let nick = Self::nick(
            ("sponsors-card-nick", index).into(),
            &card.name,
            &card.nick_style,
            &card.nick_css,
            card.subscriber,
            false,
            nick_size,
            theme,
        );
        let amount = div()
            .id(("sponsors-card-amount", index))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .font_weight(FontWeight::SEMIBOLD)
            .tnum()
            .map(|el| {
                if king {
                    el.mt(u(2.)).text_px(15., 20.).text_color(gold(1.))
                } else {
                    el.text_px(12., 16.).text_color(dim)
                }
            })
            .tip(card.amount_full.clone())
            .child(card.amount.clone());
        let note = (!card.note.is_empty()).then(|| {
            div()
                .id(("sponsors-card-note", index))
                .map(|el| {
                    if king {
                        el.clamp(3).mt(u(6.)).text_px(13., 18.).text_color(dim)
                    } else {
                        el.clamp(2).mt(u(2.)).text_px(12., 16.).text_color(faint)
                    }
                })
                .tip(card.note.clone())
                .child(card.note.clone())
        });
        let text = div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .children(king_title)
            .child(nick)
            .child(amount)
            .children(note);
        let rank = (index > 0).then(|| Self::rank(index, card.place_label.clone(), theme, zoom));
        let id: ElementId = ("sponsors-card", index).into();
        let link = !card.steam_id.is_empty();
        let steam_id = card.steam_id.clone();
        let title = if link {
            self.t("#L_SPONSORS_OPEN_PROFILE")
        } else {
            String::new()
        };
        let focus = link.then(|| focus_handle(&id, window, cx));
        let ring_on = focus.as_ref().is_some_and(|f| focus_visible(f, window));
        let (border, accent) = (theme.color("l-border"), theme.color("l-accent"));
        let radius = theme.len("l-radius-sm");
        let click =
            cx.listener(move |this, _, _, cx| this.run(Action::Profile(steam_id.clone()), cx));
        let plate = hover(id.clone(), Timing::fast(), move |t| {
            let t = if link { t } else { 0. };
            let face = div()
                .relative()
                .flex()
                .flex_1()
                .items_center()
                .min_w_0()
                .overflow_hidden()
                .rounded(u(radius));
            let face = if king {
                // Трон: золотой отсвет из левого верхнего угла и место под корону.
                let wash = glow((0., 0.), (0.7, 1.4), (0.22, 0.7), (600., 140.), gold(1.))
                    .opacity((0.16 + 0.06 * t) / 0.22);
                face.gap(u(20.))
                    .pt(u(28.))
                    .px(u(24.))
                    .pb(u(20.))
                    .bg(mix(white(0.03), gold(1.), 0.04 + 0.02 * t))
                    .shadow(vec![ring(zoom, 1., gold(0.45 + 0.25 * t))])
                    .child(wash)
            } else {
                face.gap(u(if podium { 14. } else { 12. }))
                    .py(u(if podium { 14. } else { 12. }))
                    .px(u(16.))
                    .bg(mix(white(0.03), sponsor(0.08), t))
                    .shadow(vec![ring(zoom, 1., mix(border, sponsor(0.4), t))])
            };
            div()
                .id(id)
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .when(king, |el| {
                    el.child(outer_shadow(
                        vec![shadow(zoom, 0., 8., 32., 0., gold(0.1 + 0.06 * t))],
                        radius,
                        zoom,
                    ))
                })
                .child(face.child(seat).child(text).children(rank))
                .when(!title.is_empty(), |el| el.tip(title))
                .when_some(focus, |el, focus| {
                    el.role(Role::Button)
                        .button_text()
                        .track_focus(&focus)
                        .cursor_pointer()
                        .on_click(click)
                })
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., accent))
                })
        });
        let plate =
            crate::snap::target(SharedString::from(format!("sponsors-card-{index}")), plate);
        if king {
            div()
                .mb(u(8.))
                .flex()
                .flex_col()
                .child(plate)
                .child(
                    div()
                        .mt(u(8.))
                        .text_px(12., 16.)
                        .text_center()
                        .text_color(faint)
                        .child(card.dethrone.clone()),
                )
                .into_any_element()
        } else {
            plate
        }
    }
}

impl Render for SponsorsModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        if sponsors_open(cx) != self.modal.is_open() {
            // Стопка окон и фокус — не посреди отрисовки.
            cx.defer_in(window, Self::sync);
        }
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (vw, vh) = viewport(window);
        let (width, height) = ((vw - 80.).min(980.), (vh - 80.).min(700.));
        let view = self.state.view();
        let content = if !self.modal.is_open() && view == View::Loading {
            div().into_any_element()
        } else {
            match view {
                View::Loading => self.loading(&theme),
                View::Legacy => self.legacy_view(width, window, cx),
                View::Window => self.window_view(width, window, cx),
            }
        };
        let label = self.t(if view == View::Legacy {
            "#L_S_SPONSORS"
        } else {
            "#L_SPONSORS_TITLE"
        });
        let radius = theme.len("l-radius");
        let mut glow = shadow(zoom, 0., 0., 16., 0., pink(0.2));
        glow.inset = true;
        let shadows = vec![
            ring(zoom, 1., theme.color("l-modal-border")),
            glow.clone(),
            shadow(zoom, 0., 0., 28., 0., pink(0.28)),
            shadow(zoom, 0., 24., 72., 0., gpui::black().opacity(0.6)),
        ];
        let close = self.close_button(window, cx);
        let body = div()
            .id("sponsors-window")
            .role(Role::Dialog)
            .aria_label(label)
            .relative()
            .flex()
            .flex_col()
            .w(u(width))
            .h(u(height))
            .overflow_hidden()
            .bg(theme.color("l-modal-bg"))
            .rounded(u(radius))
            .shadow(shadows);
        // Liquid Glass: розовое свечение «Спонсорства» поверх `--lg-rim` и `--lg-shadow`.
        let extra = vec![glow, shadow(zoom, 0., 0., 28., 0., pink(0.28))];
        let body = crate::liquid::glass_with(
            body,
            crate::liquid::Kind::Window,
            u(radius),
            gpui::Pixels::ZERO,
            extra,
            &theme,
            zoom,
        )
        .child(content)
        .child(close)
        .child(neon("sponsors-neon", NEON, radius, zoom));
        let on_close = listen(cx, |this: &mut SponsorsModal, _, cx| this.close(cx));
        self.modal
            .render(window, cx, on_close, move || body)
            .into_any_element()
    }
}
