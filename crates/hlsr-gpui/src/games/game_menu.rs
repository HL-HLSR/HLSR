//! Меню игры ≡ (`GameMenu.vue` 0.4.9): кнопка 64 × `--l-play-h` рядом с «Играть» и меню над ней
//! у её правого края (`bottom: 100% + 12`, ширина по самому длинному пункту, не меньше 280) с
//! переходом `gm`. Десять пунктов [`game_menu::items`] — доступность и причина из
//! `game-menu-state` (спрашивается при создании, смене игры и каждом открытии); при выбранном
//! эпизоде — «Удалить эпизод». В низком окне пункты сжимаются по ступеням (`compact`, `tight`), а
//! не влезает и так — список прокручивается (`overlay::fit_game_menu`).
//!
//! Клавиши — [`MenuNav`] слоя у кнопки (фокус остаётся на ней): ↑/↓ открывают, дальше по кругу с
//! пропуском недоступных, Enter/пробел — выбрать, Esc закрывает, Tab закрывает и идёт дальше.
//! Щелчок мимо меню и кнопки закрывает.

use crate::icons::fa_glyph;
use crate::kit::Handler;
use crate::layer::{
    MenuKey, MenuNav, Outcome, TipExt, Z_MENU, anchor_probe, menu_keys, pop_surface,
};
use crate::liquid;
use crate::motion::{Origin, Timing, hover, mix, presence, rise_in};
use crate::paint::ring;
use crate::scale::{u, zoom, zpx};
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Anchor, AnyElement, App, Bounds, Context, ElementId, FocusHandle, Hsla, IntoElement, Pixels,
    Render, Role, SharedString, Window, anchored, deferred, div, hsla, point, prelude::*, rgb,
    rgba,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::game_menu::{self, DENSITIES, Density, MenuItem};
use hlsr_ui::overlay::fit_game_menu;
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::cell::Cell;
use std::rc::Rc;

pub struct GameMenu {
    services: Services,
    lang: Language,
    /// `gameId`: игра статуса доставки; `None` — меню без ответа main.
    game_id: Option<String>,
    /// `episode`: выбран эпизод Ghosting — `Some(installed)`.
    episode: Option<bool>,
    /// Ответ `game-menu-state` (`state`).
    state: Option<Value>,
    /// Номер последнего запроса `game-menu-state`: ответы старых не применяются.
    request: u64,
    nav: MenuNav,
    focus: FocusHandle,
    /// Рамка кнопки в прошлом кадре (px окна).
    anchor: Rc<Cell<Bounds<Pixels>>>,
    on_action: Option<Handler<str>>,
}

impl GameMenu {
    pub fn new(services: &Services, cx: &mut Context<GameMenu>) -> GameMenu {
        GameMenu {
            services: services.clone(),
            lang: Language::En,
            game_id: None,
            episode: None,
            state: None,
            request: 0,
            nav: MenuNav::default(),
            focus: cx.focus_handle().tab_stop(true),
            anchor: Rc::default(),
            on_action: None,
        }
    }

    /// Свойства от панели: язык, игра (`watch.gameId`: закрыть и спросить заново), эпизод,
    /// выбор пункта (`@action`).
    pub fn set(
        &mut self,
        lang: Language,
        game_id: Option<String>,
        episode: Option<bool>,
        on_action: Handler<str>,
        cx: &mut Context<GameMenu>,
    ) {
        self.lang = lang;
        self.episode = episode;
        self.on_action = Some(on_action);
        if self.game_id != game_id {
            self.game_id = game_id;
            // Доступность прошлой игры до ответа не показывать.
            self.state = None;
            self.nav.hide();
            self.refresh(cx);
        }
    }

