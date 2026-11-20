//! «HLSR — Новая эра!» (`WelcomeModal.vue` 0.4.9): версия, заголовок, четыре плюса аккаунта,
//! согласие со ссылками на политику и условия, «Войти через Steam» (только с согласием; пока
//! ждём — кольцо), «Войти позже», ошибка сети, строка про локальный профиль. Esc — «Войти
//! позже», подложка окно не закрывает; фокус при открытии — на флажке согласия. Логика —
//! [`WelcomeState`]; показ и ответы окна — [`ShellFlows`] (`checkWelcome`, `onWelcome*`).

use crate::icons::icon;
use crate::kit::button::{focus_handle, focus_visible, white};
use crate::kit::{CheckLook, Checked, Handler, Preview, check, outline, text_link};
use crate::layer::{Modal, listen};
use crate::motion::{Timing, hover, mix};
use crate::paint::{ring, shadow};
use crate::scale::{u, viewport, zoom, zpx};
use crate::shell::root::ShellContext;
use crate::text::{RUBIK, TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, FocusHandle, FontWeight, IntoElement, Render,
    RenderOnce, Role, Task, TextRun, Window, div, font, prelude::*, rgb,
};
use hlsr_ui::controllers::shell_flows::ShellFlows;
use hlsr_ui::controllers::welcome::{Effect, WelcomeState};
use hlsr_ui::services::{Services, Standing};
use serde_json::Value;
use std::rc::Rc;

const PERKS: [(&str, &str); 4] = [
    ("#L_WELCOME_PERK_SYNC", "fas fa-cloud"),
    ("#L_WELCOME_PERK_PROFILE", "fas fa-id-badge"),
    ("#L_WELCOME_PERK_FRIENDS", "fas fa-user-friends"),
    ("#L_WELCOME_PERK_FEED", "fas fa-newspaper"),
];

pub struct WelcomeModal {
    services: Services,
    modal: Modal,
    state: WelcomeState,
    login: Option<Task<()>>,
    /// Контур на флажке, куда окно поставило фокус ([`super::opened_by_script`]).
    ring: bool,
    initial: Option<FocusHandle>,
}

impl WelcomeModal {
    pub fn new(services: &Services, cx: &mut Context<WelcomeModal>) -> WelcomeModal {
        WelcomeModal {
            services: services.clone(),
            modal: Modal::new("welcome", cx).keep_on_backdrop(),
            state: WelcomeState::new(),
            login: None,
            ring: false,
            initial: None,
        }
    }

    fn flows(cx: &App) -> Option<ShellFlows> {
        ShellContext::get(cx).map(|shell| shell.flows.clone())
    }

    fn tr(cx: &App, key: &str) -> String {
        ShellContext::get(cx).map_or_else(|| key.to_owned(), |shell| shell.shell.tr(key, &[]))
    }

    /// `v-if="welcomeOpen"`: окно открывается с чистым `data()`.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = Self::flows(cx).is_some_and(|flows| flows.with_state(|s| s.welcome_open));
        if open && !self.modal.is_open() {
            self.state = WelcomeState::new();
            self.ring = super::opened_by_script();
            self.initial = None;
            self.modal.open(window, cx);
        } else if !open && self.modal.is_open() {
            self.login = None;
            self.modal.close(window, cx);
        }
        cx.notify();
    }

    fn run(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Login(arg) => {
                    let reply = self.services.invoke("runner-login", vec![arg]);
                    self.login = Some(cx.spawn(async move |this, cx| {
                        let reply = reply.await;
                        this.update(cx, |this, cx| {
                            let effects = this.state.login_reply(reply);
                            this.run(effects, cx);
                        })
                        .ok();
                    }));
                }
                Effect::OpenLink(key) => {
                    let reply = self
                        .services
                        .invoke("open-external-link", vec![Value::from(key)]);
                    cx.background_spawn(async move {
                        let _ = reply.await;
                    })
                    .detach();
                }
                Effect::SignedIn => {
                    if let Some(flows) = Self::flows(cx) {
                        flows.on_welcome_signed_in();
                    }
                }
                Effect::Later => {
                    if let Some(flows) = Self::flows(cx) {
                        flows.on_welcome_later();
                    }
                }
            }
        }
        cx.notify();
    }

    /// Согласие: текст с двумя ссылками-кнопками (`@click.prevent`: флажок они не переключают).
    fn consent(
        &self,
        ring: bool,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let parts = [
            Self::tr(cx, "#L_WELCOME_CONSENT_A"),
            Self::tr(cx, "#L_WELCOME_PRIVACY"),
            Self::tr(cx, "#L_WELCOME_CONSENT_B"),
            Self::tr(cx, "#L_WELCOME_TERMS"),
        ];
        let view = cx.entity().downgrade();
        let text = consent_text(
            "welcome",
            parts,
            move |key, _, cx| {
                view.update(cx, |this, cx| {
                    let effects = this.state.open_link(key);
                    this.run(effects, cx);
                })
                .ok();
            },
            window,
            cx,
        );
        let agreed = self.state.view().agreed;
        let on_change = cx.listener(|this, agreed: &bool, _, cx| {
            this.state.set_agreed(*agreed);
            cx.notify();
        });
        crate::snap::target(
            "welcome-consent",
            check(
                "welcome-consent",
                if agreed { Checked::On } else { Checked::Off },
                CheckLook::Box,
            )
            .preview(Preview {
                focus: ring,
                ..Preview::default()
            })
            .items_start()
            .gap(u(10.))
            .text_px(12.5, 20.)
            .text_color(theme.color("l-text-dim"))
            .on_change(on_change)
            .child(text),
        )
    }
}

