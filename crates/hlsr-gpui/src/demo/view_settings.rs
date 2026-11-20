//! «Дополнительные настройки» вида в кадре предпросмотра (`DemoViewSettings.vue` 0.4.9, `popover`):
//! ссылка раскрывает поверх окна переключатели KeyHUD, HUD, свой конфиг, свой HUD. Чего игра не
//! умеет, — переключатель неактивен, с пояснением, и стоит так, как будет в кадре.
//! Выбор — `previewView` страницы (его запоминает главный процесс при старте предпросмотра).

use super::panel::DemoPanel;
use crate::icons::fa_glyph;
use crate::kit::toggle;
use crate::layer::{Z_MENU, anchor_probe};
use crate::motion::{Timing, hover, mix};
use crate::paint::{ring, shadow};
use crate::scale::{u, viewport, zoom, zpx};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{
    AnyElement, App, Bounds, Context, FocusHandle, FontWeight, IntoElement, Pixels, Render, Role,
    SharedString, WeakEntity, Window, anchored, canvas, deferred, div, point, prelude::*, rgb,
};
use hlsr_core::language::Language;
use serde_json::Value;
use std::cell::Cell;
use std::rc::Rc;

/// Что недоступно игре, показывается таким, каким будет в кадре (как у игрока).
fn fixed(key: &str) -> bool {
    key != "keyhud"
}

pub struct ViewSettings {
    panel: WeakEntity<DemoPanel>,
    open: bool,
    focus: FocusHandle,
    /// Ссылка в координатах окна (прошлый кадр).
    anchor: Rc<Cell<Bounds<Pixels>>>,
    /// Высота панели прошлого кадра (CSS px) — выбрать место сверху или снизу.
    height: Rc<Cell<f32>>,
}

impl ViewSettings {
    pub fn new(panel: WeakEntity<DemoPanel>, cx: &mut Context<ViewSettings>) -> ViewSettings {
        ViewSettings {
            panel,
            open: false,
            focus: cx.focus_handle(),
            anchor: Rc::default(),
            height: Rc::new(Cell::new(0.)),
        }
    }

    /// Уход со страницы закрывает панель.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        if self.open {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }
}

impl Render for ViewSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(panel) = self.panel.upgrade() else {
            return div().into_any_element();
        };
        let (language, model, caps, hl1) = {
            let panel = panel.read(cx);
            let s = panel.state();
            let caps = s
                .detail
                .as_ref()
                .and_then(|detail| detail.get("render"))
                .and_then(|render| render.get("view"))
                .cloned()
                .unwrap_or_default();
            let hl1 = s
                .item()
                .and_then(|item| item.get("game"))
                .and_then(Value::as_str)
                == Some("hl1");
            (panel.language, s.preview_view.clone(), caps, hl1)
        };
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let open = self.open;
        let view = cx.weak_entity();
        let more = more_link(
            "dvs-more",
            hlsr_ui::tr(language, "#L_DEMO_MORE", &[]),
            open,
            Some(anchor_probe(self.anchor.clone()).into_any_element()),
            Rc::new(move |_, window, cx| {
                view.update(cx, |this, cx| this.toggle(window, cx)).ok();
            }),
        );
        let popover = open.then(|| {
            let weak = self.panel.clone();
            let on_change: OnChange = Rc::new(move |key, on, _, cx| {
                weak.update(cx, |panel, cx| panel.set_view(key, on, cx))
                    .ok();
            });
            let rows = rows(
                "dvs",
                language,
                hl1,
                |key| caps.get(key).is_some_and(hlsr_core::js::truthy),
                |key| model.get(key) == Some(&Value::Bool(true)),
                Some(on_change),
                &theme,
            );
            let panel_el = div()
                .id("dvs-panel")
                .track_focus(&self.focus)
                .occlude()
                .flex()
                .flex_col()
                .gap(u(10.))
                .w(u((viewport(window).0 - 24.).min(440.)))
                .py(u(12.))
                .px(u(14.))
                .rounded(theme.rem("l-radius-sm"))
                .bg(theme.color("l-modal-bg"))
                .shadow(vec![
                    ring(zoom, 1., theme.color("l-modal-border")),
                    shadow(zoom, 0., 18., 48., 0., gpui::black().opacity(0.55)),
                ])
                .text_left()
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.close(cx);
                    }
                }))
                .on_mouse_down_out(cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    if !this.anchor.get().contains(&event.position) {
                        this.close(cx);
                    }
                }))
                .children(rows);
            // Под ссылкой, если там хватает места; иначе туда, где места больше.
            let height = self.height.clone();
            let probe = canvas(
                move |bounds, _, _| height.set(f32::from(bounds.size.height) / zoom),
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            let anchor = self.anchor.get();
            let (view_w, view_h) = viewport(window);
            let (margin, gap) = (12., 6.);
            let width = (view_w - 24.).min(440.);
            let css = |value: Pixels| f32::from(value) / zoom;
            let (top, bottom) = (css(anchor.top()), css(anchor.bottom()));
            let left = (css(anchor.left()) + css(anchor.size.width) / 2. - width / 2.)
                .min(view_w - margin - width)
                .max(margin);
            let below = view_h - bottom - gap - margin;
            let above = top - gap - margin;
            let natural = self.height.get();
            let y = if natural <= below || below >= above {
                bottom + gap
            } else {
                (top - gap - natural).max(margin)
            };
            let max = if natural <= below || below >= above {
                below
            } else {
                above
            };
            deferred(
                anchored()
                    .position(point(zpx(zoom, left), zpx(zoom, y)))
                    .child(
                        panel_el
                            .max_h(u(max.max(0.)))
                            .overflow_y_scroll()
                            .child(probe),
                    ),
            )
            .with_priority(Z_MENU)
        });
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(u(10.))
            .w_full()
            .child(crate::snap::target("dvs-more", more))
            .children(popover)
            .into_any_element()
    }
}

