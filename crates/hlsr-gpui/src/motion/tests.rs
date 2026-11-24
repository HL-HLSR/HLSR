//! Движение в окне GPUI на тестовых часах (`advance_clock`): кадры просит только то, что
//! движется; уход доживает до конца; бесконечное на паузе кадров не просит.

use super::*;
use crate::scale::zpx;
use gpui::{
    Context, Render, TestAppContext, Window, WindowHandle, div, prelude::*, size,
    transparent_black, white,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// Кадр: что попросило кадр, получает его; число просивших.
fn frame<V: Render>(window: &WindowHandle<V>, cx: &mut TestAppContext) -> usize {
    let count = window
        .update(cx, |_, window, cx| window.simulate_next_frame(cx))
        .unwrap();
    cx.run_until_parked();
    count
}

fn open<V: Render>(cx: &mut TestAppContext, view: V) -> WindowHandle<V> {
    let window = cx.open_window(size(zpx(1., 200.), zpx(1., 200.)), move |_, _| view);
    cx.run_until_parked();
    window
}

fn wait(cx: &mut TestAppContext, ms: u64) {
    cx.executor().advance_clock(Duration::from_millis(ms));
}

struct Swipe {
    active: usize,
    direction: &'static str,
    positions: Rc<Cell<[gpui::Pixels; 2]>>,
}

impl Render for Swipe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let positions = self.positions.clone();
        swap("pages", self.active, self.direction, move |index| {
            let index = *index;
            let positions = positions.clone();
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let mut values = positions.get();
                    values[index] = window.paint_transform_bounds(bounds).left();
                    positions.set(values);
                },
            )
            .size_full()
        })
        .size_full()
        .overflow_hidden()
    }
}

