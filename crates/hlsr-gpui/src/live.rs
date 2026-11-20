//! Живой инспектор (dev-hooks): что сейчас в кадре окна лаунчера — для проверки на настоящем
//! мониторе (`scripts/gpui/live.py`, канал — `hlsr-app/src/inspect.rs`).
//!
//! Контролы kit, окна-диалоги, всплывающие панели и цели стенда ([`crate::snap::target`])
//! отмечают себя [`mark`]: при каждом кадре их рамка, подпись и состояние пишутся в таблицу потока
//! окна вместе с хитбоксом (`HitboxBehavior::Normal` — чужое наведение и щелчки он не трогает).
//! Кадр начинает [`begin_frame`] из корня окна, так что таблица — всегда последний нарисованный
//! кадр целиком. По хитбоксу [`snapshot`] узнаёт наведение и то, не закрыт ли центр элемента
//! окном-диалогом (`occlude`). Без dev-hooks [`mark`] — сам элемент.
//!
//! Закэшированный вид ([`crate::cache`]) GPUI повторяет без `prepaint`: его отметки [`scope`]
//! запоминает, когда вид рисуется, и кладёт в таблицу снова, когда кадр вида повторён (хитбоксы
//! повтора — те же, с теми же id). Отметка принадлежит закэшированным видам, внутри которых шла её
//! раскладка ([`crate::cache::stack`]): так и отметки `deferred` (окно-диалог, меню), которые
//! GPUI рисует после всего дерева, остаются за своим видом. Нарисован ли вид, говорит
//! [`rendered`] из его `render`.

use gpui::{AnyElement, ElementId, EntityId, IntoElement, SharedString};

/// Что инспектор знает об элементе.
#[derive(Clone)]
pub struct Meta {
    /// `button`, `check`, `toggle`, `radio`, `link`, `tab`, `dropdown`, `option`, `input`,
    /// `slider`, `select`, `modal`, `popup`, `target`.
    pub kind: &'static str,
    pub id: ElementId,
    /// Подпись: `aria-label`, текст ссылки или вкладки, `placeholder` поля.
    pub label: Option<SharedString>,
    pub disabled: bool,
    pub focused: bool,
    /// Включён или выбран (флажок, переключатель, выбор, вкладка, пункт); у списка — раскрыт,
    /// у окна и панели — открыты (не уходят).
    pub checked: Option<bool>,
    /// Текст поля, значение ползунка, выбранное в списке.
    pub value: Option<SharedString>,
}

impl Meta {
    pub fn new(kind: &'static str, id: impl Into<ElementId>) -> Meta {
        Meta {
            kind,
            id: id.into(),
            label: None,
            disabled: false,
            focused: false,
            checked: None,
            value: None,
        }
    }

    pub fn label(mut self, label: Option<SharedString>) -> Meta {
        self.label = label;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Meta {
        self.disabled = disabled;
        self
    }

    pub fn focused(mut self, focused: bool) -> Meta {
        self.focused = focused;
        self
    }

    pub fn checked(mut self, checked: bool) -> Meta {
        self.checked = Some(checked);
        self
    }

    pub fn value(mut self, value: impl Into<SharedString>) -> Meta {
        self.value = Some(value.into());
        self
    }
}

/// Элемент, отмеченный для инспектора. Раскладка и рисование — самого элемента.
pub fn mark(meta: Meta, element: impl IntoElement) -> AnyElement {
    #[cfg(any(test, feature = "dev-hooks"))]
    {
        use gpui::Element;
        inspector::Marked {
            meta: Some(meta),
            owners: Vec::new(),
            child: element.into_any_element(),
        }
        .into_any()
    }
    #[cfg(not(any(test, feature = "dev-hooks")))]
    {
        let _ = meta;
        element.into_any_element()
    }
}

/// Что экран кадра знает лучше навигации (страница «Персонализации» — свою игру): ключ
/// поверх `screen` дампа. Без dev-hooks — ничего.
pub fn note(key: &'static str, value: impl Into<String>) {
    #[cfg(any(test, feature = "dev-hooks"))]
    inspector::put_note(key, value.into(), &crate::cache::stack());
    #[cfg(not(any(test, feature = "dev-hooks")))]
    let _ = (key, value);
}

/// Закэшированный вид `view` (элемент `element` — его `Entity::cached`): отметки вида живут в
/// таблице и в кадрах, где GPUI его повторил. Без dev-hooks — сам элемент.
pub fn scope(view: EntityId, element: impl IntoElement) -> AnyElement {
    #[cfg(any(test, feature = "dev-hooks"))]
    {
        use gpui::Element;
        inspector::Scope {
            view,
            child: element.into_any_element(),
        }
        .into_any()
    }
    #[cfg(not(any(test, feature = "dev-hooks")))]
    {
        let _ = view;
        element.into_any_element()
    }
}

/// Вид `view` рисуется в этом кадре (начало `render` закэшированного вида): без этого вид,
/// нарисованный без отметок, отдал бы отметки прошлого кадра.
pub fn rendered(view: EntityId) {
    #[cfg(any(test, feature = "dev-hooks"))]
    inspector::put_rendered(view);
    #[cfg(not(any(test, feature = "dev-hooks")))]
    let _ = view;
}

#[cfg(any(test, feature = "dev-hooks"))]
pub use inspector::*;

#[cfg(any(test, feature = "dev-hooks"))]
mod inspector {
    use super::Meta;
    use gpui::{
        AnyElement, App, Bounds, Element, ElementId, EntityId, GlobalElementId, Hitbox,
        HitboxBehavior, InspectorElementId, LayoutId, Pixels, SharedString, Window,
    };
    use serde_json::{Value, json};
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::time::Instant;

