//! HLSR-CE: пасхалка «Legacy» — интерфейс HLSR 0.2.9 (Electron + Vue, `HLSR-Legacy`) поверх
//! логики нового лаунчера. Включается и выключается набором `LEGACY` на клавиатуре
//! ([`crate::shell::eggs::LegacyCode`]), живёт до перезапуска (prefs не трогает).
//!
//! Раскладка — как в 0.2.9: окно 950 × 690 CSS px (`CreateWindow.js`), рамка 32 px
//! (`Frame.vue`), колонка 90 px (`SideBar.vue`), страница с полями 32 px (`#form`). Размеры
//! масштабируются от окна: `k = min(w / 950, h / 690)` — старый интерфейс выглядит так же на
//! любом размере окна нового.
//!
//! Что работает по-настоящему (через сценарии оболочки нового лаунчера, `ShellFlows`):
//! «Играть», «Установить» и состояния кнопки, «Открыть папку», «Создать ярлык», «Удалить»,
//! «Быстрый запуск» последней игры, язык интерфейса, ссылки, кнопки окна. Чего в новом
//! лаунчере нет (Steamworks, виджет SoundCloud, конструктор скриптов 0.2.9) — нарисовано, как
//! было, а действия объясняют это тостом.

use crate::icons::fa_glyph;
use crate::motion::{AnimGate, Timing, hover, mix, monitor_frame, tween};
use crate::shell::root::ShellContext;
use crate::text::RUBIK;
use gpui::{
    AnyElement, App, AssetSource, ClickEvent, Context, Div, FontWeight, Hsla, Img, IntoElement,
    ObjectFit, Pixels, Render, RenderImage, Rgba, SharedString, Stateful, Window,
    WindowControlArea, deferred, div, img, linear_color_stop, linear_gradient, prelude::*, px, svg,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::download_bar::button_view;
use hlsr_ui::services::StoreRead;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Окно 0.2.9 (`CreateWindow.js`: `width: 950, height: 690`, без рамки, без изменения размера).
const BASE_W: f32 = 950.;
const BASE_H: f32 = 690.;
/// `--accent-color`.
const ACCENT: u32 = 0x00ABFF;

/// Игра колонки 0.2.9 (`games.config.js` + `SideBar.vue`).
struct LegacyGame {
    id: &'static str,
    name: &'static str,
    /// Цвет лямбды (`filter: invert(.4) sepia(1) hue-rotate(…) saturate(…)`).
    lambda: u32,
    background: &'static str,
    wiki: &'static str,
    leaderboard: &'static str,
}

const GAMES: [LegacyGame; 5] = [
    LegacyGame {
        id: "70",
        name: "Half-Life",
        lambda: 0xA88B5E,
        background: "legacy/screenshots/half-life-background.jpg",
        wiki: "https://wiki.sourceruns.org/wiki/Category:Half-Life_1",
        leaderboard: "https://www.speedrun.com/hl1",
    },
    LegacyGame {
        id: "50",
        name: "Half-Life: Opposing Force",
        lambda: 0x7FA65E,
        background: "legacy/screenshots/opposing-force-background.jpg",
        wiki: "https://wiki.sourceruns.org/wiki/Category:Opposing_Force",
        leaderboard: "https://www.speedrun.com/op4",
    },
    LegacyGame {
        id: "130",
        name: "Half-Life: Blue Shift",
        lambda: 0x5E8AA8,
        background: "legacy/screenshots/blue-shift-background.jpg",
        wiki: "https://wiki.sourceruns.org/wiki/Category:Blue_Shift",
        leaderboard: "https://www.speedrun.com/hlbs",
    },
    LegacyGame {
        id: "220",
        name: "Half-Life 2",
        lambda: 0xFF7A00,
        background: "legacy/screenshots/half-life-2-background.jpg",
        wiki: "https://wiki.sourceruns.org/wiki/Category:Half-Life_2",
        leaderboard: "https://www.speedrun.com/hl2",
    },
    LegacyGame {
        id: "218",
        name: "Half-Life 2: Ghosting",
        lambda: 0xFF7A00,
        background: "legacy/screenshots/half-life-2-ghosting-background.jpg",
        wiki: "https://wiki.sourceruns.org/wiki/Category:Half-Life_2",
        leaderboard: "https://www.speedrun.com/hl2",
    },
];

fn game(id: &str) -> &'static LegacyGame {
    GAMES.iter().find(|g| g.id == id).unwrap_or(&GAMES[0])
}

/// Фоны оформления (`BackgroundTheme.vue`: 0 — градиент, 1 — синий, 2 — красный, 3 — Freelancer).
const THEMES: [&str; 4] = [
    "legacy/gradient.jpg",
    "legacy/blue.jpg",
    "legacy/red.jpg",
    "legacy/lancer.jpg",
];

/// Все картинки интерфейса 0.2.9: разбираются заранее одним пакетом ([`decode_images`]), иначе
/// `img("путь")` GPUI грузит каждую при первом показе и страница появляется без неё.
const IMAGES: [&str; 26] = [
    "legacy/gradient.jpg",
    "legacy/blue.jpg",
    "legacy/red.jpg",
    "legacy/lancer.jpg",
    "legacy/logo.png",
    "legacy/screenshots/half-life-background.jpg",
    "legacy/screenshots/opposing-force-background.jpg",
    "legacy/screenshots/blue-shift-background.jpg",
    "legacy/screenshots/half-life-2-background.jpg",
    "legacy/screenshots/half-life-2-ghosting-background.jpg",
    "legacy/customization/Standard.jpg",
    "legacy/customization/CF.jpg",
    "legacy/customization/BM.jpg",
    "legacy/customization/ST.jpg",
    "legacy/customization/XEN.jpg",
    "legacy/customization/XEN_2.jpg",
    "legacy/customization/crosshairs/Standard.jpg",
    "legacy/customization/crosshairs/Enchanced.jpg",
    "legacy/customization/crosshairs/Dot.jpg",
    "legacy/customization/crosshairs/Counter-Strike.jpg",
    "legacy/customization/crosshairs/Green.jpg",
    "legacy/customization/hud/1.jpg",
    "legacy/customization/hud/2.jpg",
    "legacy/customization/sprites/1.jpg",
    "legacy/customization/sprites/2.jpg",
    "legacy/customization/icon-checkbox.png",
];

/// Разобранные картинки 0.2.9.
pub type Images = Vec<(&'static str, Arc<RenderImage>)>;

/// Разобрать все картинки 0.2.9 (в фоне: тяжело). Делит работу на потоки — пакет готов за
/// время самой большой картинки, а не всех подряд.
pub fn decode_images() -> Images {
    let decode = |path: &'static str| -> Option<(&'static str, Arc<RenderImage>)> {
        let bytes = crate::assets::Assets.load(path).ok().flatten()?;
        let decoded = hlsr_ui::images::decode(&bytes, None, hlsr_ui::images::Fit::Contain).ok()?;
        Some((path, crate::image::render_image(&decoded.pixels)))
    };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(2, 8));
    let chunks: Vec<Vec<&'static str>> = (0..threads)
        .map(|i| IMAGES.iter().copied().skip(i).step_by(threads).collect())
        .collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                scope.spawn(move || chunk.iter().filter_map(|p| decode(p)).collect::<Vec<_>>())
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

/// Картинка из разобранных заранее; нет её — по пути (GPUI загрузит сам).
fn pic(images: &Rc<HashMap<&'static str, Arc<RenderImage>>>, path: &'static str) -> Img {
    match images.get(path) {
        Some(image) => img(image.clone()),
        None => img(path),
    }
}

/// Смена страницы (`<transition name="slide" mode="out-in">`): прежняя уходит вверх и гаснет
/// за .22 с, новая въезжает снизу за .22 с.
const SLIDE_MS: f64 = 220.;

/// Страница (`router/index.js`).
#[derive(Clone, PartialEq, Eq)]
enum Page {
    Home,
    Game(&'static str),
    Constructor,
    Editor,
    Customization,
    Settings,
}

/// Открытое окно (`md-dialog`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialog {
    Changelog,
    About,
}

/// Оболочка 0.2.9.
pub struct LegacyShell {
    page: Page,
    games_open: bool,
    configs_open: bool,
    gear_open: bool,
    /// Раздел меню игры (`selectedSection`): 0 — новости, 1 — параметры, 2 — сплиты.
    section: u8,
    theme: usize,
    language_open: bool,
    theme_open: bool,
    rpc: bool,
    no_particles: bool,
    dialog: Option<Dialog>,
    /// Логотип перекрашен двойным щелчком (`changeColor`), через 1,5 с снова белый.
    logo_color: Option<Rgba>,
    /// Последний цвет логотипа: к нему и от него цвет перетекает (`.5s color`).
    logo_last: Rgba,
    /// Уходящая страница и начало перехода.
    leaving: Option<(Page, Instant)>,
    /// Картинки, разобранные до показа.
    images: Rc<HashMap<&'static str, Arc<RenderImage>>>,
    logo_seed: u32,
    logo_reset: Option<gpui::Task<()>>,
    /// Кастомизация: игра, фон меню, прицел, HUD, спрайты, скайбоксы.
    custom_game: usize,
    custom_background: usize,
    custom_aim: usize,
    custom_hud: usize,
    custom_sprites: usize,
    custom_hd_sky: bool,
    /// Конструктор: выбранные скрипты.
    scripts: Vec<bool>,
}

/// Строка интерфейса 0.2.9 по языку окна.
fn t(lang: Language, ru: &'static str, en: &'static str) -> &'static str {
    match lang {
        Language::Ru => ru,
        _ => en,
    }
}

fn rgb_a(hex: u32, a: f32) -> Hsla {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.,
        g: ((hex >> 8) & 0xff) as f32 / 255.,
        b: (hex & 0xff) as f32 / 255.,
        a,
    }
    .into()
}

fn white_a(a: f32) -> Hsla {
    rgb_a(0xffffff, a)
}

fn black_a(a: f32) -> Hsla {
    rgb_a(0x000000, a)
}

/// Масштаб CSS px 0.2.9 → px окна; второе — зум окна (rem / 16).
#[derive(Clone, Copy)]
struct K(f32, f32);

