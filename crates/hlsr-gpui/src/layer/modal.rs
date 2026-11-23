//! Окно-диалог (ModalShell slint, `ConfirmModal`/`SettingsModal` 0.4.9): подложка во всё окно,
//! окно по центру, переход `l-modal`, Esc, ловушка фокуса, фокус при открытии и возврат.
//!
//! Состояние окна держит экран-владелец (как `data` компонента Vue): [`Modal::open`] и
//! [`Modal::close`] зовутся из его обработчиков, [`Modal::render`] — из его `render` всегда, и
//! при закрытом окне: уход доигрывает, пока элемент в кадре.

use super::{Dismiss, FocusNext, FocusPrev, Handler, Layers, Z_MODAL, Z_TOP, trap};
use crate::liquid;
use crate::motion::{self, MODAL, presence};
use crate::paint::shadow;
use crate::scale::zoom;
use crate::theme::Theme;
use gpui::{
    App, Context, Div, ElementId, FocusHandle, Hsla, IntoElement, MouseButton, Point, Window,
    anchored, deferred, div, prelude::*, px,
};
use std::rc::Rc;

pub struct Modal {
    id: ElementId,
    /// Контейнер окна: фокус внутри него — фокус в окне; Tab ходит внутри.
    scope: FocusHandle,
    open: bool,
    /// Слой окна в стопке (1 — нижнее); остаётся на время ухода.
    pub(super) depth: usize,
    /// Фокус до открытия (`lastFocused`).
    restore: Option<FocusHandle>,
    /// Окно открыто, его элементы ещё не нарисованы: фокус — после первого кадра (`$nextTick`).
    pending_focus: bool,
    initial: Option<FocusHandle>,
    z: usize,
    backdrop_closes: bool,
    scrim: Option<Hsla>,
    /// Подложка размывает то, что под ней ([`Modal::blur`]).
    blur: bool,
    /// Переход окна: `l-modal` или [`motion::INSTANT`].
    transition: &'static str,
}

