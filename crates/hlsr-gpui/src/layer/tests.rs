//! Слои в окне GPUI: Esc закрывает верхнее окно, Tab не выходит из окна, окно в полосе шапки не
//! таскает окно лаунчера, меню стрелками, тосты по времени, подсказка после нажатия.

use super::*;
use crate::motion::AnimGate;
use crate::scale::u;
use gpui::{
    AnyElement, Bounds, Context, Entity, FocusHandle, Modifiers, MouseButton, Pixels, Point,
    Render, TestAppContext, VisualTestContext, Window, WindowControlArea, div, point, prelude::*,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

struct Scene {
    outer: FocusHandle,
    a: Modal,
    b: Modal,
    a_items: [FocusHandle; 2],
    b_item: FocusHandle,
    /// Кнопка меню в окне `a`.
    trigger: FocusHandle,
    menu: MenuNav,
    picked: Option<usize>,
    anchor: Rc<Cell<Bounds<Pixels>>>,
    drag_hover: Rc<Cell<bool>>,
    drag_down: Rc<Cell<usize>>,
}

const LABELS: [&str; 4] = ["Германия", "Россия", "Румыния", "Украина"];
const DISABLED: [bool; 4] = [false, true, false, false];

impl Scene {
    fn new(cx: &mut Context<Scene>) -> Scene {
        let stop = |cx: &mut Context<Scene>| cx.focus_handle().tab_stop(true);
        Scene {
            outer: stop(cx),
            a: Modal::new("a", cx),
            b: Modal::new("b", cx).on_top(),
            a_items: [stop(cx), stop(cx)],
            b_item: stop(cx),
            trigger: stop(cx),
            menu: MenuNav::default(),
            picked: None,
            anchor: Rc::default(),
            drag_hover: Rc::default(),
            drag_down: Rc::default(),
        }
    }
}

fn stop_el(id: &'static str, handle: &FocusHandle) -> AnyElement {
    div()
        .id(id)
        .track_focus(handle)
        .w(u(40.))
        .h(u(20.))
        .into_any_element()
}

impl Render for Scene {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = window.viewport_size();
        let (hover, down) = (self.drag_hover.clone(), self.drag_down.clone());
        // Окно `a` занимает 10–90 % высоты: верх его лежит в полосе шапки (0–20 %).
        let a_items = self.a_items.clone();
        let entity = cx.entity().downgrade();
        let trigger = menu_keys(
            div()
                .id("trigger")
                .track_focus(&self.trigger)
                .relative()
                .w(u(40.))
                .h(u(20.))
                .child(anchor_probe(self.anchor.clone())),
            move |key, _, cx| {
                entity
                    .update(cx, |this, cx| {
                        let now = cx.background_executor().now();
                        if let Outcome::Pick(index) =
                            this.menu.key(key, now, &LABELS, &DISABLED, -1).settle(cx)
                        {
                            this.picked = Some(index);
                        }
                        cx.notify();
                    })
                    .ok();
            },
        );
        let on_dismiss = listen(cx, |this: &mut Scene, _, cx| {
            this.menu.hide();
            cx.notify();
        });
        let menu = popup(
            "menu",
            self.menu.open,
            Place {
                anchor: self.anchor.get(),
                clip: None,
                up: false,
                align_right: false,
            },
            PopLook::DROPDOWN,
            window,
            cx,
            on_dismiss,
            || div().w(u(100.)).h(u(60.)),
        );
        let a_content = div()
            .w(u(200.))
            .h(view.height * 0.8)
            .child(stop_el("a0", &a_items[0]))
            .child(stop_el("a1", &a_items[1]))
            .child(trigger)
            .child(menu);
        let b_item = self.b_item.clone();
        let close_a = listen(cx, |this: &mut Scene, window, cx| this.a.close(window, cx));
        let close_b = listen(cx, |this: &mut Scene, window, cx| this.b.close(window, cx));
        let modal_a = self.a.render(window, cx, close_a, move || a_content);
        let modal_b = self.b.render(window, cx, close_b, move || {
            div().size(u(50.)).child(stop_el("b0", &b_item))
        });
        tab_root(div().size_full().relative())
            .child(
                div()
                    .id("header")
                    .window_control_area(WindowControlArea::Drag)
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(view.height * 0.2)
                    .on_hover(move |hovered, _, _| hover.set(*hovered))
                    .on_mouse_down(MouseButton::Left, move |_, _, _| down.set(down.get() + 1)),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .child(stop_el("outer", &self.outer)),
            )
            .child(modal_a)
            .child(modal_b)
    }
}