impl K {
    fn of(window: &Window) -> K {
        let size = window.viewport_size();
        let k = (f32::from(size.width) / BASE_W).min(f32::from(size.height) / BASE_H);
        K(k.max(0.25), crate::scale::zoom(window).max(0.01))
    }

    /// `fa_glyph(_, size)` берёт CSS px нового интерфейса (rem окна): множитель для значка
    /// размером `size` CSS px 0.2.9.
    fn icon(self) -> f32 {
        self.0 / self.1
    }

    fn p(self, css: f32) -> Pixels {
        px(css * self.0)
    }
}

const SCRIPTS_SCRIPTLESS: [&str; 2] = ["Basic Binds", "FPS Control"];
const SCRIPTS_SCRIPTED: [&str; 8] = [
    "Auto Bhop",
    "Lambda Complex Boost",
    "Gauss Boost 180°",
    "Water Boosting",
    "Nihilanth Boost",
    "On A Rail Object Boosting",
    "Surface Tension Trigger Delay",
    "Test Chamber Skip",
];
const SEQUENCES: [&str; 4] = [
    "Lambda Complex Boost",
    "Nihilanth Boost",
    "Test Chamber Skip",
    "Surface Tension Trigger Delay",
];

impl LegacyShell {
    pub fn new(images: Images) -> LegacyShell {
        LegacyShell {
            logo_last: Rgba {
                r: 1.,
                g: 1.,
                b: 1.,
                a: 1.,
            },
            leaving: None,
            images: Rc::new(images.into_iter().collect()),
            page: Page::Home,
            games_open: false,
            configs_open: false,
            gear_open: false,
            section: 0,
            theme: 0,
            language_open: false,
            theme_open: false,
            rpc: true,
            no_particles: false,
            dialog: None,
            logo_color: None,
            logo_seed: 0x2f6e_2b1d,
            logo_reset: None,
            custom_game: 0,
            custom_background: 0,
            custom_aim: 0,
            custom_hud: 0,
            custom_sprites: 0,
            custom_hd_sky: false,
            scripts: vec![false; SCRIPTS_SCRIPTLESS.len() + SCRIPTS_SCRIPTED.len()],
        }
    }

    /// Отладочная сборка: страница из `HLSR_CE_LEGACY` (`game-70`, `settings`, `custom`,
    /// `editor`, `constructor`) — для снимков.
    pub fn with_debug_page(mut self) -> LegacyShell {
        if !cfg!(debug_assertions) {
            return self;
        }
        let page = std::env::var("HLSR_CE_LEGACY").unwrap_or_default();
        self.page = match page.as_str() {
            "settings" => Page::Settings,
            "custom" => Page::Customization,
            "editor" => Page::Editor,
            "constructor" => Page::Constructor,
            other => match other.strip_prefix("game-") {
                Some(id) => Page::Game(game(id).id),
                None => Page::Home,
            },
        };
        self
    }

    fn close_menus(&mut self) {
        self.games_open = false;
        self.configs_open = false;
        self.gear_open = false;
        self.language_open = false;
        self.theme_open = false;
    }

    /// `navigateTo(path, query)`: игра доступна новому лаунчеру — она же выбирается в нём
    /// (кнопка «Играть» берёт выбранную игру сценария).
    fn navigate(&mut self, page: Page, cx: &mut Context<Self>) {
        self.close_menus();
        if let Page::Game(id) = &page {
            if self.page != page {
                self.section = 0;
            }
            if hlsr_ui::controllers::game_availability::is_available(id)
                && let Some(context) = ShellContext::get(cx)
                && context.flows.with_state(|s| s.selected_game_id.clone()) != *id
            {
                context.nav.select_game(id);
            }
        }
        if self.page != page && !AnimGate::still(cx).any() {
            let old = std::mem::replace(&mut self.page, page);
            self.leaving = Some((old, Instant::now()));
        } else {
            self.page = page;
        }
        cx.notify();
    }

    fn toast(cx: &App, text: &str, icon: &str) {
        if let Some(context) = ShellContext::get(cx) {
            context.shell.notify(text, icon);
        }
    }

    fn not_here(lang: Language, cx: &App) {
        Self::toast(
            cx,
            t(
                lang,
                "В Legacy-режиме это только для вида. Наберите LEGACY, чтобы вернуться в новый интерфейс.",
                "In Legacy mode this is for looks only. Type LEGACY to return to the new interface.",
            ),
            "fas fa-info-circle",
        );
    }

    /// `changeColor()`: случайный цвет логотипа, через 1,5 с — снова белый.
    fn change_logo_color(&mut self, cx: &mut Context<Self>) {
        // xorshift: случайный цвет без зависимостей.
        let mut next = || {
            self.logo_seed ^= self.logo_seed << 13;
            self.logo_seed ^= self.logo_seed >> 17;
            self.logo_seed ^= self.logo_seed << 5;
            (self.logo_seed % 250) as f32 / 255.
        };
        let color = Rgba {
            r: next(),
            g: next(),
            b: next(),
            a: 1.,
        };
        self.logo_color = Some(color);
        self.logo_last = color;
        self.logo_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |this, cx| {
                this.logo_color = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl LegacyShell {
    /// Картинки — освободить при выходе из Legacy (`cx.drop_image`).
    pub fn images(&self) -> Vec<Arc<RenderImage>> {
        self.images.values().cloned().collect()
    }
}

/// Что берётся у нового лаунчера на кадр.
struct Snap {
    lang: Language,
    /// Статус доставки по играм (`DeliveryStatus`).
    delivery: serde_json::Map<String, Value>,
    library: Value,
    /// Игра, выбранная в новом лаунчере (`selectedGameId`).
    selected: String,
    version: &'static str,
}

impl Snap {
    fn of(cx: &App) -> Snap {
        let Some(context) = ShellContext::get(cx) else {
            return Snap {
                lang: Language::En,
                delivery: Default::default(),
                library: Value::Null,
                selected: String::new(),
                version: env!("CARGO_PKG_VERSION"),
            };
        };
        let delivery = context.flows.with_state(|s| {
            s.delivery
                .iter()
                .map(|(k, v)| (k.to_owned(), v.clone()))
                .collect()
        });
        let library = match context.services.store_read("library") {
            StoreRead::Found(library) => library,
            _ => Value::Null,
        };
        Snap {
            lang: context.shell.language(),
            delivery,
            library,
            selected: context.flows.with_state(|s| s.selected_game_id.clone()),
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    /// Главная кнопка игры по статусу доставки нового лаунчера.
    fn button(&self, id: &str) -> Option<hlsr_ui::controllers::download_bar::ButtonView> {
        if !hlsr_ui::controllers::game_availability::is_available(id) {
            return None;
        }
        let delivery = self.delivery.get(id).filter(|v| !v.is_null());
        Some(button_view(self.lang, delivery, None))
    }

    /// `GameControl.checkInstalled`.
    fn installed(&self, id: &str) -> bool {
        self.button(id).is_some_and(|b| {
            !matches!(
                b.state.as_str(),
                "not-installed" | "unavailable" | "episode-not-installed"
            ) && self.delivery.get(id).is_some_and(|v| !v.is_null())
        })
    }

    fn running(&self, id: &str) -> bool {
        self.button(id).is_some_and(|b| {
            matches!(
                b.state.as_str(),
                "game-running" | "starting-game" | "stopping-game"
            )
        })
    }

    /// `getPlayTime(gameid)`: часы в игре с одним знаком после запятой.
    fn play_time(&self, id: &str) -> String {
        let ms = self
            .library
            .pointer(&format!("/config/{id}/inGameTime"))
            .and_then(Value::as_f64)
            .unwrap_or(0.);
        if ms <= 0. {
            return "0".into();
        }
        format!("{:.1}", ms / 3_600_000.).replace('.', ",")
    }

    /// `store.get("lastLaunched")`; пусто (новый лаунчер его не ведёт) — игра, выбранная в нём.
    fn last_launched(&self) -> Option<&'static str> {
        let id = match self.library.get("lastLaunched") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => self.selected.clone(),
        };
        GAMES.iter().find(|g| g.id == id).map(|g| g.id)
    }
}

type Listener = Rc<dyn Fn(&mut LegacyShell, &mut Window, &mut Context<LegacyShell>)>;

/// Щелчок по элементу → метод оболочки.
fn on(
    cx: &Context<LegacyShell>,
    f: impl Fn(&mut LegacyShell, &mut Window, &mut Context<LegacyShell>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let f: Listener = Rc::new(f);
    cx.listener(move |this, _: &ClickEvent, window, cx| f(this, window, cx))
}

fn text(k: K, size: f32, color: Hsla, value: impl Into<SharedString>) -> Div {
    div()
        .text_size(k.p(size))
        .line_height(k.p(size * 1.2))
        .text_color(color)
        .child(value.into())
}

fn window_command(name: &'static str, cx: &mut App) {
    let Some(services) = ShellContext::get(cx).map(|c| c.services.clone()) else {
        return;
    };
    let reply = services.invoke("window-command", vec![json!(name)]);
    cx.background_spawn(async move {
        let _ = reply.await;
    })
    .detach();
}

impl Render for LegacyShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = K::of(window);
        let snap = Snap::of(cx);
        let any_menu = self.games_open || self.configs_open || self.gear_open;
        // Переход: первая половина — уходящая страница вверх, вторая — новая снизу.
        let mut shown = self.page.clone();
        let mut shift = 0.;
        let mut fade = 1.;
        if let Some((old, since)) = &self.leaving {
            let ms = since.elapsed().as_secs_f64() * 1000.;
            let ease = Timing::new(SLIDE_MS, EASE);
            if ms < SLIDE_MS {
                let t = ease.at(ms);
                shown = old.clone();
                shift = -t;
                fade = 1. - t;
                monitor_frame(window);
            } else if ms < 2. * SLIDE_MS {
                shift = 1. - ease.at(ms - SLIDE_MS);
                monitor_frame(window);
            } else {
                self.leaving = None;
            }
        }
        let game_page = matches!(shown, Page::Game(_));
        let area_h = f32::from(window.viewport_size().height) - 32. * k.0;
        let dy = shift * area_h;

        let page = match shown {
            Page::Home => self.home(k, &snap, cx),
            Page::Game(id) => self.game_menu(k, &snap, id, cx),
            Page::Constructor => self.constructor(k, &snap, cx),
            Page::Editor => self.editor(k, &snap, cx),
            Page::Customization => self.customization(k, &snap, cx),
            Page::Settings => self.settings(k, &snap, cx),
        };

        div()
            .id("legacy-root")
            .size_full()
            .relative()
            .overflow_hidden()
            .font_family(RUBIK)
            .text_color(rgb_a(0xd2d2d2, 1.))
            .bg(rgb_a(0x172e56, 1.))
            // #backgroundtheme: картинка темы `cover` под rgba(23, 46, 86, .7).
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(
                        pic(&self.images, THEMES[self.theme])
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                    .child(div().absolute().inset_0().bg(rgb_a(0x172e56, 0.7)))
                    // .sidebar-root::before: размытая подложка колонки во всю высоту окна.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(k.p(90.))
                            .bg(black_a(0.08)),
                    ),
            )
            // #app: рамка 32 px, под ней колонка и страница (`#app__container`). В меню игры
            // страница уходит под рамку (`top: -32px`).
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .child(div().h(k.p(32.)).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .relative()
                            .child(self.sidebar(k, &snap, cx))
                            .child(
                                div().flex_1().min_w_0().h_full().relative().child(
                                    // Меню игры уходит под рамку (`#form { top: -32px }`).
                                    div()
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .bottom(px(-dy))
                                        .top(k.p(if game_page { -32. } else { 0. }) + px(dy))
                                        .opacity(fade)
                                        .child(page),
                                ),
                            ),
                    ),
            )
            .child(self.frame(k, cx))
            // Щелчок мимо открытого списка его закрывает (`clickHandle`).
            .when(any_menu, |el| {
                el.child(
                    div()
                        .id("legacy-menu-catcher")
                        .absolute()
                        .inset_0()
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.close_menus();
                            cx.notify();
                        })),
                )
            })
            .children(
                self.dialog
                    .map(|dialog| self.dialog_view(k, &snap, dialog, cx)),
            )
    }
}

