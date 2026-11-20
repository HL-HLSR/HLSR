//! Раздел «Скрипты» HL1 (`GameScripts.vue` 0.4.9): слева список скриптов гайда yavats/hl1-scripts
//! по главам с поиском, между колонками — [`pane_splitter`] `scripts:hl1-won` (списку 220, скрипту
//! 400), справа скрипт: название, «Сохранение», «Гайд», постер видео (щелчок — YouTube в
//! браузере) и «Открыть на YouTube», клавиши действий с захватом (пульс `gsc-pulse`), вопрос о
//! занятых клавишах, подсказки, описание (перевод или оригинал, markdown гайда), источник.
//! Логика — `hlsr_ui::controllers::game_scripts` через сессию настроек ([`Session::scripts`],
//! `scr_*`); захват клавиши — [`crate::keys`] (F9).

use super::keyboard::{BtnTone, Probe, RingBtn, measure, read_pane, save_pane, search_box, white};
use super::sections::SectionHost;
use crate::icons::fa_glyph;
use crate::image::Pictures;
use crate::kit::input::FieldStyle;
use crate::kit::{FieldEvent, Orientation, TextInput, pane_splitter};
use crate::layer::{TipExt, anchor_probe};
use crate::motion::{AnimGate, Timing, hover, mix};
use crate::paint::{ring, shadow};
use crate::scale::{u, zoom};
use crate::scroll::{Scroll, ScrollArea};
use crate::text::{MONO, RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, Context, Easing, ElementId, Entity, FontStyle, FontWeight,
    HighlightStyle, Hsla, InteractiveText, IntoElement, ObjectFit, ParentElement, Pixels, Render,
    Role, SharedString, Styled, StyledText, Subscription, TimedTransformation, Transformation,
    Window, canvas, div, hsla, img, prelude::*, relative, rgb, rgba, svg,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::game_scripts::{self, ScriptsView};
use hlsr_ui::controllers::game_settings::Session;
use hlsr_ui::images::{ImageLoader, ImageRequest, ImageSource, NoRemote};
use hlsr_ui::keyboard_layout::Engine;
use hlsr_ui::markdown::{self, Options};
use hlsr_ui::richtext::{Block, Doc, Inline, List, Role as Tone, Sheet};
use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// Список не уже 220, скрипт — не уже 400 (`:min` и `:rest` разделителя), полоса 12.
const LIST_MIN: f32 = 220.;
const DETAIL_MIN: f32 = 400.;
const PANE_KEY: &str = "scripts:hl1-won";
const SCOPE: &str = "game-scripts";

/// Поле без рамки внутри `.gsc-search`: 12 px, поля 7/0, строка 20 px (`font: inherit`).
fn bare_style(theme: &Theme) -> FieldStyle {
    let none = hsla(0., 0., 0., 0.);
    FieldStyle {
        height: 2. * 7. + 20.,
        pad_x: 0.,
        pad_y: 7.,
        size: 12.,
        bg: none,
        hover_bg: none,
        border: none,
        hover_border: none,
        focus_border: none,
        invalid_border: none,
        ring: true,
        glow: None,
        ..FieldStyle::gx(theme)
    }
}

/// Цвет отметки скрипта (`.gsc-mark.mark-*`).
fn mark_color(mark: Option<&str>) -> Hsla {
    let hex = match mark {
        Some("green") => 0x5fd068,
        Some("yellow") => 0xf2cf4a,
        Some("orange") => 0xff9f43,
        Some("red") => 0xff6b6b,
        Some("purple") => 0xb48cff,
        _ => return white(0.16),
    };
    rgb(hex).into()
}

fn tone(theme: &Theme, role: Tone) -> Hsla {
    theme.color(match role {
        Tone::Text => "l-text",
        Tone::TextDim => "l-text-dim",
        Tone::TextFaint => "l-text-faint",
        Tone::LAccent => "l-accent",
        Tone::Accent => "accent",
    })
}

fn rgba8([r, g, b, a]: [u8; 4]) -> Hsla {
    rgba(u32::from_be_bytes([r, g, b, a])).into()
}

type LinkHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Описание скрипта, разобранное один раз на текст.
struct Described {
    key: (String, String, Vec<String>),
    usage: Doc,
    notes: Vec<Doc>,
}

/// Раздел «Скрипты» HL1 (`GameScripts.vue` 0.4.9).
pub struct GameScripts {
    session: Rc<Session>,
    language: Language,
    search: Option<(Entity<TextInput>, Subscription)>,
    list_scroll: Scroll,
    detail_scroll: Scroll,
    /// Ширина списка с разделителя (`listWidth`); `None` — 28 %.
    list_w: Option<f64>,
    root: Probe,
    list: Probe,
    detail: Probe,
    /// Кнопки захвата по команде: рамка прошлого кадра (область мыши для захвата).
    capture_probes: Vec<(String, Probe)>,
    /// `scroll_reset` прошлого кадра: другой скрипт — деталь в начало.
    seen_reset: u32,
    described: Option<Described>,
    pictures: Pictures,
    #[cfg(target_os = "linux")]
    modifiers: Rc<std::cell::Cell<gpui::Modifiers>>,
}

impl GameScripts {
    pub fn new(host: &SectionHost, cx: &mut Context<GameScripts>) -> Self {
        let pictures = cx.try_global::<Pictures>().cloned().unwrap_or_else(|| {
            Pictures::new(ImageLoader::new(
                Rc::new(NoRemote),
                &*crate::env::GpuiEnv::new(cx),
            ))
        });
        cx.on_release({
            let pictures = pictures.clone();
            move |_, cx| pictures.release_scope(SCOPE, cx)
        })
        .detach();
        GameScripts {
            session: host.session.clone(),
            language: host.language,
            search: None,
            list_scroll: Scroll::new(),
            detail_scroll: Scroll::new(),
            list_w: read_pane(PANE_KEY, cx),
            root: Probe::default(),
            list: Probe::default(),
            detail: Probe::default(),
            capture_probes: Vec::new(),
            seen_reset: 0,
            described: None,
            pictures,
            #[cfg(target_os = "linux")]
            modifiers: Rc::default(),
        }
    }

    fn t(&self, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(self.language, key, args)
    }

    fn search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        if self.search.is_none() {
            let placeholder = self.t("#L_SCR_SEARCH", &[]);
            let input = cx.new(|cx| {
                TextInput::new(window, cx)
                    .style(bare_style)
                    .placeholder(placeholder)
            });
            let sub = cx.subscribe(&input, |this, input, event: &FieldEvent, cx| {
                if matches!(event, FieldEvent::Input | FieldEvent::Cleared) {
                    let text = input.read(cx).text().to_owned();
                    this.session.scr_set_query(&text);
                }
            });
            self.search = Some((input, sub));
        }
        self.search.as_ref().expect("поле создано выше").0.clone()
    }

    fn probe(&mut self, command: &str) -> Probe {
        if let Some((_, probe)) = self.capture_probes.iter().find(|(c, _)| c == command) {
            return probe.clone();
        }
        let probe = Probe::default();
        self.capture_probes
            .push((command.to_owned(), probe.clone()));
        probe
    }

    /// Описание текущего скрипта: markdown гайда → документы (`MD_OPTIONS`).
    fn described(&mut self, view: &ScriptsView) -> &Described {
        // Сравнение без копий: текст гайда копируется только при смене скрипта или перевода.
        let fresh = self.described.as_ref().is_some_and(|d| {
            d.key.0 == view.current_id && d.key.1 == view.usage && d.key.2 == view.notes
        });
        if !fresh {
            let options = Options {
                image_base: Some(game_scripts::image_base()),
                script_ids: Some(
                    game_scripts::script_ids()
                        .into_iter()
                        .collect::<HashSet<_>>(),
                ),
                ..Options::default()
            };
            self.described = Some(Described {
                usage: markdown::doc(&view.usage, &options),
                notes: view
                    .notes
                    .iter()
                    .map(|note| markdown::inline_doc(note, &options))
                    .collect(),
                key: (
                    view.current_id.clone(),
                    view.usage.clone(),
                    view.notes.clone(),
                ),
            });
        }
        self.described.as_ref().expect("описание собрано выше")
    }

    /// Нажатие «Назначить»: ожидание клавиши; кнопка мыши над этой же кнопкой — уже клавиша.
    fn start_capture(
        session: &Rc<Session>,
        command: &str,
        area: Probe,
        window: &mut Window,
        cx: &mut App,
    ) {
        if !session.scr_start_capture(command) {
            return;
        }
        let session = session.clone();
        crate::keys::start(
            window,
            cx,
            command,
            Engine::GoldSrc,
            area.get(),
            move |end, _, _| session.scr_capture_end(&end.owner, end.key),
        );
    }
}

