//! Меню аккаунта в шапке (`AccountMenu.vue` 0.4.9). Кнопка 248 × 40 (в окне уже 1248 px —
//! только аватар и стрелка): аватар 40 в рамке оформления (2 px, подложка `--l-bg`) с точкой
//! присутствия, ник со значком подписчика и строка присутствия; подсказка «ник · присутствие ·
//! состояние». Меню (`l-pop`) по правому краю кнопки, 260 px: гостю — «Войти через Steam» и
//! «Друзья», вошедшему — «Открыть профиль», «Настройки профиля», «Друзья»; карточки-шапки нет.
//!
//! Состояние — `runner-state` при создании, раз в 30 с и при возврате фокуса окну, событие
//! `runner-profile-state` (логика — `hlsr_ui::controllers::account`). Клавиши у кнопки
//! ([`MenuNav`], фокус остаётся на ней): ↓ открывает на первом пункте, ↑/↓ по кругу, Enter и
//! пробел — выбрать, Esc закрывает, Tab закрывает и идёт дальше. Щелчок мимо меню закрывает.
//! Пункты — сценарии оболочки `onAccountAction` (профиль, его настройки, друзья).

use super::{SCOPE, Z_TOPBAR};
use crate::common::{avatar_frame, nick_name};
use crate::icons::{chevron, fa_glyph};
use crate::image::{Pictures, device_size};
use crate::kit::button::{focus_visible, outline, white};
use crate::layer::{MenuKey, MenuNav, Outcome, TipExt, anchor_probe, menu_keys, pop_surface};
use crate::liquid;
use crate::motion::{Origin, POP, Timing, hover, mix, presence, rise_in, tween};
use crate::paint::ring;
use crate::scale::{u, viewport, zoom, zpx};
use crate::shell::root::ShellContext;
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{
    Anchor, AnyElement, App, Bounds, ClickEvent, Context, ElementId, FocusHandle, FontWeight, Hsla,
    ObjectFit, Pixels, Render, Role, SharedString, Subscription, Task, Window, anchored, deferred,
    div, img, point, prelude::*, rgb,
};
use hlsr_ui::controllers::account::{ACCOUNT_REFRESH_MS, AccountItem, AccountState, AccountView};
use hlsr_ui::images::{Fit, ImageRequest, ImageSource};
use hlsr_ui::services::{Reply, Services};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// `@media (max-width: 1247px)`: только аватар и стрелка.
const NARROW_BELOW: f32 = 1248.;

pub struct AccountMenu {
    services: Services,
    state: AccountState,
    nav: MenuNav,
    focus: FocusHandle,
    /// Рамка кнопки в прошлом кадре (px окна).
    anchor: Rc<Cell<Bounds<Pixels>>>,
    pictures: Option<Pictures>,
    /// Возврат фокуса окну — `refreshAccount` (подписка — с первого кадра, ей нужно окно).
    activation: Option<Subscription>,
    _events: Task<()>,
    _poll: Task<()>,
}

