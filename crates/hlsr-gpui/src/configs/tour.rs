//! Обучение по вкладке «Персонализация» (`ConfigTour.vue` 0.4.9): кольцо вокруг якоря
//! ([`super::anchors`]), всё прочее окно затемнено (дыра в заливке `rgba(0,0,0,.62)` по форме
//! кольца), карточка с заголовком, текстом и «Пропустить / Назад / Дальше». Шаг другого раздела
//! шапки переключает раздел, шаг раздела страницы — раздел списка слева; якоря нет на экране
//! 400 мс (после смены раздела 1200 мс) — шаг пропускается. Кольцо и карточка переезжают за
//! .25 с, кольцо появляется за .2 с. Щит ловит мышь, пока видна карточка; Esc — выход всегда.
//!
//! Запуск — как у оболочки 0.4.9: «?» страницы ([`help`]), «С нуля» в опросе
//! (`configTourRequested`), сам после обновления (`configTourDue`: игра установлена, приветствие
//! прошло, окон нет, `configTourSeen` не стоит — через 1,2 с). Шаги — `config_tour::steps_for`.

use super::anchors;
use crate::kit::button::{focus_handle, focus_visible, outline, white};
use crate::layer::{Dismiss, FocusNext, FocusPrev, Layers, Modal, Z_TOAST, trap};
use crate::motion::{Timing, hover, mix, tween};
use crate::paint::shadow;
use crate::scale::{u, viewport, zoom};
use crate::shell::root::{ShellContext, ShellRoot};
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AbsoluteLength, AnyElement, App, Bounds, ClickEvent, Context, ElementId, FocusHandle,
    FontWeight, Global, Hsla, IntoElement, MouseButton, PathBuilder, Pixels, Point, Render, Role,
    SharedString, Task, WeakEntity, Window, anchored, canvas, deferred, div, point, prelude::*,
};
use hlsr_ui::controllers::config_chips::config_games;
use hlsr_ui::controllers::config_tour::{
    CARD_WIDTH, DUE_DELAY_MS, POLL_MS, Placement, Rect, TourStep, WAIT_MS, WAIT_SWITCH_MS, place,
    steps_for,
};
use hlsr_ui::controllers::game_settings::import::engine_of;
use hlsr_ui::controllers::shell_flows::delivery_id;
use hlsr_ui::services::Services;
use serde_json::Value;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// `transition: … .25s ease` кольца и карточки.
fn ease(ms: f64) -> Timing {
    Timing::new(ms, [0.25, 0.1, 0.25, 1.])
}

/// Окно нарисовало кадр после этого вызова: к началу следующего кадра рамки якорей — его.
/// `on_next_frame` срабатывает в начале такта, до рисования: первое ожидание — такт, в котором
/// кадр рисуется, второе — следующий, когда нарисованный кадр уже есть.
async fn drawn(this: &WeakEntity<ConfigTour>, cx: &mut gpui::AsyncWindowContext) {
    for _ in 0..2 {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let asked = this.update_in(cx, |_, window, _| {
            window.on_next_frame(move |_, _| {
                tx.send(()).ok();
            })
        });
        if asked.is_err() || rx.await.is_err() {
            return;
        }
    }
}

/// Вид обучения окна (пересоздаётся со сменой языка): к нему идёт «?» страницы.
struct TourView(WeakEntity<ConfigTour>);

impl Global for TourView {}

/// «?» на странице «Персонализация» повторяет обучение на открытой игре (`onConfigsHelp`).
pub fn help(game: &str, window: &mut Window, cx: &mut App) {
    let Some(tour) = cx
        .try_global::<TourView>()
        .and_then(|view| view.0.upgrade())
    else {
        return;
    };
    let game = game.to_owned();
    tour.update(cx, |tour, cx| tour.start(&game, "configs", window, cx));
}

/// Идущее обучение.
struct Run {
    steps: Vec<TourStep>,
    /// Раздел шапки, куда вернуть игрока в конце.
    back: String,
    /// Раздел страницы до обучения: в конце страница возвращается в него.
    page_section: Option<&'static str>,
    /// Показанный шаг; `None` — карточки ещё нет (щит не ловит мышь).
    index: Option<usize>,
    token: u64,
    _seek: Option<Task<()>>,
    /// Последнее место кольца и карточки, при котором якорь был на экране.
    placed: Option<Placement>,
}