impl GameScripts {
    /// `.gsc-list`: поиск и главы.
    fn list_el(
        &mut self,
        view: &ScriptsView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let search = self.search(window, cx);
        let (text, dim, faint) = (
            theme.color("l-text"),
            theme.color("l-text-dim"),
            theme.color("l-text-faint"),
        );
        let soft = theme.color("accent-soft");
        let mut chapters: Vec<AnyElement> = Vec::new();
        for (index, chapter) in view.chapters.iter().enumerate() {
            let items = chapter.scripts.iter().map(|item| {
                let active = item.id == view.current_id;
                let session = self.session.clone();
                let id = item.id.clone();
                let el_id = ElementId::Name(format!("gsc-item-{}", item.id).into());
                let inner = el_id.clone();
                let mark = mark_color(item.mark.as_deref());
                let title = item.title.clone();
                let keys = (!item.keys.is_empty()).then(|| item.keys.join(", "));
                crate::snap::target(
                    format!("gsc-item-{}", item.id),
                    hover(el_id, Timing::fast(), move |t| {
                        div()
                            .id(inner)
                            .flex()
                            .items_center()
                            .gap(u(8.))
                            .w_full()
                            .min_h(u(30.))
                            .px(u(8.))
                            .py(u(4.))
                            .rounded(u(6.))
                            .bg(if active { soft } else { white(0.05 * t) })
                            .text_color(if active { text } else { mix(dim, text, t) })
                            .text_px(12.5, 20.)
                            .cursor_pointer()
                            .role(Role::Button)
                            .button_text()
                            .on_click(move |_, _, _| session.scr_open(&id))
                            .child(div().flex_none().size(u(7.)).rounded_full().bg(mark))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(title),
                            )
                            .when_some(keys, |el, keys| {
                                el.child(
                                    div()
                                        .flex_none()
                                        .max_w(relative(0.4))
                                        .px(u(6.))
                                        .py(u(1.))
                                        .rounded(u(4.))
                                        .bg(white(0.08))
                                        .text_color(text)
                                        .text_px(10.5, 20.)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(keys),
                                )
                            })
                    }),
                )
            });
            chapters.push(
                div()
                    .when(index > 0, |el| el.mt(u(8.)))
                    .child(
                        div()
                            .px(u(8.))
                            .pt(u(6.))
                            .pb(u(4.))
                            .text_color(faint)
                            .text_px(11., 20.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(chapter.title.to_uppercase()),
                    )
                    .children(items)
                    .into_any_element(),
            );
        }
        if view.chapters.is_empty() {
            chapters.push(
                div()
                    .mt(u(16.))
                    .text_center()
                    .text_color(faint)
                    .text_px(12., 20.)
                    .child(self.t("#L_SCR_NOTHING_FOUND", &[]))
                    .into_any_element(),
            );
        }
        div()
            .relative()
            .flex()
            .flex_col()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .border_1()
            .border_color(theme.color("l-border"))
            .rounded(theme.rem("l-radius-sm"))
            .bg(theme.color("l-modal-bar"))
            .child(anchor_probe(self.list.clone()))
            .child(
                div()
                    .m(u(10.))
                    .child(search_box(&search, window, cx).text_px(12., 20.)),
            )
            .child(
                ScrollArea::new("gsc-list", &self.list_scroll)
                    .contain()
                    .flex_1()
                    .min_h_0()
                    .child(div().px(u(6.)).pb(u(10.)).children(chapters)),
            )
            .into_any_element()
    }

