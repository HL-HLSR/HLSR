//! Сверочный стенд (`hlsr-snap screens`, V0): цели шагов сцены и исполнитель шагов в окне GPUI.
//!
//! Цель — элемент, обёрнутый [`target`]: под `dev-hooks` (и в тестах) его рамка пишется в таблицу
//! потока окна при каждом кадре; без них обёртка — сам элемент. `debug_selector` GPUI не подошёл:
//! его рамки лежат в кадре окна, а прочитать их можно только из `VisualTestContext`.
//!
//! Шаги — те же, что у эталона (`runStep` в `tools/reference/lib/electron-screens.js`): `wait`,
//! `waitFor`, `move`, `hover`, `click`/`dblclick`/`rightclick` (+ `count`), `key` (+ `modifiers`), `type`,
//! `wheel` (+ `deltaX`/`deltaY`), `emit` (+ `payload`). Цель шага — `{"css": "…", "view": "<цель>"}`:
//! `css` — для эталона, `view` — имя [`target`]; без `view` шаг здесь не исполняется. Координаты —
//! CSS px макета 1600×900, в окне — × зум (`rem / 16`). Ввод — `dispatch_event` окна, как
//! `sendInputEvent` эталона; кадр — `render_to_image` (GPUI `test-support`).

use gpui::{AnyElement, IntoElement, SharedString};

/// Элемент — цель шагов сцены с именем `name`.
pub fn target(name: impl Into<SharedString>, element: impl IntoElement) -> AnyElement {
    #[cfg(any(test, feature = "dev-hooks"))]
    {
        use gpui::Element;
        Target {
            name: name.into(),
            owners: Vec::new(),
            child: element.into_any_element(),
        }
        .into_any()
    }
    #[cfg(not(any(test, feature = "dev-hooks")))]
    {
        let _ = name;
        element.into_any_element()
    }
}

#[cfg(any(test, feature = "dev-hooks"))]
pub use stand::*;

#[cfg(any(test, feature = "dev-hooks"))]
mod stand {
    use crate::scale::{zoom, zpx};
    use gpui::{
        AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId,
        Keystroke, LayoutId, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        Pixels, PlatformInput, Point, ScrollDelta, ScrollEvent, ScrollSource, SharedString,
        TouchPhase, Window, point,
    };
    use serde::Serialize;
    use serde_json::Value;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    thread_local! {
        /// Рамки целей последних кадров окна (поток окна).
        static TARGETS: RefCell<HashMap<SharedString, Bounds<Pixels>>> = RefCell::default();
    }

    /// Рамка цели `name` из нарисованного кадра.
    pub fn bounds_of(name: &str) -> Option<Bounds<Pixels>> {
        TARGETS.with(|targets| targets.borrow().get(name).copied())
    }

    /// Забыть рамки: дальше в таблице только цели следующих кадров.
    pub fn forget_targets() {
        TARGETS.with(|targets| targets.borrow_mut().clear());
    }

    /// Обёртка цели: раскладка и рисование — ребёнка, рамка — в таблицу.
    pub(super) struct Target {
        pub(super) name: SharedString,
        /// Закэшированные виды вокруг (живой инспектор, `live::record`).
        pub(super) owners: Vec<gpui::EntityId>,
        pub(super) child: AnyElement,
    }

    impl Element for Target {
        type RequestLayoutState = ();
        type PrepaintState = ();

        fn id(&self) -> Option<ElementId> {
            None
        }

        fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, ()) {
            self.owners = crate::cache::stack();
            (self.child.request_layout(window, cx), ())
        }

