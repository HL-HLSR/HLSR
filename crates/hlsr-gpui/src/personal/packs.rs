//! Моды игры (`CustomPacks.vue` 0.4.9): плитки сеткой (колонок столько, сколько влезает по
//! 320 px, ряды не ниже 380 и тянутся на всю высоту панели), сверху крупное превью — оно же
//! переключатель — с кадрами «до / после» (рядом в сцене шире 2:1, кадр обрезан с фокусом
//! `object-position`), ниже название, автор, переключатель, описание, тип, почему недоступен,
//! ошибка и список файлов (`<details>`); у звуков — прослушать оригинал или мод (cpal,
//! [`crate::sound`]). Логика — `CustomPacks` (`hlsr-ui`); список с диска уходит редактору
//! событием [`PacksEvent`] (предпросмотр знает о HUD-моде).

use super::editor::Probe;
use crate::assets::Assets;
use crate::env::GpuiEnv;
use crate::icons::{fa_glyph, icon};
use crate::image::{Pictures, Raster, device_size};
use crate::kit::button::{
    Look as BtnLook, button, child_id, focus_handle, focus_visible, outline, white,
};
use crate::kit::{gx, toggle};
use crate::layer::anchor_probe;
use crate::motion::{Push, Timing, hover, mix, press, registry};
use crate::paint::shadow;
use crate::scale::{u, zoom};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::sound::{Playback, play_wav};
use crate::text::{MONO, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AssetSource, ClickEvent, Context, Div, ElementId, EventEmitter, FontWeight,
    IntoElement, ObjectFit, ParentElement, Render, RenderImage, Role, Styled, Toggled, Window, div,
    hsla, img, prelude::*, rgb,
};
use hlsr_core::language::Language;
use hlsr_ui::Rgba8Image;
use hlsr_ui::controllers::custom_packs::{CustomPacks, Pack, Shot, Which, kind_icon, shots};
use hlsr_ui::images::{self, Fit, ImageLoader, ImageRequest, ImageSource, NoRemote};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Once};

/// Область загрузчика картинок раздела.
const SCOPE: &str = "custom-packs";
/// Папка снимков в ассетах.
const SHOTS_DIR: &str = "launcher/custom-packs/";
/// `.cpk__list`: поля 14; сетка: колонка от 320, промежуток 14.
const PAD: f32 = 14.;
const GAP: f32 = 14.;
const COLUMN: f32 = 320.;

/// `$emit("packs", list)`: список модов с диска.
pub struct PacksEvent(pub Value);

impl EventEmitter<PacksEvent> for CustomPacksView {}

/// Снимок, обрезанный под свою рамку: для какого размера и картинка.
#[derive(Default)]
struct ShotRaster {
    size: (u32, u32),
    raster: Raster,
}

pub struct CustomPacksView {
    packs: CustomPacks,
    services: Services,
    language: Language,
    title: String,
    loader: ImageLoader,
    scroll: Scroll,
    /// Открытые списки файлов (`<details>`) и их прокрутка.
    open: HashSet<String>,
    files_scroll: HashMap<String, Scroll>,
    /// Рамка списка (ширина контейнера `cpk`, высота панели) и сцен превью по моду.
    list_box: Probe,
    stages: HashMap<String, Probe>,
    shot_boxes: HashMap<&'static str, Probe>,
    rasters: HashMap<&'static str, ShotRaster>,
    /// Снимки, чья загрузка идёт.
    waiting: HashSet<&'static str>,
    /// Сумма размеров рамок, по которым посчитан прошлый кадр.
    measured: f64,
    sound: Option<Playback>,
    /// Переключатель нажат: пока идёт `custom-packs-set`, он показывает нажатое (`input.checked`),
    /// потом — то, что на диске.
    clicked: Option<bool>,
}