    /// `.gsc-head`: название, глава и автор, «Сохранение», «Гайд».
    fn head(
        &self,
        view: &ScriptsView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let save = view.save.clone().map(|url| {
            let session = self.session.clone();
            RingBtn::gsc("gsc-save")
                .icon("fas fa-download")
                .child(self.t("#L_SCR_SAVE", &[]))
                .render(window, cx, move |_, _| session.scr_open_link(&url))
        });
        let session = self.session.clone();
        let guide = view.guide_url.clone();
        let guide = RingBtn::gsc("gsc-guide")
            .icon("fab fa-github")
            .child(self.t("#L_SCR_GUIDE", &[]))
            .render(window, cx, move |_, _| session.scr_open_link(&guide));
        div()
            .flex()
            .items_start()
            .gap(u(8.))
            .mb(u(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_px(17., 17. * 1.3)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(view.title.clone()),
                    )
                    .child(
                        div()
                            .mt(u(2.))
                            .text_color(theme.color("l-text-dim"))
                            .text_px(12., 20.)
                            .child(view.meta.clone()),
                    ),
            )
            .children(save)
            .child(guide)
            .into_any_element()
    }

    /// `.gsc-video`: постер (щелчок — YouTube в браузере) или «видео нет»; ссылка под ним.
    fn video(
        &self,
        view: &ScriptsView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let radius = theme.rem("l-radius-sm");
        let Some(video) = &view.video else {
            return div()
                .flex()
                .items_center()
                .gap(u(8.))
                .p(u(14.))
                .rounded(radius)
                .bg(white(0.03))
                .shadow(vec![ring(zoom(window), 1., theme.color("l-border"))])
                .text_color(theme.color("l-text-faint"))
                .text_px(12., 20.)
                .child(fa_glyph("fas fa-video-slash", 12.))
                .child(self.t("#L_SCR_NO_VIDEO", &[]))
                .into_any_element();
        };
        let poster = ImageSource::parse(&video.poster).and_then(|source| {
            self.pictures
                .get(ImageRequest::new(source, SCOPE), window, cx)
                .and_then(Result::ok)
        });
        let session = self.session.clone();
        let accent = theme.color("accent");
        let play = hover("gsc-poster", Timing::fast(), move |t| {
            div()
                .id("gsc-poster-btn")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(gpui::black())
                .cursor_pointer()
                .role(Role::Button)
                .button_text()
                .on_click(move |_, _, _| session.scr_play(false))
                .when_some(poster, |el, poster| {
                    el.child(img(poster).size_full().object_fit(ObjectFit::Cover))
                })
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(u(56.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(mix(hsla(0., 0., 0., 0.6), accent, t))
                                .text_color(gpui::white())
                                .child(div().ml(u(3.)).child(fa_glyph("fas fa-play", 18.))),
                        ),
                )
        });
        let session = self.session.clone();
        let watch = video.watch.clone();
        let (dim, text) = (theme.color("l-text-dim"), theme.color("l-text"));
        let label = self.t("#L_SCR_OPEN_YOUTUBE", &[]);
        // `.gsc-link:hover`: светлее и с подчёркиванием (переход цвета у кнопки не задан).
        let link = hover("gsc-youtube", Timing::fast(), move |t| {
            let on = t > 0.;
            div()
                .id("gsc-youtube-link")
                .mt(u(6.))
                .text_px(12., 20.)
                .text_color(if on { text } else { dim })
                .when(on, |el| el.underline())
                .cursor_pointer()
                .role(Role::Link)
                // `.gsc-link` 0.4.9 — `<button>`: разрядки `body` нет.
                .button_text()
                .on_click(move |_, _, _| session.scr_open_link(&watch))
                .child(label)
        });
        div()
            .child(
                div()
                    .id("gsc-video")
                    .relative()
                    .w_full()
                    .max_w(u(720.))
                    .aspect_ratio(16. / 9.)
                    .rounded(radius)
                    .bg(gpui::black())
                    .overflow_hidden()
                    .aria_label(self.t("#L_SCR_PLAY", &[]))
                    .child(crate::snap::target("gsc-poster", play)),
            )
            .child(div().flex().child(crate::snap::target("gsc-youtube", link)))
            .into_any_element()
    }
}

