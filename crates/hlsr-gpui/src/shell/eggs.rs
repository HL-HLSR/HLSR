//! Пасхалки оболочки (`LauncherShell.vue` 0.4.9): HESOYAM — плашка «Чит активирован» на 4 px ниже
//! кнопки «Закрыть» (выезжает из-за правого края, 2,6 с), звук `gta-ui.mp3` с громкостью .35 и
//! секретное достижение `hesoyam`; пони из «Назад в прошлое!» (тройной щелчок по логотипу) —
//! снизу посередине окна, шириной `min(46%, 640px)`, выезд .45 с, на экране 1,45 с. Обе поверх
//! всего (z 70) и прозрачны для мыши. Под окнами и в полях ввода чит не набирается — это решает
//! корень ([`super::root`]); логотип под окном закрыт подложкой.
//!
//! HLSR-CE: код «LEGACY» ([`LegacyCode`]) переключает интерфейс на HLSR 0.2.9 ([`crate::legacy`])
//! и обратно; до перезапуска.

use super::topbar::text_width;
use crate::games::hero::Title;
use crate::image::decode_live;
use crate::layer::Z_TOP;
use crate::motion::{Timing, registry, tween};
use crate::scale::{u, zoom};
use crate::shell::root::ShellContext;
use crate::sound::{Playback, play_wav};
use crate::text::spaced;
use crate::theme::Theme;
use gpui::{
    Context, FontWeight, IntoElement, Render, RenderImage, Task, Window, anchored, black, deferred,
    div, img, point, prelude::*, px, rgb,
};
use std::sync::Arc;
use std::time::Duration;

/// Чит из GTA: San Andreas (`CHEAT_SEQUENCE`), буквы — по физическим клавишам.
const CHEAT: [&str; 7] = ["KeyH", "KeyE", "KeyS", "KeyO", "KeyY", "KeyA", "KeyM"];
const CHEAT_MS: u64 = 2600;
const PONY_MS: u64 = 1450;
/// Уход пони (`egg-pony-leave`).
const PONY_LEAVE_MS: u64 = 400;
/// `gta-ui.mp3` 0.4.9 в WAV с запечённой громкостью `audio.volume = .35`
/// (`ffmpeg -i gta-ui.mp3 -af volume=0.35 -c:a pcm_s16le gta-ui.wav`): cpal играет WAV.
const CHEAT_SOUND: &[u8] = include_bytes!("../../../hlsr-ui/assets/launcher/gta-ui.wav");
const PONY: &[u8] = include_bytes!("../../../hlsr-ui/assets/launcher/pony.webp");

/// Последние нажатия для чита (`keySequence.slice(-7)` у `onGlobalKeydown`).
#[derive(Debug, Default)]
pub struct Cheat(Vec<String>);

impl Cheat {
    pub fn reset(&mut self) {
        self.0.clear();
    }

    /// Нажатие (`event.code`); чит набран — сброс и `true`.
    pub fn push(&mut self, code: &str) -> bool {
        self.0.push(code.to_owned());
        if self.0.len() > CHEAT.len() {
            self.0.remove(0);
        }
        if self.0.iter().map(String::as_str).eq(CHEAT) {
            self.0.clear();
            return true;
        }
        false
    }
}

/// HLSR-CE: код «LEGACY» — интерфейс HLSR 0.2.9 ([`crate::legacy`]); повтор возвращает новый.
const LEGACY: [&str; 6] = ["KeyL", "KeyE", "KeyG", "KeyA", "KeyC", "KeyY"];

/// Последние нажатия для кода «LEGACY».
#[derive(Debug, Default)]
pub struct LegacyCode(Vec<String>);

impl LegacyCode {
    pub fn reset(&mut self) {
        self.0.clear();
    }

    /// Нажатие (`event.code`); код набран — сброс и `true`.
    pub fn push(&mut self, code: &str) -> bool {
        self.0.push(code.to_owned());
        if self.0.len() > LEGACY.len() {
            self.0.remove(0);
        }
        if self.0.iter().map(String::as_str).eq(LEGACY) {
            self.0.clear();
            return true;
        }
        false
    }
}

/// Слой пасхалок в корне окна.
#[derive(Default)]
pub struct Eggs {
    cheat: bool,
    cheat_timer: Option<Task<()>>,
    sound: Option<Playback>,
    pony: bool,
    /// Пони в пути (выезд, показ): новый не начинается (`if (this.ponyShown) return`).
    pony_timer: Option<Task<()>>,
    /// Картинка уходит из памяти и атласа после ухода пони; новый показ её отменяет.
    pony_release: Option<Task<()>>,
    pony_image: Option<Arc<RenderImage>>,
}

impl Eggs {
    pub fn new() -> Eggs {
        Eggs::default()
    }