        fn prepaint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            _: &mut (),
            window: &mut Window,
            cx: &mut App,
        ) {
            TARGETS.with(|targets| targets.borrow_mut().insert(self.name.clone(), bounds));
            self.child.prepaint(window, cx);
            // Живой инспектор — после детей, как `live::mark`.
            let meta = crate::live::target_meta(&self.name);
            crate::live::record(meta, bounds, &self.owners, window);
        }

        fn paint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut (),
            _: &mut (),
            window: &mut Window,
            cx: &mut App,
        ) {
            self.child.paint(window, cx);
        }
    }

    /// Сколько ждать цель шага (эталон — 10 с).
    const TARGET_TIMEOUT: Duration = Duration::from_secs(10);
    const TARGET_POLL: Duration = Duration::from_millis(50);
    /// Между событиями одного шага (наведение до нажатия, нажатие до отпускания).
    const INPUT_SETTLE: Duration = Duration::from_millis(60);
    /// Строк на щелчок колеса (`SPI_GETWHEELSCROLLLINES` по умолчанию).
    const WHEEL_LINES: f32 = 3.;
    /// Дольше картинки перед снимком не ждём (передний план фона — webp 2172×2896: в debug и при
    /// нескольких процессах стенда декодируется дольше пауз).
    const IMAGES_WAIT: Duration = Duration::from_secs(20);
    /// После дождавшейся картинки: проявление слоя фона (.5 с) и кадр.
    const IMAGE_FADE: Duration = Duration::from_millis(700);
    /// Загрузок нет столько подряд — значит, их нет: между готовым декодом и переводом в BGRA
    /// (ответ в поток окна, перерисовка) набор на миг пуст.
    const IMAGES_IDLE: Duration = Duration::from_millis(300);

    /// Что сделать в окне сцены.
    pub struct Plan {
        pub steps: Vec<Value>,
        /// После открытия окна: первый кадр, картинки в фоне.
        pub open_settle: Duration,
        /// После каждого шага: пружины и переходы.
        pub step_settle: Duration,
        /// После шагов, перед снимком (`settleMs` сцены).
        pub shot_settle: Duration,
        /// Куда записать кадр (PNG).
        pub png: PathBuf,
        /// Событие окну (шаг `emit`).
        pub emit: Emit,
    }

    /// Событие окну: канал и данные.
    pub type Emit = Box<dyn Fn(&str, Value)>;

    /// Шаг и как он прошёл (`ок` или причина).
    #[derive(Debug, Clone, PartialEq, Serialize)]
    pub struct StepRecord {
        pub step: Value,
        pub status: String,
    }

    pub const OK: &str = "ок";

    /// Итог прогона: шаги и ошибка снимка.
    #[derive(Debug, Default, Serialize)]
    pub struct Outcome {
        pub steps: Vec<StepRecord>,
        pub error: Option<String>,
    }

    /// Исполнить план в окне и отдать итог `done` (поток окна).
    pub fn run(
        plan: Plan,
        window: &mut Window,
        cx: &mut App,
        done: impl FnOnce(Outcome, &mut App) + 'static,
    ) {
        window
            .spawn(cx, async move |cx| {
                sleep(cx, plan.open_settle).await;
                let mut outcome = Outcome::default();
                for step in &plan.steps {
                    let status = match run_step(step, &plan, cx).await {
                        Ok(()) => OK.to_owned(),
                        Err(error) => error,
                    };
                    outcome.steps.push(StepRecord {
                        step: step.clone(),
                        status,
                    });
                    sleep(cx, plan.step_settle).await;
                }
                sleep(cx, plan.shot_settle).await;
                // Готовая картинка тянет следующую (кадр → перевод в BGRA, кадр фона → передний
                // план): после проявления — проверить снова.
                let deadline = Instant::now() + IMAGES_WAIT;
                while wait_images(cx, deadline).await {
                    sleep(cx, IMAGE_FADE).await;
                }
                let png = plan.png.clone();
                let shot = cx
                    .update(|window, _| -> Result<(), String> {
                        let image = window
                            .render_to_image()
                            .map_err(|e| format!("render_to_image: {e:#}"))?;
                        image
                            .save(&png)
                            .map_err(|e| format!("{}: {e}", png.display()))
                    })
                    .map_err(|e| format!("окно закрылось: {e:#}"))
                    .and_then(|shot| shot);
                outcome.error = shot.err();
                // Через приложение, не окно: закрытое окно не должно оставить процесс без итога.
                gpui::AsyncApp::update(cx, |cx| done(outcome, cx));
            })
            .detach();
    }

    async fn run_step(
        step: &Value,
        plan: &Plan,
        cx: &mut gpui::AsyncWindowContext,
    ) -> Result<(), String> {
        let field = |key: &str| step.get(key);
        if let Some(ms) = field("wait") {
            sleep(cx, Duration::from_millis(ms.as_u64().unwrap_or(0))).await;
            return Ok(());
        }
        if let Some(target) = field("waitFor") {
            find(target, cx).await?;
            return Ok(());
        }
        if let Some(channel) = field("emit") {
            let channel = channel.as_str().ok_or("emit: не строка")?;
            (plan.emit)(channel, field("payload").cloned().unwrap_or(Value::Null));
            return Ok(());
        }
        if let Some(at) = field("move") {
            let xy = at
                .as_array()
                .filter(|xy| xy.len() == 2)
                .ok_or("move: не [x, y]")?;
            let css = |v: &Value| v.as_f64().unwrap_or(0.) as f32;
            let (x, y) = (css(&xy[0]), css(&xy[1]));
            return in_window(cx, move |window, cx| {
                let zoom = zoom(window);
                pointer_move(window, cx, point(zpx(zoom, x), zpx(zoom, y)));
            });
        }
        for kind in ["hover", "click", "dblclick", "rightclick"] {
            let Some(target) = field(kind) else {
                continue;
            };
            let at = find(target, cx).await?;
            in_window(cx, move |window, cx| pointer_move(window, cx, at))?;
            if kind == "hover" {
                return Ok(());
            }
            let button = if kind == "rightclick" {
                MouseButton::Right
            } else {
                MouseButton::Left
            };
            // `count` — щелчков подряд (тройной щелчок по логотипу и т. п.).
            let clicks = field("count")
                .and_then(Value::as_u64)
                .map_or(if kind == "dblclick" { 2 } else { 1 }, |count| {
                    count as usize
                });
            for click_count in 1..=clicks {
                sleep(cx, INPUT_SETTLE).await;
                in_window(cx, move |window, cx| {
                    press(window, cx, at, button, click_count, true)
                })?;
                sleep(cx, INPUT_SETTLE).await;
                in_window(cx, move |window, cx| {
                    press(window, cx, at, button, click_count, false)
                })?;
            }
            return Ok(());
        }
        if let Some(key) = field("key") {
            let key = key.as_str().ok_or("key: не строка")?;
            let modifiers: Vec<String> = field("modifiers")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let keystroke = keystroke(key, &modifiers)?;
            return in_window(cx, move |window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            });
        }
        if let Some(text) = field("type") {
            let text = text.as_str().ok_or("type: не строка")?.to_owned();
            return in_window(cx, move |window, cx| {
                for ch in text.chars() {
                    window.dispatch_keystroke(typed(ch), cx);
                }
            });
        }
        if let Some(target) = field("wheel") {
            let at = find(target, cx).await?;
            let delta = |key: &str| field(key).and_then(Value::as_f64).unwrap_or(0.) as f32;
            let lines = point(
                delta("deltaX") / 120. * WHEEL_LINES,
                delta("deltaY") / 120. * WHEEL_LINES,
            );
            in_window(cx, move |window, cx| pointer_move(window, cx, at))?;
            sleep(cx, INPUT_SETTLE).await;
            return in_window(cx, move |window, cx| {
                window.dispatch_event(
                    PlatformInput::Scroll(ScrollEvent {
                        position: at,
                        delta: ScrollDelta::Lines(lines),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                        source: ScrollSource::Pointer,
                    }),
                    cx,
                );
            });
        }
        // `clock`, `freezeAt` и прочее эталона — пока не в GPUI.
        Err(format!("шаг не поддержан: {step}"))
    }

    fn sleep(cx: &gpui::AsyncWindowContext, time: Duration) -> gpui::Task<()> {
        cx.background_executor().timer(time)
    }

    /// Ждать, пока [`crate::image::Pictures`] не грузит и не переводит картинок [`IMAGES_IDLE`]
    /// подряд (не дольше `deadline`); `true` — пришлось ждать.
    async fn wait_images(cx: &mut gpui::AsyncWindowContext, deadline: Instant) -> bool {
        let mut waited = false;
        let mut idle = Duration::ZERO;
        while Instant::now() < deadline && idle < IMAGES_IDLE {
            let busy = cx
                .update(|_, cx| {
                    cx.try_global::<crate::image::Pictures>()
                        .is_some_and(crate::image::Pictures::busy)
                })
                .unwrap_or(false);
            if busy {
                waited = true;
                idle = Duration::ZERO;
            } else {
                idle += TARGET_POLL;
            }
            sleep(cx, TARGET_POLL).await;
        }
        waited
    }

    fn in_window(
        cx: &mut gpui::AsyncWindowContext,
        f: impl FnOnce(&mut Window, &mut App),
    ) -> Result<(), String> {
        cx.update(f).map_err(|e| format!("окно закрылось: {e:#}"))
    }

    /// Имя цели шага: `{"view": "…"}`.
    pub fn target_name(target: &Value) -> Result<&str, String> {
        target
            .get("view")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("нет цели GPUI (view) у {target}"))
    }

    /// Середина цели в логических px окна; ждёт, пока цель появится в кадре.
    async fn find(
        target: &Value,
        cx: &mut gpui::AsyncWindowContext,
    ) -> Result<Point<Pixels>, String> {
        let name = target_name(target)?.to_owned();
        let deadline = Instant::now() + TARGET_TIMEOUT;
        loop {
            // Таблица — только из кадров после этой точки: исчезнувшая цель не находится.
            forget_targets();
            in_window(cx, |window, _| window.refresh())?;
            sleep(cx, TARGET_POLL).await;
            if let Some(bounds) = bounds_of(&name) {
                return Ok(bounds.center());
            }
            if Instant::now() > deadline {
                return Err(format!("нет цели {name}"));
            }
        }
    }

    pub fn pointer_move(window: &mut Window, cx: &mut App, position: Point<Pixels>) {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );
    }

    pub fn press(
        window: &mut Window,
        cx: &mut App,
        position: Point<Pixels>,
        button: MouseButton,
        click_count: usize,
        down: bool,
    ) {
        let event = if down {
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position,
                modifiers: Modifiers::default(),
                click_count,
                first_mouse: false,
            })
        } else {
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position,
                modifiers: Modifiers::default(),
                click_count,
            })
        };
        window.dispatch_event(event, cx);
    }

    /// Клавиша шага эталона (`keyCode` ускорителя Electron) → нажатие GPUI.
    pub fn keystroke(key: &str, modifiers: &[String]) -> Result<Keystroke, String> {
        let mut mods = Modifiers::default();
        for m in modifiers {
            match m.to_ascii_lowercase().as_str() {
                "shift" => mods.shift = true,
                "control" | "ctrl" => mods.control = true,
                "alt" => mods.alt = true,
                "meta" | "cmd" | "command" | "super" => mods.platform = true,
                other => return Err(format!("модификатор {other}")),
            }
        }
        let lower = key.to_ascii_lowercase();
        let name = match lower.trim_start_matches("arrow") {
            "return" => "enter",
            "esc" => "escape",
            " " => "space",
            other => other,
        };
        let mut chars = key.chars();
        let (key, key_char) = match (chars.next(), chars.next()) {
            (Some(ch), None) if ch != ' ' => (
                ch.to_lowercase().to_string(),
                (!mods.control && !mods.alt && !mods.platform).then(|| ch.to_string()),
            ),
            _ => (name.to_owned(), None),
        };
        Ok(Keystroke {
            modifiers: mods,
            key,
            key_char,
        })
    }

    /// Буква шага `type`: ввод символом, как `char` эталона.
    fn typed(ch: char) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                shift: ch.is_uppercase(),
                ..Modifiers::default()
            },
            key: ch.to_lowercase().to_string(),
            key_char: Some(ch.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Context, InteractiveElement, MouseButton, ParentElement, Render,
        StatefulInteractiveElement, Styled, TestAppContext, Window, div,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    struct Page {
        clicks: Rc<Cell<usize>>,
    }

    impl Render for Page {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            div().size_full().pl(crate::scale::u(40.)).child(target(
                "play",
                div()
                    .id("play")
                    .w(crate::scale::u(120.))
                    .h(crate::scale::u(30.))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            ))
        }
    }

    #[gpui::test]
    fn a_target_is_found_and_clicked_through_window_events(cx: &mut TestAppContext) {
        let clicks = Rc::new(Cell::new(0));
        let shared = clicks.clone();
        let (_, cx) = cx.add_window_view(|_, _| Page { clicks: shared });
        cx.run_until_parked();
        let bounds = bounds_of("play").expect("рамка цели записана при кадре");
        assert_eq!(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height) * 4.
        );
        assert!(bounds_of("nope").is_none());
        cx.update(|window, cx| {
            let at = bounds.center();
            pointer_move(window, cx, at);
            press(window, cx, at, MouseButton::Left, 1, true);
            press(window, cx, at, MouseButton::Left, 1, false);
        });
        assert_eq!(clicks.get(), 1);
    }

    #[test]
    fn keys_of_the_reference_map_to_gpui() {
        let k = keystroke("ArrowDown", &["shift".into()]).unwrap();
        assert_eq!(
            (k.key.as_str(), k.modifiers.shift, k.key_char),
            ("down", true, None)
        );
        assert_eq!(keystroke("Escape", &[]).unwrap().key, "escape");
        assert_eq!(keystroke("Return", &[]).unwrap().key, "enter");
        assert_eq!(keystroke(" ", &[]).unwrap().key, "space");
        assert_eq!(keystroke("Space", &[]).unwrap().key, "space");
        let a = keystroke("A", &[]).unwrap();
        assert_eq!((a.key.as_str(), a.key_char.as_deref()), ("a", Some("A")));
        let copy = keystroke("c", &["control".into()]).unwrap();
        assert_eq!((copy.modifiers.control, copy.key_char), (true, None));
        assert!(keystroke("x", &["hyper".into()]).is_err());
        assert!(target_name(&serde_json::json!({"css": ".play"})).is_err());
        assert_eq!(
            target_name(&serde_json::json!({"css": ".play", "view": "play"})),
            Ok("play")
        );
    }
}
