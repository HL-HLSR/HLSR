//! Кнопки окна (`WindowControls.vue` 0.4.9): «Полный экран» (левее «Свернуть», если включена в
//! настройках `fullscreenButton`), «Свернуть», «Закрыть»; F11 — полный экран и без кнопки.
//!
//! Полный экран делает главный процесс (`window-command fullscreen`, A2); окно выходит из него и
//! само (свернули, спрятали, сменили масштаб) — приходит `launcher-fullscreen-changed`. Смена
//! плавная: окно гаснет (.13 с), меняет размер невидимым и проявляется (.24 с) — `App.vue`
//! `window-switching`; без анимаций — сразу. `scale(.985)` не играет: у блоков GPUI нет `scale`.

use super::root::ShellRoot;
use crate::icons::fa_glyph;
use crate::kit::button::{focus_handle, focus_visible, outline};
use crate::layer::{Layers, TipExt};
use crate::motion::{AnimGate, Timing, hover, mix};
use crate::scale::{u, zoom};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{
    AnyElement, Context, ElementId, Hsla, Window, div, prelude::*, transparent_white, white,
};
use hlsr_ui::services::{Reply, Services};
use serde_json::{Value, json};
use std::time::Duration;

/// Окно гаснет перед сменой размера.
const FADE_OUT: Duration = Duration::from_millis(130);
/// Новый размер успевает нарисоваться до проявления.
const SETTLE: Duration = Duration::from_millis(70);

/// Состояние кнопок окна (`data()` WindowControls).
#[derive(Debug, Default)]
pub struct WindowControls {
    pub fullscreen: bool,
    /// Окно погашено на время смены (`html.window-switching`).
    pub switching: bool,
    /// Идёт смена (повторное нажатие ждёт).
    busy: bool,
}

impl WindowControls {
    /// Ответ `window-command`: `{ok, fullscreen}` — новое состояние; иначе прежнее.
    fn apply(&mut self, reply: Reply, cx: &mut gpui::App) {
        if let Ok(result) = reply
            && result.get("ok") == Some(&Value::Bool(true))
        {
            self.set_fullscreen(result.get("fullscreen") == Some(&Value::Bool(true)), cx);
        }
    }

    /// `html.window-fullscreen`: углы и контур окна; подложки окон — без углов.
    pub fn set_fullscreen(&mut self, on: bool, cx: &mut gpui::App) {
        if self.fullscreen != on {
            self.fullscreen = on;
            Layers::set_fullscreen(cx, on);
        }
    }