impl GameScripts {
    /// `.gsc-keys`: «Как в гайде», строки действий с клавишами и захватом, вопрос о занятых
    /// клавишах, подсказки.
    fn keys(
        &mut self,
        view: &ScriptsView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = zoom(window);
        let (text, dim, faint) = (
            theme.color("l-text"),
            theme.color("l-text-dim"),
            theme.color("l-text-faint"),
        );
        let hint = |text: String| {
            div()
                .text_color(dim)
                .text_px(12., 12. * 1.45)
                .child(text)
                .into_any_element()
        };
        let session = self.session.clone();
        let as_guide = view.guide_keys_missing.then(|| {
            RingBtn {
                disabled: !view.installed,
                ..RingBtn::gsc("gsc-as-guide")
            }
            .icon("fas fa-magic")
            .child(self.t("#L_SCR_AS_GUIDE", &[]))
            .render(window, cx, move |_, _| session.scr_assign_guide_keys())
        });
        let mut column = div()
            .flex()
            .flex_col()
            .gap(u(6.))
            .mt(u(14.))
            .pt(u(12.))
            .border_t_1()
            .border_color(theme.color("l-border"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .min_h(u(30.))
                    .child(
                        div()
                            .text_px(13., 20.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.t("#L_SCR_KEYS", &[])),
                    )
                    .child(div().flex_1())
                    .children(as_guide),
            );
        if !view.installed {
            column = column.child(hint(self.t("#L_SCR_NOT_INSTALLED", &[])));
        }
        let mut down = shadow(zoom, 0., -2., 0., 0., hsla(0., 0., 0., 0.35));
        down.inset = true;
        for (index, row) in view.rows.iter().enumerate() {
            let command = row.row.command.clone();
            let mut line = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(u(6.))
                .min_h(u(32.))
                .child(
                    div()
                        .id(ElementId::Name(format!("gsc-command-{index}").into()))
                        .min_w_0()
                        .max_w(relative(0.5))
                        .px(u(8.))
                        .py(u(3.))
                        .rounded(u(6.))
                        .bg(white(0.06))
                        .font_family(MONO)
                        .text_px(12., 20.)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .tip(command.clone())
                        .child(command.clone()),
                );
            if row.row.guide_key.is_some() {
                line = line.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(4.))
                        .text_color(faint)
                        .text_px(11.5, 20.)
                        .child(self.t("#L_SCR_GUIDE_KEY", &[]))
                        .child(
                            div()
                                .min_w(u(18.))
                                .px(u(5.))
                                .rounded(u(4.))
                                .bg(white(0.09))
                                .shadow(vec![{
                                    let mut s =
                                        shadow(zoom, 0., -1., 0., 0., hsla(0., 0., 0., 0.35));
                                    s.inset = true;
                                    s
                                }])
                                .text_color(text)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_center()
                                .child(row.row.guide_key_name.clone()),
                        ),
                );
            }
            line = line.child(div().flex_1());
            for chip in &row.row.keys {
                let remove = chip.whole.then(|| {
                    let session = self.session.clone();
                    let key = chip.key.clone();
                    let label = self.t("#L_SCR_UNASSIGN", &[&chip.name]);
                    let el_id =
                        ElementId::Name(format!("gsc-unassign-{index}-{}", chip.key).into());
                    let inner = el_id.clone();
                    hover(el_id, Timing::fast(), move |t| {
                        div()
                            .id(inner)
                            .size(u(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(u(4.))
                            .bg(white(0.1 * t))
                            .text_color(mix(dim, text, t))
                            .cursor_pointer()
                            .role(Role::Button)
                            .button_text()
                            .aria_label(label.clone())
                            .tip(label)
                            .on_click(move |_, _, _| session.scr_unassign(&key))
                            .child(fa_glyph("fas fa-times", 10.))
                    })
                });
                line = line.child(
                    div()
                        .id(ElementId::Name(
                            format!("gsc-key-{index}-{}", chip.key).into(),
                        ))
                        .flex()
                        .items_center()
                        .gap(u(2.))
                        .pl(u(9.))
                        .pr(u(if chip.whole { 2. } else { 9. }))
                        .py(u(2.))
                        .rounded(u(6.))
                        .bg(if chip.from_config {
                            theme.color("accent").opacity(0.18)
                        } else {
                            white(0.09)
                        })
                        .shadow(vec![down.clone()])
                        .text_px(12., 20.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .when(!chip.whole, |el| el.italic())
                        .when_some(chip.title.clone(), |el, title| el.tip(title))
                        .when(chip.from_config, |el| {
                            el.child(
                                div()
                                    .mr(u(4.))
                                    .opacity(0.8)
                                    .child(fa_glyph("fas fa-file-import", 10.)),
                            )
                        })
                        .child(chip.name.clone())
                        .children(remove),
                );
            }
            let probe = self.probe(&command);
            if row.listening {
                // Кнопка сдвинулась (прокрутка, раскладка) — область захвата за ней.
                crate::keys::set_area(probe.get());
            }
            let session = self.session.clone();
            let area = probe.clone();
            let button = probe.get();
            let owner = command.clone();
            let mut capture = RingBtn {
                tone: if row.listening {
                    BtnTone::On
                } else {
                    BtnTone::Plain
                },
                disabled: !view.installed,
                ..RingBtn::gsc(ElementId::Name(format!("gsc-capture-{index}").into()))
            };
            if !row.listening {
                capture = capture.icon("fas fa-keyboard");
            }
            let capture = capture
                .child(row.capture_label.clone())
                .child(anchor_probe(probe))
                .render(window, cx, move |window, cx| {
                    Self::start_capture(&session, &owner, area.clone(), window, cx)
                });
            let capture = if row.listening {
                let ring = (!AnimGate::still(cx).any()).then(|| {
                    pulse_ring(
                        button,
                        zoom,
                        theme.len("l-radius-sm"),
                        theme.color("accent-soft"),
                    )
                });
                div()
                    .relative()
                    .child(capture)
                    .children(ring)
                    .into_any_element()
            } else {
                capture
            };
            line = line.child(crate::snap::target(format!("gsc-capture-{index}"), capture));
            column = column.child(line);
        }
        if !view.conflict.is_empty() {
            let (cancel, replace) = (self.session.clone(), self.session.clone());
            column = column.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(u(8.))
                    .px(u(10.))
                    .py(u(8.))
                    .rounded(theme.rem("l-radius-sm"))
                    .bg(rgba(0xff9f431a))
                    .shadow(vec![ring(zoom, 1., rgba(0xff9f4359).into())])
                    .child(
                        div()
                            .flex_grow_1()
                            .flex_shrink_1()
                            .flex_basis(u(240.))
                            .text_px(12., 12. * 1.45)
                            .child(view.conflict.clone()),
                    )
                    .child(
                        RingBtn::gsc("gsc-conflict-cancel")
                            .child(self.t("#L_CANCEL", &[]))
                            .render(window, cx, move |_, _| cancel.scr_cancel_conflict()),
                    )
                    .child(
                        RingBtn {
                            tone: BtnTone::Primary,
                            ..RingBtn::gsc("gsc-conflict-replace")
                        }
                        .child(self.t("#L_SCR_REPLACE", &[]))
                        .render(window, cx, move |_, _| replace.scr_confirm_conflict()),
                    ),
            );
        }
        column = column.children(view.hints.iter().cloned().map(hint));
        column.into_any_element()
    }

    /// `.gsc-desc`: перевод/оригинал, заметки плашками, описание markdown.
    fn description(
        &mut self,
        view: &ScriptsView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let session = self.session.clone();
        let translate = view.translation.then(|| {
            div()
                .flex()
                .items_center()
                .gap(u(8.))
                .min_h(u(30.))
                .child(
                    div()
                        .text_px(13., 20.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.t("#L_SCR_DESCRIPTION", &[])),
                )
                .child(div().flex_1())
                .child(
                    RingBtn {
                        width: Some(30.),
                        tone: if view.show_original {
                            BtnTone::Accent
                        } else {
                            BtnTone::Plain
                        },
                        title: Some(view.translate_label.clone().into()),
                        ..RingBtn::gsc("gsc-translate")
                    }
                    // `.gsc-btn i` (11 px) ниже `.gsc-translate i` (15 px) в CSS — побеждает он.
                    .child(fa_glyph("fas fa-language", 11.))
                    .render(window, cx, move |_, _| session.scr_toggle_original()),
                )
        });
        let session = self.session.clone();
        let pictures = self.pictures.clone();
        let described = self.described(view);
        let mut load = |src: &str| {
            ImageSource::parse(src).and_then(|source| {
                pictures
                    .get(ImageRequest::new(source, SCOPE), window, cx)
                    .and_then(Result::ok)
            })
        };
        let has_notes = !described.notes.is_empty();
        let notes: Vec<AnyElement> = described
            .notes
            .iter()
            .enumerate()
            .map(|(index, note)| {
                let id = ElementId::Name(format!("gsc-note-{index}").into());
                let links = doc_links(note, &session);
                div()
                    .px(u(9.))
                    .py(u(3.))
                    .rounded_full()
                    .bg(white(0.06))
                    .child(rich(
                        note,
                        &Sheet::GUIDE_NOTE,
                        &id,
                        theme,
                        &links,
                        &mut load,
                    ))
                    .into_any_element()
            })
            .collect();
        let links = doc_links(&described.usage, &session);
        let usage = rich(
            &described.usage,
            &Sheet::GUIDE,
            &ElementId::Name("gsc-usage".into()),
            theme,
            &links,
            &mut load,
        );
        div()
            .mt(u(14.))
            .pt(u(12.))
            .border_t_1()
            .border_color(theme.color("l-border"))
            .children(translate)
            .when(has_notes, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(u(6.))
                        .mt(u(if view.translation { 6. } else { 0. }))
                        .children(notes),
                )
            })
            .child(
                div()
                    .mt(u(if has_notes || view.translation {
                        12.
                    } else {
                        0.
                    }))
                    .child(usage),
            )
            .into_any_element()
    }
}
/// Ссылки документа: другой скрипт гайда или адрес наружу (`onLinkClick`).
fn doc_links(doc: &Doc, session: &Rc<Session>) -> LinkHandler {
    let links = doc.links.clone();
    let session = session.clone();
    Rc::new(move |index, _, _| {
        if let Some(link) = links.get(index) {
            session.scr_link(link.kind(), link.value());
        }
    })
}