fn scene(cx: &mut TestAppContext) -> (Entity<Scene>, &mut VisualTestContext) {
    cx.update(crate::init);
    let (view, cx) = cx.add_window_view(|_, cx| Scene::new(cx));
    cx.run_until_parked();
    (view, cx)
}

/// Кадр после открытия: фокус в окно ставится в следующем кадре.
fn next_frame(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
}

fn focused(cx: &mut VisualTestContext, handle: &FocusHandle) -> bool {
    let handle = handle.clone();
    cx.update(|window, _| handle.is_focused(window))
}

fn open(view: &Entity<Scene>, cx: &mut VisualTestContext, which: fn(&mut Scene) -> &mut Modal) {
    view.update_in(cx, |scene, window, cx| {
        which(scene).open(window, cx);
        cx.notify();
    });
    next_frame(cx);
}

#[gpui::test]
fn escape_closes_the_top_window_and_focus_returns(cx: &mut TestAppContext) {
    let (view, cx) = scene(cx);
    let outer = view.read_with(cx, |scene, _| scene.outer.clone());
    cx.update(|window, cx| window.focus(&outer, cx));

    open(&view, cx, |scene| &mut scene.a);
    let (a0, b0) = view.read_with(cx, |scene, _| {
        (scene.a_items[0].clone(), scene.b_item.clone())
    });
    assert!(focused(cx, &a0), "первый элемент окна в фокусе");
    open(&view, cx, |scene| &mut scene.b);
    assert!(focused(cx, &b0));
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 2);
    assert_eq!(cx.update(|_, cx| AnimGate::get(cx).top_layer), 2);

    cx.simulate_keystrokes("escape");
    let open_now = |cx: &mut VisualTestContext| {
        view.read_with(cx, |scene, _| (scene.a.is_open(), scene.b.is_open()))
    };
    assert_eq!(open_now(cx), (true, false), "закрылось верхнее");
    assert!(focused(cx, &a0), "фокус вернулся туда, где был");
    assert_eq!(cx.update(|_, cx| AnimGate::get(cx).top_layer), 1);

    cx.simulate_keystrokes("escape");
    assert_eq!(open_now(cx), (false, false));
    assert!(focused(cx, &outer));
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 0);
}

#[gpui::test]
fn escape_reaches_the_top_window_with_focus_outside(cx: &mut TestAppContext) {
    let (view, cx) = scene(cx);
    let outer = view.read_with(cx, |scene, _| scene.outer.clone());
    cx.update(|window, cx| window.focus(&outer, cx));
    open(&view, cx, |scene| &mut scene.a);
    open(&view, cx, |scene| &mut scene.b);
    // Фокус ушёл под подложку (колокольчик после Esc панели уведомлений).
    cx.update(|window, cx| window.focus(&outer, cx));
    let open_now = |cx: &mut VisualTestContext| {
        view.read_with(cx, |scene, _| (scene.a.is_open(), scene.b.is_open()))
    };
    cx.simulate_keystrokes("escape");
    assert_eq!(open_now(cx), (true, false), "закрылось верхнее");
    cx.update(|window, cx| window.focus(&outer, cx));
    cx.simulate_keystrokes("escape");
    assert_eq!(open_now(cx), (false, false));
    assert!(focused(cx, &outer), "фокус вернулся туда, где был до окна");
}

#[gpui::test]
fn tab_stays_inside_the_window(cx: &mut TestAppContext) {
    let (view, cx) = scene(cx);
    open(&view, cx, |scene| &mut scene.a);
    let (outer, a0, a1, trigger) = view.read_with(cx, |scene, _| {
        (
            scene.outer.clone(),
            scene.a_items[0].clone(),
            scene.a_items[1].clone(),
            scene.trigger.clone(),
        )
    });
    assert!(focused(cx, &a0));
    cx.simulate_keystrokes("tab");
    assert!(focused(cx, &a1));
    cx.simulate_keystrokes("tab");
    assert!(focused(cx, &trigger));
    cx.simulate_keystrokes("tab");
    assert!(focused(cx, &a0), "с последнего — на первый");
    cx.simulate_keystrokes("shift-tab");
    assert!(focused(cx, &trigger), "с первого — на последний");
    cx.simulate_keystrokes("shift-tab");
    assert!(focused(cx, &a1));
    // Фокус вне окна (щелчок по чужому): Tab возвращает его в окно.
    cx.update(|window, cx| window.focus(&outer, cx));
    cx.simulate_keystrokes("tab");
    assert!(focused(cx, &a0));
    cx.update(|window, cx| window.focus(&outer, cx));
    cx.simulate_keystrokes("shift-tab");
    assert!(focused(cx, &trigger));
}