impl AccountMenu {
    pub fn new(services: &Services, cx: &mut Context<AccountMenu>) -> AccountMenu {
        let events =
            crate::bridge::listen(cx, services.subscribe(), |this: &mut Self, event, cx| {
                if event.channel == "runner-profile-state" && this.state.apply(&event.payload) {
                    cx.notify();
                }
            });
        // `setInterval(refreshAccount, ACCOUNT_REFRESH_MS)`.
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(ACCOUNT_REFRESH_MS))
                    .await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        let menu = AccountMenu {
            services: services.clone(),
            state: AccountState::new(),
            nav: MenuNav::default(),
            focus: cx.focus_handle().tab_stop(true),
            anchor: Rc::default(),
            pictures: cx.try_global::<Pictures>().cloned(),
            activation: None,
            _events: events,
            _poll: poll,
        };
        menu.refresh(cx);
        menu
    }

    /// `refreshAccount()`: `runner-state`; не объект или отказ — без изменений.
    fn refresh(&self, cx: &mut Context<AccountMenu>) {
        let reply = self.services.invoke("runner-state", Vec::new());
        cx.spawn(async move |this, cx| {
            let reply: Reply = reply.await;
            let Ok(value) = reply else { return };
            this.update(cx, |this, cx| {
                if this.state.apply(&value) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// `close(true)`: меню закрыто, фокус на кнопке.
    fn close(&mut self, window: &mut Window, cx: &mut Context<AccountMenu>) {
        self.nav.hide();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// `select(item)`: закрыть и `$emit("action", id)` — `onAccountAction` оболочки.
    fn select(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<AccountMenu>) {
        self.close(window, cx);
        match id {
            "profile-settings" => crate::profile::modal::open_own_settings(cx),
            _ => {
                if let Some(shell) = ShellContext::get(cx) {
                    shell.flows.clone().on_account_action(id);
                }
            }
        }
    }

    fn on_key(&mut self, key: &MenuKey, window: &mut Window, cx: &mut Context<AccountMenu>) {
        // У кнопки 0.4.9 только ↓ (`openAndFocus(0)`); Home/End и буквы меню не знает.
        if matches!(key, MenuKey::Home | MenuKey::End | MenuKey::Char(_))
            || (*key == MenuKey::Up && !self.nav.open)
        {
            return;
        }
        let items = self.state.view(&super::tr(cx)).items;
        let disabled = vec![false; items.len()];
        let now = cx.background_executor().now();
        let was_open = self.nav.open;
        match self
            .nav
            .key(key, now, &[] as &[&str], &disabled, -1)
            .settle(cx)
        {
            Outcome::Pick(index) => self.select(items[index].id, window, cx),
            Outcome::Handled => cx.notify(),
            Outcome::Pass if was_open => cx.notify(),
            Outcome::Pass => {}
        }
    }

    /// `.account-avatar` 40: круг, картинка Steam или значок Steam, кольцо `outline` поверх
    /// картинки, точка присутствия (`round-dot` 16 с обводкой 2 px цветом фона).
    fn avatar(
        &self,
        view: &AccountView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>();
        let zoom = zoom(window);
        let size = theme.len("l-header-control");
        let (border, faint, bg) = (
            theme.color("l-border"),
            theme.color("l-text-faint"),
            theme.color("l-bg"),
        );
        let photo = ImageSource::parse(&view.avatar).and_then(|source| {
            let (w, h) = device_size(window, size, size);
            let request = ImageRequest::new(source, SCOPE).with_target(w, h, Fit::Cover);
            self.pictures
                .as_ref()?
                .get(request, window, cx)
                .and_then(Result::ok)
        });
        let dot: Hsla = rgb(match view.presence_state {
            "checking" => 0xe8c33d,
            "online" => 0x2fa84f,
            _ => 0x8b939e,
        })
        .into();
        let avatar = div()
            .relative()
            .size(u(size))
            .rounded_full()
            .bg(white(0.05))
            .flex()
            .items_center()
            .justify_center()
            .text_color(faint)
            .map(|el| match photo {
                Some(photo) => el.child(
                    img(photo)
                        .size_full()
                        .rounded_full()
                        .object_fit(ObjectFit::Cover),
                ),
                None => el.child(fa_glyph("fab fa-steam", 24.)),
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .shadow(vec![ring(zoom, 1., border)]),
            )
            .child(
                div()
                    .absolute()
                    .right(u(-2.))
                    .bottom(u(-2.))
                    .size(u(16.))
                    .rounded_full()
                    .bg(bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(u(12.)).rounded_full().bg(dot)),
            );
        let frame = if view.show_nick {
            view.frame_css.clone()
        } else {
            String::new()
        };
        avatar_frame("account-avatar-frame", size, avatar)
            .css(frame)
            .width(2.)
            .hole(bg)
            .into_any_element()
    }
}

/// Цвета пункта меню.
#[derive(Clone, Copy)]
struct Colors {
    text: Hsla,
    dim: Hsla,
    border: Hsla,
    accent: Hsla,
    soft: Hsla,
    /// `color-mix(in srgb, var(--l-accent) 45%, var(--l-border))`.
    edge: Hsla,
}

/// Выбор пункта щелчком.
type Pick = Rc<dyn Fn(&mut Window, &mut App)>;

/// `.account-menu__item`: плашка значка 32 и подпись 14/20 500; наведение и фокус с клавиатуры —
/// фон .05, рамка `edge`, подпись `--l-text`, плашка `--l-accent-soft` со значком `--l-accent`;
/// появление `l-rise-in` лесенкой 24 мс (`--i` = номер + 1).
fn item(
    index: usize,
    item: AccountItem,
    active: bool,
    c: Colors,
    zoom: f32,
    radius: gpui::Rems,
    pick: Pick,
) -> AnyElement {
    let id = ElementId::NamedInteger("account-item".into(), index as u64);
    let label: SharedString = item.label.into();
    let row = hover(id.clone(), Timing::fast(), move |t| {
        let lit = t.max(f32::from(u8::from(active)));
        div()
            .id(id)
            .role(Role::MenuItem)
            .button_text()
            .aria_label(label.clone())
            .flex()
            .items_center()
            .gap(u(12.))
            .w_full()
            .p(u(8.))
            .rounded(radius)
            .bg(white(0.05).opacity(lit))
            .when(lit > 0., |el| {
                el.shadow(vec![ring(zoom, 1., c.edge.opacity(lit))])
            })
            .text_px(14., 20.)
            .font_weight(FontWeight::MEDIUM)
            .text_color(mix(c.dim, c.text, lit))
            .cursor_pointer()
            .active(|s| s.bg(white(0.08)))
            .on_click(move |_, window, cx| pick(window, cx))
            .child(
                div()
                    .flex_none()
                    .size(u(32.))
                    .rounded(radius)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(mix(white(0.04), c.soft, lit))
                    .when(lit < 1., |el| {
                        el.shadow(vec![ring(zoom, 1., c.border.opacity(1. - lit))])
                    })
                    .text_color(mix(c.dim, c.accent, lit))
                    .child(fa_glyph(item.icon, 13.)),
            )
            .child(div().min_w_0().whitespace_nowrap().child(label))
    });
    rise_in(
        ElementId::NamedInteger("account-item-rise".into(), index as u64),
        index + 1,
        div()
            .w_full()
            .child(crate::snap::target(format!("account-item-{index}"), row)),
    )
    .into_any_element()
}

impl Render for AccountMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.activation.is_none() {
            // `window.addEventListener("focus", refreshAccount)`: фокус вернулся окну. Первое
            // включение окна при запуске — не возврат (страница 0.4.9 его не получает: окно уже
            // в фокусе, когда она загрузилась).
            let mut was_active = false;
            self.activation = Some(cx.observe_window_activation(
                window,
                move |this, window, cx| {
                    let active = window.is_window_active();
                    if active && was_active {
                        this.refresh(cx);
                    }
                    was_active |= active;
                },
            ));
        }
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let view = self.state.view(&super::tr(cx));
        let open = self.nav.open;
        let narrow = viewport(window).0 < NARROW_BELOW;
        let size = theme.len("l-header-control");
        let radius = theme.rem("l-radius-sm");
        let radius_px = theme.len("l-radius-sm");
        let (text, dim) = (theme.color("l-text"), theme.color("l-text-dim"));
        let (border, accent) = (theme.color("l-border"), theme.color("l-accent"));
        let ring_on = focus_visible(&self.focus, window);

        // ---- меню ----
        let colors = Colors {
            text,
            dim,
            border,
            accent,
            soft: theme.color("l-accent-soft"),
            edge: mix(border, accent, 0.45),
        };
        let rows: Vec<AnyElement> = view
            .items
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, entry)| {
                let id = entry.id;
                let weak = cx.weak_entity();
                let pick: Pick = Rc::new(move |window, cx| {
                    weak.update(cx, |menu, cx| menu.select(id, window, cx)).ok();
                });
                item(
                    index,
                    entry,
                    open && self.nav.active == index as i32,
                    colors,
                    zoom,
                    radius,
                    pick,
                )
            })
            .collect();
        let anchor = self.anchor.get();
        let surface = pop_surface(&theme, zoom);
        let weak = cx.weak_entity();
        let panel = presence(
            "account-menu-pop",
            open,
            liquid::motion(&theme, POP),
            move || {
                surface
                    .id("account-menu-panel")
                    .role(Role::Menu)
                    .occlude()
                    .w(u(260.))
                    .p(u(8.))
                    .flex()
                    .flex_col()
                    .gap(u(4.))
                    // `onDocumentDown`: нажатие мимо меню и кнопки закрывает.
                    .on_mouse_down_out(move |event, _, cx| {
                        if anchor.contains(&event.position) {
                            return;
                        }
                        weak.update(cx, |menu, cx| {
                            if menu.nav.open {
                                menu.nav.hide();
                                cx.notify();
                            }
                        })
                        .ok();
                    })
                    .children(rows)
            },
        )
        // `.account-menu__panel { transform-origin: 100% 0 }`.
        .origin(Origin::new((1., 0.), (0., 0.)));
        let popup = deferred(
            anchored()
                .position(point(anchor.right(), anchor.bottom() + zpx(zoom, 8.)))
                .anchor(Anchor::TopRight)
                .child(panel),
        )
        .with_priority(Z_TOPBAR);

        // ---- кнопка ----
        let identity = (!narrow).then(|| {
            let name = if view.show_nick {
                nick_name("account-nick", view.name.clone())
                    .look(view.nick_style.clone(), view.nick_css.clone())
                    .subscriber(view.subscriber, None)
                    .into_any_element()
            } else {
                view.name.clone().into_any_element()
            };
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                // Поля под свечение ника тут же съедаются отрицательным отступом.
                .child(
                    div()
                        .mx(u(-8.))
                        .my(u(-6.))
                        .px(u(8.))
                        .py(u(6.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_px(15., 24.)
                        .font_weight(FontWeight::MEDIUM)
                        .child(name),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_px(11., 16.)
                        .text_color(dim)
                        .child(view.presence_text.clone()),
                )
        });
        let avatar = self.avatar(&view, window, cx);
        let width = if narrow { size + 32. } else { 248. };
        let hint = view.hint.clone();
        let label: SharedString = view.label.clone().into();
        let toggle = hover("account-toggle-hover", Timing::fast(), move |t| {
            let lit = if open { 1. } else { t };
            div()
                .id("account-toggle")
                .relative()
                .flex()
                .items_center()
                .gap(u(12.))
                .w(u(width))
                .h(u(size))
                .pr(u(8.))
                .rounded(u(radius_px))
                .bg(white(0.06).opacity(lit))
                .text_color(text)
                .cursor_pointer()
                .child(avatar)
                .children(identity)
                .child(
                    div()
                        .flex_none()
                        .w(u(12.))
                        .flex()
                        .justify_center()
                        // `.open .account-menu__chev { transform: rotate(180deg) }` за --l-fast.
                        .child(tween(
                            "account-chev",
                            if open { 1. } else { 0. },
                            Timing::fast(),
                            move |t| chevron(11., 180. * t, dim),
                        )),
                )
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius_px, 0., 2., accent))
                })
        });
        let on_key = cx.listener(|menu: &mut AccountMenu, key: &MenuKey, window, cx| {
            menu.on_key(key, window, cx)
        });
        menu_keys(
            div()
                .id("account-menu")
                .role(Role::Button)
                .button_text()
                .aria_label(label)
                .aria_expanded(open)
                .track_focus(&self.focus)
                .relative()
                .flex_none()
                .tip(hint)
                .on_click(cx.listener(|menu, event: &ClickEvent, _, cx| {
                    // Enter и пробел у кнопки — клавиши меню (MenuPick), не щелчок.
                    if event.is_keyboard() {
                        return;
                    }
                    if menu.nav.open {
                        menu.nav.hide();
                    } else {
                        menu.nav.show();
                    }
                    cx.notify();
                }))
                .child(crate::snap::target("account-toggle", toggle))
                .child(anchor_probe(self.anchor.clone()))
                .child(popup),
            on_key,
        )
    }
}