    /// Сколько последних кадров помнить для замера частоты ([`frame_times`]).
    const FRAME_TIMES: usize = 4096;

    #[derive(Clone)]
    struct Entry {
        meta: Meta,
        bounds: Bounds<Pixels>,
        /// Часть рамки внутри маски содержимого (прокрутка, `overflow_hidden`).
        visible: Bounds<Pixels>,
        hitbox: Hitbox,
    }

    /// Отметки и заметки закэшированного вида с кадра, где он рисовался.
    #[derive(Clone, Default)]
    struct Saved {
        entries: Vec<Entry>,
        notes: Vec<(&'static str, String)>,
    }

    /// Заметка `key` в список: прежняя с тем же ключом уходит.
    fn put(notes: &mut Vec<(&'static str, String)>, key: &'static str, value: String) {
        notes.retain(|(k, _)| *k != key);
        notes.push((key, value));
    }

    struct Frame {
        /// Кадры начинает корень окна лаунчера; без него (окна режимов, тесты контролов) таблица
        /// не ведётся и хитбоксов нет.
        active: bool,
        number: u64,
        at: Instant,
        entries: Vec<Entry>,
        /// [`super::note`] кадра.
        notes: Vec<(&'static str, String)>,
        /// Начала последних кадров: частота и пропуски во время анимаций.
        times: VecDeque<Instant>,
        /// Закэшированные виды: что они отметили, когда рисовались ([`Scope`]).
        saved: HashMap<EntityId, Saved>,
        /// Что этот кадр отметил внутри закэшированных видов (и в их `deferred`, и повтором
        /// вложенных): нарисованным видам — в `saved` в начале следующего кадра.
        fresh: HashMap<EntityId, Saved>,
        /// Виды [`Scope`] этого кадра, которые рисовались (не повторены).
        drawn: HashSet<EntityId>,
        /// Виды [`Scope`] этого кадра: прочие уходят из `saved` в начале следующего.
        seen: HashSet<EntityId>,
        /// Виды, чей `render` прошёл после `request_layout` их [`Scope`] ([`super::rendered`]).
        rendered: HashSet<EntityId>,
    }

    thread_local! {
        /// Отмеченные элементы последнего кадра окна (поток окна).
        static FRAME: RefCell<Frame> = RefCell::new(Frame {
            active: false,
            number: 0,
            at: Instant::now(),
            entries: Vec::new(),
            notes: Vec::new(),
            times: VecDeque::new(),
            saved: HashMap::new(),
            fresh: HashMap::new(),
            drawn: HashSet::new(),
            seen: HashSet::new(),
            rendered: HashSet::new(),
        });
    }

    /// Новый кадр окна: таблица начинается заново (корень окна, в начале `render`).
    pub fn begin_frame() {
        FRAME.with(|frame| {
            let mut frame = frame.borrow_mut();
            frame.active = true;
            frame.number += 1;
            frame.at = Instant::now();
            frame.entries.clear();
            frame.notes.clear();
            // Прошлый кадр кончился (и его `deferred`): нарисованные виды — с тем, что отметили.
            let mut fresh = std::mem::take(&mut frame.fresh);
            for view in std::mem::take(&mut frame.drawn) {
                let saved = fresh.remove(&view).unwrap_or_default();
                frame.saved.insert(view, saved);
            }
            let seen = std::mem::take(&mut frame.seen);
            frame.saved.retain(|view, _| seen.contains(view));
            if frame.times.len() == FRAME_TIMES {
                frame.times.pop_front();
            }
            let at = frame.at;
            frame.times.push_back(at);
        });
    }

    pub(super) fn put_note(key: &'static str, value: String, owners: &[EntityId]) {
        FRAME.with(|frame| {
            let mut frame = frame.borrow_mut();
            if frame.active {
                for owner in owners {
                    put(
                        &mut frame.fresh.entry(*owner).or_default().notes,
                        key,
                        value.clone(),
                    );
                }
                put(&mut frame.notes, key, value);
            }
        });
    }

    pub(super) fn put_rendered(view: EntityId) {
        FRAME.with(|frame| frame.borrow_mut().rendered.insert(view));
        #[cfg(test)]
        RENDERS.with(|renders| *renders.borrow_mut().entry(view).or_default() += 1);
    }

    #[cfg(test)]
    thread_local! {
        static RENDERS: RefCell<HashMap<EntityId, usize>> = RefCell::default();
    }

    /// Сколько раз вид звал [`super::rendered`] (тесты кэша видов).
    #[cfg(test)]
    pub fn renders(view: EntityId) -> usize {
        RENDERS.with(|renders| renders.borrow().get(&view).copied().unwrap_or(0))
    }

    /// Обёртка закэшированного вида: его отметки — в таблицу и в кадрах повтора.
    pub(super) struct Scope {
        pub(super) view: EntityId,
        pub(super) child: AnyElement,
    }

    impl Element for Scope {
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
            // Без кэша (смена темы, стекло) вид рисуется уже здесь, с кэшем — в `prepaint`.
            FRAME.with(|frame| frame.borrow_mut().rendered.remove(&self.view));
            (self.child.request_layout(window, cx), ())
        }

        fn prepaint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut (),
            window: &mut Window,
            cx: &mut App,
        ) {
            self.child.prepaint(window, cx);
            // Виды снаружи этого (сам вид — последний в стопке `cache`).
            let outer: Vec<EntityId> = crate::cache::stack()
                .into_iter()
                .filter(|view| *view != self.view)
                .collect();
            FRAME.with(|frame| {
                let mut frame = frame.borrow_mut();
                if !frame.active {
                    return;
                }
                frame.seen.insert(self.view);
                let drawn =
                    frame.rendered.contains(&self.view) || frame.fresh.contains_key(&self.view);
                if drawn {
                    frame.drawn.insert(self.view);
                } else if let Some(saved) = frame.saved.get(&self.view).cloned() {
                    // Кадр вида повторён: его отметки — в таблицу и видам снаружи.
                    for owner in &outer {
                        let fresh = frame.fresh.entry(*owner).or_default();
                        fresh.entries.extend(saved.entries.iter().cloned());
                        for (key, value) in &saved.notes {
                            put(&mut fresh.notes, key, value.clone());
                        }
                    }
                    frame.entries.extend(saved.entries);
                    for (key, value) in saved.notes {
                        put(&mut frame.notes, key, value);
                    }
                }
            });
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

    /// Начала последних кадров в мс до этого вызова (старые первыми) и номер кадра.
    pub fn frame_times() -> Value {
        FRAME.with(|frame| {
            let frame = frame.borrow();
            let now = Instant::now();
            let ago: Vec<f64> = frame
                .times
                .iter()
                .map(|at| (now - *at).as_secs_f64() * 1000.)
                .collect();
            json!({"frame": frame.number, "ago": ago})
        })
    }

    /// Записать элемент кадра (из `prepaint`: рамка уже с прокруткой); `owners` — закэшированные
    /// виды, внутри которых шла его раскладка ([`crate::cache::stack`]).
    pub(crate) fn record(
        meta: Meta,
        bounds: Bounds<Pixels>,
        owners: &[EntityId],
        window: &mut Window,
    ) {
        if !FRAME.with(|frame| frame.borrow().active) {
            return;
        }
        let visible = bounds.intersect(&window.content_mask().bounds);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let entry = Entry {
            meta,
            bounds,
            visible,
            hitbox,
        };
        FRAME.with(|frame| {
            let mut frame = frame.borrow_mut();
            for owner in owners {
                let fresh = frame.fresh.entry(*owner).or_default();
                fresh.entries.push(entry.clone());
            }
            frame.entries.push(entry);
        });
    }

    pub(super) struct Marked {
        pub(super) meta: Option<Meta>,
        /// Закэшированные виды вокруг (с раскладки: `deferred` рисуется позже всего дерева).
        pub(super) owners: Vec<EntityId>,
        pub(super) child: AnyElement,
    }

    impl Element for Marked {
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
            self.child.prepaint(window, cx);
            // После своих детей: хитбокс над их `occlude`, закрыть его может только то, что
            // рисуется позже (окна-диалоги, панели, соседи сверху).
            if let Some(meta) = self.meta.take() {
                record(meta, bounds, &self.owners, window);
            }
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

    /// Рамка в физических px клиентской области: `[x, y, ширина, высота]`.
    fn rect(bounds: Bounds<Pixels>, scale: f32) -> [i64; 4] {
        let device = |v: Pixels| (f32::from(v) * scale).round() as i64;
        [
            device(bounds.origin.x),
            device(bounds.origin.y),
            device(bounds.size.width),
            device(bounds.size.height),
        ]
    }

    /// Состояние окна для инспектора: кадр, экран, окна и панели, отмеченные элементы.
    pub fn snapshot(window: &Window, cx: &App) -> Value {
        let scale = window.scale_factor();
        FRAME.with(|frame| {
            let frame = frame.borrow();
            let entries = &frame.entries;
            // Цель стенда с той же рамкой, что у контрола, — имя этого контрола, не отдельная
            // запись.
            let is_target = |e: &Entry| e.meta.kind == "target";
            let twin = |t: &Entry| {
                entries
                    .iter()
                    .position(|e| !is_target(e) && e.bounds == t.bounds)
            };
            let mut targets: Vec<Vec<String>> = vec![Vec::new(); entries.len()];
            for entry in entries.iter().filter(|e| is_target(e)) {
                if let Some(index) = twin(entry) {
                    targets[index].push(entry.meta.id.to_string());
                }
            }
            let mut seen = std::collections::HashMap::<String, usize>::new();
            let mut controls = Vec::new();
            let mut overlays = Vec::new();
            let mut focused = Value::Null;
            for (index, entry) in entries.iter().enumerate() {
                if is_target(entry) && twin(entry).is_some() {
                    continue;
                }
                let meta = &entry.meta;
                let base = meta.id.to_string();
                let count = seen.entry(format!("{}:{base}", meta.kind)).or_default();
                *count += 1;
                // Один id у нескольких элементов (строки списка): `#2`, `#3` по порядку кадра.
                let id = if *count > 1 {
                    format!("{base}#{count}")
                } else {
                    base
                };
                let shown = f32::from(entry.visible.size.width) > 0.
                    && f32::from(entry.visible.size.height) > 0.;
                let mut item = json!({
                    "id": id,
                    "kind": meta.kind,
                    "rect": rect(entry.bounds, scale),
                    "visible": shown.then(|| rect(entry.visible, scale)),
                    "hover": entry.hitbox.is_hovered(window),
                    // Центр видимой части не закрыт окном-диалогом или панелью: щелчок дойдёт.
                    "hittable": shown && entry.hitbox.is_hovered_at(entry.visible.center(), window),
                });
                let object = item.as_object_mut().expect("объект");
                let mut put = |key: &str, value: Value| {
                    object.insert(key.to_owned(), value);
                };
                if let Some(label) = &meta.label {
                    put("label", json!(label.as_ref()));
                }
                if meta.disabled {
                    put("disabled", json!(true));
                }
                if meta.focused {
                    put("focused", json!(true));
                    focused = json!(id);
                }
                if let Some(checked) = meta.checked {
                    put("checked", json!(checked));
                }
                if let Some(value) = &meta.value {
                    put("value", json!(value.as_ref()));
                }
                if !targets[index].is_empty() {
                    put("target", json!(targets[index]));
                }
                if matches!(meta.kind, "modal" | "popup") {
                    overlays.push(item);
                } else {
                    controls.push(item);
                }
            }
            let size = window.viewport_size();
            json!({
                "frame": frame.number,
                "idleMs": frame.at.elapsed().as_millis() as u64,
                "scale": scale,
                "zoom": crate::scale::zoom(window),
                "size": [
                    (f32::from(size.width) * scale).round() as i64,
                    (f32::from(size.height) * scale).round() as i64,
                ],
                "screen": screen(cx, &frame.notes),
                "overlays": overlays,
                "focused": focused,
                "imagesBusy": cx
                    .try_global::<crate::image::Pictures>()
                    .is_some_and(crate::image::Pictures::busy),
                "controls": controls,
            })
        })
    }

    /// Экран окна: заставка или оболочка с разделом, вкладкой «Игр» и выбранным.
    fn screen(cx: &App, notes: &[(&'static str, String)]) -> Value {
        let Some(shell) = crate::shell::root::ShellContext::get(cx) else {
            return json!({"view": "boot"});
        };
        let nav = &shell.nav;
        let mut screen = json!({
            "view": "launcher",
            "section": nav.section(),
            "tab": nav.active_tab(),
            "game": nav.selected_game(),
            "tool": nav.selected_tool(),
            "configsGame": nav.configs_game(),
            "railOpen": nav.rail_open(),
            "modals": crate::layer::Layers::open_count(cx),
        });
        for (key, value) in notes {
            screen[*key] = json!(value);
        }
        screen
    }

    /// Имя цели стенда как запись инспектора.
    pub(crate) fn target_meta(name: &SharedString) -> Meta {
        Meta::new("target", name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        AppContext, Context, InteractiveElement, ParentElement, Render, Styled, TestAppContext,
        Window, div,
    };

    struct Page;

    impl Render for Page {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            begin_frame();
            let u = crate::scale::u;
            let row = |name: &'static str, occlude: bool| {
                let el = div().id(name).w(u(50.)).h(u(10.));
                mark(
                    Meta::new("button", "row").label(Some(name.into())),
                    if occlude { el.occlude() } else { el },
                )
            };
            div()
                .size_full()
                .relative()
                .flex()
                .flex_col()
                .child(crate::snap::target(
                    "play",
                    mark(
                        Meta::new("button", "play").checked(true),
                        div().w(u(120.)).h(u(30.)),
                    ),
                ))
                // Свой `occlude` контрол не закрывает.
                .child(row("a", true))
                .child(row("b", false))
                // Позже и поверх «b» (как подложка окна-диалога): закрывает.
                .child(
                    div()
                        .id("cover")
                        .absolute()
                        .top(u(40.))
                        .left_0()
                        .w(u(200.))
                        .h(u(10.))
                        .occlude(),
                )
        }
    }

    /// Закэшированный вид с отметкой (пока `marked`) и отметкой окна в `deferred` (пока `popup`).
    struct Child {
        marked: bool,
        popup: bool,
    }

    impl Render for Child {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            rendered(cx.entity_id());
            let u = crate::scale::u;
            div()
                .size_full()
                .children(self.marked.then(|| {
                    mark(
                        Meta::new("button", "inner"),
                        div().id("inner").w(u(20.)).h(u(10.)),
                    )
                }))
                .children(self.popup.then(|| {
                    gpui::deferred(mark(
                        Meta::new("modal", "popup").checked(true),
                        div().id("popup").w(u(30.)).h(u(30.)),
                    ))
                }))
        }
    }

    struct Host {
        child: gpui::Entity<Child>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            begin_frame();
            let u = crate::scale::u;
            div()
                .size_full()
                .flex()
                .child(mark(
                    Meta::new("button", "outer"),
                    div().id("outer").w(u(20.)).h(u(10.)),
                ))
                .child(crate::cache::cached(
                    self.child.clone(),
                    gpui::StyleRefinement::default().w(u(100.)).h(u(50.)),
                    true,
                ))
        }
    }

    #[gpui::test]
    fn marks_of_a_cached_view_stay_while_its_frame_is_reused(cx: &mut TestAppContext) {
        let (host, cx) = cx.add_window_view(|_, cx| Host {
            child: cx.new(|_| Child {
                marked: true,
                popup: false,
            }),
        });
        cx.run_until_parked();
        let ids = |cx: &mut gpui::VisualTestContext| -> Vec<String> {
            let dump = cx.update(|window, cx| snapshot(window, cx));
            (dump["controls"].as_array().expect("список").iter())
                .map(|c| c["id"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(ids(cx), ["outer", "inner"]);
        let child = host.read_with(cx, |host, _| host.child.clone());
        let drawn = renders(child.entity_id());

        // Перерисован только хозяин: кадр вида повторён, его отметка — та же, и наведение по ней
        // считается (хитбокс повтора — с тем же id).
        host.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!(renders(child.entity_id()), drawn);
        assert_eq!(ids(cx), ["outer", "inner"]);
        let dump = cx.update(|window, cx| snapshot(window, cx));
        let (inner, scale) = (
            &dump["controls"][1]["rect"],
            dump["scale"].as_f64().unwrap(),
        );
        let center = |r: &serde_json::Value, i: usize| {
            ((r[i].as_i64().unwrap() + r[i + 2].as_i64().unwrap() / 2) as f64 / scale) as f32
        };
        let zpx = crate::scale::zpx;
        let at = gpui::point(zpx(1., center(inner, 0)), zpx(1., center(inner, 1)));
        cx.update(|window, cx| crate::snap::pointer_move(window, cx, at));
        cx.run_until_parked();
        let dump = cx.update(|window, cx| snapshot(window, cx));
        assert_eq!(dump["controls"][1]["hover"], true);

        // Вид нарисован заново без отметки — её больше нет.
        child.update(cx, |child, cx| {
            child.marked = false;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(renders(child.entity_id()), drawn + 1);
        assert_eq!(ids(cx), ["outer"]);
    }

    /// Окно-диалог закэшированного вида рисуется `deferred` (после всего дерева): в кадре повтора
    /// его отметка остаётся, а закрытое окно уходит из таблицы.
    #[gpui::test]
    fn deferred_marks_of_a_cached_view_stay_while_its_frame_is_reused(cx: &mut TestAppContext) {
        let (host, cx) = cx.add_window_view(|_, cx| Host {
            child: cx.new(|_| Child {
                marked: false,
                popup: true,
            }),
        });
        cx.run_until_parked();
        let overlays = |cx: &mut gpui::VisualTestContext| -> Vec<String> {
            let dump = cx.update(|window, cx| snapshot(window, cx));
            (dump["overlays"].as_array().expect("список").iter())
                .map(|c| c["id"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(overlays(cx), ["popup"]);
        let child = host.read_with(cx, |host, _| host.child.clone());
        let drawn = renders(child.entity_id());
        for _ in 0..2 {
            host.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            assert_eq!(renders(child.entity_id()), drawn, "кадр вида повторён");
            assert_eq!(overlays(cx), ["popup"]);
        }
        child.update(cx, |child, cx| {
            child.popup = false;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(overlays(cx).is_empty());
        host.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert!(overlays(cx).is_empty(), "закрытое окно не возвращается");
    }

    #[gpui::test]
    fn marks_of_the_last_frame_are_listed(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Page);
        cx.run_until_parked();
        let dump = cx.update(|window, cx| snapshot(window, cx));
        let controls = dump["controls"].as_array().expect("список");
        let ids: Vec<&str> = controls.iter().map(|c| c["id"].as_str().unwrap()).collect();
        // Цель стенда с рамкой контрола — его имя; одинаковые id — по порядку.
        assert_eq!(ids, ["play", "row", "row#2"]);
        assert_eq!(controls[0]["target"], serde_json::json!(["play"]));
        assert_eq!(controls[0]["checked"], true);
        assert_eq!(controls[2]["label"], "b");
        let hittable: Vec<bool> = controls
            .iter()
            .map(|c| c["hittable"].as_bool().unwrap())
            .collect();
        assert_eq!(hittable, [true, true, false]);
        let rect = controls[0]["rect"].as_array().unwrap();
        assert_eq!(rect[2].as_i64().unwrap(), 4 * rect[3].as_i64().unwrap());
        assert_eq!(dump["screen"]["view"], "boot");
    }
}