/// Снимки модов — файлы сборки `bundle:launcher/custom-packs/*` (один раз на процесс).
fn register_shots() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for name in Assets.list(SHOTS_DIR).unwrap_or_default() {
            if let Ok(Some(std::borrow::Cow::Borrowed(bytes))) = Assets.load(&name) {
                images::register_bundle(&name, bytes);
            }
        }
    });
}

/// `object-fit: cover; object-position: focus`: часть картинки с пропорциями рамки `w × h`.
fn crop_cover(image: &Rgba8Image, (w, h): (u32, u32), (fx, fy): (f32, f32)) -> Rgba8Image {
    let (iw, ih) = (image.width(), image.height());
    let aspect = w as f32 / h.max(1) as f32;
    let cw = ((ih as f32 * aspect).round() as u32).clamp(1, iw);
    let ch = ((cw as f32 / aspect).round() as u32).clamp(1, ih);
    let x = ((iw - cw) as f32 * fx).round() as u32;
    let y = ((ih - ch) as f32 * fy).round() as u32;
    let mut out = Rgba8Image::new(cw, ch);
    let (src, dst) = (image.as_slice(), out.make_mut_slice());
    for row in 0..ch as usize {
        let from = (y as usize + row) * iw as usize + x as usize;
        dst[row * cw as usize..][..cw as usize].copy_from_slice(&src[from..from + cw as usize]);
    }
    if image.is_premultiplied() {
        out.premultiplied()
    } else {
        out
    }
}