pub struct ConfigTour {
    /// Стопка окон (`overlayOpen`), Tab по кругу внутри карточки, возврат фокуса в конце.
    modal: Modal,
    next: FocusHandle,
    run: Option<Run>,
    /// Ожидание автозапуска (`configTourTimer`).
    due: Option<Task<()>>,
    /// Размер карточки в прошлом кадре, CSS px.
    card: Rc<Cell<Option<(f32, f32)>>>,
}

impl ConfigTour {
    pub fn new(_services: &Services, cx: &mut Context<ConfigTour>) -> ConfigTour {
        cx.set_global(TourView(cx.weak_entity()));
        ConfigTour {
            modal: Modal::new("config-tour", cx),
            next: cx.focus_handle().tab_stop(true),
            run: None,
            due: None,
            card: Rc::default(),
        }
    }

    fn shell(cx: &App) -> Option<ShellContext> {
        ShellContext::get(cx).cloned()
    }

    /// `configTourDue`: обучение после обновления пора показать.
    fn due(shell: &ShellContext, cx: &App) -> bool {
        let flows = &shell.flows;
        let ready = flows.with_state(|s| {
            s.prefs.get("configTourSeen") != Some(&Value::Bool(true))
                && s.welcome_checked
                && !s.config_setup_open
                && !config_games(&s.delivery).is_empty()
        });
        ready && !flows.config_setup_busy() && Layers::open_count(cx) == 0
    }