    /// `refreshState()`: без игры ответа нет.
    fn refresh(&mut self, cx: &mut Context<GameMenu>) {
        self.request += 1;
        let Some(game_id) = self.game_id.clone() else {
            self.state = None;
            return;
        };
        let request = self.request;
        let reply = self
            .services
            .invoke("game-menu-state", vec![json!(game_id)]);
        cx.spawn(async move |this, cx| {
            let Ok(state) = reply.await else { return };
            this.update(cx, |this, cx| {
                // Ответ не последнего запроса (другая игра или новее открытие) — не показывать.
                if this.request == request {
                    this.state = Some(state);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn items(&self) -> Vec<MenuItem> {
        game_menu::items(self.lang, self.state.as_ref(), self.episode)
    }

    /// Открыть (`openMenu`): состояние спрашивается заново.
    fn opened(&mut self, cx: &mut Context<GameMenu>) {
        self.refresh(cx);
        cx.notify();
    }

    /// `choose(item)`.
    fn choose(&mut self, id: &str, window: &mut Window, cx: &mut Context<GameMenu>) {
        self.nav.hide();
        cx.notify();
        if let Some(on_action) = self.on_action.clone() {
            on_action(id, window, cx);
        }
    }

    fn on_key(&mut self, key: &MenuKey, window: &mut Window, cx: &mut Context<GameMenu>) {
        let items = self.items();
        let disabled: Vec<bool> = items.iter().map(|item| !item.enabled).collect();
        let was_open = self.nav.open;
        let now = cx.background_executor().now();
        let outcome = self
            .nav
            .key(key, now, &[] as &[&str], &disabled, -1)
            .settle(cx);
        // Enter и пробел у кнопки — её щелчок: `openMenu(0)`, первый пункт в фокусе.
        if !was_open && self.nav.open && *key == MenuKey::Pick {
            self.nav.active = 0;
        }
        match outcome {
            Outcome::Pick(index) => self.choose(items[index].id, window, cx),
            Outcome::Handled if !was_open && self.nav.open => self.opened(cx),
            Outcome::Handled => cx.notify(),
            Outcome::Pass => {
                if was_open && !self.nav.open {
                    cx.notify();
                }
            }
        }
    }
}

/// Цвета меню из темы.
#[derive(Clone, Copy)]
struct Colors {
    text: Hsla,
    dim: Hsla,
    border: Hsla,
    accent: Hsla,
    soft: Hsla,
    line: Hsla,
    /// color-mix(in srgb, var(--l-accent) 45%, var(--l-border)).
    edge: Hsla,
}

fn white(alpha: f32) -> Hsla {
    hsla(0., 0., 1., alpha)
}

const DANGER: u32 = 0xff7a7a;

/// Выбор пункта щелчком.
type Pick = Rc<dyn Fn(&mut Window, &mut App)>;

struct Row {
    index: usize,
    item: MenuItem,
    density: Density,
    /// Пункт с клавиатуры (`:focus-visible`).
    active: bool,
    colors: Colors,
    zoom: f32,
    radius: gpui::Rems,
    pick: Pick,
}

/// `.game-menu__item`: плашка значка и подпись 14/20, наведение подсвечивает фон и рамку,
/// недоступный — .45 с причиной в подсказке, `danger` — красный.
fn row(row: Row) -> AnyElement {
    let Row {
        index,
        item,
        density,
        active,
        colors: c,
        zoom,
        radius,
        pick,
    } = row;
    let (pad, icon_size, _, separated_gap, line_top) = density.metrics();
    let live = item.enabled;
    let danger = live && item.danger;
    let id = ElementId::NamedInteger("game-menu-item".into(), index as u64);
    let target = format!("game-menu-{}", item.id);
    let title: SharedString = if live {
        item.label.clone().into()
    } else {
        item.hint.clone().into()
    };
    let element = hover(id.clone(), Timing::fast(), move |t| {
        let t = if live { t } else { 0. };
        let lit = t.max(f32::from(u8::from(active)));
        let (bg, edge) = if danger {
            (
                rgba(0xff6b6b14).into(),
                if active {
                    rgb(0xff6b6b).into()
                } else {
                    Hsla::from(rgba(0xff6b6b73))
                },
            )
        } else {
            (white(0.05), if active { c.accent } else { c.edge })
        };
        let color: Hsla = if danger {
            rgb(DANGER).into()
        } else if live {
            c.text
        } else {
            c.dim
        };
        let (plate, plate_color, plate_ring) = if danger {
            (rgba(0xff6b6b1f).into(), rgb(DANGER).into(), 0.)
        } else {
            (
                mix(white(0.04), c.soft, lit),
                mix(c.dim, c.accent, lit),
                1. - lit,
            )
        };
        div()
            .id(id)
            .role(Role::MenuItem)
            .button_text()
            .aria_label(SharedString::from(item.label.clone()))
            .relative()
            .flex()
            .items_center()
            .gap(u(12.))
            .w_full()
            .p(u(8.))
            .py(u(pad))
            .rounded(radius)
            .bg(bg.opacity(lit))
            .when(lit > 0., |el| {
                el.shadow(vec![ring(zoom, 1., edge.opacity(lit))])
            })
            .font_family(RUBIK)
            .text_px(14., 20.)
            .text_color(color)
            .when(!live, |el| el.opacity(0.45))
            .when(live, |el| {
                el.cursor_pointer()
                    .active(|s| s.bg(white(0.08)))
                    .on_click(move |_, window, cx| pick(window, cx))
            })
            .tip(title)
            .when(item.separated, |el| {
                el.mt(u(separated_gap)).child(
                    div()
                        .absolute()
                        .left(u(4.))
                        .right(u(4.))
                        .top(u(line_top))
                        .h(gpui::px(1.))
                        .bg(c.line),
                )
            })
            .child(
                div()
                    .flex_none()
                    .size(u(icon_size))
                    .rounded(radius)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(plate)
                    .when(plate_ring > 0., |el| {
                        el.shadow(vec![ring(zoom, 1., c.border.opacity(plate_ring))])
                    })
                    .text_color(plate_color)
                    .child(fa_glyph(&format!("fas {}", item.icon), 13.)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .child(item.label.clone()),
            )
    });
    rise_in(
        ElementId::NamedInteger("game-menu-rise".into(), index as u64),
        index,
        div().w_full().child(crate::snap::target(target, element)),
    )
    .into_any_element()
}

impl Render for GameMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let open = self.nav.open;
        let focused = self.focus.is_focused(window) && window.last_input_was_keyboard() && !open;
        let label = hlsr_ui::tr(self.lang, "#L_GM_TITLE", &[]);
        let solid = theme.color("l-surface-solid");
        let accent = theme.color("accent");
        let border = theme.color("l-border");
        let l_accent = theme.color("l-accent");
        let lit_border = mix(border, l_accent, 0.35);
        let text = theme.color("l-text");
        let (play_h, radius) = (theme.rem("l-play-h"), theme.rem("l-radius"));

        // ---- меню ----
        let anchor = self.anchor.get();
        let css = |value: Pixels| f64::from(f32::from(value) / zoom);
        let heights = DENSITIES.map(Density::height);
        let fit = fit_game_menu(
            &heights,
            css(anchor.top()),
            f64::from(theme.len("l-topbar-h")),
        );
        let density = DENSITIES[fit.step];
        let (_, _, gap, _, _) = density.metrics();
        let colors = Colors {
            text,
            dim: theme.color("l-text-dim"),
            border,
            accent: l_accent,
            soft: theme.color("l-accent-soft"),
            line: theme.color("l-modal-border"),
            edge: mix(border, l_accent, 0.45),
        };
        let item_radius = theme.rem("l-radius-sm");
        let rows: Vec<AnyElement> = self
            .items()
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let id = item.id;
                let weak = cx.weak_entity();
                let pick: Pick = Rc::new(move |window, cx| {
                    weak.update(cx, |menu, cx| menu.choose(id, window, cx)).ok();
                });
                row(Row {
                    index,
                    active: open && self.nav.active == index as i32,
                    item,
                    density,
                    colors,
                    zoom,
                    radius: item_radius,
                    pick,
                })
            })
            .collect();
        let surface = pop_surface(&theme, zoom);
        let max_height = (fit.max_height > 0.).then_some(fit.max_height as f32);
        let weak = cx.weak_entity();
        let trigger_box = anchor;
        let menu = presence(
            "game-menu-pop",
            open,
            liquid::motion(&theme, "gm"),
            move || {
                surface
                    .id("game-menu-dropdown")
                    .role(Role::Menu)
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap(u(gap))
                    .p(u(8.))
                    .min_w(u(280.))
                    .when_some(max_height, |el, max| el.max_h(u(max)).overflow_y_scroll())
                    // `onDocumentDown`: нажатие мимо меню и кнопки закрывает.
                    .on_mouse_down_out(move |event, _, cx| {
                        if trigger_box.contains(&event.position) {
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
        // `.game-menu__dropdown { transform-origin: calc(100% - 32px) 100% }`.
        .origin(Origin::new((1., -32.), (1., 0.)));
        let popup = deferred(
            anchored()
                .position(point(anchor.right(), anchor.top() - zpx(zoom, 12.)))
                .anchor(Anchor::BottomRight)
                .snap_to_window()
                .child(menu),
        )
        .with_priority(Z_MENU);

        // ---- кнопка ----
        let trigger = hover("game-menu-trigger-hover", Timing::fast(), move |t| {
            let (bg, edge, color) = if focused {
                (solid.blend(accent.opacity(0.22)), l_accent, text)
            } else if open {
                (solid.blend(accent.opacity(0.16)), lit_border, gpui::white())
            } else {
                (
                    solid.blend(white(0.08 * t)),
                    mix(border, lit_border, t),
                    mix(text, gpui::white(), t),
                )
            };
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius)
                .bg(bg)
                .shadow(vec![ring(zoom, 1., edge)])
                .text_color(color)
                .child(fa_glyph("fas fa-bars", 20.))
        });
        let on_key = cx.listener(|menu: &mut GameMenu, key: &MenuKey, window, cx| {
            menu.on_key(key, window, cx)
        });
        let pressed = solid.blend(accent.opacity(0.16));
        menu_keys(
            div()
                .id("game-menu-trigger")
                .role(Role::Button)
                .button_text()
                .aria_label(SharedString::from(label.clone()))
                .aria_expanded(open)
                .track_focus(&self.focus)
                .relative()
                .flex_none()
                .w(u(64.))
                .h(play_h)
                .rounded(radius)
                .cursor_pointer()
                .active(move |s| s.bg(pressed))
                .tip(label)
                .on_click(cx.listener(|menu, event: &gpui::ClickEvent, _, cx| {
                    // Enter и пробел у кнопки — клавиши меню (MenuPick), не щелчок.
                    if event.is_keyboard() {
                        return;
                    }
                    if menu.nav.open {
                        menu.nav.hide();
                        cx.notify();
                    } else {
                        menu.nav.show();
                        menu.opened(cx);
                    }
                }))
                // Liquid Glass: `.game-menu__trigger` — мягкое стекло под заливкой кнопки.
                .when(liquid::active(&theme), |el| {
                    liquid::with_surface(
                        el,
                        liquid::Surface::new(liquid::Kind::Soft, radius)
                            .layer(theme.liquid_share()),
                    )
                })
                .child(crate::snap::target("game-menu", trigger))
                .child(anchor_probe(self.anchor.clone()))
                .child(popup),
            on_key,
        )
    }
}