// ---- Рамка и колонка -----------------------------------------------------------------------

impl LegacyShell {
    /// `Frame.vue`: полоса 32 px, кнопки 48 × 32 справа.
    fn frame(&self, k: K, _cx: &mut Context<Self>) -> AnyElement {
        let button = |id: &'static str, icon: &'static str, red: bool, cmd: &'static str| {
            div()
                .id(id)
                .w(k.p(48.))
                .h(k.p(32.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(white_a(0.4))
                .hover(move |s| {
                    s.bg(if red {
                        rgb_a(0xff004c, 1.)
                    } else {
                        white_a(0.1)
                    })
                    .text_color(white_a(1.))
                })
                .on_click(move |_, _, cx| window_command(cmd, cx))
                .child(fa_glyph(icon, 15. * k.icon()))
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(k.p(32.))
            .flex()
            .justify_end()
            .child(
                div()
                    .id("legacy-drag")
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(button("legacy-min", "fas fa-minus", false, "minimize"))
            .child(button("legacy-close", "fas fa-times", true, "close"))
            .into_any_element()
    }

    /// `SideBar.vue`: 90 px; логотип, четыре кнопки посередине, Steam, настройки и версия внизу.
    fn sidebar(&self, k: K, snap: &Snap, cx: &mut Context<Self>) -> AnyElement {
        let lang = snap.lang;
        let page = self.page.clone();

        // .top .logo: 48 × 48, рамка 4 px; .inner 20 px, наведение — 26 px (`.2s`); цвет
        // перетекает за `.5s` (`transition: .5s color`).
        let colored = self.logo_color.is_some();
        let shown: Hsla = self.logo_last.into();
        let logo_click = cx.listener(|this, event: &ClickEvent, _, cx| {
            if event.click_count() == 2 {
                this.change_logo_color(cx);
            }
        });
        let logo = tween(
            "legacy-logo-color",
            if colored { 1. } else { 0. },
            Timing::new(500., EASE),
            move |c| {
                let color = mix(white_a(1.), shown, c);
                hover("legacy-logo-hover", Timing::new(200., EASE), move |h| {
                    div()
                        .id("legacy-logo")
                        .size(k.p(48.))
                        .rounded_full()
                        .border(k.p(4.))
                        .border_color(color)
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_click(logo_click)
                        .child(div().size(k.p(20. + 6. * h)).rounded_full().bg(color))
                })
            },
        );

        // Кнопка колонки: цвет .15 → .3 при наведении (`transition: .1s`), открытый список — .5.
        let sbutton =
            |id: &'static str, icon: &'static str, active: bool, pre: bool, click: Click| {
                hover(
                    SharedString::from(format!("{id}-hover")),
                    Timing::new(100., EASE),
                    move |h| {
                        let base = div()
                            .id(id)
                            .size(k.p(50.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(click);
                        if active {
                            base.rounded(k.p(8.))
                                .bg(rgb_a(ACCENT, 0.15))
                                .border(k.p(2.))
                                .border_color(rgb_a(ACCENT, 1.))
                                .text_color(white_a(1.))
                                .child(fa_glyph(icon, 19. * k.icon()))
                        } else {
                            let rest = if pre { 0.5 } else { 0.15 };
                            let over = if pre { 0.5 } else { 0.3 };
                            base.text_color(white_a(rest + (over - rest) * h))
                                .child(fa_glyph(icon, 19. * k.icon()))
                        }
                    },
                )
                .into_any_element()
            };

        let games_items: Vec<AnyElement> = GAMES
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let selected = page == Page::Game(g.id);
                let id = g.id;
                let time = match lang {
                    Language::Ru => format!("{} ч.", snap.play_time(g.id)),
                    _ => format!("{} h.", snap.play_time(g.id)),
                };
                let lambda = g.lambda;
                let name = g.name;
                let click: Option<Click> = (!selected).then(|| -> Click {
                    Box::new(on(cx, move |this, _, cx| this.navigate(Page::Game(id), cx)))
                });
                popup_item(
                    k,
                    SharedString::from(format!("legacy-game-{id}")),
                    i + 1 == GAMES.len(),
                    selected,
                    click,
                    move |el, h| {
                        let lambda_a = if selected { 1. } else { 0.5 + 0.1 * h };
                        el.child(
                            svg()
                                .path("legacy/lambda.svg")
                                .size(k.p(14.))
                                .mr(k.p(12.))
                                .flex_none()
                                .text_color(rgb_a(lambda, lambda_a)),
                        )
                        .child(name)
                        .child(
                            div()
                                .ml_auto()
                                .pl(k.p(20.))
                                .text_size(k.p(12.8))
                                .font_weight(FontWeight::THIN)
                                .text_color(white_a(0.15))
                                .child(time),
                        )
                    },
                )
            })
            .collect();
        let games_list = popup(k, "legacy-games-list", self.games_open, games_items);

        let configs_items = vec![
            popup_item(
                k,
                "legacy-cfg-constructor".into(),
                false,
                page == Page::Constructor,
                Some(Box::new(on(cx, |this, _, cx| {
                    this.navigate(Page::Constructor, cx)
                }))),
                move |el, _| el.child(t(lang, "Конструктор", "Constructor")),
            ),
            popup_item(
                k,
                "legacy-cfg-editor".into(),
                true,
                page == Page::Editor,
                Some(Box::new(on(cx, |this, _, cx| {
                    this.navigate(Page::Editor, cx)
                }))),
                move |el, _| el.child(t(lang, "Редактор", "Editor")),
            ),
        ];
        let configs_list = popup(k, "legacy-configs-list", self.configs_open, configs_items);

        let slot = |child: AnyElement, list: Option<AnyElement>| {
            div()
                .relative()
                .h(k.p(70.))
                .flex()
                .items_center()
                .justify_center()
                .child(child)
                .children(list.map(|list| {
                    deferred(
                        div()
                            .absolute()
                            // `.items { left: 76px }` от кнопки, а она — в 20 px от края колонки.
                            .left(k.p(96.))
                            .top(k.p(35.))
                            .child(list),
                    )
                    .with_priority(2)
                }))
        };

        let is_game = matches!(page, Page::Game(_));
        let buttons = div()
            .w(k.p(50.))
            .flex()
            .flex_col()
            .child(slot(
                sbutton(
                    "legacy-home",
                    "fas fa-home",
                    page == Page::Home,
                    false,
                    Box::new(on(cx, |this, _, cx| this.navigate(Page::Home, cx))),
                ),
                None,
            ))
            .child(slot(
                sbutton(
                    "legacy-games",
                    "fas fa-play",
                    is_game,
                    self.games_open,
                    Box::new(on(cx, |this, _, cx| {
                        let open = !this.games_open;
                        this.close_menus();
                        this.games_open = open;
                        cx.notify();
                    })),
                ),
                Some(games_list),
            ))
            .child(slot(
                sbutton(
                    "legacy-configs",
                    "fas fa-align-left",
                    matches!(page, Page::Constructor | Page::Editor),
                    self.configs_open,
                    Box::new(on(cx, |this, _, cx| {
                        let open = !this.configs_open;
                        this.close_menus();
                        this.configs_open = open;
                        cx.notify();
                    })),
                ),
                Some(configs_list),
            ))
            .child(slot(
                sbutton(
                    "legacy-custom",
                    "fas fa-sliders-h",
                    page == Page::Customization,
                    false,
                    Box::new(on(cx, |this, _, cx| this.navigate(Page::Customization, cx))),
                ),
                None,
            ));

        let settings_click: Click =
            Box::new(on(cx, |this, _, cx| this.navigate(Page::Settings, cx)));
        let bottom = div()
            .absolute()
            .left_0()
            .bottom_0()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .id("legacy-steam")
                    .mb(k.p(18.))
                    .h(k.p(24.))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        LegacyShell::toast(
                            cx,
                            t(
                                lang,
                                "Steamworks в новом лаунчере не используется",
                                "The new launcher does not use Steamworks",
                            ),
                            "fab fa-steam",
                        )
                    })
                    .child(
                        svg()
                            .path("legacy/steam.svg")
                            .size(k.p(24.))
                            .text_color(white_a(0.2)),
                    ),
            )
            .child(hover(
                "legacy-settings-hover",
                Timing::new(100., EASE),
                move |h| {
                    div()
                        .id("legacy-settings")
                        .size(k.p(46.))
                        .mb(k.p(16.))
                        .rounded(k.p(8.))
                        .bg(mix(rgb_a(ACCENT, 1.), rgb_a(0x1fb4ff, 1.), h))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(white_a(1.))
                        .cursor_pointer()
                        .on_click(settings_click)
                        .child(fa_glyph("fas fa-cog", 19. * k.icon()))
                },
            ))
            .child(
                div()
                    .mr(k.p(24.))
                    .mb(k.p(4.))
                    .text_size(k.p(10.))
                    .line_height(k.p(14.))
                    .text_color(white_a(0.1))
                    .child(format!("v{}", snap.version))
                    .child("CE BUILD"),
            );

        div()
            .id("legacy-sidebar")
            .relative()
            .w(k.p(90.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .child(logo)
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .pb(k.p(64.))
                    .child(buttons),
            )
            .child(bottom)
            .into_any_element()
    }
}

/// Обработчик щелчка, построенный заранее (его забирает элемент внутри перехода).
type Click = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// CSS `ease` — кривая переходов 0.2.9 по умолчанию.
const EASE: [f64; 4] = [0.25, 0.1, 0.25, 1.];

/// Наведение с переходом `ms` (CSS `transition`): `build(h)` — элемент при доле наведения `h`.
fn hover_fx<E: IntoElement>(
    id: impl Into<SharedString>,
    ms: f64,
    build: impl FnOnce(f32) -> E + 'static,
) -> AnyElement {
    let id: SharedString = id.into();
    hover(
        SharedString::from(format!("{id}-hover")),
        Timing::new(ms, EASE),
        build,
    )
    .into_any_element()
}

/// `.items`: тёмный список справа от кнопки колонки; появляется и гаснет за .15 с
/// (`transition: .15s ease opacity`), закрытый щелчков не ловит (`pointer-events: none`).
fn popup(k: K, id: &'static str, open: bool, items: Vec<AnyElement>) -> AnyElement {
    let height = items.len() as f32 * 44.;
    tween(
        id,
        if open { 1. } else { 0. },
        Timing::new(150., EASE),
        move |t| {
            if t <= 0. {
                return div();
            }
            div()
                .mt(k.p(-height / 2.))
                .opacity(t)
                .flex()
                .flex_col()
                .bg(rgb_a(0x141414, 0.97))
                .border(k.p(2.))
                .border_color(rgb_a(0x2a2a2a, 1.))
                .rounded(k.p(6.))
                .overflow_hidden()
                .text_size(k.p(14.4))
                .font_weight(FontWeight::MEDIUM)
                .children(items)
                .when(!open, |el| el.occlude())
        },
    )
    .into_any_element()
}

/// `.item`: строка списка; цвет .5 → .65 при наведении (`transition: .05s`).
fn popup_item(
    k: K,
    id: SharedString,
    last: bool,
    selected: bool,
    click: Option<Click>,
    content: impl FnOnce(Stateful<Div>, f32) -> Stateful<Div> + 'static,
) -> AnyElement {
    let hover_id = SharedString::from(format!("{id}-hover"));
    hover(hover_id, Timing::new(50., EASE), move |h| {
        let el = div()
            .id(id)
            .h(k.p(44.))
            .px(k.p(14.))
            .flex()
            .items_center()
            .whitespace_nowrap()
            .cursor_pointer()
            .text_color(if selected {
                rgb_a(ACCENT, 1.)
            } else {
                white_a(0.5 + 0.15 * h)
            })
            .when(!last, |el| el.border_b(k.p(2.)).border_color(white_a(0.03)));
        let el = match click {
            Some(click) => el.on_click(click),
            None => el,
        };
        content(el, h)
    })
    .into_any_element()
}

// ---- Общие элементы страниц ----------------------------------------------------------------

/// `#form`: поля 32 px, заголовок 36 px / 900 акцентом.
fn form(k: K, title: &str) -> Div {
    div().size_full().p(k.p(32.)).flex().flex_col().child(
        div()
            .flex()
            .items_center()
            .text_size(k.p(36.))
            .line_height(k.p(43.))
            .font_weight(FontWeight::BLACK)
            .text_color(rgb_a(ACCENT, 1.))
            .child(title.to_uppercase()),
    )
}

/// `.element`: строка настроек.
fn element(k: K) -> Div {
    div()
        .p(k.p(12.))
        .flex()
        .items_center()
        .bg(black_a(0.2))
        .rounded(k.p(4.))
        .mb(k.p(8.))
        .text_color(rgb_a(0xd2d2d2, 1.))
}

/// `Button.vue` (ButtonAlt): 52 px, акцент или красная, текст 13 px / 700, значок справа.
fn alt_button(
    k: K,
    id: &'static str,
    label: &str,
    icon: &'static str,
    red: bool,
    disabled: bool,
) -> Stateful<Div> {
    let (bg, hover) = if red {
        (rgb_a(0xff576e, 1.), rgb_a(0xff6e81, 1.))
    } else {
        (rgb_a(ACCENT, 1.), rgb_a(0x2db9ff, 1.))
    };
    div()
        .id(id)
        .w(k.p(185.))
        .h(k.p(52.))
        .px(k.p(25.))
        .flex()
        .items_center()
        .rounded(k.p(2.))
        .bg(bg)
        .text_color(white_a(1.))
        .font_weight(FontWeight::BOLD)
        .text_size(k.p(13.))
        .when(!disabled, |el| {
            el.cursor_pointer().hover(move |s| s.bg(hover))
        })
        .when(disabled, |el| el.opacity(0.5))
        .child(label.to_uppercase())
        .child(div().ml_auto().child(fa_glyph(icon, 18. * k.icon())))
}

/// `Checkbox.vue`.
fn checkbox(k: K, id: &'static str, label: &str, on: bool) -> Stateful<Div> {
    element(k)
        .id(id)
        .cursor_pointer()
        .hover(|s| s.bg(black_a(0.28)))
        .child(
            div()
                .size(k.p(30.))
                .rounded(k.p(4.))
                .bg(black_a(0.3))
                .flex()
                .items_center()
                .justify_center()
                .when(on, |el| el.child(fa_glyph("fas fa-check", 14. * k.icon()))),
        )
        .child(
            div()
                .ml(k.p(12.))
                .text_size(k.p(15.))
                .child(label.to_owned()),
        )
}

fn mini_title(k: K, value: &str) -> Div {
    div()
        .text_size(k.p(18.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(white_a(0.2))
        .child(value.to_owned())
}

fn warning(k: K, value: &str) -> Div {
    div()
        .p(k.p(24.))
        .flex()
        .flex_col()
        .items_center()
        .text_color(rgb_a(0xffd000, 0.6))
        .border(k.p(2.))
        .border_color(rgb_a(0xffd000, 0.4))
        .bg(rgb_a(0xffd000, 0.045))
        .rounded(k.p(16.))
        .opacity(0.7)
        .child(
            div()
                .mb(k.p(8.))
                .child(fa_glyph("fas fa-exclamation-triangle", 25. * k.icon())),
        )
        .child(
            div()
                .text_size(k.p(12.2))
                .line_height(k.p(16.))
                .child(value.to_owned()),
        )
}

// ---- Главная -------------------------------------------------------------------------------

impl LegacyShell {
    fn home(&self, k: K, snap: &Snap, cx: &mut Context<Self>) -> AnyElement {
        let lang = snap.lang;
        let last = snap.last_launched();
        let installed = last.is_some_and(|id| snap.installed(id));
        let name = match last {
            Some(id) if installed => game(id).name.to_uppercase(),
            _ => t(lang, "Нет игры", "No game").to_uppercase(),
        };
        let gray = last.is_some_and(|id| snap.running(id));
        let box_ = || {
            div()
                .bg(black_a(0.16))
                .rounded(k.p(8.))
                .p(k.p(16.))
                .flex()
                .flex_col()
                .overflow_hidden()
        };

        let quick_click: Click = Box::new(on(cx, move |_, _, cx| {
            let Some(id) = last else { return };
            let snap = Snap::of(cx);
            if !snap.installed(id) || snap.running(id) {
                return;
            }
            if let Some(context) = ShellContext::get(cx).cloned() {
                if context.flows.with_state(|s| s.selected_game_id.clone()) != id {
                    context.nav.select_game(id);
                }
                context.flows.on_play();
            }
        }));
        let cog_click: Click = Box::new(on(cx, move |this, _, cx| {
            cx.stop_propagation();
            if let Some(id) = last.filter(|_| installed) {
                this.navigate(Page::Game(id), cx);
                this.section = 1;
            }
        }));
        let quick = box_()
            .flex_none()
            .h(k.p(118.))
            .child(mini_title(k, t(lang, "Быстрый запуск", "Quick Start")))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .mt(k.p(16.))
                    .child(hover_fx("legacy-quick", 50., move |h| {
                        div()
                            .id("legacy-quick")
                            .w_full()
                            .h(k.p(47.))
                            .rounded(k.p(4.))
                            .bg(white_a(0.06 + 0.06 * h))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .font_weight(FontWeight::MEDIUM)
                            .on_click(quick_click)
                            .child(
                                div()
                                    .ml(k.p(16.))
                                    .text_size(k.p(16.))
                                    .text_color(mix(
                                        white_a(if gray { 0.4 } else { 0.6 }),
                                        white_a(1.),
                                        if gray { 0. } else { h },
                                    ))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .id("legacy-quick-cog")
                                    .ml_auto()
                                    .size(k.p(40.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(white_a(if installed { 0.7 } else { 0.25 }))
                                    .when(installed, |el| el.hover(|s| s.bg(white_a(0.08))))
                                    .on_click(cog_click)
                                    .child(fa_glyph("fas fa-cog", 18. * k.icon())),
                            )
                    })),
            );

        let friends = box_()
            .flex_1()
            .min_h_0()
            .p_0()
            .child(
                div()
                    .m(k.p(16.))
                    .child(mini_title(k, t(lang, "Друзья в стиме", "Steam Friends"))),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(k.p(16.))
                    .pb(k.p(16.))
                    .child(warning(
                        k,
                        t(lang, "Запустите Steam", "Start Steam to play this game"),
                    )),
            );

        let soundcloud = box_()
            .flex_1()
            .h_full()
            .child(
                div()
                    .id("legacy-sc-title")
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.4))
                    .on_click(move |_, _, cx| LegacyShell::not_here(lang, cx))
                    .child(mini_title(
                        k,
                        t(lang, "SoundCloud Виджет", "SoundCloud Widget"),
                    ))
                    .child(
                        div()
                            .ml_auto()
                            .text_color(white_a(0.2))
                            .child(fa_glyph("fas fa-music", 16. * k.icon())),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(white_a(0.4))
                    .child(fa_glyph("fas fa-spinner", 28. * k.icon())),
            );

        form(k, t(lang, "Главная", "Home"))
            .child(
                div().absolute().top(k.p(32.)).right(k.p(32.)).child(
                    div()
                        .id("legacy-changelog")
                        .size(k.p(40.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb_a(ACCENT, 1.))
                        .hover(|s| s.bg(white_a(0.08)))
                        .on_click(on(cx, |this, _, cx| {
                            this.dialog = Some(Dialog::Changelog);
                            cx.notify();
                        }))
                        .child(fa_glyph("fas fa-clipboard-list", 20. * k.icon())),
                ),
            )
            .child(
                div()
                    .mt(k.p(24.))
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(k.p(24.))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(k.p(24.))
                            .child(quick)
                            .child(friends),
                    )
                    .child(div().flex_1().flex().child(soundcloud)),
            )
            .relative()
            .into_any_element()
    }
}

// ---- Меню игры -----------------------------------------------------------------------------

impl LegacyShell {
    fn game_menu(&self, k: K, snap: &Snap, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let lang = snap.lang;
        let g = game(id);
        let available = hlsr_ui::controllers::game_availability::is_available(id);
        let installed = snap.installed(id);
        let button = snap.button(id);
        let state = button.as_ref().map(|b| b.state.clone()).unwrap_or_default();
        let (label, icon, disabled) = match state.as_str() {
            "ready" => (t(lang, "Играть", "Play").to_owned(), "fas fa-play", false),
            "not-installed" => (
                t(lang, "Установить", "Install").to_owned(),
                "fas fa-download",
                false,
            ),
            "game-running" | "starting-game" | "stopping-game" => (
                t(lang, "Запущено", "Started").to_owned(),
                "fas fa-play",
                true,
            ),
            "" => (
                t(lang, "Установить", "Install").to_owned(),
                "fas fa-download",
                true,
            ),
            _ => {
                let b = button.as_ref().expect("состояние есть — есть и кнопка");
                (b.label.clone(), b.icon, !b.enabled)
            }
        };
        let action = button.as_ref().and_then(|b| b.pressed().map(str::to_owned));

        // .md-raised: 200 × 48, белая .22 или акцентная у установленной.
        let main_click = move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            if disabled {
                return;
            }
            let Some(action) = action.as_deref() else {
                return;
            };
            if let Some(context) = ShellContext::get(cx).cloned() {
                if context.flows.with_state(|s| s.selected_game_id.clone()) != id {
                    context.nav.select_game(id);
                }
                crate::games::download_bar::press(action, &context.flows);
            }
        };
        let main_button = hover(
            "legacy-game-button-hover",
            Timing::new(400., [0.4, 0., 0.2, 1.]),
            move |h| {
                let h = if disabled { 0. } else { h };
                div()
                    .id("legacy-game-button")
                    .w(k.p(200.))
                    .h(k.p(48.))
                    .rounded(k.p(2.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(k.p(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .bg(if installed {
                        rgb_a(ACCENT, if disabled { 0.5 } else { 0.78 + 0.12 * h })
                    } else {
                        white_a(0.22 + 0.11 * h)
                    })
                    .text_color(if installed {
                        white_a(1.)
                    } else {
                        rgb_a(0xd2d2d2, if disabled { 0.5 } else { 1. })
                    })
                    .when(!disabled, |el| el.cursor_pointer())
                    .on_click(main_click)
                    .child(div().mr(k.p(8.)).child(fa_glyph(icon, 13. * k.icon())))
                    .child(label.to_uppercase())
            },
        );

        let gear_menu = self.gear_open.then(|| {
            let item = |id: &'static str,
                        label: &str,
                        icon: &'static str,
                        red: bool,
                        action: &'static str| {
                div()
                    .id(id)
                    .h(k.p(48.))
                    .px(k.p(16.))
                    .flex()
                    .items_center()
                    .text_size(k.p(16.))
                    .text_color(if red {
                        rgb_a(0xff5252, 1.)
                    } else {
                        white_a(0.87)
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(white_a(0.08)))
                    .on_click(on(cx, move |this, _, cx| {
                        this.close_menus();
                        cx.notify();
                        if let Some(context) = ShellContext::get(cx) {
                            context.flows.on_menu_action(action);
                        }
                    }))
                    .child(label.to_owned())
                    .child(div().ml_auto().child(fa_glyph(icon, 16. * k.icon())))
            };
            deferred(
                div()
                    .absolute()
                    .top(k.p(51.))
                    .left_0()
                    .w(k.p(280.))
                    .py(k.p(8.))
                    .bg(rgb_a(0x424242, 1.))
                    .rounded(k.p(2.))
                    .shadow_lg()
                    .child(item(
                        "legacy-gm-folder",
                        t(lang, "Открыть папку с игрой", "Open game directory"),
                        "fas fa-folder",
                        false,
                        "folder",
                    ))
                    .child(item(
                        "legacy-gm-shortcut",
                        t(lang, "Создать ярлык", "Create desktop shortcut"),
                        "fas fa-star",
                        false,
                        "shortcut",
                    ))
                    .child(item(
                        "legacy-gm-uninstall",
                        t(lang, "Удалить", "Uninstall"),
                        "fas fa-trash",
                        true,
                        "uninstall",
                    )),
            )
            .with_priority(2)
        });

        let top = div()
            .mt(k.p(32.))
            .flex()
            .items_center()
            .child(main_button)
            .when(installed, |el| {
                el.child(
                    div()
                        .relative()
                        .ml(k.p(16.))
                        .child(
                            div()
                                .id("legacy-gear")
                                .p(k.p(4.))
                                .rounded(k.p(2.))
                                .bg(white_a(0.22))
                                .cursor_pointer()
                                .on_click(on(cx, |this, _, cx| {
                                    let open = !this.gear_open;
                                    this.close_menus();
                                    this.gear_open = open;
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .size(k.p(40.))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .hover(|s| s.bg(white_a(0.1)))
                                        .child(fa_glyph("fas fa-cog", 18. * k.icon())),
                                ),
                        )
                        .children(gear_menu),
                )
            });

        // .navpanel: разделы, ссылки внизу, предупреждение.
        let section = self.section;
        let nav_button = |i: u8, label: &str, needs_install: bool| {
            let active = section == i;
            let disabled = needs_install && !installed;
            let click: Option<Click> = (!disabled).then(|| -> Click {
                Box::new(on(cx, move |this, _, cx| {
                    this.section = i;
                    cx.notify();
                }))
            });
            let label = label.to_owned();
            hover_fx(format!("legacy-section-{i}"), 200., move |h| {
                div()
                    .id(SharedString::from(format!("legacy-section-{i}")))
                    .w_full()
                    .h(k.p(32.))
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if active {
                        rgb_a(ACCENT, 1.)
                    } else if disabled {
                        rgb_a(0x787878, 1.)
                    } else {
                        mix(rgb_a(0x969696, 1.), rgb_a(0xd2d2d2, 1.), h)
                    })
                    .when(active, |el| {
                        el.border_l(k.p(4.)).border_color(rgb_a(ACCENT, 1.))
                    })
                    .when(!active && !disabled, |el| el.cursor_pointer())
                    .when_some(click, |el, click| el.on_click(click))
                    .child(div().ml(k.p(if active { 12. } else { 16. })).child(label))
            })
        };
        let link = |id: &'static str, label: &str, url: &'static str, border: bool| {
            let label = label.to_owned();
            hover_fx(id, 200., move |h| {
                div()
                    .id(id)
                    .w_full()
                    .h(k.p(32.))
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::BOLD)
                    .text_color(mix(rgb_a(0x969696, 1.), rgb_a(0xd2d2d2, 1.), h))
                    .cursor_pointer()
                    .when(border, |el| {
                        el.mt_auto()
                            .pt(k.p(16.))
                            .h(k.p(48.))
                            .border_t_1()
                            .border_color(white_a(0.1))
                    })
                    .on_click(move |_, _, cx| cx.open_url(url))
                    .child(
                        div()
                            .ml(k.p(16.))
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .mr(k.p(16.))
                                    .child(fa_glyph("fas fa-link", 13. * k.icon())),
                            )
                            .child(label),
                    )
            })
        };
        let warning_text = if !available {
            Some(t(
                lang,
                "Эта игра появится в новом лаунчере позже",
                "This game is coming to the new launcher later",
            ))
        } else {
            None
        };
        let navpanel = div()
            .relative()
            .h_full()
            .min_w(k.p(260.))
            .w(k.p(260.))
            .py(k.p(16.))
            .mr(k.p(16.))
            .rounded(k.p(2.))
            .bg(rgb_a(0x212121, 0.62))
            .flex()
            .flex_col()
            .gap(k.p(2.))
            .text_size(k.p(16.))
            .child(
                div()
                    .ml(k.p(16.))
                    .mb(k.p(8.))
                    .text_size(k.p(12.8))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(rgb_a(0x505050, 1.))
                    .child(t(lang, "Управление", "Management").to_uppercase()),
            )
            .child(nav_button(0, t(lang, "Новости игры", "Game News"), false))
            .child(nav_button(1, t(lang, "Параметры", "Configurator"), true))
            .child(nav_button(
                2,
                t(lang, "LiveSplit сплиты", "LiveSplit Splits"),
                true,
            ))
            .children(warning_text.map(|text| {
                div()
                    .absolute()
                    .left_0()
                    .w_full()
                    .bottom(k.p(98.))
                    .p(k.p(16.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .text_color(rgb_a(0xffd000, 0.45))
                    .child(
                        div()
                            .mb(k.p(8.))
                            .child(fa_glyph("fas fa-exclamation-triangle", 25. * k.icon())),
                    )
                    .child(div().text_size(k.p(12.2)).line_height(k.p(16.)).child(text))
            }))
            .child(link(
                "legacy-wiki",
                t(lang, "SourceRuns Вики", "SourceRuns Wiki"),
                g.wiki,
                true,
            ))
            .child(link(
                "legacy-lb",
                t(lang, "Таблица лидеров", "Leaderboard"),
                g.leaderboard,
                false,
            ));

        let main_panel = div()
            .id("legacy-mainpanel")
            .flex_1()
            .h_full()
            .py(k.p(16.))
            .rounded(k.p(2.))
            .bg(rgb_a(0x212121, 0.62))
            .overflow_y_scroll()
            .child(self.game_section(k, snap, id, cx));

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .overflow_hidden()
            // #mainview: скриншот игры `cover` под linear-gradient(0deg, #070707 10%, transparent 200%).
            .child(
                pic(&self.images, g.background)
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
            .child(div().absolute().inset_0().bg(linear_gradient(
                0.,
                linear_color_stop(rgb_a(0x070707, 0.53), 0.),
                linear_color_stop(rgb_a(0x070707, 1.), 0.9),
            )))
            .child(
                div()
                    .relative()
                    .mt(k.p(112.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .text_size(k.p(48.))
                            .line_height(k.p(57.))
                            .mb(k.p(8.))
                            .text_color(rgb_a(0xd2d2d2, 1.))
                            .child(g.name.to_uppercase()),
                    )
                    .child(top),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .p(k.p(16.))
                    .pt(k.p(32.))
                    .flex()
                    .child(navpanel)
                    .child(main_panel),
            )
            .into_any_element()
    }

    fn game_section(
        &self,
        k: K,
        snap: &Snap,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = snap.lang;
        let title = |value: &str| {
            div()
                .ml(k.p(16.))
                .text_size(k.p(20.))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb_a(ACCENT, 1.))
                .child(value.to_owned())
        };
        let post = |author: &str, body: &str| {
            div()
                .m(k.p(16.))
                .text_size(k.p(16.))
                .text_color(rgb_a(0x9b9b9b, 1.))
                .child(div().opacity(0.4).child(author.to_owned()))
                .child(body.to_owned())
        };
        match self.section {
            1 | 2 => {
                let name = if self.section == 1 {
                    t(lang, "Параметры", "Configurator")
                } else {
                    t(lang, "LiveSplit сплиты", "LiveSplit Splits")
                };
                let open = div().id("legacy-open-new").m(k.p(16.)).w(k.p(260.)).child(
                    alt_button(
                        k,
                        "legacy-open-new-btn",
                        t(lang, "Открыть", "Open"),
                        "fas fa-sliders-h",
                        false,
                        false,
                    )
                    .on_click(on(cx, move |_, _, cx| {
                        // Параметры игры — окно нового лаунчера, оно рисуется и поверх Legacy.
                        if let Some(context) = ShellContext::get(cx) {
                            if context.flows.with_state(|s| s.selected_game_id.clone()) != id {
                                context.nav.select_game(id);
                            }
                            context.flows.on_menu_action("settings");
                        }
                    })),
                );
                div()
                    .child(title(name))
                    .child(post(
                        "HLSR",
                        t(
                            lang,
                            "Эти настройки теперь живут в новом окне параметров игры.",
                            "These settings now live in the new game settings window.",
                        ),
                    ))
                    .when(self.section == 1, |el| el.child(open))
                    .into_any_element()
            }
            _ => div()
                .child(title(t(lang, "Новости игры", "Game News")))
                .child(post(
                    "Steam",
                    t(
                        lang,
                        "Новости игры открываются в Steam",
                        "Game news open in Steam",
                    ),
                ))
                .child(
                    div()
                        .id("legacy-news")
                        .cursor_pointer()
                        .hover(|s| s.text_color(rgb_a(0xb9b9b9, 1.)))
                        .on_click(move |_, _, cx| {
                            let app = if id == "218" { "220" } else { id };
                            cx.open_url(&format!("https://store.steampowered.com/news/app/{app}"))
                        })
                        .child(post("store.steampowered.com", game(id).name)),
                )
                .into_any_element(),
        }
    }
}

// ---- Конфиги -------------------------------------------------------------------------------

impl LegacyShell {
    fn constructor(&self, k: K, snap: &Snap, cx: &mut Context<Self>) -> AnyElement {
        let lang = snap.lang;
        let installed = snap.installed("70");
        let list = |title: &str, names: &'static [&'static str], offset: usize, primary: bool| {
            let rows: Vec<AnyElement> = names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let index = offset + i;
                    let selected = self.scripts.get(index).copied().unwrap_or(false);
                    let mark = if primary {
                        rgb_a(ACCENT, 1.)
                    } else {
                        rgb_a(0xff5252, 1.)
                    };
                    div()
                        .id(SharedString::from(format!("legacy-script-{index}")))
                        .h(k.p(48.))
                        .pl(k.p(16.))
                        .pr(k.p(16.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(white_a(0.06)))
                        .on_click(on(cx, move |this, _, cx| {
                            if let Some(v) = this.scripts.get_mut(index) {
                                *v = !*v;
                            }
                            cx.notify();
                        }))
                        .child(
                            div()
                                .size(k.p(18.))
                                .mr(k.p(32.))
                                .rounded(k.p(2.))
                                .border(k.p(2.))
                                .border_color(if selected { mark } else { white_a(0.7) })
                                .when(selected, |el| el.bg(mark))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(rgb_a(0x303030, 1.))
                                .when(selected, |el| {
                                    el.child(fa_glyph("fas fa-check", 11. * k.icon()))
                                }),
                        )
                        .child(
                            div()
                                .text_size(k.p(16.))
                                .text_color(white_a(1.))
                                .child(*name),
                        )
                        .when(SEQUENCES.contains(name), |el| {
                            el.child(
                                div()
                                    .ml(k.p(12.))
                                    .px(k.p(10.))
                                    .py(k.p(4.))
                                    .rounded_full()
                                    .text_size(k.p(12.))
                                    .bg(white_a(0.1))
                                    .text_color(white_a(0.5))
                                    .child("Sequence"),
                            )
                        })
                        .into_any_element()
                })
                .collect();
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .h(k.p(48.))
                        .px(k.p(16.))
                        .flex()
                        .items_center()
                        .text_size(k.p(14.))
                        .text_color(white_a(0.7))
                        .child(title.to_owned()),
                )
                .children(rows)
        };
        form(k, t(lang, "Конструктор", "Constructor"))
            .child(
                div()
                    .absolute()
                    .top(k.p(32.))
                    .left(k.p(330.))
                    .right(k.p(100.))
                    .child(
                        div()
                            .border_b_1()
                            .border_color(white_a(0.42))
                            .pb(k.p(6.))
                            .child(text(k, 12., white_a(0.54), "Game"))
                            .child(text(k, 16., white_a(0.87), "Half-Life")),
                    ),
            )
            .relative()
            .child(
                div()
                    .id("legacy-constructor-lists")
                    .flex_1()
                    .min_h_0()
                    .mt(k.p(16.))
                    .flex()
                    .overflow_y_scroll()
                    .child(list(
                        t(lang, "Scriptless категория", "Scriptless Category"),
                        &SCRIPTS_SCRIPTLESS,
                        0,
                        true,
                    ))
                    .child(list(
                        t(lang, "Scripted категория", "Scripted Category"),
                        &SCRIPTS_SCRIPTED,
                        SCRIPTS_SCRIPTLESS.len(),
                        false,
                    )),
            )
            .child(
                div()
                    .mt(k.p(16.))
                    .flex()
                    .gap(k.p(10.))
                    .child(
                        alt_button(
                            k,
                            "legacy-script-gen",
                            t(lang, "Установить", "Install"),
                            "fas fa-save",
                            false,
                            !installed,
                        )
                        .on_click(move |_, _, cx| LegacyShell::not_here(lang, cx)),
                    )
                    .child(
                        alt_button(
                            k,
                            "legacy-script-file",
                            t(lang, "Сохранить в файл", "Save to File"),
                            "fas fa-file-alt",
                            true,
                            false,
                        )
                        .on_click(move |_, _, cx| LegacyShell::not_here(lang, cx)),
                    ),
            )
            .into_any_element()
    }

    /// `ConfigEditor.vue`: меню и CodeMirror с темой Dracula.
    fn editor(&self, k: K, snap: &Snap, _cx: &mut Context<Self>) -> AnyElement {
        let lang = snap.lang;
        let menu = |id: &'static str, label: &str| {
            div()
                .id(id)
                .px(k.p(12.))
                .h(k.p(36.))
                .flex()
                .items_center()
                .text_size(k.p(14.))
                .cursor_pointer()
                .hover(|s| s.bg(white_a(0.08)))
                .on_click(move |_, _, cx| LegacyShell::not_here(lang, cx))
                .child(label.to_owned())
        };
        let lines: [(&str, u32); 8] = [
            ("// userconfig.cfg", 0x6272a4),
            ("", 0xf8f8f2),
            ("bind \"SPACE\" \"+jump\"", 0xf8f8f2),
            ("bind \"MWHEELDOWN\" \"+jump\"", 0xf8f8f2),
            ("fps_max \"100\"", 0xf8f8f2),
            ("bxt_autojump \"1\"", 0xff79c6),
            ("cl_showfps \"1\"", 0xf8f8f2),
            ("", 0xf8f8f2),
        ];
        let code = div()
            .id("legacy-code")
            .flex_1()
            .min_h_0()
            .rounded(k.p(6.))
            .bg(rgb_a(0x282a36, 0.92))
            .overflow_y_scroll()
            .py(k.p(4.))
            .font_family(crate::text::MONO)
            .text_size(k.p(14.))
            .children(lines.iter().enumerate().map(|(i, (line, color))| {
                div()
                    .flex()
                    .h(k.p(20.))
                    .items_center()
                    .when(i == 2, |el| el.bg(white_a(0.05)))
                    .child(
                        div()
                            .w(k.p(40.))
                            .pr(k.p(8.))
                            .flex()
                            .justify_end()
                            .text_color(rgb_a(0x6d8a88, 1.))
                            .child((i + 1).to_string()),
                    )
                    .child(div().pl(k.p(8.)).text_color(rgb_a(*color, 1.)).child(*line))
            }));
        form(k, t(lang, "Редактор", "Editor"))
            .child(
                div()
                    .mt(k.p(16.))
                    .mb(k.p(8.))
                    .flex()
                    .items_center()
                    .child(menu("legacy-ed-open", t(lang, "Открыть файл", "Open File")))
                    .child(menu(
                        "legacy-ed-save",
                        t(lang, "Сохранить файл", "Save File"),
                    ))
                    .child(menu(
                        "legacy-ed-hints",
                        t(lang, "Показать подсказки", "Display Hints"),
                    ))
                    .child(
                        div()
                            .ml_auto()
                            .mr(k.p(10.))
                            .text_size(k.p(14.))
                            .child("userconfig.cfg"),
                    ),
            )
            .child(code)
            .into_any_element()
    }
}

// ---- Кастомизация --------------------------------------------------------------------------

const CUSTOM_BACKGROUNDS: [&str; 6] = [
    "legacy/customization/Standard.jpg",
    "legacy/customization/CF.jpg",
    "legacy/customization/BM.jpg",
    "legacy/customization/ST.jpg",
    "legacy/customization/XEN.jpg",
    "legacy/customization/XEN_2.jpg",
];
const CUSTOM_AIMS: [&str; 5] = [
    "legacy/customization/crosshairs/Standard.jpg",
    "legacy/customization/crosshairs/Enchanced.jpg",
    "legacy/customization/crosshairs/Dot.jpg",
    "legacy/customization/crosshairs/Counter-Strike.jpg",
    "legacy/customization/crosshairs/Green.jpg",
];

impl LegacyShell {
    fn customization(&self, k: K, snap: &Snap, cx: &mut Context<Self>) -> AnyElement {
        let lang = snap.lang;
        let installed = snap.installed("70");
        let running = snap.running("70");
        let section_title = |value: &str| {
            div()
                .text_size(k.p(14.))
                .line_height(k.p(17.))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb_a(0xd2d2d2, 1.))
                .child(value.to_uppercase())
        };
        let images = self.images.clone();
        let picture =
            |id: String, path: &'static str, active: bool, height: f32, pick: Listener| {
                div()
                    .id(SharedString::from(id))
                    .relative()
                    .flex_1()
                    .h(k.p(height))
                    .rounded(k.p(10.))
                    .overflow_hidden()
                    .when(!active, |el| el.cursor_pointer().hover(|s| s.opacity(0.85)))
                    .on_click(
                        cx.listener(move |this, _: &ClickEvent, window, cx| pick(this, window, cx)),
                    )
                    .child(
                        pic(&images, path)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                    .when(active, |el| {
                        el.child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded(k.p(10.))
                                .border(k.p(3.))
                                .border_color(rgb_a(ACCENT, 1.)),
                        )
                        .child(
                            pic(&images, "legacy/customization/icon-checkbox.png")
                                .absolute()
                                .right(k.p(12.5))
                                .bottom(k.p(12.))
                                .size(k.p(21.)),
                        )
                    })
            };
        let row = |items: Vec<Stateful<Div>>| div().flex().gap(k.p(10.)).children(items);

        let backgrounds: Vec<Stateful<Div>> = CUSTOM_BACKGROUNDS
            .iter()
            .enumerate()
            .map(|(i, path)| {
                picture(
                    format!("legacy-bg-{i}"),
                    path,
                    self.custom_background == i,
                    92.,
                    Rc::new(
                        move |this: &mut LegacyShell,
                              _: &mut Window,
                              cx: &mut Context<LegacyShell>| {
                            this.custom_background = i;
                            cx.notify();
                        },
                    ),
                )
            })
            .collect();
        let mut backgrounds = backgrounds.into_iter();
        let bg_row1: Vec<_> = backgrounds.by_ref().take(3).collect();
        let bg_row2: Vec<_> = backgrounds.collect();
        let aims: Vec<Stateful<Div>> = CUSTOM_AIMS
            .iter()
            .enumerate()
            .map(|(i, path)| {
                picture(
                    format!("legacy-aim-{i}"),
                    path,
                    self.custom_aim == i,
                    92.,
                    Rc::new(
                        move |this: &mut LegacyShell,
                              _: &mut Window,
                              cx: &mut Context<LegacyShell>| {
                            this.custom_aim = i;
                            cx.notify();
                        },
                    ),
                )
            })
            .collect();
        let pair = |prefix: &'static str,
                    paths: [&'static str; 2],
                    current: usize,
                    set: fn(&mut LegacyShell, usize)| {
            row(paths
                .iter()
                .enumerate()
                .map(|(i, path)| {
                    picture(
                        format!("legacy-{prefix}-{i}"),
                        path,
                        current == i,
                        150.,
                        Rc::new(
                            move |this: &mut LegacyShell,
                                  _: &mut Window,
                                  cx: &mut Context<LegacyShell>| {
                                set(this, i);
                                cx.notify();
                            },
                        ),
                    )
                })
                .collect())
        };

        // GameSelector.vue: подчёркнутые вкладки игр.
        let tab = |i: usize, name: &str, color: u32, enabled: bool| {
            let active = self.custom_game == i;
            div()
                .id(SharedString::from(format!("legacy-cg-{i}")))
                .px(k.p(14.))
                .pb(k.p(12.))
                .flex()
                .items_center()
                .text_size(k.p(16.))
                .font_weight(FontWeight::BOLD)
                .border_b(k.p(3.))
                .border_color(if active {
                    rgb_a(0x2db9ff, 1.)
                } else {
                    white_a(0.25)
                })
                .text_color(if active {
                    white_a(1.)
                } else {
                    rgb_a(0xc8c8c8, 1.)
                })
                .when(!enabled, |el| el.opacity(0.3))
                .when(enabled, |el| {
                    el.cursor_pointer().on_click(on(cx, move |this, _, cx| {
                        this.custom_game = i;
                        cx.notify();
                    }))
                })
                .child(
                    svg()
                        .path("legacy/lambda.svg")
                        .size(k.p(18.))
                        .mr(k.p(10.))
                        .text_color(rgb_a(color, 1.)),
                )
                .child(name.to_owned())
        };
        let selector = div()
            .mt(k.p(16.))
            .h(k.p(60.))
            .flex()
            .items_end()
            .child(tab(0, "Half-Life", 0xFF6000, installed))
            .child(tab(1, "Half-Life: Opposing Force", 0x6ED594, false))
            .child(tab(2, "Half-Life: Blue Shift", 0x94BFFD, false))
            .child(
                div()
                    .flex_1()
                    .self_stretch()
                    .border_b(k.p(3.))
                    .border_color(white_a(0.25))
                    .opacity(0.3),
            );

        let disabled = !installed || running;
        form(k, t(lang, "Кастомизация", "Customization"))
            .child(selector)
            .child(
                div()
                    .id("legacy-custom-scroll")
                    .flex_1()
                    .min_h_0()
                    .mt(k.p(40.))
                    .pr(k.p(20.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(section_title(t(lang, "Фоны меню", "Menu Backgrounds")))
                    .child(
                        div()
                            .mt(k.p(12.))
                            .flex()
                            .flex_col()
                            .gap(k.p(10.))
                            .child(row(bg_row1))
                            .child(row(bg_row2)),
                    )
                    .child(div().mt(k.p(16.)).child(section_title(t(
                        lang,
                        "Прицелы",
                        "Crosshairs",
                    ))))
                    .child(div().mt(k.p(12.)).child(row(aims)))
                    .child(div().mt(k.p(18.)).child(section_title("HUD")))
                    .child(div().mt(k.p(12.)).child(pair(
                        "hud",
                        [
                            "legacy/customization/hud/1.jpg",
                            "legacy/customization/hud/2.jpg",
                        ],
                        self.custom_hud,
                        |this, i| this.custom_hud = i,
                    )))
                    .child(div().mt(k.p(18.)).child(section_title(t(
                        lang,
                        "Спрайты",
                        "Item Sprites",
                    ))))
                    .child(div().mt(k.p(12.)).child(pair(
                        "sprites",
                        [
                            "legacy/customization/sprites/1.jpg",
                            "legacy/customization/sprites/2.jpg",
                        ],
                        self.custom_sprites,
                        |this, i| this.custom_sprites = i,
                    )))
                    .child(div().mt(k.p(18.)).child(section_title(t(
                        lang,
                        "Скайбоксы",
                        "Skyboxes",
                    ))))
                    .child(
                        div()
                            .mt(k.p(15.))
                            .child(
                                radio(
                                    k,
                                    "legacy-sky-lq",
                                    t(lang, "Стандартная версия", "Standard Version"),
                                    !self.custom_hd_sky,
                                )
                                .on_click(on(cx, |this, _, cx| {
                                    this.custom_hd_sky = false;
                                    cx.notify();
                                })),
                            )
                            .child(
                                radio(
                                    k,
                                    "legacy-sky-hd",
                                    t(lang, "HD вариант", "HD Version"),
                                    self.custom_hd_sky,
                                )
                                .on_click(on(cx, |this, _, cx| {
                                    this.custom_hd_sky = true;
                                    cx.notify();
                                })),
                            ),
                    ),
            )
            .child(
                div()
                    .mt(k.p(32.))
                    .flex()
                    .gap(k.p(10.))
                    .child(
                        alt_button(
                            k,
                            "legacy-custom-save",
                            t(lang, "Сохранить", "Save"),
                            "fas fa-save",
                            false,
                            disabled,
                        )
                        .on_click(move |_, _, cx| {
                            if !disabled {
                                LegacyShell::not_here(lang, cx)
                            }
                        }),
                    )
                    .child(
                        alt_button(
                            k,
                            "legacy-custom-reset",
                            t(lang, "Сбросить", "Reset"),
                            "fas fa-ban",
                            true,
                            disabled,
                        )
                        .on_click(on(cx, move |this, _, cx| {
                            if disabled {
                                return;
                            }
                            this.custom_background = 0;
                            this.custom_aim = 0;
                            this.custom_hud = 0;
                            this.custom_sprites = 0;
                            this.custom_hd_sky = false;
                            cx.notify();
                        })),
                    ),
            )
            .into_any_element()
    }
}

/// `customization/checkbox/CheckBox.vue`: круглая отметка.
fn radio(k: K, id: &'static str, label: &str, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .mb(k.p(10.))
        .cursor_pointer()
        .child(
            div()
                .size(k.p(18.))
                .mr(k.p(10.))
                .rounded_full()
                .border(k.p(2.))
                .border_color(if on { rgb_a(ACCENT, 1.) } else { white_a(0.5) })
                .flex()
                .items_center()
                .justify_center()
                .when(on, |el| {
                    el.child(div().size(k.p(8.)).rounded_full().bg(rgb_a(ACCENT, 1.)))
                }),
        )
        .child(
            div()
                .text_size(k.p(14.))
                .text_color(rgb_a(0xd2d2d2, 1.))
                .child(label.to_uppercase()),
        )
}

// ---- Настройки -----------------------------------------------------------------------------

impl LegacyShell {
    fn settings(&self, k: K, snap: &Snap, cx: &mut Context<Self>) -> AnyElement {
        let lang = snap.lang;
        let section = |value: &str| {
            div()
                .ml(k.p(26.))
                .my(k.p(16.))
                .font_weight(FontWeight::BOLD)
                .text_size(k.p(12.))
                .text_color(rgb_a(0xd2d2d2, 1.))
                .child(value.to_uppercase())
        };
        let languages = [
            (t(lang, "Английский", "English"), Language::En),
            (t(lang, "Русский", "Russian"), Language::Ru),
        ];
        let themes = [
            t(lang, "Градиент", "Gradient"),
            t(lang, "Синие цвета", "Blue Colors"),
            t(lang, "Красные цвета", "Red Colors"),
            "Freelancer",
        ];
        let current_language = if lang == Language::Ru { 1 } else { 0 };

        // Dropdown.vue: подпись слева, выбор 256 px справа, список под ним.
        let dropdown = |id: &'static str,
                        label: &str,
                        selected: &str,
                        open: bool,
                        items: Vec<(String, Listener)>,
                        toggle: Listener| {
            let list = open.then(|| {
                div()
                    .mt(k.p(8.))
                    .rounded(k.p(4.))
                    .overflow_hidden()
                    .children(items.into_iter().enumerate().map(|(i, (name, pick))| {
                        div()
                            .id(SharedString::from(format!("{id}-{i}")))
                            .p(k.p(8.))
                            .bg(black_a(0.3))
                            .hover(|s| s.bg(black_a(0.4)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                pick(this, window, cx)
                            }))
                            .child(name)
                    }))
            });
            element(k)
                .items_start()
                .child(
                    div()
                        .mt(k.p(9.))
                        .text_size(k.p(15.))
                        .child(label.to_owned()),
                )
                .child(
                    div()
                        .ml_auto()
                        .w(k.p(256.))
                        .child(
                            div()
                                .id(id)
                                .p(k.p(8.))
                                .bg(black_a(0.3))
                                .rounded(k.p(4.))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    toggle(this, window, cx)
                                }))
                                .child(selected.to_owned())
                                .child(
                                    div()
                                        .ml_auto()
                                        .child(fa_glyph("fas fa-sort-down", 14. * k.icon())),
                                ),
                        )
                        .children(list),
                )
        };

        let language_items: Vec<(String, Listener)> = languages
            .iter()
            .map(|(name, code)| {
                let code = *code;
                let pick: Listener = Rc::new(
                    move |this: &mut LegacyShell,
                          window: &mut Window,
                          cx: &mut Context<LegacyShell>| {
                        this.language_open = false;
                        cx.notify();
                        crate::settings::change_language_to(code, window, cx);
                    },
                );
                (name.to_string(), pick)
            })
            .collect();
        let theme_items: Vec<(String, Listener)> = themes
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let pick: Listener = Rc::new(
                    move |this: &mut LegacyShell, _: &mut Window, cx: &mut Context<LegacyShell>| {
                        this.theme = i;
                        this.theme_open = false;
                        cx.notify();
                    },
                );
                (name.to_string(), pick)
            })
            .collect();