    /// Пора — через 1,2 с (за приветствием или опросом сразу идёт другое окно), если всё ещё пора.
    fn watch_due(&mut self, shell: &ShellContext, window: &mut Window, cx: &mut Context<Self>) {
        if self.run.is_some() || self.due.is_some() || !Self::due(shell, cx) {
            return;
        }
        self.due = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DUE_DELAY_MS))
                .await;
            this.update_in(cx, |this, window, cx| {
                this.due = None;
                this.auto_start(window, cx);
            })
            .ok();
        }));
    }

    /// Автозапуск: на выбранной игре; первый шаг подсвечивает вкладку там, где игрок сейчас.
    fn auto_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(shell) = Self::shell(cx) else {
            return;
        };
        if !Self::due(&shell, cx) {
            return;
        }
        let games = shell.flows.with_state(|s| config_games(&s.delivery));
        let selected = shell.nav.selected_game();
        let Some(game) = games
            .iter()
            .find(|game| game.id == selected)
            .or(games.first())
        else {
            return;
        };
        let back = shell.nav.section();
        shell.nav.set_configs_game(&game.id);
        shell.flows.with_state(|s| {
            s.configs_section = "configs".into();
            s.configs_open_token += 1;
        });
        self.start(&game.id, &back, window, cx);
    }

    /// `startConfigTour(gameId, back)`.
    fn start(&mut self, game: &str, back: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.run.is_some() {
            return;
        }
        let Some(shell) = Self::shell(cx) else {
            return;
        };
        let games = shell.flows.with_state(|s| config_games(&s.delivery));
        let Some(game) = games.iter().find(|g| g.id == game).or(games.first()) else {
            return;
        };
        let binds = delivery_id(&game.id).unwrap_or_default();
        let steps = steps_for(engine_of(binds).unwrap_or_default(), binds);
        let page_section = configs_page(cx).and_then(|page| page.read(cx).current_page_section(cx));
        anchors::begin();
        self.card.set(None);
        self.run = Some(Run {
            steps,
            back: back.to_owned(),
            page_section,
            index: None,
            token: 0,
            _seek: None,
            placed: None,
        });
        self.modal.open(window, cx);
        self.go(0, 1, window, cx);
    }

    /// `go(i, dir)`: к шагу `i`; вперёд или назад пропускает шаги без якоря. Вперёд шагов не
    /// осталось — конец; назад — остаёмся на текущем.
    fn go(&mut self, from: usize, dir: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        run.token += 1;
        let token = run.token;
        let len = run.steps.len() as isize;
        run._seek = Some(cx.spawn_in(window, async move |this, cx| {
            let mut i = from as isize;
            while (0..len).contains(&i) {
                let Ok(Some((anchor, mut page, wait))) =
                    this.update(cx, |this, cx| this.enter(i as usize, cx))
                else {
                    return;
                };
                let until = Instant::now() + wait;
                // `waitFor`: сначала проверка (якорь уже в кадре — шаг сразу), пауза — после.
                // Первая повторная — после кадра с новым разделом (у 0.4.9 якорь виден сразу
                // после `$nextTick`), дальше — раз в 40 мс.
                let mut quick = true;
                let found = loop {
                    let seen = this.update_in(cx, |this, window, cx| {
                        if this.run.as_ref()?.token != token {
                            return None;
                        }
                        // Страница могла только что появиться: раздел — когда она готова.
                        if let Some(id) = page
                            && select_page_section(id, window, cx)
                        {
                            page = None;
                        }
                        cx.notify();
                        Some(anchors::get(anchor).is_some())
                    });
                    let Ok(Some(seen)) = seen else {
                        return;
                    };
                    if seen || Instant::now() > until {
                        break seen;
                    }
                    if std::mem::take(&mut quick) {
                        drawn(&this, cx).await;
                    } else {
                        cx.background_executor()
                            .timer(Duration::from_millis(POLL_MS))
                            .await;
                    }
                };
                if found {
                    this.update_in(cx, |this, window, cx| this.show(i as usize, window, cx))
                        .ok();
                    return;
                }
                i += dir;
            }
            if dir > 0 {
                this.update_in(cx, |this, window, cx| this.finish(window, cx))
                    .ok();
            }
        }));
    }

    /// Шаг `i`: раздел шапки другой — переключить; что искать и сколько ждать.
    fn enter(
        &mut self,
        i: usize,
        cx: &mut Context<Self>,
    ) -> Option<(&'static str, Option<&'static str>, Duration)> {
        let step = self.run.as_ref()?.steps.get(i)?.clone();
        let nav = Self::shell(cx)?.nav;
        let switched = step.section.filter(|section| *section != nav.section());
        if let Some(section) = switched {
            nav.select_section(section);
        }
        let wait = if switched.is_some() {
            WAIT_SWITCH_MS
        } else {
            WAIT_MS
        };
        Some((step.anchor, step.page_section, Duration::from_millis(wait)))
    }

    fn show(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        run.index = Some(i);
        window.focus(&self.next, cx);
        cx.notify();
    }

    /// `finish()` + `onConfigTourDone`: раздел шапки и страницы — как до обучения;
    /// `configTourSeen`.
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.run.take() else {
            return;
        };
        anchors::end();
        self.modal.close(window, cx);
        if let Some(shell) = Self::shell(cx) {
            shell.nav.select_section(&run.back);
            if let Some(id) = run.page_section {
                cx.defer_in(window, move |_, window, cx| {
                    select_page_section(id, window, cx);
                });
            }
            shell.flows.mark_config_tour_seen();
        }
        cx.notify();
    }

    /// Карточка шага: заголовок, текст, кнопки.
    fn card(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (vw, _) = viewport(window);
        let Some(run) = self.run.as_ref() else {
            return gpui::Empty.into_any_element();
        };
        let step = &run.steps[index];
        let tr = |key: &str| {
            Self::shell(cx).map_or_else(|| key.to_owned(), |shell| shell.shell.tr(key, &[]))
        };
        let title = tr(&step.title);
        let text = tr(&step.text);
        let last = index + 1 == run.steps.len();
        let (skip, back, next) = (
            tr("#L_TOUR_SKIP"),
            tr("#L_TOUR_BACK"),
            tr(if last { "#L_TOUR_DONE" } else { "#L_TOUR_NEXT" }),
        );
        let go = |delta: isize| {
            cx.listener(move |this: &mut Self, _: &ClickEvent, window, cx| {
                let Some(index) = this.run.as_ref().and_then(|run| run.index) else {
                    return;
                };
                let to = index as isize + delta;
                if to >= 0 {
                    this.go(to as usize, delta, window, cx);
                }
            })
        };
        let actions =
            div()
                .flex()
                .items_center()
                .gap(u(8.))
                .mt(u(6.))
                .child(TourBtn::new("tour-skip", skip, Kind::Ghost).on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| this.finish(window, cx)),
                ))
                .child(div().flex_1())
                .child(
                    TourBtn::new("tour-back", back, Kind::Plain)
                        .disabled(index == 0)
                        .on_click(go(-1)),
                )
                .child(crate::snap::target(
                    "tour-next",
                    TourBtn::new("tour-next", next, Kind::Primary)
                        .focus(&self.next)
                        .on_click(go(1)),
                ));
        let measured = self.card.clone();
        let size_now = self.card.get();
        let weak = cx.weak_entity();
        let measure = canvas(
            move |bounds: Bounds<Pixels>, window, _| {
                let zoom = crate::scale::zoom(window);
                let size = (
                    f32::from(bounds.size.width) / zoom,
                    f32::from(bounds.size.height) / zoom,
                );
                measured.set(Some(size));
                if size_now != Some(size) {
                    crate::motion::monitor_notify(window, weak.entity_id());
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div()
            .id("tour-card")
            .occlude()
            .role(Role::Dialog)
            .aria_label(SharedString::from(title.clone()))
            .relative()
            .w(u(CARD_WIDTH.min(vw - 24.)))
            .flex()
            .flex_col()
            .gap(u(8.))
            .px(u(18.))
            .py(u(16.))
            .bg(theme.color("l-modal-bg"))
            .border_1()
            .border_color(theme.color("l-modal-border"))
            .rounded(theme.rem("l-radius"))
            .shadow(vec![shadow(
                zoom,
                0.,
                18.,
                48.,
                0.,
                gpui::black().opacity(0.55),
            )])
            .font_family(RUBIK)
            .text_color(theme.color("l-text"))
            .child(
                div()
                    // `h2` без своей высоты строки: 20 px от `body`.
                    .text_px(15., 20.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .text_px(13., 13. * 1.45)
                    .text_color(theme.color("l-text-dim"))
                    .child(text),
            )
            .child(actions)
            .child(measure)
            .into_any_element()
    }
}

/// `$refs.configsPanel`: страница, если её уже открывали.
fn configs_page(cx: &App) -> Option<gpui::Entity<super::page::GameConfigsPanel>> {
    ShellRoot::get(cx).and_then(|root| root.read(cx).configs_page())
}

/// `onConfigTourPageSection` → `panel.selectPageSection(id)`; ложь — страницы или окна игры ещё
/// нет (шаг повторит).
fn select_page_section(id: &'static str, window: &mut Window, cx: &mut App) -> bool {
    configs_page(cx).is_some_and(|page| {
        page.update(cx, |page, cx| page.select_page_section(id, window, cx))
            .is_some()
    })
}

/// Рамка окна в CSS px.
fn css(bounds: Bounds<Pixels>, zoom: f32) -> Rect {
    Rect {
        left: f32::from(bounds.origin.x) / zoom,
        top: f32::from(bounds.origin.y) / zoom,
        width: f32::from(bounds.size.width) / zoom,
        height: f32::from(bounds.size.height) / zoom,
    }
}

/// Скруглённый прямоугольник контуром пути (px окна).
fn rounded(path: &mut PathBuilder, (x, y, w, h): (f32, f32, f32, f32), r: f32) {
    let r = r.min(w / 2.).min(h / 2.).max(0.);
    let p = |x: f32, y: f32| point(Pixels::from(x), Pixels::from(y));
    let radii = p(r, r);
    let arc = |path: &mut PathBuilder, to: Point<Pixels>| {
        path.arc_to(radii, Pixels::from(0.), false, true, to)
    };
    path.move_to(p(x + r, y));
    path.line_to(p(x + w - r, y));
    arc(path, p(x + w, y + r));
    path.line_to(p(x + w, y + h - r));
    arc(path, p(x + w - r, y + h));
    path.line_to(p(x + r, y + h));
    arc(path, p(x, y + h - r));
    path.line_to(p(x, y + r));
    arc(path, p(x + r, y));
    path.close();
}

/// Кольцо и затемнение вокруг него (`box-shadow: 0 0 0 9999px rgba(0,0,0,.62)`): заливка окна с
/// дырой по форме кольца (чётно-нечётное правило пути), углы окна — по его скруглению.
fn ring(rect: Rect, opacity: f32, accent: Hsla, radius: f32, window_radius: f32) -> AnyElement {
    let dim = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let zoom = crate::scale::zoom(window);
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let mut path = PathBuilder::fill();
            rounded(&mut path, (0., 0., w, h), window_radius);
            rounded(
                &mut path,
                (
                    rect.left * zoom,
                    rect.top * zoom,
                    rect.width * zoom,
                    rect.height * zoom,
                ),
                radius * zoom,
            );
            if let Ok(path) = path.build() {
                window.paint_path(path, gpui::black().opacity(0.62 * opacity));
            }
        },
    )
    .absolute()
    .inset_0();
    let mut border = div()
        .absolute()
        .left(u(rect.left))
        .top(u(rect.top))
        .w(u(rect.width))
        .h(u(rect.height))
        .rounded(u(radius))
        .border_color(accent)
        .opacity(opacity);
    let width: AbsoluteLength = u(2.).into();
    let edges = &mut border.style().border_widths;
    edges.top = Some(width);
    edges.right = Some(width);
    edges.bottom = Some(width);
    edges.left = Some(width);
    div()
        .absolute()
        .inset_0()
        .child(dim)
        .child(border)
        .into_any_element()
}

