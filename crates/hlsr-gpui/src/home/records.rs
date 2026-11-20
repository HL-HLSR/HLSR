//! Страница «Рекорды» (`#home-page-records` HomePanel 0.4.9): подпись «Рекорды и новые забеги»,
//! фильтр по игре кнопками как категории speedrun.com ([`Chip`]), список карточек со своей
//! прокруткой. Карточка — забег на speedrun.com: игра и категория, дата, метка места и
//! подкатегории, время и бегуны; бегун с профилем HLSR — ссылка на профиль. Места 1–3 —
//! [`super::run_place`]. Смена игры: прокрутка к началу, карточки входят лесенкой с той
//! стороны, где новая кнопка (`home-record-in(-prev)`). Строятся только карточки у видимой
//! области ([`crate::rows`]).

use super::panel::{HomePanel, open_profile};
use super::run_place::{self, Podium};
use crate::kit::button::{child_id, focus_handle, focus_visible, outline, white};
use crate::layer::TipExt;
use crate::motion::{Timing, apply, hover, keyframes, mix};
use crate::paint::ring;
use crate::rows::{Rows, Slot};
use crate::scale::{u, zoom, zpx};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::text::{RUBIK, TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, ElementId, FontWeight, IntoElement, ParentElement,
    Role, SharedString, Styled, Toggled, Window, div, prelude::*, px,
};
use hlsr_ui::controllers::home::RecordView;
use std::time::{Duration, Instant};

/// Сколько после смены игры карточки ещё входят: .26 с + лесенка 8 × 28 мс.
const ENTER: Duration = Duration::from_millis(600);

/// Состояние страницы в каркасе: прокрутка (`v-show` — живёт при смене вкладок), окно
/// видимости карточек и последняя смена игры (номер и начало).
#[derive(Default)]
pub struct Records {
    pub(super) scroll: Scroll,
    rows: Rows,
    swap: Option<(usize, Instant)>,
    swaps: usize,
}

/// `setFilter` + `resetRecordsScroll`.
fn set_filter(panel: &mut HomePanel, key: &str, window: &mut Window, cx: &mut Context<HomePanel>) {
    if key == panel.state.filter() {
        return;
    }
    panel.state.set_filter(key);
    let records = &mut panel.records;
    records.swaps += 1;
    records.swap = Some((records.swaps, cx.background_executor().now()));
    records.scroll.scroll_to(px(0.), false, window, cx);
    cx.notify();
}

pub(super) fn page(
    panel: &mut HomePanel,
    window: &mut Window,
    cx: &mut Context<HomePanel>,
) -> AnyElement {
    let view = panel.frame_view(cx);
    let theme = cx.global::<Theme>().clone();
    let tr = HomePanel::tr(cx);
    let (dim, faint, shade) = (
        theme.color("l-text-dim"),
        theme.color("l-text-faint"),
        theme.color("l-shade-rgb"),
    );

    let chips: Vec<AnyElement> = view
        .chips
        .iter()
        .map(|item| {
            let key = item.key.clone();
            Chip {
                name: format!("home-chip-{}", item.key),
                content: vec![item.label.clone().into_any_element()],
                active: item.active,
                title: Some(item.title.clone().into()),
                inset_focus: false,
            }
            .render(
                cx.listener(move |panel, _: &ClickEvent, window, cx| {
                    set_filter(panel, &key, window, cx)
                }),
                window,
                cx,
            )
        })
        .collect();
    let head = div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(u(12.))
        .child(spaced(
            tr("#L_HOME_RECORDS", &[]).to_uppercase(),
            12.,
            16.,
            FontWeight::BOLD,
            0.12,
            dim,
        ))
        .when(!chips.is_empty(), |el| {
            el.child(div().flex().flex_wrap().gap(u(6.)).children(chips))
        });

    let now = cx.background_executor().now();
    let entering = panel
        .records
        .swap
        .filter(|(_, at)| now.saturating_duration_since(*at) < ENTER);
    let entry = if panel.state.filter_next() {
        "home-record-next"
    } else {
        "home-record-prev"
    };
    let rows = panel.records.rows.clone();
    let keys = view
        .records
        .iter()
        .map(|record| record.id.clone())
        .collect();
    let slots = rows.plan(&panel.records.scroll, keys, 12., window, cx);
    let cards: Vec<AnyElement> = slots
        .into_iter()
        .map(|slot| {
            let index = match slot {
                Slot::Row(index) => index,
                Slot::Space(height) => return div().flex_none().h(height).into_any_element(),
            };
            let card = card(index, &view.records[index], &rows, &theme, window, cx);
            match entering {
                // Карточка, построенная позже (прокрутили к ней), идёт по той же лесенке.
                Some((swap, at)) => keyframes(
                    ElementId::named_usize(format!("home-record-in-{swap}"), index),
                    entry,
                    move |frame| apply(div().child(card), frame),
                )
                .index(index.min(8))
                .since(at)
                .into_any_element(),
                None => card,
            }
        })
        .collect();
    let empty = (!view.empty_records.is_empty()).then(|| {
        div()
            .text_px(14., 20.)
            .text_color(faint)
            .child(view.empty_records.clone())
    });
    // shortcut: края — заливкой цвета фона, как у ленты (S6).
    let list = ScrollArea::new("home-records", &panel.records.scroll)
        .bar(Bar::List)
        .contain()
        .fade(shade)
        .flex_1()
        .min_h_0()
        .mr(u(-4.))
        .child(
            rows.measure(
                &panel.records.scroll,
                div()
                    .flex()
                    .flex_col()
                    .gap(u(12.))
                    .pr(u(4.))
                    .pb(u(32.))
                    .children(cards)
                    .children(empty),
            ),
        );
    div()
        .size_full()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(u(16.))
        .child(head)
        .child(list)
        .into_any_element()
}