impl Modal {
    /// Окно экрана-владельца: владелец ушёл при открытом окне — окно уходит из стопки.
    pub fn new<V: 'static>(id: impl Into<ElementId>, cx: &mut Context<V>) -> Modal {
        let scope = cx.focus_handle();
        let open = scope.clone();
        cx.on_release(move |_, cx| Layers::remove(cx, &open))
            .detach();
        Modal {
            id: id.into(),
            scope,
            open: false,
            depth: 1,
            restore: None,
            pending_focus: false,
            initial: None,
            z: Z_MODAL,
            backdrop_closes: true,
            scrim: None,
            blur: false,
            transition: MODAL,
        }
    }

    /// Своя подложка вместо `--l-modal-scrim` (просмотр картинок «Главной»).
    pub fn scrim(mut self, color: Hsla) -> Modal {
        self.scrim = Some(color);
        self
    }

    /// Подложка размывает то, что под ней: `backdrop-filter: blur(8px)` (`.news-viewer`). Под
    /// ней — снимок окна перед подложкой (патч `gpui-pre-windows`), размытый на ЦП
    /// ([`liquid::Kind::Scrim`]); пока снимка нет (первые кадры, Linux) — только заливка.
    pub fn blur(mut self) -> Modal {
        self.blur = true;
        self
    }

    /// Без `l-modal`: окно и подложка встают и уходят сразу (`<Teleport v-if>` без `<transition>`).
    pub fn instant(mut self) -> Modal {
        self.transition = motion::INSTANT;
        self
    }

    /// Окно над прочими окнами и тостами (z-index 70: подтверждение, профиль).
    pub fn on_top(mut self) -> Modal {
        self.z = Z_TOP;
        self
    }

    /// Нажатие на подложку окно не закрывает (приветствие).
    pub fn keep_on_backdrop(mut self) -> Modal {
        self.backdrop_closes = false;
        self
    }

    /// Что получает фокус при открытии («Отмена» подтверждения, крестик, вкладка). Без него —
    /// первый элемент окна с `tab_stop`.
    pub fn focus_initial(&mut self, handle: FocusHandle) {
        self.initial = Some(handle);
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Контейнер окна: внутри — ловушка Tab; щелчок по пустому месту окна ставит фокус сюда.
    pub fn scope(&self) -> &FocusHandle {
        &self.scope
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut App) {
        if self.open {
            return;
        }
        self.open = true;
        self.restore = window.focused(cx);
        self.depth = Layers::push(cx, self.scope.clone(), self.blur);
        // Esc и Tab до первого кадра окна уже идут в окно.
        window.focus(&self.scope, cx);
        self.pending_focus = true;
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut App) {
        if !self.open {
            return;
        }
        self.open = false;
        self.pending_focus = false;
        Layers::remove(cx, &self.scope);
        match self.restore.take() {
            Some(handle) => window.focus(&handle, cx),
            None if self.scope.contains_focused(window, cx) => window.blur(cx),
            None => {}
        }
    }

    /// Окно в кадре: подложка `--l-modal-scrim` во всё окно лаунчера и `content` по центру
    /// (вид окна — у `content`, обычно [`card`]). `on_close` — Esc и нажатие на подложку.
    pub fn render<E, F, C>(
        &mut self,
        window: &mut Window,
        cx: &mut App,
        on_close: F,
        content: C,
    ) -> impl IntoElement + use<E, F, C>
    where
        E: IntoElement,
        F: Fn(&mut Window, &mut App) + 'static,
        C: FnOnce() -> E + 'static,
    {
        if self.open
            && let Some(depth) = Layers::depth(cx, &self.scope)
        {
            self.depth = depth;
        }
        if std::mem::take(&mut self.pending_focus) {
            let scope = self.scope.clone();
            let initial = self.initial.clone();
            // Этот кадр рисует окно; следующий видит его элементы в порядке Tab.
            window.on_next_frame(move |window, cx| {
                // До кадра окно закрыли или над ним открыли другое: фокус уже не его.
                if Layers::top(cx).as_ref() != Some(&scope) {
                    return;
                }
                match initial {
                    Some(handle) => window.focus(&handle, cx),
                    None => trap(&scope, true, window, cx),
                }
            });
        }
        let theme = cx.global::<Theme>();
        let scrim = self.scrim.unwrap_or_else(|| theme.color("l-modal-scrim"));
        // Размытая подложка на всё окно: углы окна скругляет GPU-маска `window_corner_mask`.
        let blur = self.blur.then(|| {
            liquid::Surface::new(liquid::Kind::Scrim, gpui::rems(0.)).own(scrim)
        });
        let view = window.viewport_size();
        let scope = self.scope.clone();
        let backdrop_closes = self.backdrop_closes;
        let on_close: Handler = Rc::new(on_close);
        let meta = crate::live::Meta::new("modal", self.id.clone()).checked(self.open);
        let layer = presence(self.id.clone(), self.open, self.transition, move || {
            let on_backdrop = on_close.clone();
            let on_escape = on_close;
            let next = scope.clone();
            let prev = scope.clone();
            let el = div().id("modal-scrim").size_full();
            match blur {
                Some(surface) => liquid::with_surface(el, surface),
                None => el.bg(scrim),
            }
            .flex()
            .items_center()
            .justify_center()
            // Подложка ловит мышь: под ней ни наведения, ни полосы перетаскивания шапки.
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if backdrop_closes {
                    on_backdrop(window, cx);
                }
            })
            .child(crate::live::mark(
                meta,
                div()
                    .id("modal-window")
                    .track_focus(&scope)
                    .tab_group()
                    .occlude()
                    .on_action(move |_: &Dismiss, window, cx| on_escape(window, cx))
                    .on_action(move |_: &FocusNext, window, cx| trap(&next, true, window, cx))
                    .on_action(move |_: &FocusPrev, window, cx| trap(&prev, false, window, cx))
                    .child(content()),
            ))
        })
        .w(view.width)
        .h(view.height);
        deferred(
            anchored()
                .position(Point::default())
                .child(motion::layer(self.depth, layer)),
        )
        .with_priority(self.z)
    }
}

/// Окно `.confirm__window`: `--l-modal-bg`, рамка `--l-modal-border`, радиус `--l-radius`, тень
/// `0 24px 72px rgba(0, 0, 0, .6)`; в теме Liquid Glass — стекло окна. Ширину и поля задаёт окно.
pub fn card(theme: &Theme, window: &Window) -> Div {
    let zoom = zoom(window);
    liquid::glass(
        card_plain(theme, window),
        liquid::Kind::Window,
        theme.rem("l-radius"),
        px(1.),
        theme,
        zoom,
    )
}

/// [`card`] без стекла Liquid Glass: окна, которых нет в правилах темы (`.crash__window`,
/// окно превью «Персонализации»).
pub fn card_plain(theme: &Theme, window: &Window) -> Div {
    let zoom = zoom(window);
    div()
        .flex()
        .flex_col()
        .bg(theme.color("l-modal-bg"))
        .border_1()
        .border_color(theme.color("l-modal-border"))
        .rounded(theme.rem("l-radius"))
        .shadow(vec![shadow(
            zoom,
            0.,
            24.,
            72.,
            0.,
            gpui::black().opacity(0.6),
        )])
}