#[gpui::test]
fn window_over_the_header_strip_does_not_drag(cx: &mut TestAppContext) {
    let (view, cx) = scene(cx);
    let size = cx.update(|window, _| window.viewport_size());
    // Верх окна в полосе шапки и подложка в ней же, сбоку от окна.
    let on_window = point(size.width * 0.5, size.height * 0.15);
    let on_scrim = point(size.width * 0.02, size.height * 0.15);
    let mouse = |cx: &mut VisualTestContext, at: Point<Pixels>| {
        cx.simulate_mouse_move(at, None, Modifiers::none());
        cx.run_until_parked();
    };
    let (hover, down) = view.read_with(cx, |scene, _| {
        (scene.drag_hover.clone(), scene.drag_down.clone())
    });
    mouse(cx, on_window);
    assert!(hover.get(), "без окна полоса шапки под мышью");

    open(&view, cx, |scene| &mut scene.a);
    mouse(cx, on_scrim);
    assert!(!hover.get(), "подложка закрывает полосу шапки");
    mouse(cx, on_window);
    assert!(!hover.get(), "окно закрывает полосу шапки");
    cx.simulate_mouse_down(on_window, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(on_window, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(down.get(), 0, "нажатие до шапки не дошло");
    assert!(
        view.read_with(cx, |scene, _| scene.a.is_open()),
        "нажатие в окне не закрывает"
    );

    // Нажатие на подложку закрывает окно (@mousedown.self) и тоже не таскает.
    cx.simulate_mouse_down(on_scrim, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(down.get(), 0);
    assert!(!view.read_with(cx, |scene, _| scene.a.is_open()));
}

#[gpui::test]
fn menu_follows_arrows_letters_and_escape(cx: &mut TestAppContext) {
    let (view, cx) = scene(cx);
    open(&view, cx, |scene| &mut scene.a);
    let trigger = view.read_with(cx, |scene, _| scene.trigger.clone());
    cx.update(|window, cx| window.focus(&trigger, cx));
    let nav = |cx: &mut VisualTestContext| {
        view.read_with(cx, |scene, _| (scene.menu.open, scene.menu.active))
    };

    cx.simulate_keystrokes("down");
    assert_eq!(nav(cx), (true, 0));
    cx.simulate_keystrokes("down");
    assert_eq!(nav(cx), (true, 2), "недоступный пропущен");
    cx.simulate_keystrokes("up");
    assert_eq!(nav(cx), (true, 0));
    cx.simulate_keystrokes("end");
    assert_eq!(nav(cx), (true, 3));
    cx.simulate_keystrokes("home");
    assert_eq!(nav(cx), (true, 0));
    // «р»: Россия недоступна — следующий доступный, Румыния.
    cx.simulate_input("р");
    assert_eq!(nav(cx), (true, 2));
    // Панель меряется в первом кадре и встаёт под кнопку.
    next_frame(cx);
    next_frame(cx);
    cx.simulate_keystrokes("enter");
    assert_eq!(nav(cx), (false, -1));
    assert_eq!(view.read_with(cx, |scene, _| scene.picked), Some(2));

    // Esc при открытом меню закрывает только меню, второй — окно.
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("escape");
    assert_eq!(nav(cx), (false, -1));
    assert!(view.read_with(cx, |scene, _| scene.a.is_open()));
    // Tab закрывает меню и уходит дальше по окну.
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("tab");
    assert_eq!(nav(cx), (false, -1));
    let a0 = view.read_with(cx, |scene, _| scene.a_items[0].clone());
    assert!(
        focused(cx, &a0),
        "кнопка последняя в окне — Tab по кругу на первый"
    );
    cx.simulate_keystrokes("escape");
    assert!(!view.read_with(cx, |scene, _| scene.a.is_open()));
}

#[test]
fn menu_nav_typeahead_resets_after_700_ms() {
    let mut nav = MenuNav::default();
    let labels = ["Германия", "Россия", "Румыния", "Украина"];
    let disabled = [false; 4];
    let t0 = std::time::Instant::now();
    let key = |c: &str| MenuKey::Char(c.into());
    // Закрыто: поиск от выбранного (0) открывает меню.
    assert_eq!(
        nav.key(&key("р"), t0, &labels, &disabled, 0),
        Outcome::Handled
    );
    assert_eq!((nav.open, nav.active), (true, 1));
    // «ру» подряд уточняет префикс.
    nav.key(
        &key("у"),
        t0 + Duration::from_millis(300),
        &labels,
        &disabled,
        0,
    );
    assert_eq!(nav.active, 2);
    // Через 700 мс — новый поиск.
    nav.key(
        &key("у"),
        t0 + Duration::from_millis(1100),
        &labels,
        &disabled,
        0,
    );
    assert_eq!(nav.active, 3);
    // Без подписей буквы не меню.
    assert_eq!(
        nav.key(&key("г"), t0, &[] as &[&str], &disabled, 0),
        Outcome::Pass
    );
    // Esc при закрытом меню — дальше по окну.
    nav.hide();
    assert_eq!(
        nav.key(&MenuKey::Dismiss, t0, &labels, &disabled, 0),
        Outcome::Pass
    );
    // Enter у кнопки — щелчок: открыть без пункта в фокусе.
    assert_eq!(
        nav.key(&MenuKey::Pick, t0, &labels, &disabled, 0),
        Outcome::Handled
    );
    assert_eq!((nav.open, nav.active), (true, -1));
    // Стрелка вверх у кнопки — последний доступный.
    nav.hide();
    nav.key(&MenuKey::Up, t0, &labels, &[false, false, false, true], 0);
    assert_eq!(nav.active, 2);
}

struct Host {
    toasts: Entity<Toasts>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.toasts.clone())
    }
}

#[gpui::test]
fn toasts_live_by_ms_and_keep_three(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let toasts = cx.new(|_| Toasts::new());
    let host = toasts.clone();
    let (_, cx) = cx.add_window_view(move |_, _| Host { toasts: host });
    let shown = |cx: &mut VisualTestContext| {
        toasts.read_with(cx, |toasts, _| {
            toasts
                .shown()
                .iter()
                .map(|toast| (toast.id, toast.leaving))
                .collect::<Vec<_>>()
        })
    };
    let wait = |cx: &mut VisualTestContext, ms: u64| {
        cx.executor().advance_clock(Duration::from_millis(ms));
        cx.run_until_parked();
    };
    toasts.update(cx, |toasts, cx| {
        toasts.notify(
            "путь C:\\HLSR\\games",
            Some("fas fa-file-archive"),
            15_000,
            cx,
        );
        for _ in 0..3 {
            toasts.notify("готово", Some("fas fa-check"), 0, cx);
        }
    });
    cx.run_until_parked();
    // Четвёртый вытеснил первый: тот уходит на своём месте.
    assert_eq!(shown(cx), [(1, true), (2, false), (3, false), (4, false)]);
    wait(cx, 430);
    assert_eq!(shown(cx), [(2, false), (3, false), (4, false)]);
    // 2800 мс: обычные ушли.
    wait(cx, 2400);
    assert_eq!(shown(cx), [(2, true), (3, true), (4, true)]);
    wait(cx, 430);
    assert!(shown(cx).is_empty());

    // 15 с для путей: на 2,9 с ещё на месте; щелчок по тосту закрывает раньше.
    let id = toasts.update(cx, |toasts, cx| {
        toasts.notify("путь", Some("fas fa-file-archive"), 15_000, cx)
    });
    wait(cx, 2900);
    assert_eq!(shown(cx), [(id, false)]);
    wait(cx, 12_200);
    assert_eq!(shown(cx), [(id, true)]);
}

#[gpui::test]
fn depth_follows_the_stack_after_a_lower_window_closes(cx: &mut TestAppContext) {
    let (view, cx) = scene(cx);
    open(&view, cx, |scene| &mut scene.a);
    open(&view, cx, |scene| &mut scene.b);
    view.update_in(cx, |scene, window, cx| {
        scene.a.close(window, cx);
        cx.notify();
    });
    next_frame(cx);
    // Окно `b` теперь нижнее: открытое снова `a` встаёт над ним, `b` под ним стоит.
    open(&view, cx, |scene| &mut scene.a);
    let depths = view.read_with(cx, |scene, _| (scene.a.depth, scene.b.depth));
    assert_eq!(depths, (2, 1));
    // Бесконечное идёт при слое >= top_layer: `b` (1) на паузе.
    assert_eq!(cx.update(|_, cx| AnimGate::get(cx).top_layer), 2);
}

struct TipScene;

impl Render for TipScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .child(div().id("tip").w_1_2().h_full().tip("Обновить"))
            .child(
                div()
                    .id("twin")
                    .absolute()
                    .top_0()
                    .right_0()
                    .w_1_4()
                    .h_1_4()
                    .tip("Обновить"),
            )
            .child(tip_watch())
    }
}