impl Render for ConfigTour {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let Some(shell) = Self::shell(cx) else {
            return gpui::Empty.into_any_element();
        };
        // «С нуля» в опросе: страница уже открыта на игре (`configTourRequested`).
        if shell
            .flows
            .with_state(|s| std::mem::take(&mut s.config_tour_requested))
        {
            let game = shell.nav.configs_game();
            cx.defer_in(window, move |this, window, cx| {
                this.start(&game, "configs", window, cx)
            });
        }
        self.watch_due(&shell, window, cx);
        let Some(run) = self.run.as_ref() else {
            return gpui::Empty.into_any_element();
        };
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (vw, vh) = viewport(window);
        let index = run.index;
        let anchor = index.map(|i| run.steps[i].anchor);
        let rect = anchor.and_then(anchors::get);
        let card_size = self.card.get();
        let placed = match (rect, card_size) {
            (Some(rect), Some(size)) => Some(place(css(rect, zoom), size, (vw, vh))),
            _ => run.placed,
        };
        let card = index.map(|index| self.card(index, window, cx));
        if let Some(run) = self.run.as_mut() {
            run.placed = placed;
        }
        // Карточку ещё не мерили (первый шаг): она в (0, 0), кольцо прозрачно — потом обе едут.
        let shown = card_size.is_some() && placed.is_some();
        let (accent, radius) = (theme.color("accent"), theme.len("l-radius-sm"));
        let window_radius = f32::from(Layers::radius(cx));
        let ring_el = tween("tour-ring-o", if shown { 1. } else { 0. }, ease(200.), {
            move |opacity| match placed.filter(|_| shown) {
                Some(p) => {
                    let r = p.ring;
                    let t = ease(250.);
                    tween("tour-ring-l", r.left, t, move |left| {
                        tween("tour-ring-t", r.top, t, move |top| {
                            tween("tour-ring-w", r.width, t, move |width| {
                                tween("tour-ring-h", r.height, t, move |height| {
                                    let rect = Rect {
                                        left,
                                        top,
                                        width,
                                        height,
                                    };
                                    ring(rect, opacity, accent, radius, window_radius)
                                })
                            })
                        })
                    })
                    .into_any_element()
                }
                None => gpui::Empty.into_any_element(),
            }
        });
        let (x, y) = match (placed, shown) {
            (Some(p), true) => p.card,
            _ => (0., 0.),
        };
        let card_el = card.map(|card| {
            tween("tour-card-x", x, ease(250.), move |x| {
                tween("tour-card-y", y, ease(250.), move |y| {
                    div().absolute().left(u(x)).top(u(y)).child(card)
                })
            })
        });
        // Кадр нарисован: рамки якорей этого кадра; якорь сдвинулся — подсветка вслед (`track`).
        let weak = cx.weak_entity();
        let frame = canvas(
            move |_, window, _| {
                anchors::settle();
                if anchor.and_then(anchors::get) != rect {
                    crate::motion::monitor_notify(window, weak.entity_id());
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size(gpui::px(0.));
        let size = window.viewport_size();
        let (next, prev) = (self.modal.scope().clone(), self.modal.scope().clone());
        // Контейнер окна — весь слой обучения, а не карточка: он в кадре с первого кадра (карточки
        // ещё нет, пока ищется якорь), и Esc и Tab доходят до него от любого фокуса внутри.
        // Клавиши — действия слоёв: привязка Esc срабатывает раньше сырых `keydown`.
        let layer = div()
            .id("tour")
            .track_focus(self.modal.scope())
            .tab_group()
            // Esc — выход всегда (`keydown` документа в фазе захвата); фокус вне обучения
            // отдаёт Esc сюда корень окна (`dismiss_top`).
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| this.finish(window, cx)))
            // Tab — по кругу внутри карточки (`trapFocus`).
            .on_action(move |_: &FocusNext, window, cx| trap(&next, true, window, cx))
            .on_action(move |_: &FocusPrev, window, cx| trap(&prev, false, window, cx))
            .relative()
            .w(size.width)
            .h(size.height)
            // Щит: пока видна карточка, мышь под обучением не доходит до вкладки.
            .when(index.is_some(), |el| {
                el.occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
            .child(ring_el)
            .children(card_el)
            .child(frame);
        deferred(anchored().position(Point::default()).child(layer))
            .with_priority(Z_TOAST)
            .into_any_element()
    }
}

/// Вид кнопки карточки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    /// `.primary`: заливка .14.
    Primary,
    /// `.ghost`: без заливки и рамки, текст тусклее.
    Ghost,
}

/// `.tour__btn`: поля 7 × 14, рамка `--l-border-strong`, фон .05, 13 px / 500; при наведении .1
/// (у всех видов: правило `:hover:not(:disabled)` сильнее), `:disabled` — .4.
#[derive(IntoElement)]
struct TourBtn {
    id: &'static str,
    label: String,
    kind: Kind,
    disabled: bool,
    focus: Option<FocusHandle>,
    on_click: Option<crate::kit::Handler<ClickEvent>>,
}

impl TourBtn {
    fn new(id: &'static str, label: String, kind: Kind) -> TourBtn {
        TourBtn {
            id,
            label,
            kind,
            disabled: false,
            focus: None,
            on_click: None,
        }
    }

    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    fn focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(std::rc::Rc::new(f));
        self
    }
}

