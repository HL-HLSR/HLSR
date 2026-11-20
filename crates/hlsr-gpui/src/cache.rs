//! Кэш крупных видов (`Entity::cached` GPUI): закэшированный вид рисуется заново, только когда
//! его уведомили (`cx.notify` — его самого или вида внутри него), сменились его рамка, маска,
//! стиль текста или прозрачность, или окно обновили целиком (`window.refresh`: наведение,
//! фокус, зум, выбор темы, окна-слои). Иначе GPUI повторяет прошлый кадр вида: анимация одной части
//! окна (параллакс фона, ник в ленте) не пересчитывает остальные.
//!
//! Что должен закэшированный вид:
//! - всё, что он читает вне себя, его уведомляет. Контроллеры оболочки доходят до видов через
//!   корень: каждое уведомление [`crate::shell::root::ShellRoot`] уведомляет и его виды (окна-диалоги
//!   и страницу «Персонализации» тоже); сессия настроек игры доходит до разделов «Персонализации»
//!   через `GameSettings::sync`;
//! - уведомление посреди отрисовки окна доходит только к следующему кадру;
//! - в начале `render` — [`crate::live::rendered`] (реестр живого инспектора, dev-hooks);
//! - стиль места (`style`) задаёт размер сам: содержимое вида для раскладки не меряется.
//!
//! Кэша нет, пока чему-то нужен `prepaint` или `render` каждого кадра:
//! - стекло Liquid Glass: куски стекла ищет `prepaint` каждого блока по подложке (параллакс,
//!   снимок окна), неувиденные отпускаются из атласа, слои нижних панелей ставит их `prepaint`.
//!   Поэтому без кэша — только виды, в последней отрисовке которых стекло было ([`glass`]), и все
//!   закэшированные виды вокруг них; прочие виды и в этой теме берут кэш;
//! - открыто окно с размытой подложкой (просмотр картинок новости): её кусок так же ищет
//!   `prepaint` по снимку окна ([`crate::layer::Layers::blurred`]); кэш снят у всех видов;
//! - смена темы (.42 с): цвета всех видов перетекают с каждым кадром корня;
//! - обучение «Персонализации»: якоря ([`crate::configs::anchors`], вкладка в шапке) пишут рамки
//!   в каждом кадре, повтор кадра их бы не записал.

use crate::theme::Theme;
use gpui::{
    AnyElement, AnyView, App, Bounds, Element, ElementId, EntityId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, StyleRefinement, Window,
};
use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    /// Закэшированные виды, чьи раскладка или `prepaint` идут сейчас (внешний первым).
    static STACK: RefCell<Vec<EntityId>> = const { RefCell::new(Vec::new()) };
    /// Виды, в последней отрисовке которых было стекло Liquid Glass ([`glass`]).
    static GLASS: RefCell<HashSet<EntityId>> = RefCell::default();
}

/// Берут ли виды кэш сейчас (не под размытой подложкой, не в смене темы и не во время обучения).
pub fn on(cx: &App) -> bool {
    let theme = cx.global::<Theme>();
    !crate::layer::Layers::blurred(cx) && !theme.blending() && !crate::configs::anchors::active()
}

/// Вид `view` на месте `style`: закэшированный, если `on` ([`on`]) и в его прошлой отрисовке не
/// было стекла Liquid Glass, иначе обычный.
pub fn cached(view: impl Into<AnyView>, style: StyleRefinement, on: bool) -> AnyElement {
    let view: AnyView = view.into();
    let id = view.entity_id();
    let on = on && !GLASS.with(|glass| glass.borrow().contains(&id));
    let child = if on {
        view.cached(style).into_any_element()
    } else {
        view.into_any_element()
    };
    Slot {
        view: id,
        child: crate::live::scope(id, child),
    }
    .into_any()
}

/// Внутри каких закэшированных видов (внешний первым) идёт раскладка или `prepaint` сейчас.
pub fn stack() -> Vec<EntityId> {
    STACK.with(|stack| stack.borrow().clone())
}