/// Картинка гайда по адресу (`imageBase`); `None` — ещё грузится или битая.
type Load<'a> = &'a mut dyn FnMut(&str) -> Option<Arc<gpui::RenderImage>>;

fn child_id(id: &ElementId, part: impl std::fmt::Display) -> ElementId {
    ElementId::NamedChild(Arc::new(id.clone()), part.to_string().into())
}

/// Блоки подряд: между соседними — большее из полей (схлопывание CSS).
#[derive(Default)]
struct Flow {
    children: Vec<AnyElement>,
    first: bool,
    last: f32,
}

impl Flow {
    fn push(&mut self, el: AnyElement, top: f32, bottom: f32) {
        let el = if self.first {
            div().mt(u(self.last.max(top))).child(el).into_any_element()
        } else {
            self.first = true;
            el
        };
        self.children.push(el);
        self.last = bottom;
    }
}

/// Документ гайда (`.gsc-usage`, `.gsc-notes li`) по таблице стилей `sheet`.
fn rich(
    doc: &Doc,
    sheet: &Sheet,
    id: &ElementId,
    theme: &Theme,
    links: &LinkHandler,
    load: Load,
) -> AnyElement {
    let mut flow = Flow::default();
    for (index, block) in doc.blocks.iter().enumerate() {
        let (el, top, bottom) = block_el(block, sheet, &child_id(id, index), 0, theme, links, load);
        flow.push(el, top, bottom);
    }
    div()
        .flex()
        .flex_col()
        .font_family(RUBIK)
        .text_px(sheet.size, sheet.line_height.px(sheet.size))
        .text_color(tone(theme, sheet.color))
        .children(flow.children)
        .into_any_element()
}