impl gpui::RenderOnce for TourBtn {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let (text, dim, strong) = (
            theme.color("l-text"),
            theme.color("l-text-dim"),
            theme.color("l-border-strong"),
        );
        let focus_color = theme.color("accent-focus");
        let radius = theme.len("l-radius-sm");
        let zoom = zoom(window);
        let TourBtn {
            id,
            label,
            kind,
            disabled,
            focus,
            on_click,
        } = self;
        let element: ElementId = id.into();
        let focus = focus.unwrap_or_else(|| focus_handle(&element, window, cx));
        let ring_on = focus_visible(&focus, window);
        let meta = crate::live::Meta::new("button", element.clone())
            .label(Some(label.clone().into()))
            .disabled(disabled)
            .focused(focus.is_focused(window));
        let base = match kind {
            Kind::Plain => white(0.05),
            Kind::Primary => white(0.14),
            Kind::Ghost => white(0.),
        };
        let (border, color) = match kind {
            Kind::Ghost => (gpui::transparent_black(), dim),
            _ => (strong, text),
        };
        let button = hover(
            ElementId::Name(format!("{id}-hover").into()),
            Timing::fast(),
            move |t| {
                let t = if disabled { 0. } else { t };
                div()
                    .id(element)
                    .role(Role::Button)
                    .button_text()
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .py(u(7.))
                    .px(u(14.))
                    .border_1()
                    .border_color(border)
                    .rounded(u(radius))
                    .bg(mix(base, white(0.1), t))
                    .font_family(RUBIK)
                    .text_px(13., 13. * 1.185)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color)
                    .whitespace_nowrap()
                    .child(label)
                    .when(disabled, |el| el.opacity(0.4))
                    .when(!disabled, |el| {
                        el.track_focus(&focus).cursor_pointer().when_some(
                            on_click,
                            |el, handler| {
                                el.on_click(move |event, window, cx| handler(event, window, cx))
                            },
                        )
                    })
                    .when(ring_on, |el| {
                        el.child(outline(zoom, radius, 2., 2., focus_color))
                    })
            },
        );
        crate::live::mark(meta, button)
    }
}