/// Текст согласия «{A} <button>политикой…</button> {B} <button>условия…</button>» (WelcomeModal,
/// ProfileModal), кегль 12,5 px: слова и ссылки-кнопки ([`text_link`]) — блоки строки с
/// переносом по словам и пробелом шрифта между ними; ссылка (`inline-block`) переносится целиком
/// и стоит в порядке Tab после флажка. `on_link` получает `privacy` или `terms`.
pub(crate) fn consent_text(
    id: &str,
    parts: [String; 4],
    on_link: impl Fn(&'static str, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    // Пробел строки `<span>` — с разрядкой `body`, как и слова.
    let run = TextRun {
        len: 1,
        font: font(RUBIK),
        color: gpui::black(),
        letter_spacing: window
            .text_style()
            .letter_spacing
            .to_pixels(window.rem_size()),
        ..Default::default()
    };
    let space = window
        .text_system()
        .layout_line(" ", zpx(zoom(window), 12.5), &[run], None)
        .width;
    let on_link = Rc::new(on_link);
    let mut text = div().flex_1().min_w_0().flex().flex_wrap().gap_x(space);
    for (index, part) in parts.into_iter().enumerate() {
        if index % 2 == 0 {
            text = text.children(
                part.split_whitespace()
                    .map(|word| div().child(word.to_owned())),
            );
            continue;
        }
        let key = if index == 1 { "privacy" } else { "terms" };
        let on_link = on_link.clone();
        let link = text_link(
            format!("{id}-{key}"),
            part,
            move |window, cx| on_link(key, window, cx),
            window,
            cx,
        );
        text = text.child(link);
    }
    text
}

/// `.welcome__button`: от 44 px, поля 10 × 20, кольцо `--l-border-strong` (у главной — заливка
/// `--accent`), 13,5 px / 600; наведение .09 (`--accent-hover`), `:disabled` .45, контур фокуса
/// 2 px `--accent`.
#[derive(IntoElement)]
struct WelcomeButton {
    id: &'static str,
    primary: bool,
    disabled: bool,
    content: AnyElement,
    on_click: Option<Handler<ClickEvent>>,
}

impl RenderOnce for WelcomeButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let (accent, accent_hover) = (theme.color("accent"), theme.color("accent-hover"));
        let (strong, text) = (theme.color("l-border-strong"), theme.color("l-text"));
        let contrast = theme.color("accent-contrast-text");
        let radius = theme.len("l-radius-sm");
        let zoom = zoom(window);
        let WelcomeButton {
            id,
            primary,
            disabled,
            content,
            on_click,
        } = self;
        let focus = focus_handle(&id.into(), window, cx);
        let ring_on = focus_visible(&focus, window);
        hover(id, Timing::fast(), move |t| {
            let t = if disabled { 0. } else { t };
            let (bg, color) = if primary {
                (mix(accent, accent_hover, t), contrast)
            } else {
                (mix(white(0.05), white(0.09), t), text)
            };
            div()
                .id(id)
                .relative()
                .flex()
                .items_center()
                .gap(u(8.))
                .min_h(u(44.))
                .py(u(10.))
                .px(u(20.))
                .rounded(u(radius))
                .bg(bg)
                .when(!primary, |el| el.shadow(vec![ring(zoom, 1., strong)]))
                .font_family(RUBIK)
                .text_px(13.5, 16.)
                .font_weight(FontWeight::BOLD)
                .text_color(color)
                .child(content)
                .when(disabled, |el| el.opacity(0.45))
                .when(!disabled, |el| {
                    el.track_focus(&focus)
                        .cursor_pointer()
                        .when_some(on_click, |el, handler| {
                            el.on_click(move |event, window, cx| handler(event, window, cx))
                        })
                })
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., accent))
                })
        })
    }
}

