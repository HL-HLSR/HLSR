//! Кнопка спонсоров в шапке (`SponsorsButton.vue`, в 0.4.9 без изменений): сердце розового
//! GitHub Sponsors (`#db61a2`) 40 × 40. Под курсором и пока открыто окно «Спонсорство» сердце
//! бьётся (`sponsors-beat` 1,1 с, при `.no-anim` стоит) и заливается: заливка проявляется
//! `--l-fast` и растёт от .6 пружиной `--l-dur`. Щелчок открывает окно (S12). Масштаб — кеглем
//! глифа в квадрате 24 (у блоков GPUI нет `scale`); нажатие — `scale(.88)` ([`active`]).

use crate::icons::fa_glyph;
use crate::kit::button::{focus_handle, focus_visible, outline};
use crate::layer::TipExt;
use crate::motion::{AnimGate, Timing, active, hover, keyframes, registry};
use crate::scale::{u, zoom};
use crate::sponsors::modal::{open_sponsors, sponsors_open};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{
    AnyElement, Context, ElementId, Hsla, Render, Role, SharedString, Window, div, prelude::*, rgb,
};
use hlsr_ui::services::Services;

/// `--sponsor`.
const SPONSOR: u32 = 0xdb61a2;
/// `.sponsors__btn { font-size: 22px }`.
const GLYPH: f32 = 22.;

pub struct SponsorsButton;

impl SponsorsButton {
    pub fn new(_services: &Services, _cx: &mut Context<SponsorsButton>) -> SponsorsButton {
        SponsorsButton
    }
}

/// `.sponsors__heart` 24 × 24: контур и заливка (прозрачность `fill`, масштаб `grow`), всё
/// сердце — в масштабе `beat`.
fn heart(beat: f32, fill: f32, grow: f32) -> AnyElement {
    let glyph = |classes: &str, scale: f32| {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(fa_glyph(classes, GLYPH * beat * scale))
    };
    div()
        .relative()
        .size(u(24.))
        .child(glyph("far fa-heart", 1.))
        .child(glyph("fas fa-heart", grow).opacity(fill))
        .into_any_element()
}

impl Render for SponsorsButton {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let zoom = zoom(window);
        let size = theme.len("l-header-control");
        let radius = theme.len("l-radius-sm");
        let ring_color = theme.color("accent-focus");
        let open = sponsors_open(cx);
        let title: SharedString = super::tr(cx)("#L_S_SPONSORS", &[]).into();
        let id = ElementId::Name("sponsors-btn".into());
        let focus = focus_handle(&id, window, cx);
        let ring = focus_visible(&focus, window);
        let tokens = &registry().tokens;
        let spring = Timing::new(tokens.l_dur, tokens.l_ease_spring);
        let still = AnimGate::still(cx).any();
        let button = active("sponsors-btn-press", true, move |held| {
            hover("sponsors-btn-hover", Timing::fast(), move |lit| {
                hover("sponsors-btn-grow", spring, move |grow| {
                    let (lit, grow) = if open { (1., 1.) } else { (lit, grow) };
                    let grow = 0.6 + 0.4 * grow;
                    let beating = open || lit > 0.;
                    // `:active` — `scale(.88)` сразу (перехода у сердца нет); пока сердце
                    // бьётся, анимация его перекрывает — видно при `.no-anim` и если увели мышь.
                    let heart = if held && (still || !beating) {
                        heart(0.88, lit, grow)
                    } else if beating {
                        // `:hover`/`.open` — бьётся; снова навели — с начала.
                        keyframes("sponsors-beat", "sponsors-beat", move |frame| {
                            heart(frame.scale as f32, lit, grow)
                        })
                        .into_any_element()
                    } else {
                        heart(1., lit, grow)
                    };
                    div()
                        .id(id)
                        .role(Role::Button)
                        .button_text()
                        .aria_label(title.clone())
                        .track_focus(&focus)
                        .relative()
                        .size(u(size))
                        .rounded(u(radius))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(Hsla::from(rgb(SPONSOR)))
                        .tip(title)
                        .on_click(|_, _, cx| open_sponsors(cx))
                        .child(heart)
                        .when(ring, |el| {
                            el.child(outline(zoom, radius, 0., 2., ring_color))
                        })
                })
            })
        });
        div().child(crate::snap::target("sponsors-btn", button))
    }
}