/// Переключатель строки панели: ключ 0.4.9 и новое значение.
pub(super) type OnChange = Rc<dyn Fn(&'static str, bool, &mut Window, &mut App)>;

/// Ссылка `.dvs__more` «Дополнительные настройки» со стрелкой; `probe` — рамка ссылки для
/// всплывающей панели.
pub(super) fn more_link(
    id: &'static str,
    label: String,
    open: bool,
    probe: Option<AnyElement>,
    on_click: super::OnClick,
) -> impl IntoElement {
    let inner = SharedString::from(format!("{id}-btn"));
    let meta = crate::live::Meta::new("link", inner.clone())
        .label(Some(SharedString::from(label.clone())))
        .checked(open);
    let element = hover(id, Timing::fast(), move |hot| {
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .relative()
            .flex()
            .items_center()
            .gap(u(6.))
            .px(u(4.))
            .py(u(2.))
            .rounded(u(4.))
            .cursor_pointer()
            .text_size(u(13.))
            .font_weight(FontWeight::SEMIBOLD)
            // Голубая в любой теме (просьба владельца): на тёмном фоне читается везде.
            .text_color(mix(rgb(0x4db2ff).into(), rgb(0x7cc6ff).into(), hot))
            .child(div().underline().child(label))
            .child(fa_glyph(
                if open {
                    "fas fa-chevron-up"
                } else {
                    "fas fa-chevron-down"
                },
                10.,
            ))
            .children(probe)
            .button_text()
            .on_click(move |event, window, cx| on_click(event, window, cx))
    });
    crate::live::mark(meta, element)
}

/// Строки панели (`.dvs__row`): переключатель, подпись, пояснение. Чего игра не умеет
/// (`available`), — неактивно и стоит так, как будет в кадре; `checked` — выбор игрока;
/// `on_change` — `None` у недоступной формы (`inert`). `prefix` — начало id переключателей.
pub(super) fn rows(
    prefix: &str,
    language: Language,
    hl1: bool,
    available: impl Fn(&str) -> bool,
    checked: impl Fn(&str) -> bool,
    on_change: Option<OnChange>,
    theme: &Theme,
) -> Vec<AnyElement> {
    let t = |key: &str| hlsr_ui::tr(language, key, &[]);
    let (text, dim) = (theme.color("l-text"), theme.color("l-text-dim"));
    [
        (
            "keyhud",
            "KeyHUD".to_owned(),
            "#L_DEMO_EX_KEYHUD",
            "#L_DEMO_EX_KEYHUD_NA",
        ),
        (
            "hud",
            t("#L_DEMO_VIEW_HUD"),
            "#L_DEMO_VIEW_HUD_HINT",
            "#L_DEMO_VIEW_HUD_NA",
        ),
        (
            "ownConfig",
            t("#L_DEMO_VIEW_CONFIG"),
            "#L_DEMO_VIEW_CONFIG_HINT",
            "#L_DEMO_VIEW_CONFIG_NA",
        ),
        (
            "ownHud",
            t("#L_DEMO_VIEW_OWN_HUD"),
            if hl1 {
                "#L_DEMO_VIEW_OWN_HUD_HL1"
            } else {
                "#L_DEMO_VIEW_OWN_HUD_HINT"
            },
            "#L_DEMO_VIEW_OWN_HUD_NA",
        ),
    ]
    .into_iter()
    .map(|(key, label, hint, missing)| {
        let available = available(key);
        let checked = if available { checked(key) } else { fixed(key) };
        let mut switch = toggle(SharedString::from(format!("{prefix}-{key}")), checked)
            .disabled(!available)
            .label(label.clone());
        if let Some(on_change) = on_change.clone() {
            switch = switch.on_change(move |on, window, cx| on_change(key, *on, window, cx));
        }
        div()
            .flex()
            .items_start()
            .gap(u(12.))
            .child(div().mt(u(1.)).child(switch))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(u(2.))
                    .child(
                        div()
                            .text_px(13., 18.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_px(12., 12. * 1.4)
                            .text_color(dim)
                            .child(t(if available { hint } else { missing })),
                    ),
            )
            .into_any_element()
    })
    .collect()
}