impl Render for WelcomeModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let flows = Self::flows(cx);
        let open = flows
            .as_ref()
            .is_some_and(|flows| flows.with_state(|s| s.welcome_open));
        if open != self.modal.is_open() {
            cx.defer_in(window, Self::sync);
        }
        // `updateStatus.currentVersion || ""`.
        let version = flows
            .map(|flows| {
                flows.with_state(|s| {
                    s.update_status
                        .get("currentVersion")
                        .filter(|v| hlsr_core::js::truthy(v))
                        .map(hlsr_core::js::to_string)
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default();
        let theme = cx.global::<Theme>().clone();
        let view = self.state.view();
        let tr = |key: &str| Self::tr(cx, key);
        let title = tr("#L_WELCOME_TITLE");
        let lead = tr("#L_WELCOME_LEAD");
        let perks: Vec<(String, &str)> = PERKS.iter().map(|(key, i)| (tr(key), *i)).collect();
        let (login, later) = (tr("#L_RUNNER_LOGIN"), tr("#L_WELCOME_LATER"));
        let error = view.error.then(|| tr("#L_RUNNER_NETWORK"));
        let local = tr("#L_WELCOME_LOCAL");
        // Фокус окна при открытии — флажок (первый в порядке Tab): запомнить его для контура.
        let scope = self.modal.scope().clone();
        let focused = window.focused(cx);
        if self.ring
            && self.initial.is_none()
            && focused.as_ref().is_some_and(|f| *f != scope)
            && scope.contains_focused(window, cx)
        {
            self.initial = focused.clone();
        }
        let consent_ring = self.ring && self.initial.is_some() && focused == self.initial;
        let consent = self.consent(consent_ring, &theme, window, cx);
        let zoom = zoom(window);
        let (vw, vh) = viewport(window);
        let accent = theme.color("accent");
        let primary_fg = theme.color("accent-contrast-text");
        let perk = |(text, glyph): (String, &str)| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(u(10.))
                .py(u(10.))
                .px(u(12.))
                .rounded(theme.rem("l-radius-sm"))
                .bg(white(0.04))
                .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
                .text_px(12.5, 18.)
                .text_color(theme.color("l-text"))
                .child(
                    div()
                        .w(u(16.))
                        .flex()
                        .justify_center()
                        .child(icon(glyph, 12.5, accent)),
                )
                .child(div().flex_1().min_w_0().child(text))
        };
        let mut perks = perks.into_iter();
        let mut perk_row = || {
            div()
                .flex()
                .gap(u(8.))
                .children(perks.next().map(perk))
                .children(perks.next().map(perk))
        };
        let sign_in = WelcomeButton {
            id: "welcome-sign-in",
            primary: true,
            disabled: !view.sign_in_enabled,
            content: div()
                .flex()
                .items_center()
                .gap(u(8.))
                .child(icon(
                    if view.busy {
                        "fas fa-spinner fa-spin"
                    } else {
                        "fab fa-steam"
                    },
                    13.5,
                    primary_fg,
                ))
                .child(login)
                .into_any_element(),
            on_click: Some(Rc::new(cx.listener(|this, _, _, cx| {
                let effects = this.state.sign_in();
                this.run(effects, cx);
            }))),
        };
        let later = WelcomeButton {
            id: "welcome-later",
            primary: false,
            disabled: view.busy,
            content: later.into_any_element(),
            on_click: Some(Rc::new(cx.listener(|this, _, _, cx| {
                let effects = this.state.later();
                this.run(effects, cx);
            }))),
        };
        // Вход через Steam — только в официальной сборке; самосборка — заглушка со ссылкой на
        // выпуски, пока сервер не ответил — ни того, ни другого.
        let build = crate::shell::root::standing(cx);
        let stub = (build == Standing::Unofficial).then(|| {
            crate::common::official_stub(
                "welcome-official-releases",
                "#L_OFFICIAL_LOGIN",
                13.,
                window,
                cx,
            )
            .mt(u(16.))
        });
        let actions = div()
            .flex()
            .flex_wrap()
            .gap(u(12.))
            .mt(u(20.))
            .when(build == Standing::Official, |el| {
                el.child(crate::snap::target("welcome-sign-in", sign_in))
            })
            .child(later);
        // .welcome__window: min(760px, 100% − 64px), до 100% − 64px по высоте, поля 32 × 40.
        let body = div()
            .id("welcome-window")
            .role(Role::Dialog)
            .aria_label(title.clone())
            .w(u((vw - 64.).min(760.)))
            .max_h(u(vh - 64.))
            .overflow_y_scroll()
            .bg(theme.color("l-modal-bg"))
            .rounded(theme.rem("l-radius"))
            .shadow(vec![
                ring(zoom, 1., theme.color("l-modal-border")),
                shadow(zoom, 0., 24., 72., 0., gpui::black().opacity(0.6)),
            ]);
        let radius = theme.rem("l-radius");
        let body = crate::liquid::glass(
            body,
            crate::liquid::Kind::Window,
            radius,
            gpui::Pixels::ZERO,
            &theme,
            zoom,
        )
        .pt(u(32.))
        .pb(u(32.))
        .px(u(40.))
        .child(spaced(
            format!("HLSR {version}").to_uppercase(),
            11.5,
            20.,
            FontWeight::BOLD,
            0.12,
            theme.color("l-text-faint"),
        ))
        .child(div().mt(u(4.)).mb(u(8.)).child(spaced(
            title,
            30.,
            36.,
            FontWeight::BOLD,
            -0.01,
            theme.color("l-text"),
        )))
        .child(
            div()
                .mb(u(16.))
                .text_px(13.5, 22.)
                .text_color(theme.color("l-text-dim"))
                .child(lead),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(u(8.))
                .mb(u(20.))
                .child(perk_row())
                .child(perk_row()),
        )
        .child(consent)
        .children(stub)
        .child(actions)
        .children(error.map(|error| {
            div()
                .mt(u(12.))
                .text_px(12.5, 20.)
                .text_color(rgb(0xff6b6b))
                .child(error)
        }))
        // «Войти можно через аватар в углу» — только когда аватар в шапке есть.
        .when(build == Standing::Official, |el| {
            el.child(
                div()
                    .mt(u(16.))
                    .text_px(12., 18.)
                    .text_color(theme.color("l-text-faint"))
                    .child(local),
            )
        });
        let on_close = listen(cx, |this: &mut WelcomeModal, _, cx| {
            // Esc — «Войти позже» (`onKeydown`).
            let effects = this.state.later();
            this.run(effects, cx);
        });
        self.modal
            .render(window, cx, on_close, move || body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Form {
        agreed: bool,
        changes: Vec<bool>,
        links: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Render for Form {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let links = self.links.clone();
            let parts = ["A a", "privacy", "B", "terms"].map(String::from);
            let text = consent_text(
                "t",
                parts,
                move |key, _, _| links.borrow_mut().push(key),
                window,
                cx,
            );
            let state = if self.agreed {
                Checked::On
            } else {
                Checked::Off
            };
            div().size_full().child(
                check("t-consent", state, CheckLook::Box)
                    .on_change(cx.listener(|this, on: &bool, _, cx| {
                        this.changes.push(*on);
                        this.agreed = *on;
                        cx.notify();
                    }))
                    .child(text),
            )
        }
    }

    /// Ссылки согласия — остановки Tab после флажка: Enter и пробел открывают ссылку, флажок
    /// не переключают (`<button>` в `<label>` 0.4.9).
    #[gpui::test]
    fn consent_links_take_tab_and_keys(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (form, cx) = cx.add_window_view(|_, _| Form::default());
        cx.run_until_parked();
        // Tab в GPUI не привязан (обход фокуса — F7): следующая остановка напрямую.
        let tab = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| window.focus_next(cx));
            cx.run_until_parked();
        };
        // Щелчок с клавиатуры — нажатие и отпускание.
        let press = |cx: &mut gpui::VisualTestContext, key: &'static str| {
            cx.simulate_keystrokes(key);
            cx.simulate_event(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse(key).unwrap(),
            });
        };
        tab(cx);
        tab(cx);
        press(cx, "enter");
        tab(cx);
        press(cx, "space");
        tab(cx);
        press(cx, "space");
        form.update(cx, |form, _| {
            assert_eq!(*form.links.borrow(), ["privacy", "terms"]);
            assert_eq!(form.changes, [true], "флажок — только своим пробелом");
        });
    }
}