        let tool = |id: &'static str, icon: &'static str, act: Listener| {
            div()
                .id(id)
                .size(k.p(50.))
                .ml(k.p(8.))
                .rounded(k.p(4.))
                .bg(black_a(0.2))
                .hover(|s| s.bg(black_a(0.3)).text_color(white_a(0.4)))
                .flex()
                .items_center()
                .justify_center()
                .text_color(white_a(0.3))
                .cursor_pointer()
                .on_click(
                    cx.listener(move |this, _: &ClickEvent, window, cx| act(this, window, cx)),
                )
                .child(fa_glyph(icon, 24. * k.icon()))
        };

        form(k, t(lang, "Настройки", "Settings"))
            .child(
                div()
                    .id("legacy-settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .mb(k.p(32.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(section(t(
                        lang,
                        "Настройки интерфейса",
                        "Interface Settings",
                    )))
                    .child(dropdown(
                        "legacy-lang",
                        t(lang, "Язык интерфейса", "Interface Language"),
                        languages[current_language].0,
                        self.language_open,
                        language_items,
                        Rc::new(|this, _, cx| {
                            this.language_open = !this.language_open;
                            this.theme_open = false;
                            cx.notify();
                        }),
                    ))
                    .child(dropdown(
                        "legacy-theme",
                        t(lang, "Оформление", "Interface Theme"),
                        themes[self.theme],
                        self.theme_open,
                        theme_items,
                        Rc::new(|this, _, cx| {
                            this.theme_open = !this.theme_open;
                            this.language_open = false;
                            cx.notify();
                        }),
                    ))
                    .child(section(t(lang, "Прочие настройки", "Misc Settings")))
                    .child(
                        checkbox(
                            k,
                            "legacy-rpc",
                            t(lang, "Discord RPC статус", "Discord RPC Status"),
                            self.rpc,
                        )
                        .on_click(on(cx, |this, _, cx| {
                            this.rpc = !this.rpc;
                            cx.notify();
                        })),
                    )
                    .child(
                        checkbox(
                            k,
                            "legacy-particles",
                            t(lang, "Отключить частицы", "Disable particles"),
                            self.no_particles,
                        )
                        .on_click(on(cx, |this, _, cx| {
                            this.no_particles = !this.no_particles;
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_end()
                    .items_center()
                    .child(tool(
                        "legacy-discord",
                        "fab fa-discord",
                        Rc::new(|_, _, cx| cx.open_url("https://discord.com/invite/8Np6brWBuz")),
                    ))
                    .child(tool(
                        "legacy-site",
                        "fas fa-globe-asia",
                        Rc::new(|_, _, cx| cx.open_url("https://hlsr.pro")),
                    ))
                    .child(tool(
                        "legacy-about",
                        "fas fa-question-circle",
                        Rc::new(|this, _, cx| {
                            this.dialog = Some(Dialog::About);
                            cx.notify();
                        }),
                    )),
            )
            .into_any_element()
    }

    /// `md-dialog`: окно 760 px на затемнении, щелчок мимо закрывает.
    fn dialog_view(&self, k: K, snap: &Snap, dialog: Dialog, cx: &mut Context<Self>) -> AnyElement {
        let lang = snap.lang;
        let (title, body): (String, Vec<AnyElement>) = match dialog {
            Dialog::Changelog => (
                t(lang, "Что нового?", "Changelog").to_owned(),
                vec![
                    text(k, 16., white_a(0.4), format!("HLSR-CE v{}", snap.version)).into_any_element(),
                    text(
                        k,
                        14.,
                        white_a(0.4),
                        t(
                            lang,
                            "• Пасхалка: интерфейс HLSR 0.2.9 (наберите LEGACY ещё раз, чтобы вернуться)",
                            "• Easter egg: the HLSR 0.2.9 interface (type LEGACY again to go back)",
                        ),
                    )
                    .mt(k.p(12.))
                    .into_any_element(),
                    text(
                        k,
                        14.,
                        white_a(0.4),
                        t(
                            lang,
                            "• Настройки → Интерфейс: «Снять ограничение 60 FPS»",
                            "• Settings → Interface: \"Uncap 60 FPS\"",
                        ),
                    )
                    .mt(k.p(6.))
                    .into_any_element(),
                ],
            ),
            Dialog::About => (
                format!("HLSR [v. {}]", snap.version),
                vec![
                    div()
                        .flex()
                        .child(
                            div()
                                .flex_1()
                                .text_size(k.p(14.))
                                .line_height(k.p(20.))
                                .text_color(white_a(0.7))
                                .child(div().child(t(lang, "HLSR Community Edition", "HLSR Community Edition")))
                                .child(div().child(t(
                                    lang,
                                    "Форк лаунчера HLSR (github.com/HL-HLSR/HLSR)",
                                    "A fork of the HLSR launcher (github.com/HL-HLSR/HLSR)",
                                )))
                                .child(div().mt(k.p(12.)).child(t(
                                    lang,
                                    "Интерфейс 0.2.9: InfiniteC0re, N4Rk0t1k, Parklez, bmd.GG, ALLAN",
                                    "0.2.9 interface: InfiniteC0re, N4Rk0t1k, Parklez, bmd.GG, ALLAN",
                                ))),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(k.p(220.))
                                .child(pic(&self.images, "legacy/logo.png").w(k.p(180.)).h(k.p(180.)).rounded(k.p(4.))),
                        )
                        .into_any_element(),
                ],
            ),
        };
        div()
            .id("legacy-dialog-overlay")
            .absolute()
            .inset_0()
            .bg(black_a(0.6))
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.dialog = None;
                cx.notify();
            }))
            .child(
                div()
                    .id("legacy-dialog")
                    .w(k.p(760.))
                    .max_h(k.p(600.))
                    .bg(rgb_a(0x3e3e3e, 1.))
                    .rounded(k.p(2.))
                    .shadow_lg()
                    .p(k.p(24.))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .mb(k.p(20.))
                            .text_size(k.p(18.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb_a(ACCENT, 1.))
                            .child(title),
                    )
                    .children(body),
            )
            .into_any_element()
    }
}