impl CustomPacksView {
    /// Раздел открыт (`mounted`): список с диска.
    pub fn new(
        game_id: &str,
        title: String,
        services: Services,
        language: Language,
        cx: &mut Context<Self>,
    ) -> CustomPacksView {
        register_shots();
        let loader = cx.try_global::<Pictures>().map_or_else(
            || ImageLoader::new(Rc::new(NoRemote), &*GpuiEnv::new(cx)),
            Pictures::loader,
        );
        cx.on_release(|this: &mut Self, cx| {
            // `beforeUnmount`: звук смолкает (поток уходит вместе с видом), снимки — из атласа.
            for shot in this.rasters.values_mut() {
                shot.raster.release(cx);
            }
            this.loader.release_scope(SCOPE);
        })
        .detach();
        let this = CustomPacksView {
            packs: CustomPacks::new(game_id),
            services,
            language,
            title,
            loader,
            scroll: Scroll::new(),
            open: HashSet::new(),
            files_scroll: HashMap::new(),
            list_box: Probe::default(),
            stages: HashMap::new(),
            shot_boxes: HashMap::new(),
            rasters: HashMap::new(),
            waiting: HashSet::new(),
            measured: 0.,
            sound: None,
            clicked: None,
        };
        let reply = this
            .services
            .invoke("custom-packs-get", vec![json!(game_id)]);
        cx.spawn(async move |this, cx| {
            let reply = reply.await.map_err(|error| format!("{error:?}"));
            this.update(cx, |this, cx| {
                if let Some(list) = this.packs.loaded(reply) {
                    cx.emit(PacksEvent(list));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        this
    }

    fn t(&self, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(self.language, key, args)
    }

    /// `toggle(pack, enabled)`: файлы мода сразу в игру или из неё.
    fn toggle(&mut self, pack_id: String, enabled: bool, cx: &mut Context<Self>) {
        if !self.packs.begin_toggle(&pack_id) {
            return;
        }
        self.sound = None;
        cx.notify();
        let args = vec![json!(self.packs.game_id), json!(pack_id), json!(enabled)];
        let reply = self.services.invoke("custom-packs-set", args);
        cx.spawn(async move |this, cx| {
            let reply = reply.await.map_err(|error| format!("{error:?}"));
            this.update(cx, |this, cx| {
                if let Some(list) = this.packs.toggled(reply) {
                    cx.emit(PacksEvent(list));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `play(pack, file, which)`: та же кнопка ещё раз — стоп.
    fn play(&mut self, pack: String, file: String, which: Which, cx: &mut Context<Self>) {
        self.sound = None;
        cx.notify();
        let Some(id) = self.packs.play(&pack, &file, which) else {
            return;
        };
        let args = vec![
            json!(self.packs.game_id),
            json!(pack),
            json!(file),
            json!(which.name()),
        ];
        let reply = self.services.invoke("custom-packs-sound", args);
        cx.spawn(async move |this, cx| {
            let reply = reply.await.map_err(|error| format!("{error:?}"));
            this.update(cx, |this, cx| {
                cx.notify();
                let Some(bytes) = this.packs.sound_reply(&id, &pack, reply) else {
                    return;
                };
                let (done, ended) = tokio::sync::oneshot::channel::<()>();
                let Some(playback) = play_wav(&bytes, move || {
                    let _ = done.send(());
                }) else {
                    this.packs.ended(&id);
                    return;
                };
                this.sound = Some(playback);
                cx.spawn(async move |this, cx| {
                    // Звук остановили раньше — отправитель ушёл вместе с потоком.
                    if ended.await.is_ok() {
                        this.update(cx, |this, cx| {
                            if this.packs.playing == id {
                                this.sound = None;
                            }
                            this.packs.ended(&id);
                            cx.notify();
                        })
                        .ok();
                    }
                })
                .detach();
            })
            .ok();
        })
        .detach();
    }

    /// Снимок, обрезанный под рамку прошлого кадра; пока грузится — `None`.
    fn shot_image(
        &mut self,
        shot: &Shot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        let frame = self.shot_boxes.entry(shot.src).or_default().get();
        let zoom = zoom(window);
        let (w, h) = (
            f32::from(frame.size.width) / zoom,
            f32::from(frame.size.height) / zoom,
        );
        if w < 1. || h < 1. {
            return None;
        }
        let size = device_size(window, w, h);
        let entry = self.rasters.entry(shot.src).or_default();
        if entry.size != size {
            let source = ImageSource::Bundle(format!("{SHOTS_DIR}{}", shot.src));
            let request = ImageRequest::new(source, SCOPE).with_target(size.0, size.1, Fit::Cover);
            match self.loader.cached(&request) {
                Some(image) => {
                    entry.raster.set(&crop_cover(&image, size, shot.focus));
                    entry.size = size;
                }
                None if self.waiting.insert(shot.src) => {
                    let src = shot.src;
                    let view = cx.entity().downgrade();
                    let app = cx.to_async();
                    // Из кеша или известная ошибка — колбэк зовётся сразу, внутри `request`.
                    let inside = Rc::new(Cell::new(true));
                    let sync = inside.clone();
                    self.loader.request(request, move |_| {
                        if sync.get() {
                            return;
                        }
                        app.update(|cx| {
                            view.update(cx, |this, cx| {
                                this.waiting.remove(src);
                                cx.notify();
                            })
                            .ok();
                        });
                    });
                    inside.set(false);
                }
                None => {}
            }
        }
        self.rasters.get_mut(shot.src)?.raster.image(window, cx)
    }

    /// Сцена превью: кадры «до / после» или значок типа на подложке акцента.
    fn stage(&mut self, pack: &Pack, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let probe = self.stages.entry(pack.id.clone()).or_default().clone();
        let frame = probe.get();
        let (sw, sh) = (
            f32::from(frame.size.width) / zoom,
            f32::from(frame.size.height) / zoom,
        );
        let list = shots(&pack.id);
        let body = if list.is_empty() {
            let accent = theme.color("accent");
            // Подложка — среднее радиального и линейного градиента 0.4.9 (в GPUI нет радиального).
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(mix(theme.color("l-border"), accent, 0.22))
                .rounded(u(10.))
                .bg(accent.opacity(0.14))
                .text_color(mix(theme.color("l-text"), accent, 0.7))
                .child(fa_glyph(
                    &format!("fas {}", kind_icon(&pack.kind)),
                    (sw.min(sh) * 0.34).clamp(40., 128.),
                ))
                .into_any_element()
        } else {
            // `@container (min-aspect-ratio: 2/1)`: кадры рядом.
            let wide = sh > 0. && sw / sh >= 2.;
            let mut shots_el = div().flex_1().min_w_0().flex().gap(u(6.));
            if !wide {
                shots_el = shots_el.flex_col();
            }
            for shot in list {
                let probe = self.shot_boxes.entry(shot.src).or_default().clone();
                let image = self.shot_image(shot, window, cx);
                let label = shot.label.map(|label| {
                    div()
                        .absolute()
                        .top(u(5.))
                        .left(u(5.))
                        .px(u(6.))
                        .py(u(2.))
                        .rounded(u(5.))
                        .bg(hsla(0., 0., 0., 0.62))
                        .text_color(rgb(0xffffff))
                        .text_px(11.5, 11.5 * 1.3)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.t(label, &[]))
                });
                shots_el = shots_el.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .rounded(u(8.))
                        .bg(hsla(0., 0., 0., 0.3))
                        .child(anchor_probe(probe))
                        .children(image.map(|image| {
                            img(image)
                                .absolute()
                                .inset_0()
                                .size_full()
                                .rounded(u(8.))
                                .object_fit(ObjectFit::Fill)
                        }))
                        .children(label),
                );
            }
            shots_el.into_any_element()
        };
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .flex()
            .child(anchor_probe(probe))
            .child(body)
            .into_any_element()
    }

    /// Превью-переключатель `.cpk-card__preview`.
    fn preview(
        &mut self,
        pack: &Pack,
        row: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let disabled = !pack.available || !self.packs.switching.is_empty();
        let enabled = pack.enabled;
        let title = self.t(&pack.title_key(), &[]);
        let label = self.t("#L_CPK_PREVIEW", &[&title]);
        let stage = self.stage(pack, window, cx);
        let id = ElementId::Name(format!("cpk-preview-{}", pack.id).into());
        let focus = focus_handle(&id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let text = theme.color("l-text");
        let focus_color = theme.color("accent-focus");
        let pack_id = pack.id.clone();
        let click = cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.clicked = None;
            this.toggle(pack_id.clone(), !enabled, cx)
        });
        let check = enabled.then(|| {
            div()
                .absolute()
                .top(u(-7.))
                .right(u(-7.))
                .size(u(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(text)
                .text_color(rgb(0x15181c))
                .shadow(vec![shadow(zoom, 0., 1., 4., 0., hsla(0., 0., 0., 0.45))])
                .child(fa_glyph("fas fa-check", 11.))
        });
        let inner = id.clone();
        let press_id = child_id(&id, "press");
        // `transform 90ms var(--l-ease)`, `:active:not(:disabled) { scale(.99) }`.
        let push = Push::new(0.99, Timing::new(90., registry().tokens.l_ease));
        let target = format!("cpk-preview-{}", pack.id);
        let el = hover(id, Timing::fast(), move |t| {
            let t = if disabled { 0. } else { t };
            let el = div()
                .id(inner)
                .role(Role::Button)
                .button_text()
                .aria_label(label)
                .aria_toggled(if enabled {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .relative()
                .flex()
                .min_h(u(150.))
                .rounded(u(10.))
                .map(|el| {
                    if row {
                        el.flex_none().w(gpui::relative(0.46))
                    } else {
                        el.flex_1()
                    }
                })
                // `0 0 0 2px --l-text` — контуром поверх. Мягкое свечение `0 0 18px` не
                // переносится: тень GPUI ложится и под полупрозрачную плитку.
                .when(enabled, |el| el.child(outline(zoom, 10., 0., 2., text)))
                .when(disabled, |el| el.opacity(0.5))
                .child(stage)
                // `filter: brightness(1.1)` под курсором — светлая дымка поверх.
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(u(10.))
                        .bg(white(0.06 * t)),
                )
                .children(check)
                .when(!disabled, |el| {
                    el.track_focus(&focus).cursor_pointer().on_click(click)
                })
                .when(ring_on, |el| {
                    el.child(outline(zoom, 10., 4., 2., focus_color))
                });
            press(press_id, push, el).live(!disabled)
        });
        crate::snap::target(target, el)
    }

    /// Список файлов `<details class="cpk-files">`.
    fn files(&mut self, pack: &Pack, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = cx.global::<Theme>().clone();
        let open = self.open.contains(&pack.id);
        let (dim, text) = (theme.color("l-text-dim"), theme.color("l-text"));
        let pack_id = pack.id.clone();
        let summary_id = ElementId::Name(format!("cpk-files-{}", pack.id).into());
        let focus = focus_handle(&summary_id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let zoom = zoom(window);
        let focus_color = theme.color("accent-focus");
        let click = cx.listener(move |this, _: &ClickEvent, _, cx| {
            if !this.open.remove(&pack_id) {
                this.open.insert(pack_id.clone());
            }
            cx.notify();
        });
        let summary_text = self.t("#L_CPK_FILES", &[&pack.files.len().to_string()]);
        let inner = summary_id.clone();
        let summary = hover(summary_id, Timing::fast(), move |t| {
            div()
                .id(inner)
                .role(Role::Button)
                .button_text()
                .aria_toggled(if open { Toggled::True } else { Toggled::False })
                .track_focus(&focus)
                .relative()
                .flex()
                .items_center()
                .gap(u(5.))
                .rounded(u(4.))
                .text_px(12.5, 20.)
                .text_color(mix(dim, text, t))
                .cursor_pointer()
                .on_click(click)
                .child(fa_glyph(
                    if open {
                        "fas fa-caret-down"
                    } else {
                        "fas fa-caret-right"
                    },
                    10.,
                ))
                .child(summary_text)
                .when(ring_on, |el| {
                    el.child(outline(zoom, 4., 2., 2., focus_color))
                })
        });
        let mut root =
            div()
                .mt(u(2.))
                .flex()
                .flex_col()
                .child(div().flex().child(crate::snap::target(
                    format!("cpk-files-{}", pack.id),
                    summary,
                )));
        if !open {
            return root;
        }
        let sounds = pack.kind == "sounds" && pack.available;
        let rows: Vec<AnyElement> = pack
            .files
            .iter()
            .map(|file| {
                let mut row = div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .min_h(u(30.))
                    .py(u(3.))
                    .border_t_1()
                    .border_color(white(0.05))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(MONO)
                            .text_px(12., 20.)
                            .text_color(dim)
                            .child(file.clone()),
                    );
                if sounds {
                    for which in Which::BOTH {
                        let playing = self.packs.is_playing(&pack.id, file, which);
                        let label = self.t(which.label_key(), &[]);
                        let (pack_id, file_name) = (pack.id.clone(), file.clone());
                        row = row.child(
                            button(
                                ElementId::Name(
                                    format!("cpk-play-{}-{file}-{}", pack.id, which.name()).into(),
                                ),
                                BtnLook::Gx,
                            )
                            // Высота 26 из `.cpk-files .gx-btn` в 0.4.9 не доходит (на снимке
                            // 30, как у `.gx-btn`), поля 9 — доходят.
                            .px(u(9.))
                            .label(format!("{label}: {file}"))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.play(pack_id.clone(), file_name.clone(), which, cx)
                            }))
                            .child(fa_glyph(
                                if playing {
                                    "fas fa-stop"
                                } else {
                                    "fas fa-play"
                                },
                                11.,
                            ))
                            .child(label),
                        );
                    }
                }
                row.into_any_element()
            })
            .collect();
        let scroll = self
            .files_scroll
            .entry(pack.id.clone())
            .or_default()
            .clone();
        root = root.child(
            ScrollArea::new(
                ElementId::Name(format!("cpk-files-list-{}", pack.id).into()),
                &scroll,
            )
            .bar(Bar::Fields)
            .max_h(u(200.))
            .mt(u(8.))
            .pr(u(4.))
            .child(div().flex().flex_col().children(rows)),
        );
        root
    }

    /// Плитка `.cpk-card`.
    fn card(&mut self, pack: &Pack, row: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = cx.global::<Theme>().clone();
        let text = theme.color("l-text");
        let (dim, faint) = (theme.color("l-text-dim"), theme.color("l-text-faint"));
        let title = self.t(&pack.title_key(), &[]);
        let author = match &pack.author {
            Some(author) => self.t("#L_CPK_AUTHOR", &[author]),
            None => self.t("#L_CPK_AUTHOR_UNKNOWN", &[]),
        };
        let switching = &self.packs.switching;
        let busy = *switching == pack.id;
        let locked = !pack.available || !switching.is_empty();
        let checked = self.clicked.filter(|_| busy).unwrap_or(pack.enabled);
        let mut switch = toggle(format!("cpk-switch-{}", pack.id), checked).label(title.clone());
        if !locked {
            let pack_id = pack.id.clone();
            switch = switch.on_change(cx.listener(move |this, on: &bool, _, cx| {
                this.clicked = Some(*on);
                this.toggle(pack_id.clone(), *on, cx)
            }));
        }
        let switch = div()
            .flex_none()
            .when(!pack.available, |el| el.opacity(0.45))
            .when(busy && pack.available, |el| el.opacity(0.6))
            .child(crate::snap::target(
                format!("cpk-switch-{}", pack.id),
                switch,
            ));
        let error = (!self.packs.error.is_empty() && self.packs.error_pack == pack.id)
            .then(|| error_line(self.t(self.packs.error, &[])));
        let needs = (!pack.available).then(|| {
            div()
                .text_px(12., 12. * 1.4)
                .text_color(faint)
                .child(self.t(self.packs.needs_key(), &[]))
        });
        let preview = self.preview(pack, row, window, cx);
        let files = self.files(pack, window, cx);
        let body = div()
            .flex()
            .flex_col()
            .gap(u(6.))
            .min_w_0()
            .map(|el| if row { el.flex_1() } else { el.flex_none() })
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(u(12.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(u(2.))
                            .min_w_0()
                            .child(
                                div()
                                    .text_px(14.5, 14.5 * 1.3)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(div().text_px(12., 20.).text_color(dim).child(author)),
                    )
                    .child(switch),
            )
            .child(
                div()
                    .text_px(13., 13. * 1.45)
                    .text_color(text)
                    .child(crate::text::pretty(self.t(&pack.desc_key(), &[]))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(7.))
                    .text_px(12., 20.)
                    .text_color(dim)
                    .child(
                        div()
                            .w(u(14.))
                            .flex()
                            .justify_center()
                            .child(fa_glyph(&format!("fas {}", kind_icon(&pack.kind)), 12.)),
                    )
                    .child(self.t(&pack.kind_key(), &[&pack.files.len().to_string()])),
            )
            .children(needs)
            .children(error)
            .child(files);
        let on = pack.enabled;
        div()
            .flex()
            .gap(u(12.))
            .min_w_0()
            .p(u(12.))
            .border_1()
            .border_color(if on {
                text.opacity(0.42)
            } else {
                theme.color("l-border")
            })
            .rounded(theme.rem("l-radius-sm"))
            .bg(white(0.025))
            .map(|el| if row { el.flex_row() } else { el.flex_col() })
            .child(preview)
            .child(body)
    }
}

/// `.cpk__error`: 12,5 px / 1.4, `#ff8a96`.
fn error_line(text: String) -> Div {
    div()
        .text_px(12.5, 12.5 * 1.4)
        .text_color(rgb(0xff8a96))
        .child(text)
}

impl Render for CustomPacksView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let frame = self.list_box.get();
        let (width, height) = (
            f32::from(frame.size.width) / zoom,
            f32::from(frame.size.height) / zoom,
        );
        // Ширина контейнера `cpk` — без полей списка и без полосы, когда список прокручивается.
        let bar = if f32::from(self.scroll.max_top()) > 0. {
            Bar::Fields.thickness()
        } else {
            0.
        };
        let inner = (width - 2. * PAD - bar).max(0.);
        let packs = self.packs.packs.clone().unwrap_or_default();
        let n = packs.len().max(1);
        let cols = (((inner + GAP) / (inner.min(COLUMN) + GAP)).floor() as usize).clamp(1, n);
        let card_w = ((inner - GAP * (cols - 1) as f32) / cols as f32).max(0.);
        let narrow = inner <= 653.;
        let row_cards = narrow && inner >= 460.;
        let centered_last = (654.0..=987.).contains(&inner) && packs.len() % 2 == 1;

        let mut grid_rows = Vec::new();
        for (index, chunk) in packs.chunks(cols).enumerate() {
            let last_row = (index + 1) * cols >= packs.len();
            let mut row = div()
                .flex()
                .gap(u(GAP))
                .flex_grow(1.)
                .flex_shrink_0()
                .min_h(u(if narrow { 220. } else { 380. }));
            if centered_last && last_row && chunk.len() == 1 {
                row = row.justify_center();
            }
            for pack in chunk {
                let card = self.card(pack, row_cards, window, cx);
                row = row.child(card.flex_none().w(u(card_w)));
            }
            grid_rows.push(row);
        }

        let mut content = div()
            .flex()
            .flex_col()
            .gap(u(12.))
            .p(u(PAD))
            .min_h(u(height));
        if self.packs.packs.is_none() {
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(6.))
                    .text_px(13., 20.)
                    .text_color(theme.color("l-text-dim"))
                    .child(icon(
                        "fas fa-spinner fa-spin",
                        13.,
                        theme.color("l-text-dim"),
                    ))
                    .child(self.t("#L_GS_LOADING", &[])),
            );
        }
        if !self.packs.error.is_empty() && self.packs.error_pack.is_empty() {
            content = content.child(error_line(self.t(self.packs.error, &[])));
        }
        content = content.child(
            div()
                .flex()
                .flex_col()
                .gap(u(GAP))
                .flex_grow(1.)
                .flex_shrink_0()
                .children(grid_rows),
        );
        // Раскладка — по рамкам прошлого кадра: изменились — ещё кадр с новыми.
        let measured = std::iter::once(&self.list_box)
            .chain(self.stages.values())
            .chain(self.shot_boxes.values())
            .map(|probe| {
                let size = probe.get().size;
                f64::from(f32::from(size.width)) * 7919. + f64::from(f32::from(size.height))
            })
            .sum::<f64>();
        if measured != self.measured {
            self.measured = measured;
            crate::motion::monitor_notify(window, cx.entity_id());
        }

        let head = gx::pane_head(self.title.clone()).note(self.t(self.packs.note_key(), &[]));
        let list_box = self.list_box.clone();
        gx::pane(&theme)
            .bg(gpui::transparent_black())
            .size_full()
            .child(head)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(anchor_probe(list_box))
                    .child(
                        ScrollArea::new("cpk-list", &self.scroll)
                            .bar(Bar::Fields)
                            .contain()
                            .size_full()
                            .child(content),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crop_keeps_the_aspect_and_the_focus() {
        // 4 × 2, столбцы по номеру: под рамку 1 × 1 с фокусом справа — правый квадрат 2 × 2.
        let mut image = Rgba8Image::new(4, 2);
        for (i, p) in image.make_mut_slice().iter_mut().enumerate() {
            p.r = (i % 4) as u8;
        }
        let out = crop_cover(&image, (10, 10), (1., 0.5));
        assert_eq!((out.width(), out.height()), (2, 2));
        let cols: Vec<u8> = out.as_slice().iter().map(|p| p.r).collect();
        assert_eq!(cols, vec![2, 3, 2, 3]);
        let out = crop_cover(&image, (8, 2), (0., 0.));
        assert_eq!((out.width(), out.height()), (4, 1));
    }
}