#[gpui::test]
fn press_hides_the_tip_until_the_pointer_leaves(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (_, cx) = cx.add_window_view(|_, _| TipScene);
    // Подсказка — на левой половине окна, такая же у соседа в правом верхнем углу.
    let size = cx.update(|window, _| window.viewport_size());
    let at = |x: f32, y: f32| point(size.width * x, size.height * y);
    let (on, still_on, off, twin) = (at(0.1, 0.1), at(0.2, 0.3), at(0.8, 0.8), at(0.9, 0.1));
    let mouse = |cx: &mut VisualTestContext, at: Point<Pixels>| {
        cx.simulate_mouse_move(at, None, Modifiers::none());
    };
    let click = |cx: &mut VisualTestContext, at: Point<Pixels>| {
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    };
    // Подсказки, построенные за 600 мс (`true` — пустая).
    let wait = |cx: &mut VisualTestContext| {
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        super::tooltip::BUILT.take()
    };
    // Видна ли подсказка: кадр рисует её, только пока она показана.
    let shown = |cx: &mut VisualTestContext| {
        super::tooltip::SHOWN.set(0);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        super::tooltip::SHOWN.get() > 0
    };

    mouse(cx, on);
    assert_eq!(wait(cx), [false], "500 мс наведения — подсказка");
    assert!(shown(cx));
    click(cx, on);
    assert_eq!(wait(cx), [] as [bool; 0]);
    assert!(!shown(cx), "нажатие спрятало");
    mouse(cx, still_on);
    assert_eq!(wait(cx), [true], "указатель не уходил — не видна");
    assert!(!shown(cx));
    mouse(cx, off);
    mouse(cx, on);
    assert_eq!(wait(cx), [false], "ушёл и вернулся — снова видна");
    assert!(shown(cx));

    // Нажатие и сразу на соседа с тем же текстом: его подсказка видна.
    click(cx, on);
    mouse(cx, twin);
    assert_eq!(wait(cx), [false]);
    assert!(shown(cx), "сосед с тем же текстом");

    // Нажатие, пока подсказка ждёт своих 500 мс.
    mouse(cx, off);
    mouse(cx, on);
    click(cx, on);
    assert_eq!(wait(cx), [true]);
}