/// Здесь блок стекла Liquid Glass (кусок подложки, слой нижней панели): закэшированные виды вокруг
/// него рисуются без кэша, пока он есть. Звать из `request_layout` или `prepaint` блока.
pub fn glass() {
    STACK.with(|stack| {
        GLASS.with(|glass| glass.borrow_mut().extend(stack.borrow().iter().copied()))
    });
}

/// Вид в стопке [`STACK`], пока жив.
struct Inside;

impl Inside {
    fn enter(view: EntityId) -> Inside {
        STACK.with(|stack| stack.borrow_mut().push(view));
        Inside
    }
}

impl Drop for Inside {
    fn drop(&mut self) {
        STACK.with(|stack| stack.borrow_mut().pop());
    }
}

/// Место вида [`cached`]: пока идут его раскладка и `prepaint` (там рисуется закэшированный вид,
/// там же раскладка детей `deferred`), вид — в стопке.
struct Slot {
    view: EntityId,
    child: AnyElement,
}

impl Element for Slot {
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
        // Стекло вида — то, что он отметит в этой отрисовке; повторённый кадр стекла не держит
        // (вид со стеклом в кэш не попадает).
        GLASS.with(|glass| glass.borrow_mut().remove(&self.view));
        let _inside = Inside::enter(self.view);
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
        let _inside = Inside::enter(self.view);
        self.child.prepaint(window, cx);
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, Context, Entity, ParentElement, Render, Styled, TestAppContext, div};
    use std::cell::Cell;
    use std::rc::Rc;

    /// Вид со счётчиком отрисовок; `glass` — блок стекла Liquid Glass внутри.
    struct Counted {
        count: Rc<Cell<usize>>,
        glass: bool,
    }

    impl Render for Counted {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.count.set(self.count.get() + 1);
            let theme = cx.global::<Theme>();
            let el = div().size_full();
            if self.glass {
                crate::liquid::glass(
                    el,
                    crate::liquid::Kind::Panel,
                    gpui::rems(0.5),
                    gpui::px(1.),
                    theme,
                    1.,
                )
            } else {
                el
            }
        }
    }

    /// Корень окна, как `LauncherRoot`: шаг темы в начале кадра.
    struct Host(Entity<Counted>);

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            crate::theme::frame(window, cx);
            let on = on(cx);
            div().size_full().child(cached(
                self.0.clone(),
                StyleRefinement::default().size_full(),
                on,
            ))
        }
    }

    type Opened<'a> = (
        Rc<Cell<usize>>,
        Entity<Host>,
        &'a mut gpui::VisualTestContext,
    );

    fn open(cx: &mut TestAppContext) -> Opened<'_> {
        cx.update(crate::theme::init);
        let count = Rc::new(Cell::new(0));
        let shared = count.clone();
        let (host, cx) = cx.add_window_view(|_, cx| {
            Host(cx.new(|_| Counted {
                count: shared,
                glass: false,
            }))
        });
        cx.run_until_parked();
        (count, host, cx)
    }

    /// Кадр окна через 20 мс часов теста (кадры движения — не чаще кадра монитора).
    fn next_frame(cx: &mut gpui::VisualTestContext) {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(20));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }

    /// Корень нарисован заново: сколько раз с этим нарисован вид.
    fn host_frame(host: &Entity<Host>, cx: &mut gpui::VisualTestContext) {
        host.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }

    /// Перетекание темы меняет цвета и закэшированных видов: каждый его кадр — окно целиком.
    #[gpui::test]
    fn theme_change_redraws_cached_views_every_frame(cx: &mut TestAppContext) {
        let (count, host, cx) = open(cx);
        let drawn = count.get();
        host_frame(&host, cx);
        assert_eq!(
            count.get(),
            drawn,
            "корень нарисован заново, вид — повторён"
        );
        // Начало смены — через минуту: перетекание идёт по настоящим часам и в загруженном
        // прогоне тестов не должно успеть кончиться.
        let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
        cx.update(|window, cx| {
            cx.global_mut::<Theme>().set("paper", None, true, later);
            window.refresh();
        });
        cx.run_until_parked();
        let first = count.get();
        next_frame(cx);
        next_frame(cx);
        assert_eq!(count.get(), first + 2);
    }

    /// Пока идёт обучение, виды рисуются с каждым кадром корня: якоря (вкладка в шапке) пишут
    /// рамки только в `prepaint`.
    #[gpui::test]
    fn tour_anchors_draw_views_without_cache(cx: &mut TestAppContext) {
        let (count, host, cx) = open(cx);
        crate::configs::anchors::begin();
        let drawn = count.get();
        host_frame(&host, cx);
        assert_eq!(count.get(), drawn + 1);
        crate::configs::anchors::end();
        host_frame(&host, cx);
        let drawn = count.get();
        host_frame(&host, cx);
        assert_eq!(count.get(), drawn, "обучение кончилось — снова кэш");
    }

    /// Liquid Glass: вид без стекла берёт кэш; вид со стеклом рисуется с каждым кадром корня
    /// (кусок ищет его `prepaint`), а стекло ушло — снова кэш.
    #[gpui::test]
    fn liquid_glass_caches_views_without_glass(cx: &mut TestAppContext) {
        let (count, host, cx) = open(cx);
        cx.update(|window, cx| {
            cx.global_mut::<Theme>().set(
                crate::theme::LIQUID_GLASS,
                None,
                false,
                std::time::Instant::now(),
            );
            window.refresh();
        });
        cx.run_until_parked();
        let drawn = count.get();
        host_frame(&host, cx);
        assert_eq!(count.get(), drawn, "без стекла — повтор кадра");

        // Стекло появилось (данные вида): этот кадр рисует вид, следующие — тоже.
        let view = host.read_with(cx, |host, _| host.0.clone());
        view.update(cx, |view, cx| {
            view.glass = true;
            cx.notify();
        });
        cx.run_until_parked();
        let drawn = count.get();
        host_frame(&host, cx);
        host_frame(&host, cx);
        assert_eq!(count.get(), drawn + 2, "со стеклом — без кэша");

        // Стекло ушло: кадр без него, следующий — первый в кэше (GPUI запоминает кадр вида),
        // дальше — повтор.
        view.update(cx, |view, cx| {
            view.glass = false;
            cx.notify();
        });
        cx.run_until_parked();
        host_frame(&host, cx);
        let drawn = count.get();
        host_frame(&host, cx);
        assert_eq!(count.get(), drawn, "стекло ушло — снова кэш");
    }

    /// Стекло внутри вида внутри закэшированного: без кэша оба, пока оно есть.
    #[gpui::test]
    fn glass_deep_inside_keeps_outer_views_uncached(cx: &mut TestAppContext) {
        struct Outer(Entity<Counted>, Rc<Cell<usize>>);

        impl Render for Outer {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.1.set(self.1.get() + 1);
                div().size_full().child(cached(
                    self.0.clone(),
                    StyleRefinement::default().size_full(),
                    true,
                ))
            }
        }

        struct Top(Entity<Outer>);

        impl Render for Top {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                crate::theme::frame(window, cx);
                div().size_full().child(cached(
                    self.0.clone(),
                    StyleRefinement::default().size_full(),
                    true,
                ))
            }
        }

        cx.update(crate::theme::init);
        let (inner, outer) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let (inner_count, outer_count) = (inner.clone(), outer.clone());
        let (top, cx) = cx.add_window_view(|_, cx| {
            let view = cx.new(|_| Counted {
                count: inner_count,
                glass: true,
            });
            Top(cx.new(|_| Outer(view, outer_count)))
        });
        cx.update(|window, cx| {
            cx.global_mut::<Theme>().set(
                crate::theme::LIQUID_GLASS,
                None,
                false,
                std::time::Instant::now(),
            );
            window.refresh();
        });
        cx.run_until_parked();
        let (i, o) = (inner.get(), outer.get());
        top.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!((inner.get(), outer.get()), (i + 1, o + 1));
    }
}