/// `.home-record`: кнопка во всю ширину, 16/20, радиус `--l-radius`, `--l-surface-solid` и
/// кольцо `--l-border`; наведение — кольцо `--l-border-strong` и белый .03 поверх; щелчок —
/// забег на speedrun.com (`runner-open-link`). Места 1–3 — фон, свечение и кубок места.
fn card(
    index: usize,
    record: &RecordView,
    rows: &Rows,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<HomePanel>,
) -> AnyElement {
    let zoom = zoom(window);
    let radius = theme.len("l-radius");
    let (solid, border, strong, accent) = (
        theme.color("l-surface-solid"),
        theme.color("l-border"),
        theme.color("l-border-strong"),
        theme.color("l-accent"),
    );
    let (faint, dim) = (theme.color("l-text-faint"), theme.color("l-text-dim"));
    let id = ElementId::Name(format!("home-record-{}", record.id).into());
    let focus = focus_handle(&id, window, cx);
    if focus.is_focused(window) {
        rows.focus(index);
    }
    let ring_on = focus_visible(&focus, window);
    let title = HomePanel::tr(cx)("#L_HOME_OPEN_RUN", &[]);
    let url = record.url.clone();
    let on_click = cx.listener(move |panel, _: &ClickEvent, _, cx| {
        let effects = panel.state.open_record(&url);
        panel.run(effects, cx);
    });
    let podium: Option<Podium> = run_place::podium(&id, record.place, window, cx);
    let pod = podium.as_ref().map(Podium::pod);

    let meta = div()
        .flex()
        .justify_between()
        .gap(u(12.))
        .text_px(12., 16.)
        .text_color(faint)
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(record.game.clone()),
        )
        .child(div().flex_none().child(record.date.clone()));
    // `.home-record__sub`: 1/7, пилюля, белый .05 и кольцо; метка места — цветом места.
    let sub = |text: String| {
        div()
            .px(u(7.))
            .py(u(1.))
            .rounded_full()
            .text_px(11., 16.)
            .font_weight(FontWeight::SEMIBOLD)
            .whitespace_nowrap()
            .child(text)
    };
    let subs: Vec<Div> = pod
        .filter(|_| !record.place_label.is_empty())
        .map(|pod| pod.chip(sub(record.place_label.clone()), zoom))
        .into_iter()
        .chain(record.subs.iter().map(|part| {
            sub(part.clone())
                .bg(white(0.05))
                .shadow(vec![ring(zoom, 1., border)])
                .text_color(dim)
        }))
        .collect();
    let subs = (!subs.is_empty()).then(|| div().flex().flex_wrap().gap(u(4.)).children(subs));

    let time = match pod {
        Some(pod) => run_place::time(
            child_id(&id, "time"),
            record.time.clone().into(),
            pod,
            window,
            cx,
        ),
        None => div()
            .flex_none()
            .text_px(20., 24.)
            .font_weight(FontWeight::BOLD)
            .tnum()
            .text_color(accent)
            .child(record.time.clone()),
    };
    let runners = runners(index, &id, record, rows, theme, window, cx);
    let (shift, height) = baseline_row(window);
    let main = div()
        .flex()
        .items_start()
        .gap(u(12.))
        .min_w_0()
        .h(u(height))
        .when(pod.is_some(), |el| el.pr(u(64.)))
        .child(time)
        .child(div().flex_1().min_w_0().mt(u(shift)).child(runners));

    let inner = id.clone();
    let target = format!("home-record-{index}");
    let card = hover(id, Timing::fast(), move |t| {
        let body = move |backdrop: Option<Div>, trophy: Option<AnyElement>| {
            div()
                .id(inner)
                .role(Role::Button)
                .button_text()
                .aria_label(title.clone())
                .tip(title)
                .track_focus(&focus)
                .relative()
                .flex()
                .flex_col()
                .gap(u(8.))
                .w_full()
                .px(u(20.))
                .py(u(16.))
                .rounded(u(radius))
                .cursor_pointer()
                .on_click(on_click)
                .map(|el| match pod {
                    Some(pod) => el.bg(solid).shadow(pod.shadows(t, zoom)),
                    None => el.bg(mix(solid, gpui::white(), 0.03 * t)).shadow(vec![ring(
                        zoom,
                        1.,
                        mix(border, strong, t),
                    )]),
                })
                .children(backdrop)
                .child(meta)
                .children(subs)
                .child(main)
                .children(trophy)
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., accent))
                })
                .into_any_element()
        };
        match podium {
            Some(podium) => podium.render(t, radius, move |backdrop, trophy| {
                body(Some(backdrop), Some(trophy))
            }),
            None => body(None, None),
        }
    });
    crate::snap::target(target, card)
}