    /// `command(name)`: `window-command` и состояние из ответа.
    pub fn command(name: &'static str, cx: &mut Context<ShellRoot>) {
        let Some(services) = services(cx) else { return };
        let reply = services.invoke("window-command", vec![json!(name)]);
        cx.spawn(async move |this, cx| {
            let reply = reply.await;
            this.update(cx, |root, cx| {
                root.controls.apply(reply, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `mounted`: окно могло уже быть на весь экран (шапку пересоздала смена языка).
    pub fn query(cx: &mut Context<ShellRoot>) {
        Self::command("fullscreen-state", cx);
    }

    /// `toggleFullscreen`: гаснет, команда, проявляется.
    pub fn toggle(&mut self, cx: &mut Context<ShellRoot>) {
        if self.busy {
            return;
        }
        let Some(services) = services(cx) else { return };
        let animate = !AnimGate::still(cx).any();
        self.busy = true;
        self.switching = animate;
        cx.notify();
        cx.spawn(async move |this, cx| {
            if animate {
                cx.background_executor().timer(FADE_OUT).await;
            }
            let reply = services
                .invoke("window-command", vec![json!("fullscreen")])
                .await;
            this.update(cx, |root, cx| {
                root.controls.apply(reply, cx);
                cx.notify();
            })
            .ok();
            if animate {
                cx.background_executor().timer(SETTLE).await;
            }
            this.update(cx, |root, cx| {
                root.controls.switching = false;
                root.controls.busy = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Обработчик нажатия кнопки окна.
type OnClick = Box<dyn Fn(&mut Window, &mut gpui::App)>;

fn services(cx: &gpui::App) -> Option<Services> {
    super::root::ShellContext::get(cx).map(|context| context.services.clone())
}

/// Кнопки окна в шапке: 40 (48 на высоком окне) × 40, `--l-radius-sm`, между ними шаг шапки
/// минус поля; наведение `rgba(255,255,255,.09)` и белый значок, нажатие `.14`, «Закрыть» —
/// `--btn-danger-*`; фокус — контур 2 px `--accent-focus` внутрь на 3 px.
pub(super) fn buttons(
    root: &ShellRoot,
    window: &mut Window,
    cx: &mut Context<ShellRoot>,
) -> impl IntoElement + use<> {
    let theme = cx.global::<Theme>().clone();
    let zoom = zoom(window);
    let fullscreen = root.controls.fullscreen;
    let show_fullscreen = root.prefs.borrow().get("fullscreenButton") != Some(&Value::Bool(false));
    let shell = root.shell.clone();
    let size = theme.len("l-header-control");
    let radius = theme.len("l-radius-sm");
    let focus_ring = theme.color("accent-focus");
    let services = root.services.clone();
    let mut button = |id: &'static str,
                      class: &'static str,
                      title: String,
                      (hover_bg, active_bg): (Hsla, Hsla),
                      on_click: OnClick|
     -> AnyElement {
        let element_id = ElementId::Name(id.into());
        let focus = focus_handle(&element_id, window, cx);
        let ring = focus_visible(&focus, window);
        let meta = crate::live::Meta::new("button", element_id.clone())
            .label(Some(title.clone().into()))
            .focused(focus.is_focused(window));
        let element = hover(
            ElementId::Name(format!("{id}-hover").into()),
            Timing::fast(),
            move |k| {
                div()
                    .id(element_id)
                    .track_focus(&focus)
                    .relative()
                    .occlude()
                    .size(u(size))
                    .rounded(u(radius))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(mix(transparent_white(), hover_bg, k))
                    .text_color(mix(white().opacity(0.55), white(), k))
                    .active(move |s| s.bg(active_bg))
                    .tip(title)
                    .button_text()
                    .on_click(move |_, window, cx| on_click(window, cx))
                    .child(
                        div()
                            .w(u(24.))
                            .flex()
                            .justify_center()
                            .child(fa_glyph(class, 17.)),
                    )
                    .when(ring, |el| {
                        el.child(outline(zoom, radius, -3., 2., focus_ring))
                    })
            },
        );
        crate::live::mark(meta, element)
    };
    let plain = (white().opacity(0.09), white().opacity(0.14));
    let danger = (
        theme.color("btn-danger-bg"),
        theme.color("btn-danger-pressed"),
    );
    let command = |name: &'static str| -> OnClick {
        let services = services.clone();
        Box::new(move |_, cx| {
            let reply = services.invoke("window-command", vec![json!(name)]);
            cx.background_spawn(async move {
                let _ = reply.await;
            })
            .detach();
        })
    };
    let toggle: OnClick = Box::new(|_, cx| {
        if let Some(root) = ShellRoot::get(cx) {
            root.update(cx, |root, cx| root.controls.toggle(cx));
        }
    });
    let (expand_class, expand_title) = if fullscreen {
        ("fas fa-compress", "#L_FULLSCREEN_EXIT")
    } else {
        ("fas fa-expand", "#L_FULLSCREEN")
    };
    let gap = theme.len("l-header-gap") - 2. * theme.len("l-header-inset");
    div()
        .flex()
        .gap(u(gap))
        .when(show_fullscreen, |row| {
            row.child(button(
                "window-fullscreen",
                expand_class,
                shell.tr(expand_title, &[]),
                plain,
                toggle,
            ))
        })
        .child(button(
            "window-minimize",
            "fas fa-minus",
            shell.tr("#L_MINIMIZE", &[]),
            plain,
            command("minimize"),
        ))
        .child(button(
            "window-close",
            "fas fa-times",
            shell.tr("#L_CLOSE", &[]),
            danger,
            command("close"),
        ))
}