#[gpui::test]
fn released_owner_takes_its_open_window_off_the_stack(cx: &mut TestAppContext) {
    let (_view, cx) = scene(cx);
    struct Owner(Modal);
    cx.update(|window, cx| {
        let owner = cx.new(|cx| Owner(Modal::new("gone", cx)));
        owner.update(cx, |owner, cx| owner.0.open(window, cx));
        assert_eq!(Layers::open_count(cx), 1);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 0);
    assert_eq!(cx.update(|_, cx| AnimGate::get(cx).top_layer), 0);
}

/// Окно с размытой подложкой (`.news-viewer`).
struct Blurred {
    modal: Modal,
}

impl Render for Blurred {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let modal = self
            .modal
            .render(window, cx, |_, _| {}, || div().size(u(10.)));
        div().size_full().child(modal)
    }
}

/// Пока открыто окно с размытой подложкой, кэша видов нет: её кусок ищет `prepaint` каждого
/// кадра по снимку окна, как стекло Liquid Glass.
#[gpui::test]
fn blurred_scrim_turns_the_view_cache_off_while_open(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (view, cx) = cx.add_window_view(|_, cx| Blurred {
        modal: Modal::new("viewer", cx).blur(),
    });
    cx.run_until_parked();
    let state =
        |cx: &mut VisualTestContext| cx.update(|_, cx| (Layers::blurred(cx), crate::cache::on(cx)));
    assert_eq!(state(cx), (false, true));
    view.update_in(cx, |this, window, cx| {
        this.modal.open(window, cx);
        cx.notify();
    });
    next_frame(cx);
    assert_eq!(state(cx), (true, false));
    view.update_in(cx, |this, window, cx| {
        this.modal.close(window, cx);
        cx.notify();
    });
    next_frame(cx);
    assert_eq!(state(cx), (false, true));
}