fn block_el(
    block: &Block,
    sheet: &Sheet,
    id: &ElementId,
    depth: usize,
    theme: &Theme,
    links: &LinkHandler,
    load: Load,
) -> (AnyElement, f32, f32) {
    match block {
        Block::Paragraph(nodes) => {
            let (top, bottom) = sheet.paragraph;
            (inline_el(id, nodes, sheet, theme, links, load), top, bottom)
        }
        Block::Inline(nodes) => (inline_el(id, nodes, sheet, theme, links, load), 0., 0.),
        Block::Heading(_, nodes) => {
            let el = div()
                .font_weight(FontWeight(f32::from(sheet.heading.weight)))
                .child(inline_el(id, nodes, sheet, theme, links, load));
            (
                el.into_any_element(),
                sheet.heading.margins.0,
                sheet.heading.margins.1,
            )
        }
        Block::List(list) => {
            let (top, bottom) = if depth == 0 {
                sheet.list.margins
            } else {
                sheet.list.nested_margins
            };
            (
                list_el(list, sheet, id, depth, theme, links, load),
                top,
                bottom,
            )
        }
        Block::Code(code) => {
            let pre = sheet.pre;
            let (top, bottom) = pre.margins.unwrap_or((sheet.size, sheet.size));
            // shortcut: `overflow-x: auto` без полосы — длинная строка обрезается.
            let el = div()
                .px(u(pre.pad_x))
                .py(u(pre.pad_y))
                .rounded(u(pre.radius))
                .bg(rgba8(pre.background))
                .overflow_hidden()
                .font_family(sheet.code.family)
                .text_size(u(sheet.code_size(sheet.size)))
                .whitespace_nowrap()
                .child(code.clone());
            (el.into_any_element(), top, bottom)
        }
    }
}

/// `ul`/`ol`: поле слева под маркер, пункты через 2 px.
fn list_el(
    list: &List,
    sheet: &Sheet,
    id: &ElementId,
    depth: usize,
    theme: &Theme,
    links: &LinkHandler,
    load: Load,
) -> AnyElement {
    let start = list.start.map_or(1, |start| start as i64);
    let color = tone(theme, sheet.color);
    let line = sheet.line_height.px(sheet.size);
    let pad = sheet.list.padding_left;
    let mut items = Vec::new();
    for (index, item) in list.items.iter().enumerate() {
        let marker = if list.ordered {
            div()
                .flex_none()
                .w(u(pad))
                .flex()
                .justify_end()
                .whitespace_nowrap()
                .child(format!("{}.\u{a0}", start + index as i64))
        } else {
            let shape = div().size(u(4.));
            let shape = match depth {
                0 => shape.rounded_full().bg(color),
                1 => shape.rounded_full().border_1().border_color(color),
                _ => shape.bg(color),
            };
            div()
                .flex_none()
                .w(u(pad))
                .h(u(line))
                .flex()
                .items_center()
                .pl(u((pad - 17.).max(0.)))
                .child(shape)
        };
        let mut flow = Flow::default();
        for (at, block) in item.iter().enumerate() {
            let child = child_id(id, format!("{index}-{at}"));
            let (el, top, bottom) = block_el(block, sheet, &child, depth + 1, theme, links, load);
            flow.push(el, top, bottom);
        }
        items.push(
            div()
                .flex()
                .when(index > 0, |el| {
                    el.mt(u(sheet.list.item_margins.0.max(sheet.list.item_margins.1)))
                })
                .child(marker)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .children(flow.children),
                ),
        );
    }
    div().flex().flex_col().children(items).into_any_element()
}

/// Строчное содержимое: прогоны текста с `strong`/`em`/`code`/ссылками; картинка
/// (`display: block`) делит его на части.
fn inline_el(
    id: &ElementId,
    nodes: &[Inline],
    sheet: &Sheet,
    theme: &Theme,
    links: &LinkHandler,
    load: Load,
) -> AnyElement {
    let mut parts: Vec<AnyElement> = Vec::new();
    let mut run: Vec<&Inline> = Vec::new();
    let flush = |run: &mut Vec<&Inline>, parts: &mut Vec<AnyElement>| {
        if !run.is_empty() {
            let at = parts.len();
            parts.push(text_el(&child_id(id, at), run, sheet, theme, links));
            run.clear();
        }
    };
    for node in nodes {
        match node {
            Inline::Image { src, .. } => {
                flush(&mut run, &mut parts);
                if let Some(image) = load(src) {
                    let size = image.size(0);
                    let (w, h) = (size.width.0 as f32, size.height.0 as f32);
                    let width = w.min(sheet.image.max_width);
                    let height = if w > 0. { width * h / w } else { 0. };
                    parts.push(
                        div()
                            .my(u(sheet.image.margins.0))
                            .max_w_full()
                            .child(
                                img(image)
                                    .w(u(width))
                                    .h(u(height))
                                    .max_w_full()
                                    .rounded(u(sheet.image.radius)),
                            )
                            .into_any_element(),
                    );
                }
            }
            other => run.push(other),
        }
    }
    flush(&mut run, &mut parts);
    if parts.len() == 1 {
        return parts.pop().expect("одна часть");
    }
    div().flex().flex_col().children(parts).into_any_element()
}

