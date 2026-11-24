//! Ник (`NickName.vue` 0.4.9). Оформление от админки или заготовка подсветки — растр
//! `hlsr_ui::render::nick` по кадрам времени (свечения размыты на ЦП, как в 0.4.9); без
//! оформления — обычный текст. Подписчику слева значок (`.nick-badge`: 1.05em, поле .3em,
//! низ на .2em ниже линии букв, строку не поднимает), его свечение «дышит» 3,6 с, подсказка —
//! «Поддерживает HLSR N мес. — с ДД.ММ.ГГГГ» (без `supporterSince` — «Подписчик HLSR»).
//!
//! Шрифт, кегль, вес, цвет и высота строки — от родителя, как наследование CSS. Кадры просит
//! только то, что движется; при `.no-anim`/reduced-motion ник и значок стоят.

use super::{Art, TICK, elapsed, rgba8};
use crate::kit::button::child_id;
use crate::layer::TipExt;
use crate::motion::{AnimGate, keyframes};
use crate::scale::{u, zoom};
use crate::shell::root::ShellContext;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ElementId, ObjectFit, RenderImage, SharedString, Window, canvas, div, img,
};
use hlsr_core::language::Language;
use hlsr_ui::render::nick::{self, BADGE_DROP_EM, BADGE_EM, BADGE_GAP_EM};
use hlsr_ui::render::profile_art::{self, ProfileArt};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(IntoElement)]
pub struct NickName {
    id: ElementId,
    text: SharedString,
    style: SharedString,
    css: SharedString,
    subscriber: bool,
    since: Option<i64>,
}

/// Ник `text`; `id` — место на экране (часы анимации и растры живут, пока оно в кадре).
pub fn nick_name(id: impl Into<ElementId>, text: impl Into<SharedString>) -> NickName {
    NickName {
        id: id.into(),
        text: text.into(),
        style: SharedString::default(),
        css: SharedString::default(),
        subscriber: false,
        since: None,
    }
}

impl NickName {
    /// Оформление карточки профиля `{style, css}` (`nick` 0.4.9).
    pub fn look(mut self, style: impl Into<SharedString>, css: impl Into<SharedString>) -> Self {
        self.style = style.into();
        self.css = css.into();
        self
    }

    /// Подписка активна сейчас; `since` — `supporterSince` (unix-секунды, известен в профиле).
    pub fn subscriber(mut self, on: bool, since: Option<i64>) -> Self {
        self.subscriber = on;
        self.since = since;
        self
    }
}