/// `.home-record__main { align-items: baseline }`: время 20/24 и бегуны 14/20 на одной линии
/// букв. Сдвиг бегунов — по линиям букв GPUI; высота строки — как у Blink (подъём и спуск
/// округлены в физических px, половина интерлиньяжа вниз): при ×1 это 25, а не 24. CSS px.
fn baseline_row(window: &Window) -> (f32, f32) {
    let scale = zoom(window) * window.scale_factor();
    let text_system = window.text_system();
    let font_id = text_system.resolve_font(&gpui::font(RUBIK));
    let line = |size: f32, line: f32| {
        let font_px = zpx(scale, size);
        let ascent = f32::from(text_system.ascent(font_id, font_px));
        let descent = f32::from(text_system.descent(font_id, font_px)).abs();
        let line = line * scale;
        let exact = (line - ascent - descent) / 2. + ascent;
        let blink = ((line - ascent.round() - descent.round()) / 2.).floor() + ascent.round();
        (exact, blink)
    };
    let ((time, time_blink), (runner, runner_blink)) = (line(20., 24.), line(14., 20.));
    let height = (24. * scale).max(time_blink - runner_blink + 20. * scale);
    ((time - runner) / scale, height / scale)
}

/// Бегуны через запятую (14/600, многоточие); у бегуна с профилем HLSR — ссылка: подсказка
/// «Открыть профиль HLSR: имя», наведение и фокус — акцент с подчёркиванием, щелчок и Enter
/// открывают профиль, до карточки (забега) щелчок не доходит.
fn runners(
    at: usize,
    card: &ElementId,
    record: &RecordView,
    rows: &Rows,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<HomePanel>,
) -> AnyElement {
    let (text, accent) = (theme.color("l-text"), theme.color("l-accent"));
    let zoom = zoom(window);
    let tr = HomePanel::tr(cx);
    let mut row = div()
        .flex()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_px(14., 20.)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(text);
    for (index, runner) in record.runners.iter().enumerate() {
        if index > 0 {
            row = row.child(div().flex_none().child(", "));
        }
        let name: SharedString = runner.name.clone().into();
        if runner.profile.is_empty() {
            row = row.child(div().min_w_0().text_ellipsis().child(name));
            continue;
        }
        let id = ElementId::NamedChild(
            std::sync::Arc::new(card.clone()),
            format!("runner-{index}").into(),
        );
        let focus = focus_handle(&id, window, cx);
        if focus.is_focused(window) {
            rows.focus(at);
        }
        let ring_on = focus_visible(&focus, window);
        let label = tr("#L_HOME_RUNNER_PROFILE", &[runner.name.as_str()]);
        let profile = runner.profile.clone();
        let inner = id.clone();
        let link = hover(id, Timing::fast(), move |t| {
            let t = if ring_on { 1. } else { t };
            let color = mix(text, accent, t);
            div()
                .id(inner)
                .role(Role::Link)
                .aria_label(label.clone())
                .tip(label)
                .track_focus(&focus)
                .relative()
                .min_w_0()
                .text_ellipsis()
                .cursor_pointer()
                .text_color(color)
                .underline()
                .text_decoration_color(color.opacity(t))
                .on_click(move |_, _, cx: &mut App| {
                    cx.stop_propagation();
                    open_profile(&profile, cx);
                })
                .child(name)
                .when(ring_on, |el| el.child(outline(zoom, 3., 2., 2., accent)))
        });
        row = row.child(crate::snap::target(
            format!("home-runner-{at}-{index}"),
            link,
        ));
    }
    row.into_any_element()
}