    /// `showCheat()`: плашка заново на 2,6 с, звук с начала, достижение (main выдаёт его один раз).
    pub fn show_cheat(&mut self, cx: &mut Context<Self>) {
        self.cheat = true;
        self.cheat_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(CHEAT_MS))
                .await;
            this.update(cx, |this, cx| {
                this.cheat = false;
                this.sound = None;
                cx.notify();
            })
            .ok();
        }));
        // Тесты и сцены стенда — без звука: он ушёл бы в динамики машины.
        if !cfg!(test) && std::env::var_os("HLSR_SNAP_SCENE").is_none() {
            self.sound = None;
            self.sound = play_wav(CHEAT_SOUND, || {});
        }
        if let Some(shell) = ShellContext::get(cx) {
            shell.shell.unlock_achievement("hesoyam");
        }
        cx.notify();
    }

    /// `showPony()`: пони выезжает, стоит и падает обратно. Картинка разбирается в фоне при
    /// показе и уходит после него.
    pub fn show_pony(&mut self, cx: &mut Context<Self>) {
        if self.pony_timer.is_some() {
            return;
        }
        self.pony_release = None;
        let ready = self.pony_image.clone();
        self.pony_timer = Some(cx.spawn(async move |this, cx| {
            let image = match ready {
                Some(image) => Some(image),
                None => {
                    cx.background_executor()
                        .spawn(async { decode_live(PONY) })
                        .await
                }
            };
            let shown = this
                .update(cx, |this, cx| {
                    this.pony_image = image.clone().or(this.pony_image.take());
                    this.pony = this.pony_image.is_some();
                    if !this.pony {
                        this.pony_timer = None;
                    }
                    cx.notify();
                    this.pony
                })
                .unwrap_or(false);
            if !shown {
                return;
            }
            cx.background_executor()
                .timer(Duration::from_millis(PONY_MS))
                .await;
            this.update(cx, |this, cx| {
                this.pony = false;
                this.pony_timer = None;
                this.pony_release = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(PONY_LEAVE_MS))
                        .await;
                    this.update(cx, |this, cx| {
                        this.pony_release = None;
                        if let Some(image) = this.pony_image.take() {
                            cx.drop_image(image, None);
                        }
                        cx.notify();
                    })
                    .ok();
                }));
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for Eggs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let tokens = &registry().tokens;
        let zoom = zoom(window);
        let view = window.viewport_size();
        let width = f32::from(view.width) / zoom;
        let label = ShellContext::get(cx).map_or_else(
            || "Cheat activated".to_owned(),
            |shell| shell.shell.tr("#L_CHEAT_ACTIVATED", &[]),
        );

        // .egg-cheat: верх (--l-topbar-h + --l-header-control) / 2 + 4, справа
        // --l-header-gap − --l-header-inset; выезд translateX(100% + 32px) — сдвигом `right`.
        let top = (theme.len("l-topbar-h") + theme.len("l-header-control")) / 2. + 4.;
        let right = theme.len("l-header-gap") - theme.len("l-header-inset");
        let plate = text_width(window, &label, 22., FontWeight::BOLD)
            + 0.02 * 22. * label.chars().count() as f32
            + 40.;
        let shown = self.cheat;
        let timing = Timing::new(
            tokens.l_dur,
            if shown {
                tokens.l_ease
            } else {
                tokens.l_ease_in
            },
        );
        let cheat = tween("egg-cheat", if shown { 1. } else { 0. }, timing, move |t| {
            let text = label.clone();
            div()
                .absolute()
                .top(u(top))
                .right(u(right - (1. - t) * (plate + 32.)))
                // Уход ещё и гаснет (`egg-cheat-leave-to`).
                .opacity(if shown { 1. } else { t })
                .px(u(20.))
                .py(u(10.))
                .bg(black().opacity(0.62))
                .whitespace_nowrap()
                // text-shadow: 0 0 2px #000, 1px 1px 0 #000 — мягкая размытием, жёсткая копией.
                .child(
                    div()
                        .relative()
                        .child(div().absolute().left(u(1.)).top(u(1.)).child(spaced(
                            text.clone(),
                            22.,
                            28.,
                            FontWeight::BOLD,
                            0.02,
                            black(),
                        )))
                        .child(Title {
                            id: "egg-cheat-text".into(),
                            text: text.into(),
                            size: 22.,
                            line_height: 28.,
                            spacing_em: 0.02,
                            color: rgb(0xd9dadb).into(),
                            shadow: (0., 2., 1.),
                        }),
                )
        });

        // .egg-pony: снизу посередине, translateY(100%) → 0 (.45 с --l-ease), назад .4 с --l-ease-in.
        let pony_w = (width * 0.46).min(640.);
        let image = self.pony_image.clone();
        let pony_h = image.as_ref().map_or(0., |image| {
            let size = image.size(0);
            pony_w * size.height.0 as f32 / size.width.0.max(1) as f32
        });
        let (target, timing) = if self.pony {
            (1., Timing::new(450., tokens.l_ease))
        } else {
            (0., Timing::new(PONY_LEAVE_MS as f64, tokens.l_ease_in))
        };
        let pony = tween("egg-pony", target, timing, move |t| {
            div()
                .absolute()
                .left(u((width - pony_w) / 2.))
                .bottom(u(-(1. - t) * pony_h))
                .w(u(pony_w))
                .h(u(pony_h))
                .children(image.map(|image| img(image).size_full()))
        });

        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .w(view.width)
                    .h(view.height)
                    .relative()
                    .overflow_hidden()
                    .child(cheat)
                    .child(pony),
            ),
        )
        .with_priority(Z_TOP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hesoyam_is_the_last_seven_codes_and_starts_over() {
        let mut cheat = Cheat::default();
        let typed = |cheat: &mut Cheat, codes: &[&str]| codes.iter().any(|code| cheat.push(code));
        assert!(typed(
            &mut cheat,
            &[
                "KeyX", "KeyH", "KeyE", "KeyS", "KeyO", "KeyY", "KeyA", "KeyM"
            ]
        ));
        // Сброс после набора: «M» снова — не чит.
        assert!(!cheat.push("KeyM"));
        assert!(!typed(
            &mut cheat,
            &["KeyH", "KeyE", "KeyS", "KeyO", "KeyY", "KeyA"]
        ));
        cheat.reset();
        assert!(!cheat.push("KeyM"));
        assert!(typed(&mut cheat, &CHEAT));
    }

    #[gpui::test]
    fn hesoyam_and_the_pony_show_up_and_go_away(cx: &mut gpui::TestAppContext) {
        use crate::app::LauncherRoot;
        use crate::shell::root::ShellRoot;
        use crate::snap::{bounds_of, pointer_move, press};
        use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
        use serde_json::json;

        cx.update(crate::init);
        let scene: Scene = serde_json::from_value(json!({
            "id": "t", "route": "/launcher", "platform": "win32",
            "window": {"width": 1600, "height": 900},
            "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
            "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
            "store": {"settings": {"config": {"language": null}}},
            "invoke": {"window-command": {"$else": {"ok": true}}}
        }))
        .unwrap();
        let variant = Variant {
            theme: "hlsr".into(),
            lang: "ru".into(),
            scale: 1.0,
            zoom: None,
            rail: None,
        };
        let fx = Arc::new(FixtureServices::new(&scene, &variant).unwrap());
        let services: hlsr_ui::services::Services = fx.clone();
        let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
        cx.run_until_parked();
        let eggs = cx.update(|_, cx| ShellRoot::get(cx).unwrap().read(cx).eggs.clone());
        let unlocked = || {
            fx.log()
                .into_iter()
                .filter(|entry| entry["channel"] == "runner-secret-unlock")
                .map(|entry| entry["args"].clone())
                .collect::<Vec<_>>()
        };

        // Фокус у вкладки шапки: клавиши поднимаются к корню (как у S1).
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let tab = bounds_of("topbar-tab-games").unwrap().center();
        cx.update(|window, cx| {
            pointer_move(window, cx, tab);
            press(window, cx, tab, gpui::MouseButton::Left, 1, true);
            press(window, cx, tab, gpui::MouseButton::Left, 1, false);
        });
        cx.run_until_parked();
        // HESOYAM в главном окне: плашка, достижение; через 2,6 с плашка уходит.
        cx.simulate_keystrokes("h e s o y a m");
        cx.run_until_parked();
        assert!(cx.update(|_, cx| eggs.read(cx).cheat));
        assert_eq!(unlocked(), [json!(["hesoyam"])]);
        cx.executor().advance_clock(Duration::from_millis(CHEAT_MS));
        cx.run_until_parked();
        assert!(!cx.update(|_, cx| eggs.read(cx).cheat));

        // Три щелчка по логотипу: пони на 1,45 с.
        for _ in 0..3 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
            let at = bounds_of("topbar-logo").unwrap().center();
            cx.update(|window, cx| {
                pointer_move(window, cx, at);
                press(window, cx, at, gpui::MouseButton::Left, 1, true);
                press(window, cx, at, gpui::MouseButton::Left, 1, false);
            });
            cx.run_until_parked();
        }
        assert!(cx.update(|_, cx| eggs.read(cx).pony));
        cx.executor().advance_clock(Duration::from_millis(PONY_MS));
        cx.run_until_parked();
        assert!(!cx.update(|_, cx| eggs.read(cx).pony));
        assert!(cx.update(|_, cx| eggs.read(cx).pony_image.is_some()));
        // После ухода картинки в памяти нет.
        cx.executor()
            .advance_clock(Duration::from_millis(PONY_LEAVE_MS));
        cx.run_until_parked();
        assert!(cx.update(|_, cx| eggs.read(cx).pony_image.is_none()));
    }
}