#[gpui::test]
fn reversing_a_tab_slide_preserves_both_positions(cx: &mut TestAppContext) {
    let positions = Rc::new(Cell::new([gpui::px(0.); 2]));
    let window = open(
        cx,
        Swipe {
            active: 0,
            direction: SLIDE_NEXT,
            positions: positions.clone(),
        },
    );
    window
        .update(cx, |view, _, cx| {
            view.active = 1;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    wait(cx, 48);
    frame(&window, cx);
    let before_reverse = positions.get();
    assert!(before_reverse[0] < gpui::px(0.) && before_reverse[1] > gpui::px(0.));

    window
        .update(cx, |view, _, cx| {
            view.active = 0;
            view.direction = SLIDE_PREV;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(positions.get(), before_reverse);
    cx.executor().advance_clock(Duration::from_micros(4_167));
    frame(&window, cx);
    assert!(positions.get()[0] > before_reverse[0]);
    assert!(positions.get()[1] > before_reverse[1]);
}

struct Shown {
    show: bool,
    renders: Rc<Cell<usize>>,
}

impl Render for Shown {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let renders = self.renders.clone();
        div()
            .size_full()
            .child(presence("menu", self.show, POP, move || {
                renders.set(renders.get() + 1);
                div().size(u(10.))
            }))
    }
}

#[gpui::test]
fn leave_lives_to_the_end(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let window = open(
        cx,
        Shown {
            show: true,
            renders: renders.clone(),
        },
    );
    assert_eq!(renders.get(), 1);
    // Первый показ без входа: кадров не просит.
    assert_eq!(frame(&window, cx), 0);

    window
        .update(cx, |view, _, cx| {
            view.show = false;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(renders.get(), 2);
    // l-pop уходит .16 с: на 100 мс ещё в кадре.
    wait(cx, 100);
    assert!(frame(&window, cx) > 0);
    assert_eq!(renders.get(), 3);
    // На 200 мс ушёл: не строится и кадров больше не просит.
    wait(cx, 100);
    assert!(frame(&window, cx) > 0);
    assert_eq!(renders.get(), 3);
    assert_eq!(frame(&window, cx), 0);

    // Вход .26 с: на 130 мс идёт, на 300 мс встал.
    window
        .update(cx, |view, _, cx| {
            view.show = true;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(renders.get(), 4);
    wait(cx, 130);
    assert!(frame(&window, cx) > 0);
    wait(cx, 170);
    assert!(frame(&window, cx) > 0);
    assert_eq!(frame(&window, cx), 0);
}

struct Swapped {
    key: u32,
    seen: Rc<Cell<Vec<u32>>>,
}

impl Render for Swapped {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.seen.set(Vec::new());
        let seen = self.seen.clone();
        div()
            .size_full()
            .child(swap("value", self.key, VALUE, move |key| {
                let mut keys = seen.take();
                keys.push(*key);
                seen.set(keys);
                div().size(u(10.))
            }))
    }
}

#[gpui::test]
fn value_crossfades_old_and_new(cx: &mut TestAppContext) {
    let seen = Rc::new(Cell::new(Vec::new()));
    let window = open(
        cx,
        Swapped {
            key: 1,
            seen: seen.clone(),
        },
    );
    window
        .update(cx, |view, _, cx| {
            view.key = 2;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    // l-value .5 с: старое и новое вместе, новое сверху (последним).
    wait(cx, 250);
    frame(&window, cx);
    assert_eq!(seen.take(), vec![1, 2]);
    wait(cx, 300);
    frame(&window, cx);
    assert_eq!(seen.take(), vec![2]);
    assert_eq!(frame(&window, cx), 0);
}

struct Chosen {
    key: Option<u32>,
    seen: Rc<Cell<Vec<u32>>>,
}

impl Render for Chosen {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.seen.set(Vec::new());
        let seen = self.seen.clone();
        div().size_full().child(
            choose("note", self.key, FADE, move |key| {
                let mut keys = seen.take();
                keys.push(*key);
                seen.set(keys);
                div().size(u(10.))
            })
            .out_in(),
        )
    }
}

#[gpui::test]
fn choose_enters_from_nothing_at_once_and_after_the_leave_otherwise(cx: &mut TestAppContext) {
    let seen = Rc::new(Cell::new(Vec::new()));
    let window = open(
        cx,
        Chosen {
            key: None,
            seen: seen.clone(),
        },
    );
    assert_eq!(seen.take(), Vec::<u32>::new());
    let set = |cx: &mut TestAppContext, key: Option<u32>| {
        window
            .update(cx, |view, _, cx| {
                view.key = key;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
    };
    // Из ничего — сразу (уходить нечему).
    set(cx, Some(1));
    assert_eq!(seen.take(), vec![1]);
    wait(cx, 300);
    frame(&window, cx);
    // out-in: новое ждёт ухода старого (l-fade уходит .16 с).
    set(cx, Some(2));
    wait(cx, 100);
    frame(&window, cx);
    assert_eq!(seen.take(), vec![1]);
    wait(cx, 100);
    frame(&window, cx);
    assert_eq!(seen.take(), vec![2]);

    wait(cx, 300);
    frame(&window, cx);
    set(cx, Some(3));
    wait(cx, 40);
    frame(&window, cx);
    assert_eq!(seen.take(), vec![2]);
    // Смена ещё ждущего ключа не продлевает текущий уход.
    set(cx, Some(4));
    wait(cx, 130);
    frame(&window, cx);
    assert_eq!(seen.take(), vec![4]);
}

struct Pulse {
    name: &'static str,
    layer: Option<usize>,
    top: f32,
}

impl Render for Pulse {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let top = self.top;
        let dot = keyframes("dot", self.name, move |frame| {
            div()
                .absolute()
                .top(u(top))
                .size(u(10.))
                .opacity(frame.opacity as f32)
        });
        let root = div().relative().size_full();
        match self.layer {
            Some(depth) => root.child(layer(depth, dot)).into_any_element(),
            None => root.child(dot).into_any_element(),
        }
    }
}

fn set_gate(cx: &mut TestAppContext, change: impl FnOnce(&mut gpui::App)) {
    cx.update(change);
    cx.run_until_parked();
}

/// После смены ворот: кадр, попрошенный до неё, приходит один раз; дальше — ни одного.
fn paused<V: Render>(window: &WindowHandle<V>, cx: &mut TestAppContext) -> bool {
    wait(cx, 16);
    frame(window, cx);
    wait(cx, 16);
    frame(window, cx) == 0
}

#[gpui::test]
fn paused_infinite_asks_no_frames(cx: &mut TestAppContext) {
    let pulse = |layer, top| Pulse {
        name: PULSE,
        layer,
        top,
    };
    let window = open(cx, pulse(None, 0.));
    assert!(frame(&window, cx) > 0);
    assert!(frame(&window, cx) > 0);

    // Окно спрятано — стоит; показано — снова идёт.
    set_gate(cx, |cx| AnimGate::set_window_hidden(cx, true));
    assert!(paused(&window, cx));
    set_gate(cx, |cx| AnimGate::set_window_hidden(cx, false));
    assert!(frame(&window, cx) > 0);
    assert!(frame(&window, cx) > 0);

    // Поверх окно-слой: основа стоит.
    set_gate(cx, |cx| AnimGate::set_top_layer(cx, 1));
    assert!(paused(&window, cx));

    // Внутри верхнего слоя идёт.
    let inside = open(cx, pulse(Some(1), 0.));
    assert!(frame(&inside, cx) > 0);
    assert!(frame(&inside, cx) > 0);
    set_gate(cx, |cx| AnimGate::set_top_layer(cx, 0));

    // Вне видимой области (ниже окна 200 px) — стоит.
    let below = open(cx, pulse(None, 400.));
    assert!(paused(&below, cx));
}

struct Life {
    id: &'static str,
    since: Option<std::time::Instant>,
    seen: Rc<Cell<f64>>,
}

impl Render for Life {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let seen = self.seen.clone();
        let life = keyframes(self.id, "toast-life", move |frame| {
            seen.set(frame.scale_x);
            div().size(u(10.))
        });
        div().size_full().child(match self.since {
            Some(at) => life.since(at),
            None => life,
        })
    }
}

/// `since`: элемент, перестроенный под другим id (уходящий тост), продолжает анимацию с того же
/// момента, а не начинает её заново.
#[gpui::test]
fn keyframes_since_continue_from_the_given_moment(cx: &mut TestAppContext) {
    let seen = Rc::new(Cell::new(-1.));
    let start = cx.executor().now();
    let window = open(
        cx,
        Life {
            id: "life",
            since: None,
            seen: seen.clone(),
        },
    );
    assert!(seen.get() > 0.99, "{}", seen.get());
    wait(cx, 3000);
    let rebuild = |id: &'static str, since, cx: &mut TestAppContext| {
        window
            .update(cx, |view, _, cx| {
                view.id = id;
                view.since = since;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
    };
    // Полоса жизни (2,8 с) уже растаяла и такой остаётся; кадров (кроме попрошенного раньше)
    // не просит.
    rebuild("life-moved", Some(start), cx);
    assert!(seen.get() < 0.01, "{}", seen.get());
    frame(&window, cx);
    assert!(seen.get() < 0.01, "{}", seen.get());
    assert_eq!(frame(&window, cx), 0);
    // Без `since` новый id начинает с начала.
    rebuild("life-new", None, cx);
    assert!(seen.get() > 0.99, "{}", seen.get());
}
#[cfg_attr(not(windows), allow(dead_code))]
struct Counted {
    renders: Rc<Cell<usize>>,
}

impl Render for Counted {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().size_full().child(keyframes("dot", PULSE, |frame| {
            div().size(u(10.)).opacity(frame.opacity as f32)
        }))
    }
}

/// Бесконечное движение перерисовывает окно по сетке монитора окна, а не с каждым будильником:
/// будильник раз в 8 мс на мониторе 60 Гц — каждый второй; раз в 9 мс на мониторе 120 Гц (шаг —
/// кадр монитора, 8,3 мс) — каждый, 12 из 12. Сетка — только Windows (`driver::grid`): на Linux
/// будильник и есть кадр монитора окна.
#[cfg(windows)]
#[gpui::test]
fn infinite_frames_follow_the_monitor(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let window = open(
        cx,
        Counted {
            renders: renders.clone(),
        },
    );
    let drawn = |cx: &mut TestAppContext, step: u64, frames: usize| {
        let before = renders.get();
        for _ in 0..frames {
            wait(cx, step);
            frame(&window, cx);
        }
        renders.get() - before
    };
    assert_eq!(drawn(cx, 8, 12), 6);
    assert_eq!(drawn(cx, 16, 6), 6);
    let display = window
        .update(cx, |_, window, cx| window.display(cx).expect("экран").id())
        .unwrap();
    cx.simulate_display_refresh_interval_change(display, Some(Duration::from_secs(1) / 120));
    assert_eq!(drawn(cx, 9, 12), 12);
}

#[cfg_attr(not(windows), allow(dead_code))]
struct Mixed {
    target: f32,
    renders: Rc<Cell<usize>>,
}

impl Render for Mixed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .size_full()
            .child(keyframes("dot", PULSE, |frame| {
                div().size(u(10.)).opacity(frame.opacity as f32)
            }))
            .child(tween(
                "bar",
                self.target,
                Timing::new(400., [0., 0., 1., 1.]),
                |t| div().size(u(10.)).bg(mix(transparent_black(), white(), t)),
            ))
    }
}

/// Переход вместе с бесконечным движением — одна сетка кадров окна (монитор 120 Гц — 120 к/с)
/// при будильнике 280 Гц: у них были свои сетки со сдвигом фазы, и кадры шли по 4/4/8 мс.
/// Будильник быстрее монитора окна — только Windows (`driver::grid`).
#[cfg(windows)]
#[gpui::test]
fn finite_and_infinite_share_the_monitor_frames(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let window = open(
        cx,
        Mixed {
            target: 0.,
            renders: renders.clone(),
        },
    );
    let display = window
        .update(cx, |_, window, cx| window.display(cx).expect("экран").id())
        .unwrap();
    cx.simulate_display_refresh_interval_change(display, Some(Duration::from_secs(1) / 120));
    window
        .update(cx, |view, _, cx| {
            view.target = 1.;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    let before = renders.get();
    let mut last = cx.executor().now();
    let mut gaps = Vec::new();
    // 0,3 с будильников 280 Гц.
    for _ in 0..84 {
        cx.executor().advance_clock(Duration::from_micros(3_571));
        let was = renders.get();
        frame(&window, cx);
        if renders.get() > was {
            let now = cx.executor().now();
            gaps.push(now - last);
            last = now;
        }
    }
    let drawn = renders.get() - before;
    assert!((32..=37).contains(&drawn), "{drawn} кадров за 0,3 с");
    assert!(
        gaps.iter()
            .skip(1)
            .all(|gap| *gap > Duration::from_micros(6_250)),
        "{gaps:?}"
    );
}

#[gpui::test]
fn still_stops_motion(cx: &mut TestAppContext) {
    set_gate(cx, |cx| AnimGate::set_reduce_motion(cx, true));
    assert!(cx.update(|cx| cx.reduce_motion()));
    // Свечение значка при reduced-motion стоит (still: none).
    let glow = open(
        cx,
        Pulse {
            name: "nick-badge-glow",
            layer: None,
            top: 0.,
        },
    );
    assert_eq!(frame(&glow, cx), 0);
    // Уход без анимации: в следующем кадре элемента нет.
    let renders = Rc::new(Cell::new(0));
    let shown = open(
        cx,
        Shown {
            show: true,
            renders: renders.clone(),
        },
    );
    shown
        .update(cx, |view, _, cx| {
            view.show = false;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(renders.get(), 1);
    assert_eq!(frame(&shown, cx), 0);
}

struct Tweened {
    target: f32,
    values: Rc<Cell<Vec<f32>>>,
}

impl Render for Tweened {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let values = self.values.clone();
        div().child(tween("bg", self.target, Timing::fast(), move |t| {
            let mut all = values.take();
            all.push(t);
            values.set(all);
            div().size(u(10.)).bg(mix(transparent_black(), white(), t))
        }))
    }
}

#[gpui::test]
fn tween_takes_exact_time(cx: &mut TestAppContext) {
    let values = Rc::new(Cell::new(Vec::new()));
    let window = open(
        cx,
        Tweened {
            target: 0.,
            values: values.clone(),
        },
    );
    window
        .update(cx, |view, _, cx| {
            view.target = 1.;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    wait(cx, 80);
    frame(&window, cx);
    // cubic-bezier(.22,1,.36,1) в середине времени ≈ 0,96.
    let mid = *values.take().last().unwrap();
    assert!(mid > 0.9 && mid < 1., "{mid}");
    wait(cx, 81);
    frame(&window, cx);
    assert_eq!(values.take().last(), Some(&1.));
    assert_eq!(frame(&window, cx), 0);
}

struct Hovered {
    values: Rc<Cell<Vec<f32>>>,
}

impl Render for Hovered {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let values = self.values.clone();
        div().size_full().child(hover(
            "button",
            Timing::new(0., [0., 0., 1., 1.]),
            move |t| {
                let mut all = values.take();
                all.push(t);
                values.set(all);
                // Кнопка окна: `.occlude()` загораживает всё, что под ней.
                div()
                    .id("button")
                    .occlude()
                    .size(u(20.))
                    .bg(mix(transparent_black(), white(), t))
            },
        ))
    }
}

struct Popped {
    show: bool,
    seen: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    outer: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
}

impl Render for Popped {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (seen, outer) = (self.seen.clone(), self.outer.clone());
        let probe = |cell: Rc<Cell<gpui::Bounds<gpui::Pixels>>>| {
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| cell.set(window.paint_transform_bounds(bounds)),
            )
            .absolute()
            .size_full()
        };
        div().size_full().child(
            div()
                .relative()
                .w(u(150.))
                .child(probe(outer))
                .child(
                    presence("pop", self.show, POP, move || {
                        div().relative().size(u(100.)).child(probe(seen))
                    })
                    .origin(Origin::new((1., 0.), (0., 0.))),
                )
                .child(div().h(u(20.))),
        )
    }
}

#[gpui::test]
fn hover_sees_occluding_child(cx: &mut TestAppContext) {
    let values = Rc::new(Cell::new(Vec::new()));
    let (_, cx) = cx.add_window_view(|_, _| Hovered {
        values: values.clone(),
    });
    cx.run_until_parked();
    assert_eq!(values.take().last(), Some(&0.));
    cx.simulate_mouse_move(
        gpui::point(zpx(1., 10.), zpx(1., 10.)),
        None,
        gpui::Modifiers::none(),
    );
    assert_eq!(values.take().last(), Some(&1.));
    cx.simulate_mouse_move(
        gpui::point(zpx(1., 100.), zpx(1., 100.)),
        None,
        gpui::Modifiers::none(),
    );
    assert_eq!(values.take().last(), Some(&0.));
}

/// Кнопка с наведением и, по `cover`, подложка окна-диалога поверх всего (`.occlude()`).
struct Covered {
    values: Rc<Cell<Vec<f32>>>,
    cover: bool,
}

impl Render for Covered {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let values = self.values.clone();
        div()
            .size_full()
            .child(hover(
                "button",
                Timing::new(0., [0., 0., 1., 1.]),
                move |t| {
                    let mut all = values.take();
                    all.push(t);
                    values.set(all);
                    div().id("button").size(u(20.))
                },
            ))
            .when(self.cover, |el| {
                el.child(div().id("scrim").absolute().inset_0().occlude())
            })
    }
}

/// Окно открылось поверх кнопки под неподвижной мышью: наведение уходит без движения мыши
/// (подложка `.news-viewer`/`.settings-modal` перекрывает кнопку колонки, как в Chromium).
#[gpui::test]
fn hover_leaves_when_a_layer_covers_a_still_pointer(cx: &mut TestAppContext) {
    let values = Rc::new(Cell::new(Vec::new()));
    let (view, cx) = cx.add_window_view(|_, _| Covered {
        values: values.clone(),
        cover: false,
    });
    cx.run_until_parked();
    cx.simulate_mouse_move(
        gpui::point(zpx(1., 10.), zpx(1., 10.)),
        None,
        gpui::Modifiers::none(),
    );
    assert_eq!(values.take().last(), Some(&1.));
    view.update(cx, |view, cx| {
        view.cover = true;
        cx.notify();
    });
    cx.run_until_parked();
    for _ in 0..3 {
        cx.executor().advance_clock(Duration::from_millis(20));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    assert_eq!(values.take().last(), Some(&0.));
}

struct Held {
    show: bool,
    value: u32,
    seen: Rc<Cell<u32>>,
}

impl Render for Held {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.seen
            .set(held("status", self.show, self.value, window, cx));
        div()
    }
}

/// Уходящее держит то, что было при показе; показанное — текущее.
#[gpui::test]
fn held_keeps_the_last_shown_value_while_leaving(cx: &mut TestAppContext) {
    let seen = Rc::new(Cell::new(0));
    let window = open(
        cx,
        Held {
            show: true,
            value: 1,
            seen: seen.clone(),
        },
    );
    assert_eq!(seen.get(), 1);
    let set = |show: bool, value: u32, cx: &mut TestAppContext| {
        window
            .update(cx, |view, _, cx| {
                (view.show, view.value) = (show, value);
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        seen.get()
    };
    assert_eq!(set(false, 2, cx), 1, "уходит с прежним");
    assert_eq!(set(true, 3, cx), 3);
}

struct Holding {
    values: Rc<Cell<Vec<f32>>>,
}

impl Render for Holding {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let values = self.values.clone();
        div().size_full().child(active("button", true, move |held| {
            let mut all = values.take();
            all.push(f32::from(u8::from(held)));
            values.set(all);
            div().id("button").occlude().size(u(20.))
        }))
    }
}

/// `:active`: нажатие над кнопкой держится, пока кнопку мыши не отпустят где угодно; нажатие
/// мимо её не трогает.
#[gpui::test]
fn active_holds_until_release_anywhere(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, point};
    let values = Rc::new(Cell::new(Vec::new()));
    let (_, cx) = cx.add_window_view(|_, _| Holding {
        values: values.clone(),
    });
    cx.run_until_parked();
    let (inside, outside) = (
        point(zpx(1., 10.), zpx(1., 10.)),
        point(zpx(1., 100.), zpx(1., 100.)),
    );
    cx.simulate_mouse_move(inside, None, Modifiers::none());
    cx.simulate_mouse_down(inside, MouseButton::Left, Modifiers::none());
    assert_eq!(values.take().last(), Some(&1.));
    cx.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::none());
    assert_ne!(
        values.take().last(),
        Some(&0.),
        "ушла мышь — нажатие держится"
    );
    cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());
    assert_eq!(values.take().last(), Some(&0.));
    cx.simulate_mouse_down(outside, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());
    assert!(values.take().iter().all(|value| *value == 0.));
    // MouseUp потерян (Alt+Tab): нажатие снимает первое движение без левой кнопки.
    cx.simulate_mouse_move(inside, None, Modifiers::none());
    cx.simulate_mouse_down(inside, MouseButton::Left, Modifiers::none());
    assert_eq!(values.take().last(), Some(&1.));
    cx.simulate_mouse_move(outside, None, Modifiers::none());
    assert_eq!(values.take().last(), Some(&0.));
}

#[gpui::test]
fn pop_scales_around_its_origin_without_moving_the_layout(cx: &mut TestAppContext) {
    let seen = Rc::new(Cell::new(gpui::Bounds::default()));
    let outer = Rc::new(Cell::new(gpui::Bounds::default()));
    let window = open(
        cx,
        Popped {
            show: false,
            seen: seen.clone(),
            outer: outer.clone(),
        },
    );
    let rest = outer.get();
    window
        .update(cx, |view, _, cx| {
            view.show = true;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    // С первого же кадра входа масштаб применяется в `paint` без пропуска кадра (l-pop: .97).
    assert!(
        (seen.get().size.width - zpx(1., 97.)).abs() < zpx(1., 0.1),
        "{:?}",
        seen.get()
    );
    let open_height = outer.get().size.height;
    wait(cx, 10);
    frame(&window, cx);
    // .97 → 1: уже меньше, правый верхний угол на месте, раскладка вокруг та же.
    let mid = seen.get();
    assert!(
        mid.size.width < zpx(1., 99.9) && mid.size.width > zpx(1., 96.9),
        "{mid:?}"
    );
    assert!(
        (mid.right() - zpx(1., 100.)).abs() < zpx(1., 0.01),
        "{mid:?}"
    );
    // Верх — на месте, только translateY(-4px) входа (`l-pop`).
    assert!(
        mid.top() <= zpx(1., 0.) && mid.top() > zpx(1., -4.),
        "{mid:?}"
    );
    assert_eq!(outer.get().size.height, open_height);
    assert!(rest.size.height < open_height);
    wait(cx, 300);
    frame(&window, cx);
    frame(&window, cx);
    assert_eq!(seen.get().size.width, zpx(1., 100.));
    assert_eq!(seen.get().origin.x, zpx(1., 0.));
}

struct Pressed {
    live: bool,
    forced: bool,
    seen: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    next: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
}

impl Render for Pressed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let probe = |cell: Rc<Cell<gpui::Bounds<gpui::Pixels>>>| {
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| cell.set(window.paint_transform_bounds(bounds)),
            )
            .absolute()
            .size_full()
        };
        // Кнопка с отступом в ряду: место в потоке при нажатии не меняется.
        let button = div()
            .id("b")
            .relative()
            .ml(u(20.))
            .w(u(100.))
            .h(u(40.))
            .child(probe(self.seen.clone()));
        div()
            .flex()
            .w(u(200.))
            .child(
                press("press", Push::fast(0.9), button)
                    .live(self.live)
                    .forced(self.forced),
            )
            .child(
                div()
                    .relative()
                    .w(u(30.))
                    .h(u(10.))
                    .child(probe(self.next.clone())),
            )
    }
}

#[gpui::test]
fn press_scales_in_place_while_held(cx: &mut TestAppContext) {
    let seen = Rc::new(Cell::new(gpui::Bounds::default()));
    let next = Rc::new(Cell::new(gpui::Bounds::default()));
    let window = open(
        cx,
        Pressed {
            live: true,
            forced: false,
            seen: seen.clone(),
            next: next.clone(),
        },
    );
    let rest = seen.get();
    let rest_next = next.get();
    assert_eq!(rest.origin.x, zpx(1., 20.));
    let mouse = |down: bool, cx: &mut TestAppContext| {
        window
            .update(cx, |_, window, cx| {
                crate::snap::press(
                    window,
                    cx,
                    gpui::point(zpx(1., 70.), zpx(1., 20.)),
                    gpui::MouseButton::Left,
                    1,
                    down,
                )
            })
            .unwrap();
        cx.run_until_parked();
    };
    // Мышь ушла с зажатой кнопки и отпущена за ней: `:active` до отпускания, как в Chromium.
    let outside = gpui::point(zpx(1., 180.), zpx(1., 150.));
    let away = |up: bool, cx: &mut TestAppContext| {
        window
            .update(cx, |_, window, cx| {
                crate::snap::pointer_move(window, cx, outside);
                if up {
                    crate::snap::press(window, cx, outside, gpui::MouseButton::Left, 1, false);
                }
            })
            .unwrap();
        cx.run_until_parked();
    };
    mouse(true, cx);
    // --l-fast .16 с: к концу — scale(.9) вокруг центра, сосед на месте.
    wait(cx, 170);
    frame(&window, cx);
    frame(&window, cx);
    let held = seen.get();
    assert!(
        (held.size.width - zpx(1., 90.)).abs() < zpx(1., 0.5),
        "{held:?}"
    );
    assert!(
        (held.center().x - rest.center().x).abs() < zpx(1., 0.5),
        "{held:?}"
    );
    assert!(
        (held.center().y - rest.center().y).abs() < zpx(1., 0.5),
        "{held:?}"
    );
    assert_eq!(next.get(), rest_next);
    assert_eq!(frame(&window, cx), 0);
    away(false, cx);
    frame(&window, cx);
    assert_eq!(seen.get(), held);
    // Отпустили где угодно — обратно к 1.
    away(true, cx);
    wait(cx, 170);
    frame(&window, cx);
    frame(&window, cx);
    assert_eq!(seen.get(), rest);
    assert_eq!(frame(&window, cx), 0);

    // `:disabled` не нажимается.
    window
        .update(cx, |view, _, cx| {
            view.live = false;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    mouse(true, cx);
    wait(cx, 170);
    frame(&window, cx);
    assert_eq!(seen.get(), rest);
    mouse(false, cx);

    // Принудительное `:active` (витрина): сжата без мыши, но не у неживой.
    window
        .update(cx, |view, _, cx| {
            view.forced = true;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    frame(&window, cx);
    assert_eq!(seen.get(), rest);
    window
        .update(cx, |view, _, cx| {
            view.live = true;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    wait(cx, 170);
    frame(&window, cx);
    frame(&window, cx);
    assert_eq!(seen.get(), held);
}

#[test]
fn scale_steps_are_continuous() {
    assert_eq!(zoom::step(1.), 1.);
    assert!((zoom::step(0.97) - 0.97).abs() < 1e-6);
    assert!((zoom::step(0.9701) - zoom::step(0.9702)).abs() > 1e-5);
}

#[gpui::test]
fn swap_keeps_cached_views_cached_during_slide(cx: &mut TestAppContext) {
    struct Leaf(Rc<Cell<usize>>);
    impl Render for Leaf {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            div().size_full().bg(white())
        }
    }
    struct Host {
        tab: usize,
        pages: [gpui::Entity<Leaf>; 2],
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let pages = self.pages.clone();
            div().size(u(400.)).child(
                swap("slide-cache", self.tab, SLIDE_NEXT, move |&tab| {
                    crate::cache::cached(
                        pages[tab].clone(),
                        gpui::StyleRefinement::default().size_full(),
                        true,
                    )
                })
                .size_full()
                .overflow_hidden(),
            )
        }
    }
    let a = Rc::new(Cell::new(0));
    let b = Rc::new(Cell::new(0));
    let (a_, b_) = (a.clone(), b.clone());
    let (host, cx) = cx.add_window_view(|_, cx| Host {
        tab: 0,
        pages: [cx.new(|_| Leaf(a_)), cx.new(|_| Leaf(b_))],
    });
    cx.run_until_parked();
    let a_before = a.get();
    host.update(cx, |h, cx| {
        h.tab = 1;
        cx.notify();
    });
    cx.run_until_parked();
    let (a_start, b_start) = (a.get(), b.get());
    // На первом кадре свайпа уходящий вид А не перерисовывался вовсе, входящий B нарисовался 1 раз.
    assert_eq!(a_start, a_before);
    assert_eq!(b_start, 1);
    // Несколько кадров свайпа подряд: оба закэшированных вида берутся из кэша (0 перерисовок).
    for _ in 0..5 {
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    assert_eq!(
        a.get(),
        a_start,
        "уходящий кэшированный вид не должен перерисовываться в свайпе"
    );
    assert_eq!(
        b.get(),
        b_start,
        "входящий кэшированный вид не должен перерисовываться в свайпе"
    );
}

#[test]
fn mix_is_premultiplied() {
    let half = Rgba::from(mix(transparent_black(), white(), 0.5));
    assert_eq!((half.r, half.a), (1., 0.5));
}

#[test]
fn registry_has_049_presets() {
    let registry = registry();
    for name in [POP, FADE, MODAL, SWAP, SLIDE_NEXT, SLIDE_PREV, VALUE] {
        assert!(registry.transitions.contains_key(name), "{name}");
    }
    // INSTANT встаёт и уходит сразу только потому, что такого перехода в реестре нет.
    assert!(!registry.transitions.contains_key(INSTANT));
    for name in [RISE_IN, PULSE, SPIN] {
        assert!(registry.animations.contains_key(name), "{name}");
    }
    // Лесенка l-rise-in: третий элемент стоит на 48 мс.
    let before = registry.keyframes_frame(RISE_IN, Some(40.), 2, Still::MOVING, true);
    assert_eq!((before.opacity, before.y), (0., 8.));
    let swap = registry.transition_frame(SWAP, false, None, Still::MOVING, None);
    assert_eq!((swap.opacity, swap.y), (0., -4.));
}