fn text_el(
    id: &ElementId,
    nodes: &[&Inline],
    sheet: &Sheet,
    theme: &Theme,
    links: &LinkHandler,
) -> AnyElement {
    let strong_color = sheet.strong_color.map(|role| tone(theme, role));
    let accent = tone(theme, sheet.link_color);
    let mut text = String::new();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut mono: Vec<(Range<usize>, SharedString)> = Vec::new();
    let mut targets: Vec<(Range<usize>, usize)> = Vec::new();
    for node in nodes {
        match node {
            Inline::Text { text: part, style } => {
                let start = text.len();
                text.push_str(part);
                let range = start..text.len();
                if range.is_empty() {
                    continue;
                }
                let mut look = HighlightStyle::default();
                if style.strong {
                    look.font_weight = Some(FontWeight(f32::from(sheet.strong_weight)));
                    look.color = strong_color;
                }
                if style.em {
                    look.font_style = Some(FontStyle::Italic);
                }
                if style.code {
                    // shortcut: без полей 4 px и скругления плашки — у прогона текста их нет.
                    look.background_color = Some(rgba8(sheet.code.background));
                    look.color = sheet.code.color.map(|role| tone(theme, role));
                    mono.push((range.clone(), MONO.into()));
                }
                if let Some(link) = style.link {
                    look.color = Some(accent);
                    targets.push((range.clone(), link as usize));
                }
                if look != HighlightStyle::default() {
                    highlights.push((range, look));
                }
            }
            Inline::Break => text.push('\n'),
            Inline::Image { .. } => {}
        }
    }
    let styled = StyledText::new(SharedString::from(text))
        .with_highlights(highlights)
        .with_font_family_overrides(mono);
    if targets.is_empty() {
        return div().child(styled).into_any_element();
    }
    let ranges: Vec<Range<usize>> = targets.iter().map(|(range, _)| range.clone()).collect();
    let indices: Vec<usize> = targets.iter().map(|(_, link)| *link).collect();
    let links = links.clone();
    div()
        .child(
            InteractiveText::new(id.clone(), styled).on_click(ranges, move |at, window, cx| {
                if let Some(link) = indices.get(at) {
                    links(*link, window, cx);
                }
            }),
        )
        .into_any_element()
}

/// Ввод окна для захвата на Linux, где перехвата сообщений нет (`keys::events` F9).
#[cfg(target_os = "linux")]
fn linux_capture(el: gpui::Div, modifiers: Rc<std::cell::Cell<gpui::Modifiers>>) -> gpui::Div {
    use crate::keys::{events, feed};
    use hlsr_ui::keys::Verdict;
    el.capture_key_down(|event, _, cx| {
        if feed(&events::key_down(event)) == Verdict::Consume {
            cx.stop_propagation();
        }
    })
    .on_modifiers_changed(move |event, _, cx| {
        let before = modifiers.replace(event.modifiers);
        for input in events::modifiers_changed(before, event) {
            if feed(&input) == Verdict::Consume {
                cx.stop_propagation();
            }
        }
    })
    .capture_any_mouse_down(|event, _, cx| {
        if feed(&events::mouse_down(event)) == Verdict::Consume {
            cx.stop_propagation();
        }
    })
    .on_scroll(|event, _, cx| {
        if feed(&events::scroll_wheel(event)) == Verdict::Consume {
            cx.stop_propagation();
        }
    })
}

/// `gsc-pulse` (1,2 с ease-in-out, 50 % — `0 0 0 3px --accent-soft`): кольцо 3 px вокруг кнопки
/// `button` (px окна прошлого кадра) нарастает и гаснет. Анимация сцены GPUI
/// ([`TimedTransformation`]): кадр — только `present`, раздел не перерисовывается. Своими кадрами
/// пульс перерисовывал «Персонализацию» целиком, ~4 мс на кадр: на мониторе 280 Гц — целое ядро.
/// Кольцо — SVG под размер кнопки; сменился размер (подпись «Нажми клавишу») — ещё один кадр раздела.
/// Отступление: фаза — от общих часов анимаций GPUI, а не от начала ожидания клавиши.
fn pulse_ring(button: Bounds<Pixels>, zoom: f32, radius: f32, color: Hsla) -> impl IntoElement {
    let drawn = button.size;
    let (w, h) = (
        f32::from(drawn.width) / zoom + 6.,
        f32::from(drawn.height) / zoom + 6.,
    );
    let ring = (w > 6. && h > 6.).then(|| {
        svg()
            .data(ring_svg(w, h, radius).as_bytes())
            .absolute()
            .top(u(-3.))
            .left(u(-3.))
            .w(u(w))
            .h(u(h))
            .text_color(color)
            .with_timed_transformation(
                TimedTransformation::new(std::time::Duration::from_millis(1200))
                    .from(Transformation::alpha(0.))
                    .to(Transformation::alpha(1.))
                    .with_easing(Easing::EaseInOut)
                    .alternate(),
            )
    });
    div()
        .absolute()
        .inset_0()
        .child(
            canvas(
                move |bounds, window, _| {
                    let off = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() > 0.01;
                    if off(bounds.size.width, drawn.width) || off(bounds.size.height, drawn.height)
                    {
                        crate::motion::monitor_frame(window);
                    }
                },
                |_, _, _, _| {},
            )
            .size_full(),
        )
        .children(ring)
}