/// Кэш текстур значка подписчика (иконка + два крайних свечения): один на размер и масштаб,
/// общий для всех карточек на экране; дыхание идёт плавно на GPU без перерастра.
#[derive(Clone)]
struct BadgeGpu {
    icon: Arc<RenderImage>,
    glow_min: Arc<RenderImage>,
    glow_max: Arc<RenderImage>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

fn badge_gpu(size: f32, scale: f32) -> Option<BadgeGpu> {
    thread_local! {
        static CACHE: RefCell<HashMap<(u32, u32), BadgeGpu>> = RefCell::new(HashMap::new());
    }
    let key = ((size * 100.).round() as u32, (scale * 1000.).round() as u32);
    if let Some(hit) = CACHE.with_borrow(|c| c.get(&key).cloned()) {
        return Some(hit);
    }
    let parts = profile_art::badge_parts_art(size, scale)?;
    let gpu = BadgeGpu {
        icon: crate::image::render_image(&parts.icon),
        glow_min: crate::image::render_image(&parts.glow_min),
        glow_max: crate::image::render_image(&parts.glow_max),
        x: parts.x,
        y: parts.y,
        width: parts.width,
        height: parts.height,
    };
    CACHE.with_borrow_mut(|c| {
        if c.len() >= 32 {
            c.clear();
        }
        c.insert(key, gpu.clone());
    });
    Some(gpu)
}

/// Часы и растры одного ника.
struct State {
    start: Instant,
    nick: Art,
    badge: Art,
    /// Видимость в прошлом кадре: скрытые за прокруткой карточки не тратят ЦП и атлас.
    visible: Rc<Cell<bool>>,
    last_key: String,
    last_art: Option<ProfileArt>,
}

impl RenderOnce for NickName {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let text_style = window.text_style();
        let rem = window.rem_size();
        let zoom = zoom(window);
        let scale = zoom * window.scale_factor();
        let font_px = text_style.font_size.to_pixels(rem);
        let size = f32::from(font_px) / zoom;
        let line_px = text_style.line_height_in_pixels(rem);
        let line = f32::from(line_px) / zoom;
        let still = AnimGate::still(cx);
        let now = cx.background_executor().now();
        let step_ms = profile_art::step_ms_for_hz(crate::motion::refresh_hz(window, cx));
        let state = window.use_keyed_state(child_id(&self.id, "nick"), cx, |_, cx| {
            cx.on_release(|state: &mut State, cx| {
                state.nick.release(cx);
                state.badge.release(cx);
            })
            .detach();
            State {
                start: now,
                nick: Art::default(),
                badge: Art::default(),
                visible: Rc::new(Cell::new(true)),
                last_key: String::new(),
                last_art: None,
            }
        });
        let (ms, visible, nick_key) = {
            let s = state.read(cx);
            let weight = text_style.font_weight.0.round() as u16;
            let color = rgba8(text_style.color);
            let key = format!(
                "{}|{}|{}|{size}|{weight}|{color:?}|{scale}|{still:?}",
                self.style, self.css, self.text
            );
            (elapsed(now, s.start), s.visible.clone(), key)
        };
        let moving_nick = profile_art::nick_moving(&self.style, &self.css, still);
        let cached_offscreen = state.read(cx);
        let art = if moving_nick
            && !visible.get()
            && cached_offscreen.last_key == nick_key
            && let Some(prev) = &cached_offscreen.last_art
        {
            prev.clone()
        } else {
            let art = profile_art::nick_art_paced(
                &self.style,
                &self.css,
                &self.text,
                size,
                text_style.font_weight.0.round() as u16,
                0.,
                rgba8(text_style.color),
                scale,
                ms,
                still,
                step_ms,
            );
            state.update(cx, |s, _| {
                s.last_key = nick_key;
                s.last_art = Some(art.clone());
            });
            art
        };
        let mut moving = art.styled && moving_nick;
        let shown = art.styled.then_some(&art.image);
        let image = state.update(cx, |state, cx| state.nick.image(shown, window, cx));
        let nick: AnyElement = if art.styled {
            // Строка ника: ширина — шаг текста, по высоте глифы в середине строки (половинный
            // интерлиньяж CSS), картинка с полем под свечение выходит за неё.
            let content_top = (line - art.content_height) / 2.;
            div()
                .relative()
                .flex_none()
                .w(u(art.advance))
                .h(u(line))
                .children(image.map(|image| {
                    img(image)
                        .absolute()
                        .left(u(art.x))
                        .top(u(content_top + art.y))
                        .w(u(art.width))
                        .h(u(art.height))
                        .object_fit(ObjectFit::Fill)
                        .opacity(art.opacity)
                }))
                .into_any_element()
        } else {
            div()
                .min_w_0()
                .overflow_hidden()
                .child(self.text.clone())
                .into_any_element()
        };

        let badge_mix = self
            .subscriber
            .then(|| nick::badge_mix(ms, still))
            .flatten();
        if !self.subscriber || badge_mix.is_some() {
            state.update(cx, |state, cx| state.badge.image(None, window, cx));
        }
        let badge = self.subscriber.then(|| {
            moving |= nick::badge_moving(still);
            // Линия букв, как у строки текста GPUI: половинный интерлиньяж + подъём (спуск в
            // метриках отрицательный, поэтому не `baseline_offset`).
            let text_system = window.text_system();
            let font_id = text_system.resolve_font(&text_style.font());
            let ascent = f32::from(text_system.ascent(font_id, font_px));
            let descent = f32::from(text_system.descent(font_id, font_px)).abs();
            let baseline = (f32::from(line_px) - ascent - descent) / 2. + ascent;
            let top = baseline / zoom + (BADGE_DROP_EM - BADGE_EM) * size;
            let since = self.since;
            let mut box_el = div()
                .relative()
                .flex_none()
                .w(u((BADGE_EM + BADGE_GAP_EM) * size))
                .h(u(line));
            if let Some(mix) = badge_mix {
                if let Some(parts) = badge_gpu(size, scale) {
                    let place = |image: Arc<RenderImage>, opacity: f32| {
                        img(image)
                            .absolute()
                            .left(u(parts.x))
                            .top(u(top + parts.y))
                            .w(u(parts.width))
                            .h(u(parts.height))
                            .object_fit(ObjectFit::Fill)
                            .opacity(opacity)
                    };
                    box_el = box_el
                        .child(place(parts.glow_min, (1. - mix).clamp(0., 1.)))
                        .child(place(parts.glow_max, mix.clamp(0., 1.)))
                        .child(place(parts.icon, 1.));
                }
            } else {
                let art = profile_art::badge_art(size, scale, ms, still);
                let image = state.update(cx, |state, cx| {
                    state.badge.image(Some(&art.image), window, cx)
                });
                box_el = box_el.children(image.map(|image| {
                    img(image)
                        .absolute()
                        .left(u(art.x))
                        .top(u(top + art.y))
                        .w(u(art.width))
                        .h(u(art.height))
                        .object_fit(ObjectFit::Fill)
                }));
            }
            box_el.child(
                // title у самой картинки 1.05em.
                div()
                    .id(child_id(&self.id, "badge"))
                    .absolute()
                    .left_0()
                    .top(u(top))
                    .size(u(BADGE_EM * size))
                    .tip_with(move |cx| {
                        let language = ShellContext::get(cx)
                            .map_or(Language::Ru, |shell| shell.shell.language());
                        nick::badge_title(language, since, now_ms()).into()
                    }),
            )
        });