/// Кнопка выбора как категории speedrun.com (`.home-chip`, `.players__chip` 0.4.9): 32 px, поля
/// 14, радиус 6, 13/700 `--l-text`, `--l-chip-bg` и кольцо `--l-border`; наведение —
/// `--l-chip-hover` и `--l-border-strong`; выбранная — `--l-accent-soft` поверх подложки и
/// кольцо 2 px акцентом.
pub(super) struct Chip {
    /// Цель шагов сцены.
    pub name: String,
    /// Подпись (у игроков — значок, текст, счётчик через 6).
    pub content: Vec<AnyElement>,
    pub active: bool,
    pub title: Option<SharedString>,
    /// Фокус кольцом внутри (`.players__chip`), иначе контур снаружи (`.home-chip`).
    pub inset_focus: bool,
}

impl Chip {
    pub fn render(
        self,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let theme = cx.global::<Theme>();
        let (bg, hover_bg, soft) = (
            theme.color("l-chip-bg"),
            theme.color("l-chip-hover"),
            theme.color("l-accent-soft"),
        );
        let (border, strong, accent, page) = (
            theme.color("l-border"),
            theme.color("l-border-strong"),
            theme.color("l-accent"),
            theme.color("l-bg"),
        );
        let text = theme.color("l-text");
        let zoom = zoom(window);
        let id = ElementId::Name(self.name.clone().into());
        let focus = focus_handle(&id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let Chip {
            name,
            content,
            active,
            title,
            inset_focus,
        } = self;
        let inner = id.clone();
        let chip = hover(id, Timing::fast(), move |t| {
            let t = if active { 0. } else { t };
            let mut rings = vec![if active {
                ring(zoom, 2., accent)
            } else {
                ring(zoom, 1., mix(border, strong, t))
            }];
            if ring_on && inset_focus {
                rings = vec![ring(zoom, 2., accent), ring(zoom, 4., page)];
            }
            div()
                .id(inner)
                .role(Role::RadioButton)
                .button_text()
                .aria_toggled(if active {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .track_focus(&focus)
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(6.))
                .h(u(32.))
                .px(u(14.))
                .rounded(u(6.))
                .bg(mix(bg, hover_bg, t))
                .shadow(rings)
                .text_px(13., 32.)
                .font_weight(FontWeight::BOLD)
                .whitespace_nowrap()
                .text_color(text)
                .cursor_pointer()
                .when_some(title, |el, title| el.tip(title))
                .on_click(on_click)
                // `linear-gradient(soft, soft), --l-chip-bg`: оттенок поверх подложки.
                .when(active, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .rounded(u(6.))
                            .bg(soft),
                    )
                })
                .children(content)
                .when(ring_on && !inset_focus, |el| {
                    el.child(outline(zoom, 6., 2., 2., accent))
                })
        });
        crate::snap::target(name, chip)
    }
}