/// Кольцо `w` × `h` CSS px: снаружи скругление `radius + 3`, внутри — кнопка со скруглением `radius`.
fn ring_svg(w: f32, h: f32, radius: f32) -> String {
    let rounded = |x: f32, y: f32, w: f32, h: f32, r: f32| {
        format!(
            "M{a} {y}H{b}A{r} {r} 0 0 1 {x2} {c}V{d}A{r} {r} 0 0 1 {b} {y2}H{a}A{r} {r} 0 0 1 {x} {d}V{c}A{r} {r} 0 0 1 {a} {y}Z",
            a = x + r,
            b = x + w - r,
            c = y + r,
            d = y + h - r,
            x2 = x + w,
            y2 = y + h,
        )
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><path fill="#fff" fill-rule="evenodd" d="{}{}"/></svg>"##,
        rounded(0., 0., w, h, radius + 3.),
        rounded(3., 3., w - 6., h - 6., radius),
    )
}

impl Render for GameScripts {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let view = self.session.scripts();
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let search = self.search(window, cx);
        if search.read(cx).text() != view.query {
            search.update(cx, |input, cx| input.set_text(view.query.clone(), cx));
        }
        if view.scroll_reset != self.seen_reset {
            self.seen_reset = view.scroll_reset;
            self.detail_scroll
                .scroll_to(gpui::px(0.), false, window, cx);
        }
        let total = f32::from(self.root.get().size.width) / zoom;
        // shortcut: до первого кадра ширина раздела не известна — 2/3 окна, кадром позже по мерке.
        let total = if total > 0. {
            total
        } else {
            crate::scale::viewport(window).0 * 0.66
        };
        let list_w = list_width(total, self.list_w.map(|w| w as f32));
        let list = self.list_el(&view, &theme, window, cx);
        let head = self.head(&view, &theme, window, cx);
        let video = self.video(&view, &theme, window, cx);
        let keys = self.keys(&view, &theme, window, cx);
        let description = self.description(&view, &theme, window, cx);
        let splitter = pane_splitter(
            "gsc-split",
            Orientation::Vertical,
            f64::from(LIST_MIN),
            self.list_w,
        )
        .panes(self.list.clone(), self.detail.clone())
        .rest(f64::from(DETAIL_MIN))
        .label(self.t("#L_SPLIT_LIST", &[]))
        .hint(self.t("#L_SPLIT_HINT", &[]))
        .on_input(cx.listener(|this, value: &Option<f64>, _, cx| {
            this.list_w = *value;
            cx.notify();
        }))
        .on_save(cx.listener(|_, value: &Option<f64>, _, cx| save_pane(PANE_KEY, *value, cx)));
        let detail = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .border_1()
            .border_color(theme.color("l-border"))
            .rounded(theme.rem("l-radius-sm"))
            .bg(theme.color("l-modal-bar"))
            .child(anchor_probe(self.detail.clone()))
            .child(
                ScrollArea::new("gsc-detail", &self.detail_scroll)
                    .contain()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .pt(u(14.))
                            .px(u(16.))
                            .pb(u(16.))
                            .child(head)
                            .child(video)
                            .child(keys)
                            .child(description)
                            .child(
                                div()
                                    .mt(u(12.))
                                    .text_color(theme.color("l-text-faint"))
                                    .text_px(11.5, 20.)
                                    .child(view.source_note.clone()),
                            ),
                    ),
            );
        let root = div()
            .relative()
            .flex()
            .size_full()
            .min_h_0()
            .text_color(theme.color("l-text"))
            .font_family(RUBIK)
            .child(measure(self.root.clone()))
            .child(
                div()
                    .flex_none()
                    .w(u(list_w))
                    .h_full()
                    .min_h_0()
                    .child(list),
            )
            .child(splitter)
            .child(detail);
        #[cfg(target_os = "linux")]
        let root = linux_capture(root, self.modifiers.clone());
        root
    }
}

/// Ширина списка, `clamp(220px, var(--gsc-list, 28%), calc(100% - 412px))`: `wanted` — с
/// разделителя, `None` — 28 % раздела `total`.
fn list_width(total: f32, wanted: Option<f32>) -> f32 {
    wanted
        .unwrap_or(total * 0.28)
        .min(total - DETAIL_MIN - 12.)
        .max(LIST_MIN)
}

#[cfg(test)]
mod tests {
    /// Ширина списка как у CSS 0.4.9: `clamp(220px, var(--gsc-list, 28%), calc(100% - 412px))`.
    #[test]
    fn list_width_is_clamped_like_css() {
        use super::list_width;
        assert_eq!(list_width(1000., None), 280.);
        assert_eq!(list_width(600., None), 220.);
        assert_eq!(list_width(1000., Some(700.)), 588.);
        assert_eq!(list_width(500., Some(300.)), 220.);
    }

    /// Кольцо пульса: снаружи — кнопка + 3 px со скруглением `r + 3`, внутри — сама кнопка (`r`).
    #[test]
    fn pulse_ring_is_3px_around_the_button() {
        let svg = super::ring_svg(106., 40., 6.);
        assert!(svg.contains(r#"width="106" height="40" viewBox="0 0 106 40""#));
        assert!(svg.contains("M9 0H97A9 9 0 0 1 106 9V31A9 9 0 0 1 97 40H9A9 9 0 0 1 0 31V9"));
        assert!(svg.contains("M9 3H97A6 6 0 0 1 103 9V31A6 6 0 0 1 97 37H9A6 6 0 0 1 3 31V9"));
        assert!(svg.contains(r#"fill-rule="evenodd""#));
    }
}