        let probe = moving_nick.then(|| {
            let visible = visible.clone();
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let on = bounds.intersects(&window.content_mask().bounds);
                    visible.set(on);
                },
            )
            .absolute()
            .inset_0()
        });
        let line_el = div()
            .relative()
            .flex()
            .flex_row()
            .min_w_0()
            .children(probe)
            .children(badge)
            .child(nick);
        if moving {
            keyframes(child_id(&self.id, "tick"), TICK, move |_| line_el).into_any_element()
        } else {
            line_el.into_any_element()
        }
    }
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0., |since| since.as_secs_f64() * 1000.)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::avatar_frame;
    use crate::scale::zpx;
    use gpui::{Context, Render, TestAppContext, WindowHandle, size, white};
    use std::time::Duration;

    struct Nicks {
        style: &'static str,
        subscriber: bool,
        frame: &'static str,
    }

    impl Render for Nicks {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .text_size(u(15.))
                .line_height(u(19.))
                .child(
                    nick_name("nick", "Kanoyo")
                        .look(self.style, "")
                        .subscriber(self.subscriber, None),
                )
                .child(
                    avatar_frame("frame", 40., div().size(u(40.)))
                        .css(self.frame)
                        .hole(white()),
                )
        }
    }

    fn open(cx: &mut TestAppContext, view: Nicks) -> WindowHandle<Nicks> {
        let window = cx.open_window(size(zpx(1., 200.), zpx(1., 200.)), move |_, _| view);
        cx.run_until_parked();
        window
    }

    /// Кадр и число тех, кто его просил.
    fn frame(window: &WindowHandle<Nicks>, cx: &mut TestAppContext) -> usize {
        cx.executor().advance_clock(Duration::from_millis(16));
        let count = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        cx.run_until_parked();
        count
    }

    #[gpui::test]
    fn only_moving_styles_ask_frames_and_gate_pauses_them(cx: &mut TestAppContext) {
        let still = open(
            cx,
            Nicks {
                style: "",
                subscriber: false,
                frame: "background: #8af7e4",
            },
        );
        assert_eq!(frame(&still, cx), 0);

        let gold = open(
            cx,
            Nicks {
                style: "gold",
                subscriber: true,
                frame: "background: #8af7e4; animation: frame-pulse 2s ease-in-out infinite",
            },
        );
        assert!(frame(&gold, cx) > 0);
        assert!(frame(&gold, cx) > 0);
        // Окно спрятано: кадр, попрошенный до этого, приходит один раз, дальше — ни одного.
        cx.update(|cx| AnimGate::set_window_hidden(cx, true));
        cx.run_until_parked();
        frame(&gold, cx);
        assert_eq!(frame(&gold, cx), 0);
        cx.update(|cx| AnimGate::set_window_hidden(cx, false));
        cx.run_until_parked();
        assert!(frame(&gold, cx) > 0);
        // Без анимаций ник, значок и рамка стоят.
        cx.update(|cx| AnimGate::set_no_anim(cx, true));
        cx.run_until_parked();
        frame(&gold, cx);
        assert_eq!(frame(&gold, cx), 0);
    }
}
