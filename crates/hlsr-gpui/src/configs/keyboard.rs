//! Раздел «Клавиатура» (`GameKeyboard.vue` 0.4.9): над разделителем — панель схемы (легенда,
//! клавиши вне схемы, «Вся клавиатура / Только основная», схема клавиш с биндами), под ним —
//! сведения о клавише и назначение (режим рана, своя команда, поиск, категории, заготовки).
//! Высота схемы — [`pane_splitter`] `keyboard:<игра>` (мин. 162, списку — 190); схема
//! вписывается в коробку по ширине и высоте, клавиши не ниже 28 px. Логика —
//! `hlsr_ui::controllers::game_keyboard` через сессию настроек ([`Session::keyboard`] и `kb_*`).
//!
//! Здесь же общее для «Скриптов»: кнопка `.kb-btn`/`.gsc-btn` ([`ring_btn`]), поиск с лупой
//! ([`search_box`]), рамка прошлого кадра ([`measure`]), запись размера разделителя
//! ([`save_pane`]).

use super::sections::SectionHost;
use crate::icons::fa_glyph;
use crate::kit::button::{focus_handle, focus_visible, outline};
use crate::kit::input::FieldStyle;
use crate::kit::{FieldEvent, Orientation, TextInput, pane_splitter};
use crate::layer::TipExt;
use crate::motion::{Timing, hover, mix};
use crate::paint::{ring, shadow};
use crate::scale::{u, viewport, zoom, zpx};
use crate::scroll::{Scroll, ScrollArea};
use crate::text::{MONO, RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AbsoluteLength, AnyElement, App, Bounds, Context, ElementId, Entity, Focusable, FontWeight,
    Hsla, IntoElement, ParentElement, PathBuilder, Pixels, Render, Role, SharedString, Styled,
    StyledText, Subscription, Window, canvas, div, hsla, point, prelude::*, relative, rgb,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::game_keyboard::{CapView, KeyboardView, Shown};
use hlsr_ui::controllers::game_settings::Session;
use hlsr_ui::controllers::pane_size;
use serde_json::Map;
use std::cell::Cell;
use std::rc::Rc;

pub(super) type Probe = Rc<Cell<Bounds<Pixels>>>;

/// Мин. высота коробки схемы (28 px на клавишу) и сколько остаётся списку под разделителем.
const BOX_MIN: f32 = 162.;
const BOTTOM_MIN: f32 = 190.;
/// Высота схемы в u для `aspect-ratio: cols / 5.8` (клавиши стоят на 6,25 u).
const ASPECT_H: f32 = 5.8;

const RED: u32 = 0xff6b6b;
const PURPLE: u32 = 0xb48cff;
const RUNTIME: u32 = 0x8fe3ff;
const ERROR_TEXT: u32 = 0xff8a96;
/// `maxlength` своей команды (`game_binds::MAX_COMMAND`).
const MAX_COMMAND: usize = 255;

pub(super) fn white(alpha: f32) -> Hsla {
    hsla(0., 0., 1., alpha)
}

/// `--kb-c` тона (`.tone-*`).
fn tone_color(tone: &str) -> Option<Hsla> {
    let hex = match tone {
        "blue" => 0x4aa3ff,
        "orange" => 0xff9f43,
        "green" => 0x5fd068,
        "yellow" => 0xf2cf4a,
        "purple" => PURPLE,
        "pink" => 0xff7ac6,
        "red" => RED,
        "grey" => 0x9aa4b5,
        "other" => 0x3fd0b0,
        "alias" => 0xd8b98a,
        "runtime" => RUNTIME,
        _ => return None,
    };
    Some(rgb(hex).into())
}

/// Рамка элемента прошлого кадра (`ResizeObserver`): сменился размер — вид перерисуется кадром
/// позже. Сдвиг без смены размера (переход раздела едет каждый кадр) кадра не просит: раскладка
/// берёт только размер.
pub(super) fn measure(cell: Probe) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            let resized = cell.get().size != bounds.size;
            cell.set(bounds);
            if resized {
                crate::motion::monitor_frame(window);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Рамка шириной `width` CSS px (у GPUI `border_N` — в px окна, без зума).
pub(super) fn border_u<E: Styled>(mut el: E, width: f32) -> E {
    let w: AbsoluteLength = u(width).into();
    let edges = &mut el.style().border_widths;
    edges.top = Some(w);
    edges.right = Some(w);
    edges.bottom = Some(w);
    edges.left = Some(w);
    el
}

/// Ромб `width × width`, повёрнутый на 45° (`transform: rotate(45deg)`).
fn diamond(width: f32, color: Hsla) -> impl IntoElement {
    let half = width * std::f32::consts::SQRT_2 / 2.;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let zoom = zoom(window);
            let c = bounds.center();
            let r = zpx(zoom, half);
            let mut path = PathBuilder::fill();
            path.move_to(point(c.x, c.y - r));
            path.line_to(point(c.x + r, c.y));
            path.line_to(point(c.x, c.y + r));
            path.line_to(point(c.x - r, c.y));
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .flex_none()
    .size(u(width))
}

/// Вид кнопки `.kb-btn` / `.gsc-btn`.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum BtnTone {
    /// Кольцо `--l-border-strong`, заливка .05 → .1.
    Plain,
    /// Нажата (`.is-listening`, `.is-original`): `--accent-soft`, кольцо `--accent`.
    On,
    /// `.is-primary`: заливка `--accent`, текст `--accent-contrast-text`.
    Primary,
    /// `.gsc-translate.is-original`: `--l-accent-soft`, кольцо и значок `--accent`.
    Accent,
}

/// Кнопка `.kb-btn` (мин. 28, поля 3/10) или `.gsc-btn` (30, 4/10): 12 px / 600, значок 11 px,
/// между ними 6; `width` — квадратная кнопка-значок без полей.
pub(super) struct RingBtn {
    pub id: ElementId,
    pub min_h: f32,
    pub pad: (f32, f32),
    pub width: Option<f32>,
    pub tone: BtnTone,
    pub disabled: bool,
    pub title: Option<SharedString>,
    pub children: Vec<AnyElement>,
}

impl RingBtn {
    pub fn new(id: impl Into<ElementId>, min_h: f32, pad: (f32, f32)) -> RingBtn {
        RingBtn {
            id: id.into(),
            min_h,
            pad,
            width: None,
            tone: BtnTone::Plain,
            disabled: false,
            title: None,
            children: Vec::new(),
        }
    }

    pub fn kb(id: impl Into<ElementId>) -> RingBtn {
        RingBtn::new(id, 28., (3., 10.))
    }

    pub fn gsc(id: impl Into<ElementId>) -> RingBtn {
        RingBtn::new(id, 30., (4., 10.))
    }

    pub fn child(mut self, child: impl IntoElement) -> RingBtn {
        self.children.push(child.into_any_element());
        self
    }

    pub fn icon(self, classes: &str) -> RingBtn {
        self.child(fa_glyph(classes, 11.))
    }

    pub fn render(
        self,
        window: &mut Window,
        cx: &mut App,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let live = !self.disabled;
        let focus = focus_handle(&self.id, window, cx);
        let ring_on = live && focus_visible(&focus, window);
        let radius = theme.len("l-radius-sm");
        let (accent, soft, text) = (
            theme.color("accent"),
            theme.color("accent-soft"),
            theme.color("l-text"),
        );
        let strong = theme.color("l-border-strong");
        let contrast = theme.color("accent-contrast-text");
        let l_soft = theme.color("l-accent-soft");
        let focus_color = theme.color("accent-focus");
        let RingBtn {
            id,
            min_h,
            pad,
            width,
            tone,
            title,
            children,
            ..
        } = self;
        let inner = id.clone();
        hover(id, Timing::fast(), move |t| {
            let t = if live { t } else { 0. };
            let (bg, fg, shadows) = match tone {
                BtnTone::Plain => (
                    mix(white(0.05), white(0.1), t),
                    text,
                    vec![ring(zoom, 1., strong)],
                ),
                BtnTone::On => (soft, text, vec![ring(zoom, 1., accent)]),
                // `filter: brightness(1.1)` при наведении — светлее на десятую.
                BtnTone::Primary => (mix(accent, white(1.), 0.1 * t), contrast, Vec::new()),
                BtnTone::Accent => (l_soft, accent, vec![ring(zoom, 1., accent)]),
            };
            let base = div()
                .id(inner)
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(6.))
                .min_h(u(min_h))
                .rounded(u(radius))
                .bg(bg)
                .shadow(shadows)
                .text_color(fg)
                .font_family(RUBIK)
                .text_px(12., 20.)
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .role(Role::Button)
                .button_text()
                .map(|el| match width {
                    Some(width) => el.w(u(width)).justify_center(),
                    None => el.py(u(pad.0)).px(u(pad.1)),
                })
                .when_some(title, |el, title| el.tip(title))
                .when(!live, |el| el.opacity(0.45))
                .when(live, |el| {
                    el.track_focus(&focus)
                        .cursor_pointer()
                        .on_click(move |_, window, cx| on_click(window, cx))
                })
                .children(children);
            base.when(ring_on, |el| {
                el.child(outline(zoom, radius, 1., 2., focus_color))
            })
        })
        .into_any_element()
    }
}

/// `.kb-search` / `.gsc-search`: подпись-рамка с лупой, внутри поле без своей рамки; фокус
/// внутри — рамка `--accent`.
pub(super) fn search_box(input: &Entity<TextInput>, window: &Window, cx: &App) -> gpui::Div {
    let theme = cx.global::<Theme>();
    let focused = input.read(cx).focus_handle(cx).is_focused(window);
    div()
        .flex()
        .items_center()
        .gap(u(8.))
        .px(u(10.))
        .border_1()
        .border_color(theme.color(if focused { "accent" } else { "l-border" }))
        .rounded(theme.rem("l-radius-sm"))
        .bg(white(0.05))
        .text_color(theme.color("l-text-faint"))
        .text_px(11., 20.)
        .child(fa_glyph("fas fa-search", 11.))
        .child(div().flex_1().min_w_0().child(input.clone()))
}

/// Поле без рамки и заливки внутри `.kb-search`: 11,5 px, поля 4/0, строка 20 px (`font: inherit`
/// берёт и `line-height` body).
fn bare_style(theme: &Theme) -> FieldStyle {
    let none = hsla(0., 0., 0., 0.);
    FieldStyle {
        height: 2. * 4. + 20.,
        pad_x: 0.,
        pad_y: 4.,
        size: 11.5,
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

/// `.kb-custom__input`: Consolas 11,5 px, поля 5/10, рамка `--l-border` (фокус — `--accent`,
/// `.invalid` — `#ff4d5e`), `:disabled` — .5. Высота — как у кнопки рядом (`align-items: stretch`).
fn custom_style(theme: &Theme) -> FieldStyle {
    FieldStyle {
        height: 28.,
        pad_x: 10.,
        pad_y: 5.,
        family: MONO,
        size: 11.5,
        hover_bg: white(0.05),
        hover_border: theme.color("l-border"),
        glow: None,
        disabled_opacity: 0.5,
        ..FieldStyle::gx(theme)
    }
}

/// Размер разделителя `key` в prefs (`pane`): `None` — забыть.
pub(super) fn save_pane(key: &str, size: Option<f64>, cx: &mut App) {
    if let Some(root) =
        crate::shell::root::ShellContext::get(cx).and_then(|context| context.root.upgrade())
    {
        let prefs = root.read(cx).prefs();
        let mut prefs = prefs.borrow_mut();
        let pane = pane_size::write_size(prefs.get("pane"), key, size);
        let mut changes = Map::new();
        changes.insert("pane".into(), pane);
        prefs.save(&changes);
    }
}

/// Размер разделителя `key` из prefs.
pub(super) fn read_pane(key: &str, cx: &App) -> Option<f64> {
    let root = crate::shell::root::ShellContext::get(cx)?.root.upgrade()?;
    let prefs = root.read(cx).prefs();
    let prefs = prefs.borrow();
    pane_size::read_size(prefs.get("pane"), key)
}

/// Режим рана и «вся клавиатура» в `launcher-prefs.json` (`keyboardRunMode`, `keyboardFull`).
struct ShellPrefs(Rc<std::cell::RefCell<crate::shell::root::Prefs>>);

impl hlsr_ui::controllers::game_keyboard::KeyboardPrefs for ShellPrefs {
    fn run_mode(&self) -> Option<String> {
        self.0
            .borrow()
            .get("keyboardRunMode")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    }

    fn set_run_mode(&self, mode: &str) {
        let mut changes = Map::new();
        changes.insert("keyboardRunMode".into(), mode.into());
        self.0.borrow_mut().save(&changes);
    }

    fn keyboard_full(&self) -> bool {
        self.0.borrow().get("keyboardFull") == Some(&serde_json::Value::Bool(true))
    }

    fn set_keyboard_full(&self, full: bool) {
        let mut changes = Map::new();
        changes.insert("keyboardFull".into(), full.into());
        self.0.borrow_mut().save(&changes);
    }
}

/// Поля ввода раздела: создаются при первом показе (нужно окно).
struct Fields {
    custom: Entity<TextInput>,
    search: Entity<TextInput>,
    _subs: [Subscription; 2],
}

/// Раздел «Клавиатура» (`GameKeyboard.vue` 0.4.9).
pub struct GameKeyboard {
    session: Rc<Session>,
    language: Language,
    fields: Option<Fields>,
    scroll: Scroll,
    info_scroll: Scroll,
    list_scroll: Scroll,
    cats: Scroll,
    /// Высота схемы с разделителя (`boardHeight`); `None` — по умолчанию.
    board_h: Option<f64>,
    /// Игра, чей размер разделителя прочитан (`storageKey` меняется вместе с игрой).
    pane_game: Option<String>,
    board_box: Probe,
    bottom: Probe,
    /// Поле своей команды на прошлом кадре: ошибка и доступность.
    custom_state: (bool, bool),
}

impl GameKeyboard {
    pub fn new(host: &SectionHost, cx: &mut Context<GameKeyboard>) -> Self {
        if let Some(root) =
            crate::shell::root::ShellContext::get(cx).and_then(|context| context.root.upgrade())
        {
            let prefs = root.read(cx).prefs();
            host.session.set_keyboard_prefs(Rc::new(ShellPrefs(prefs)));
        }
        GameKeyboard {
            session: host.session.clone(),
            language: host.language,
            fields: None,
            scroll: Scroll::new(),
            info_scroll: Scroll::new(),
            list_scroll: Scroll::new(),
            cats: Scroll::horizontal(),
            board_h: None,
            pane_game: None,
            board_box: Rc::default(),
            bottom: Rc::default(),
            custom_state: (false, true),
        }
    }

    fn t(&self, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(self.language, key, args)
    }

    fn fields(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<TextInput>, Entity<TextInput>) {
        if self.fields.is_none() {
            let placeholder = self.t("#L_KB_CUSTOM_PLACEHOLDER", &[]);
            let search_text = self.t("#L_KB_SEARCH", &[]);
            let custom = cx.new(|cx| {
                TextInput::new(window, cx)
                    .style(custom_style)
                    .max_length(MAX_COMMAND)
                    .placeholder(placeholder)
            });
            let search = cx.new(|cx| {
                TextInput::new(window, cx)
                    .style(bare_style)
                    .placeholder(search_text)
            });
            let subs = [
                cx.subscribe(&custom, |this, input, event: &FieldEvent, cx| match event {
                    FieldEvent::Input => {
                        let text = input.read(cx).text().to_owned();
                        this.session.kb_set_custom(&text);
                    }
                    FieldEvent::Enter => this.session.kb_assign_custom(),
                    _ => {}
                }),
                cx.subscribe(&search, |this, input, event: &FieldEvent, cx| {
                    if matches!(event, FieldEvent::Input | FieldEvent::Cleared) {
                        let text = input.read(cx).text().to_owned();
                        this.session.kb_set_query(&text);
                    }
                }),
            ];
            custom.update(cx, |input, cx| input.set_disabled(true, cx));
            self.fields = Some(Fields {
                custom,
                search,
                _subs: subs,
            });
        }
        let fields = self.fields.as_ref().expect("поля созданы выше");
        (fields.custom.clone(), fields.search.clone())
    }

    /// Поля следуют за состоянием клавиатуры: текст (выбор клавиши и «Применить» очищают),
    /// ошибка рамкой, без выбранной клавиши — недоступно.
    fn sync_fields(&mut self, view: &KeyboardView, cx: &mut Context<Self>) {
        let Some(fields) = &self.fields else {
            return;
        };
        let invalid = !view.custom_error.is_empty();
        let disabled = view.selected.is_none();
        let (was_invalid, was_disabled) = self.custom_state;
        fields.custom.update(cx, |input, cx| {
            if input.text() != view.custom {
                input.set_text(view.custom.clone(), cx);
            }
            if invalid != was_invalid {
                input.set_invalid(invalid, cx);
            }
            if disabled != was_disabled {
                input.set_disabled(disabled, cx);
            }
        });
        fields.search.update(cx, |input, cx| {
            if input.text() != view.query {
                input.set_text(view.query.clone(), cx);
            }
        });
        self.custom_state = (invalid, disabled);
    }

    /// Размер разделителя этой игры (`watch storageKey` PaneSplitter).
    fn sync_pane(&mut self, cx: &mut Context<Self>) {
        let game = self.session.binds_game_id();
        if game != self.pane_game {
            self.board_h = game
                .as_deref()
                .and_then(|id| read_pane(&format!("keyboard:{id}"), cx));
            self.pane_game = game;
        }
    }
}

/// Размер схемы в CSS px при ширине коробки `width` и её высоте `height` (`.kb-board`:
/// `min(100cqw, 100cqh × cols / 5.8)`, `aspect-ratio: cols / 5.8`).
fn board_size(cols: f32, width: f32, height: f32) -> (f32, f32) {
    let board_w = width.min(height * cols / ASPECT_H).max(0.);
    (board_w, board_w * ASPECT_H / cols)
}

/// Высота коробки схемы (`.kb-board-box`): с разделителя или по умолчанию — не выше, чем
/// влезает по ширине, и не ниже 162.
fn box_height(cols: f32, width: f32, viewport_h: f32, value: Option<f64>) -> f32 {
    // min(cols × 37, max(cols × 28, (100vh − 530) × cols / 5.8)) × 5.8 / cols.
    let default = (viewport_h - 530.).clamp(28. * ASPECT_H, 37. * ASPECT_H);
    let wanted = value.map_or(default, |v| v as f32);
    wanted.min(width * ASPECT_H / cols).max(BOX_MIN)
}

/// Схема на экране: размер, ступени `@container` и кегли подписей.
#[derive(Clone, Copy)]
struct Geo {
    w: f32,
    h: f32,
    /// `@container (max-width: 820px)`: поля клавиши 1/3/5.
    narrow: bool,
    /// `@container (max-height: 186px)`: рамка 1, поля сбоку 2, бинда нет.
    short: bool,
    cap: f32,
    bind: f32,
}

impl Geo {
    fn new(w: f32, h: f32, cols: f32) -> Geo {
        let unit = w / cols;
        Geo {
            w,
            h,
            narrow: w <= 820.,
            short: h <= 186.,
            cap: (unit * 0.25).clamp(9., 16.),
            bind: (unit * 0.21).clamp(7.5, 13.),
        }
    }

    fn border(&self) -> f32 {
        if self.short { 1. } else { 2. }
    }

    /// Поля клавиши: (верх, бок, низ).
    fn pad(&self) -> (f32, f32, f32) {
        let (top, bottom) = if self.narrow { (1., 5.) } else { (2., 6.) };
        let side = if self.short {
            2.
        } else if self.narrow {
            3.
        } else {
            4.
        };
        (top, side, bottom)
    }
}

/// `fitCaps`: подпись шире клавиши мельчает ровно до её ширины (шаг 0,1 px).
fn fit_cap(text: &str, size: f32, room: f32, window: &Window) -> f32 {
    if text.chars().count() < 2 || room <= 0. {
        return size;
    }
    let zoom = zoom(window);
    let mut font = gpui::font(RUBIK);
    font.weight = FontWeight::SEMIBOLD;
    let run = gpui::TextRun {
        len: text.len(),
        font,
        color: gpui::black(),
        ..Default::default()
    };
    let line = window
        .text_system()
        .layout_line(text, zpx(zoom, size), &[run], None);
    let width = f32::from(line.width) / zoom;
    if width <= room {
        size
    } else {
        (size * (room / width) * 10.).floor() / 10.
    }
}

/// Наведение на плашку (`mouseenter`/`mouseleave`).
type OnHover = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// Плашка `.kb-chip`: клавиша вне схемы или заготовка.
struct Chip {
    id: ElementId,
    active: bool,
    disabled: bool,
    pad_x: f32,
    title: Option<String>,
    children: Vec<AnyElement>,
}

impl Chip {
    fn render(
        self,
        window: &mut Window,
        cx: &mut App,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
        on_hover: Option<OnHover>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let live = !self.disabled;
        let focus = focus_handle(&self.id, window, cx);
        let ring_on = live && focus_visible(&focus, window);
        let Chip {
            id,
            active,
            pad_x,
            title,
            children,
            ..
        } = self;
        let inner = id.clone();
        let (border, accent, soft) = (
            theme.color("l-border"),
            theme.color("accent"),
            theme.color("accent-soft"),
        );
        let (text, focus_color) = (theme.color("l-text"), theme.color("accent-focus"));
        hover(id, Timing::fast(), move |t| {
            let t = if live { t } else { 0. };
            // `.active` ниже `:hover:not(:disabled)` в CSS: заливка акцента побеждает.
            let (bg, edge) = if active {
                (soft, accent)
            } else {
                (mix(white(0.035), white(0.08), t), border)
            };
            div()
                .id(inner)
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(4.))
                .px(u(pad_x))
                .py(u(2.))
                .border_1()
                .border_color(edge)
                .rounded_full()
                .bg(bg)
                .text_color(text)
                .text_px(11., 11. * 1.3)
                .role(Role::Button)
                .button_text()
                .when_some(title, |el, title| el.tip(title))
                .when_some(on_hover, |el, on_hover| {
                    el.on_hover(move |hovered, window, cx| on_hover(*hovered, window, cx))
                })
                .when(live, |el| {
                    el.track_focus(&focus)
                        .cursor_pointer()
                        .on_click(move |_, window, cx| on_click(window, cx))
                })
                .children(children)
                .when(ring_on, |el| {
                    el.child(outline(zoom, 999., 1., 2., focus_color))
                })
        })
        .into_any_element()
    }
}

/// Квадрат цвета 7 × 7 (`::before` категорий и групп).
fn swatch(color: Hsla, size: f32, radius: f32) -> gpui::Div {
    div().flex_none().size(u(size)).rounded(u(radius)).bg(color)
}

impl GameKeyboard {
    /// Клавиша схемы `.kb-key`: рамка 2 px (пунктир у `.is-runtime`), под ней заливка
    /// `padding-box`, подпись, бинд, полоса цвета, точка лаунчера, ромб скрипта.
    fn cap(
        &self,
        cap: &CapView,
        geo: Geo,
        theme: &Theme,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let zoom = zoom(window);
        let (x, y) = ((cap.x as f32) * geo.w, (cap.y as f32) * geo.h);
        let (w, h) = ((cap.w as f32) * geo.w, (cap.h as f32) * geo.h);
        let bw = geo.border();
        let (top, side, bottom) = geo.pad();
        let label_size = fit_cap(&cap.text, geo.cap, w - 2. * bw - 2. * side, window);
        // По месту клавиши (`:key="cap.id"`): смена схемы не переносит наведение и фокус.
        let id = ElementId::Name(format!("kb-key-{}", cap.id).into());
        let live = cap.key.is_some();
        let focus = live.then(|| focus_handle(&id, window, cx));
        let ring_on = focus.as_ref().is_some_and(|f| focus_visible(f, window));
        let c = cap.tone.as_deref().and_then(tone_color);
        let runtime: Hsla = rgb(RUNTIME).into();
        let accent = theme.color("accent");
        let (text, dim) = (theme.color("l-text"), theme.color("l-text-dim"));
        let soft = theme.color("accent-soft");
        let focus_color = theme.color("accent-focus");
        let (bound, selected, launcher, script) =
            (cap.bound, cap.selected, cap.launcher, cap.script);
        let dashed = cap.runtime;
        let runtime_idle = cap.runtime && !cap.bound;
        let key = cap.key.map(str::to_owned);
        let select = key.clone();
        let (click, hovered) = (self.session.clone(), self.session.clone());
        let label = cap.text.clone();
        let bind = (!geo.short && !cap.bind.is_empty()).then(|| cap.bind.clone());
        let aria = cap.aria.clone();
        let dashes = ((w - 2. * bw - 8.) / 7.).ceil().max(0.) as usize;
        let inner = id.clone();
        let el = hover(id, Timing::fast(), move |t| {
            let t = if live { t } else { 0. };
            let base = if selected {
                soft
            } else if bound {
                mix(white(0.04), c.unwrap_or(white(0.)), 0.14)
            } else {
                white(0.045)
            };
            // `:hover:not(:disabled)` сильнее `.is-bound` и `.is-selected`, но не
            // `.is-runtime:not(.is-bound)` (то правило ниже в CSS).
            let bg = if runtime_idle {
                mix(white(0.03), runtime, 0.08)
            } else {
                mix(base, white(0.09), t)
            };
            let fg = if bound || selected || runtime_idle {
                text
            } else {
                mix(dim, text, t)
            };
            let shadows = if selected {
                vec![ring(zoom, 2., accent)]
            } else {
                let mut down = shadow(zoom, 0., -2., 0., 0., hsla(0., 0., 0., 0.35));
                down.inset = true;
                vec![down, ring(zoom, 1., white(0.06))]
            };
            // `::after`: полоса цвета 3 px; у пустой «по нажатию» — пунктир 4/3.
            let bar = div()
                .absolute()
                .left(u(4.))
                .right(u(4.))
                .bottom(u(2.))
                .h(u(3.))
                .opacity(0.85);
            let bar =
                if runtime_idle {
                    Some(bar.flex().gap(u(3.)).overflow_hidden().children(
                        (0..dashes).map(|_| div().flex_none().w(u(4.)).h_full().bg(runtime)),
                    ))
                } else {
                    c.map(|c| bar.rounded(u(2.)).bg(c))
                };
            let body = div()
                .size_full()
                .relative()
                .flex()
                .flex_col()
                .items_start()
                .justify_between()
                .pt(u(top))
                .px(u(side))
                .pb(u(bottom))
                .rounded(u((6. - bw).max(0.)))
                .bg(bg)
                .shadow(shadows)
                .overflow_hidden()
                .text_color(fg)
                // .kb-key — `<button>`; `fit_cap` меряет подпись без разрядки.
                .button_text()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_px(label_size, label_size)
                        .whitespace_nowrap()
                        .child(label),
                )
                .when_some(bind, |el, bind| {
                    el.child(
                        div()
                            .w_full()
                            .text_color(text)
                            .text_px(geo.bind, geo.bind * 1.1)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(bind),
                    )
                })
                .when(script, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(u(5.))
                            .right(u(14.))
                            .child(diamond(6., rgb(RED).into())),
                    )
                })
                .when(launcher, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(u(4.))
                            .right(u(4.))
                            .size(u(6.))
                            .rounded_full()
                            .bg(accent),
                    )
                })
                .children(bar);
            border_u(
                div()
                    .id(inner)
                    .absolute()
                    .left(u(x))
                    .top(u(y))
                    .w(u(w))
                    .h(u(h))
                    .rounded(u(6.)),
                bw,
            )
            .role(Role::Button)
            .button_text()
            .aria_label(aria)
            .when(dashed, |el| {
                el.border_dashed().border_color(runtime.opacity(0.55))
            })
            .child(body)
            .on_hover(move |on, _, _| {
                if *on {
                    hovered.kb_hover(key.as_deref());
                }
            })
            .when(!live, |el| el.opacity(0.35))
            .when_some(focus.zip(select), |el, (focus, select)| {
                el.track_focus(&focus)
                    .cursor_pointer()
                    .on_click(move |_, _, _| click.kb_select(&select))
            })
            .when(ring_on, |el| {
                el.child(outline(zoom, 6. - bw, bw - 2., 2., focus_color))
            })
        });
        match cap.key {
            Some(key) => crate::snap::target(format!("kb-key-{key}"), el),
            None => el.into_any_element(),
        }
    }
}
impl GameKeyboard {
    /// `.kb-legend`: цвета, точка лаунчера, ромб скрипта, «не установлена», клавиши вне схемы,
    /// «Вся клавиатура / Только основная».
    fn legend(
        &self,
        view: &KeyboardView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item = || div().flex().items_center().gap(u(6.));
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(u(14.))
            .gap_y(u(4.))
            .min_h(u(20.))
            .mb(u(6.))
            .text_color(theme.color("l-text-dim"))
            .text_px(11., 20.)
            .children(view.legend.iter().map(|entry| {
                item()
                    .child(swatch(tone_color(entry.tone).unwrap_or(white(0.)), 10., 3.))
                    .child(entry.title.clone())
            }))
            .child(
                item()
                    .child(
                        div()
                            .flex_none()
                            .size(u(6.))
                            .rounded_full()
                            .bg(theme.color("accent"))
                            .shadow(vec![shadow(zoom(window), 0., 0., 0., 2., white(0.08))]),
                    )
                    .child(self.t("#L_KB_LEGEND_LAUNCHER", &[])),
            )
            .when(view.scriptless, |el| {
                el.child(
                    item()
                        .child(diamond(8., rgb(RED).into()))
                        .child(self.t("#L_KB_LEGEND_SCRIPT", &[])),
                )
            })
            .when(view.not_installed, |el| {
                el.child(
                    div()
                        .text_color(theme.color("l-text-faint"))
                        .child(self.t("#L_KB_NOT_INSTALLED", &[])),
                )
            })
            .child(div().flex_1());
        if !view.off_board.is_empty() {
            row = row.child(
                div()
                    .text_color(theme.color("l-text-faint"))
                    .child(self.t("#L_KB_OFF_BOARD", &[])),
            );
            for entry in &view.off_board {
                let key = entry.item.key.clone();
                let (select, hovered) = (self.session.clone(), self.session.clone());
                let on_hover: OnHover = {
                    let key = key.clone();
                    Rc::new(move |on, _, _| {
                        hovered.kb_hover(on.then_some(key.as_str()));
                    })
                };
                let chip = Chip {
                    id: ElementId::Name(format!("kb-off-{key}").into()),
                    active: entry.active,
                    disabled: false,
                    pad_x: 9.,
                    title: None,
                    children: vec![
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(entry.item.name.clone())
                            .into_any_element(),
                        div().child(entry.item.label.clone()).into_any_element(),
                    ],
                };
                row = row.child(chip.render(
                    window,
                    cx,
                    move |_, _| select.kb_select(&key),
                    Some(on_hover),
                ));
            }
        }
        let session = self.session.clone();
        let full = view.full;
        let toggle = RingBtn {
            min_h: 24.,
            pad: (2., 10.),
            ..RingBtn::kb("kb-full")
        }
        .icon("fas fa-keyboard")
        .child(self.t(if full { "#L_KB_MAIN" } else { "#L_KB_FULL" }, &[]))
        .render(window, cx, move |_, _| session.kb_set_full(!full));
        row.child(crate::snap::target("kb-full", toggle))
            .into_any_element()
    }

    /// `.kb-info`: что на клавише под указателем или выбранной.
    fn info(
        &self,
        view: &KeyboardView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let dim = theme.color("l-text-dim");
        let note = |text: String| {
            div()
                .text_color(dim)
                .text_px(12., 12. * 1.45)
                .child(text)
                .into_any_element()
        };
        let warn = |text: String| {
            div()
                .text_color(rgb(ERROR_TEXT))
                .text_px(12., 12. * 1.45)
                .child(text)
                .into_any_element()
        };
        let mut column = div()
            .flex()
            .flex_col()
            .gap(u(6.))
            .px(u(12.))
            .py(u(10.))
            .min_h(relative(1.));
        let Some(shown) = &view.shown else {
            return column
                .justify_center()
                .child(
                    div()
                        .text_center()
                        .text_color(theme.color("l-text-faint"))
                        .text_px(12., 12. * 1.4)
                        .child(self.t("#L_KB_PICK_KEY", &[])),
                )
                .into_any_element();
        };
        column = column.child(self.info_head(shown, view.is_selected_shown, theme, window, cx));
        let c = tone_color(&shown.tone).unwrap_or(white(0.15));
        let label = if shown.label.is_empty() {
            self.t("#L_KB_UNBOUND", &[])
        } else {
            shown.label.clone()
        };
        column = column.child(
            div()
                .pl(u(10.))
                .border_l(u(3.))
                .border_color(c)
                .text_px(13.5, 13.5 * 1.3)
                .font_weight(FontWeight::SEMIBOLD)
                .child(label),
        );
        for item in &shown.runtime {
            let text = match &item.file {
                Some(file) => self.t("#L_KB_RUNTIME_FILE", &[&item.trigger, file, &item.label]),
                None => self.t("#L_KB_RUNTIME", &[&item.trigger, &item.label]),
            };
            column = column.child(
                div()
                    .text_color(rgb(RUNTIME))
                    .text_px(12., 12. * 1.45)
                    .child(text),
            );
        }
        if !shown.command.is_empty() && shown.label != shown.command {
            column = column.child(
                div()
                    .font_family(MONO)
                    .text_px(12., 20.)
                    .text_color(dim)
                    .child(shown.command.clone()),
            );
        }
        if !shown.note.is_empty() {
            column = column.child(note(shown.note.clone()));
        }
        if shown.script_warning {
            column = column.child(warn(self.t("#L_KB_SCRIPT_WARNING", &[])));
        }
        if shown.practice {
            column = column.child(warn(self.t("#L_KB_PRACTICE_WARNING", &[])));
        }
        if let Some(alias) = &shown.alias {
            let lead = self.t(
                if shown.custom_alias {
                    "#L_KB_ALIAS_CUSTOM"
                } else {
                    "#L_KB_ALIAS"
                },
                &[],
            );
            let text = format!("{lead} {alias}");
            let code = lead.len() + 1..text.len();
            column = column.child(div().text_color(dim).text_px(12., 12. * 1.45).child(
                StyledText::new(text).with_font_family_overrides(vec![(code, MONO.into())]),
            ));
        }
        if shown.overridden {
            let was = if shown.base_label.is_empty() {
                self.t("#L_KB_UNBOUND", &[])
            } else {
                shown.base_label.clone()
            };
            column = column.child(note(format!("{} {was}", self.t("#L_KB_WAS", &[]))));
        }
        if !shown.other_keys.is_empty() {
            column = column.child(note(format!(
                "{} {}",
                self.t("#L_KB_ALSO_ON", &[]),
                shown.other_keys.join(", ")
            )));
        }
        if !view.is_selected_shown {
            column = column.child(
                div()
                    .text_color(theme.color("l-text-faint"))
                    .text_px(12., 12. * 1.4)
                    .child(self.t("#L_KB_CLICK_HINT", &[])),
            );
        }
        column.into_any_element()
    }

    fn info_head(
        &self,
        shown: &Shown,
        selected_shown: bool,
        theme: &Theme,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let zoom = zoom(window);
        let mut down = shadow(zoom, 0., -2., 0., 0., hsla(0., 0., 0., 0.35));
        down.inset = true;
        let badge = |text: String, launcher: bool| {
            div()
                .px(u(8.))
                .py(u(2.))
                .rounded_full()
                .bg(if launcher {
                    theme.color("accent-soft")
                } else {
                    white(0.07)
                })
                .text_color(theme.color(if launcher { "l-text" } else { "l-text-dim" }))
                .text_px(11., 20.)
                .child(text)
        };
        let mut head = div().flex().flex_wrap().items_center().gap(u(8.)).child(
            div()
                .px(u(9.))
                .py(u(3.))
                .rounded(u(6.))
                .bg(white(0.08))
                .shadow(vec![down])
                .text_px(12., 20.)
                .font_weight(FontWeight::SEMIBOLD)
                .child(shown.name.clone()),
        );
        if shown.overridden {
            head = head.child(badge(self.t("#L_KB_SRC_LAUNCHER", &[]), true));
        } else if let Some(source) = &shown.source {
            head = head.child(badge(source.clone(), false));
        }
        head = head.child(div().flex_1());
        if selected_shown {
            let unbind = self.t("#L_KB_UNBIND", &[]);
            let session = self.session.clone();
            head = head.child(
                RingBtn {
                    width: Some(28.),
                    disabled: shown.command.is_empty(),
                    title: Some(unbind.into()),
                    ..RingBtn::kb("kb-unbind")
                }
                .icon("fas fa-eraser")
                .render(window, cx, move |_, _| session.kb_assign("")),
            );
            if shown.overridden {
                let session = self.session.clone();
                head = head.child(
                    RingBtn {
                        width: Some(28.),
                        title: Some(self.t("#L_KB_REVERT", &[]).into()),
                        ..RingBtn::kb("kb-revert")
                    }
                    .icon("fas fa-undo")
                    .render(window, cx, move |_, _| session.kb_revert()),
                );
            }
        }
        head.into_any_element()
    }

    /// `.kb-actions`: заголовок с режимом и своей командой, поиск и категории, заготовки.
    fn actions(
        &self,
        view: &KeyboardView,
        [custom, search]: [&Entity<TextInput>; 2],
        low: bool,
        theme: &Theme,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let border = theme.color("l-border");
        let (text, dim) = (theme.color("l-text"), theme.color("l-text-dim"));
        let soft = theme.color("accent-soft");
        let radius = theme.len("l-radius-sm");
        let zoom = zoom(window);
        let title = div()
            .flex_1()
            .min_w(u(if low { 0. } else { 160. }))
            .text_px(13., 20.)
            .font_weight(FontWeight::SEMIBOLD)
            .when(low, |el| {
                el.overflow_hidden().whitespace_nowrap().text_ellipsis()
            })
            .child(view.title.clone());
        let modes = view.run_modes.then(|| {
            let modes = [
                ("scriptless", "Scriptless", "#L_KB_SCRIPTLESS_HINT"),
                ("scripted", "Scripted", "#L_KB_SCRIPTED_HINT"),
            ];
            div()
                .id("kb-modes")
                .flex()
                .flex_none()
                .p(u(2.))
                .rounded(u(radius))
                .shadow(vec![ring(zoom, 1., theme.color("l-border-strong"))])
                .role(Role::RadioGroup)
                .aria_label(self.t("#L_KB_RUN_MODE", &[]))
                .children(modes.into_iter().map(|(id, name, hint)| {
                    let active = view.run_mode.id() == id;
                    let session = self.session.clone();
                    let hint = self.t(hint, &[]);
                    let el_id = ElementId::Name(format!("kb-mode-{id}").into());
                    let inner = el_id.clone();
                    crate::snap::target(
                        format!("kb-mode-{id}"),
                        hover(el_id, Timing::fast(), move |t| {
                            div()
                                .id(inner)
                                .px(u(10.))
                                .py(u(4.))
                                .rounded(u((radius - 2.).max(0.)))
                                .text_px(12., 20.)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if active { text } else { mix(dim, text, t) })
                                .when(active, |el| el.bg(soft))
                                .role(Role::RadioButton)
                                .button_text()
                                .cursor_pointer()
                                .tip(hint)
                                .on_click(move |_, _, _| session.kb_set_run_mode(id))
                                .child(name)
                        }),
                    )
                }))
        });
        let session = self.session.clone();
        let apply = RingBtn {
            disabled: !view.custom_enabled,
            ..RingBtn::kb("kb-apply")
        }
        .child(self.t("#L_KB_CUSTOM_APPLY", &[]))
        .render(window, cx, move |_, _| session.kb_assign_custom());
        let form = div()
            .flex()
            .gap(u(8.))
            .flex_grow_1()
            .flex_shrink_1()
            .flex_basis(u(if low { 220. } else { 280. }))
            .when(low, |el| el.min_w_0())
            .child(div().flex_1().min_w_0().child(custom.clone()))
            .child(apply);
        let head = div()
            .flex()
            .items_center()
            .gap(u(10.))
            .when(!low, |el| el.flex_wrap())
            .px(u(10.))
            .py(u(if low { 5. } else { 6. }))
            .border_b_1()
            .border_color(border)
            .child(title)
            .children(modes)
            .child(form);
        let cats = self.cats(view, theme);
        let filter = div()
            .flex()
            .items_center()
            .gap(u(10.))
            .px(u(10.))
            .py(u(if low { 4. } else { 5. }))
            .border_b_1()
            .border_color(border)
            .child(search_box(search, window, cx).flex_none().w(u(176.)))
            .child(cats);
        let error = (!view.custom_error.is_empty()).then(|| {
            div()
                .mx(u(14.))
                .mt(u(8.))
                .text_color(rgb(ERROR_TEXT))
                .text_px(12., 20.)
                .child(view.custom_error.clone())
        });
        let mut groups: Vec<AnyElement> = Vec::new();
        for group in &view.groups {
            let c = tone_color(&group.tone).unwrap_or(white(0.));
            let mut items = div().flex().flex_wrap().items_center().gap(u(4.)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(5.))
                    .mr(u(4.))
                    .text_color(dim)
                    .text_px(11., 20.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .child(swatch(c, 7., 2.))
                    .child(group.title.clone()),
            );
            for chip in &group.chips {
                let action = &chip.action;
                let tag = |text: String, color: u32| {
                    div()
                        .px(u(6.))
                        .rounded_full()
                        .bg(Hsla::from(rgb(color)).opacity(0.22))
                        .text_color(dim)
                        .text_px(10., 16.)
                        .child(text)
                        .into_any_element()
                };
                let mut children = vec![div().child(action.label.clone()).into_any_element()];
                if action.scripted {
                    children.push(tag(self.t("#L_KB_TAG_SCRIPT", &[]), PURPLE));
                }
                if action.practice {
                    children.push(tag(self.t("#L_KB_TAG_PRACTICE", &[]), RED));
                }
                if !action.keys.is_empty() {
                    children.push(
                        div()
                            .text_color(theme.color("l-text-faint"))
                            .text_px(10., 13.)
                            .child(action.keys.join(", "))
                            .into_any_element(),
                    );
                }
                let session = self.session.clone();
                let command = action.command.clone();
                items = items.child(
                    Chip {
                        id: ElementId::Name(
                            format!("kb-chip-{}-{}", group.id, action.command).into(),
                        ),
                        active: chip.active,
                        disabled: view.selected.is_none(),
                        pad_x: 7.,
                        title: Some(action.title.clone()),
                        children,
                    }
                    .render(
                        window,
                        cx,
                        move |_, _| session.kb_assign(&command),
                        None,
                    ),
                );
            }
            groups.push(div().pt(u(4.)).child(items).into_any_element());
        }
        if view.groups.is_empty() {
            groups.push(
                div()
                    .mt(u(16.))
                    .text_center()
                    .text_color(theme.color("l-text-faint"))
                    .text_px(12., 20.)
                    .child(self.t("#L_KB_NOTHING_FOUND", &[]))
                    .into_any_element(),
            );
        }
        let list = ScrollArea::new("kb-list", &self.list_scroll)
            .contain()
            .flex_1()
            .min_h_0()
            .when(view.selected.is_none(), |el| el.opacity(0.5))
            .child(div().pt(u(2.)).px(u(10.)).pb(u(8.)).children(groups));
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .rounded(u(radius))
            .bg(theme.color("l-modal-bar"))
            .text_color(text)
            .child(head)
            .child(filter)
            .children(error)
            .child(list)
            .into_any_element()
    }

    /// `.kb-cats`: «Все» и категории в одну строку; не влезли — строка листается вбок, колесо
    /// листает её же.
    fn cats(&self, view: &KeyboardView, theme: &Theme) -> AnyElement {
        let (text, dim) = (theme.color("l-text"), theme.color("l-text-dim"));
        let (accent, soft) = (theme.color("accent"), theme.color("accent-soft"));
        let mut items: Vec<(String, Option<Hsla>, String, bool)> = vec![(
            String::new(),
            None,
            self.t("#L_KB_ALL", &[]),
            view.category.is_none(),
        )];
        for item in &view.categories {
            items.push((
                item.id.clone(),
                tone_color(&item.tone),
                item.title.clone(),
                view.category.as_deref() == Some(item.id.as_str()),
            ));
        }
        self.cats
            .overflow(div().id("kb-cats"))
            .flex()
            .gap(u(2.))
            .flex_1()
            .min_w_0()
            .pr(u(24.))
            .role(Role::TabList)
            .aria_label(self.t("#L_KB_CATEGORIES", &[]))
            .children(items.into_iter().map(|(id, color, title, active)| {
                let session = self.session.clone();
                let el_id = ElementId::Name(format!("kb-cat-{id}").into());
                let inner = el_id.clone();
                hover(el_id, Timing::fast(), move |t| {
                    div()
                        .id(inner)
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(5.))
                        .px(u(8.))
                        .py(u(2.))
                        .border_1()
                        .rounded_full()
                        .text_px(11., 20.)
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .role(Role::Tab)
                        .button_text()
                        .map(|el| {
                            if active {
                                el.border_color(accent).bg(soft).text_color(text)
                            } else {
                                el.border_color(hsla(0., 0., 0., 0.))
                                    .bg(white(0.05 * t))
                                    .text_color(mix(dim, text, t))
                            }
                        })
                        .when_some(color, |el, color| el.child(swatch(color, 7., 2.)))
                        .child(title)
                        .on_click(move |_, _, _| session.kb_toggle_category(&id))
                })
            }))
            .into_any_element()
    }
}

impl Render for GameKeyboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let Some(view) = self.session.keyboard() else {
            return div().into_any_element();
        };
        self.sync_pane(cx);
        let (custom, search) = self.fields(window, cx);
        self.sync_fields(&view, cx);
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (vw, vh) = viewport(window);
        // Full HD (окно 648–676 px): `@media (max-height: 700px)`.
        let low = vh <= 700.;
        let bottom_min = if low { 120. } else { BOTTOM_MIN };
        let cols = view.cols as f32;
        let measured = self.board_box.get();
        let width = f32::from(measured.size.width) / zoom;
        // shortcut: до первого кадра ширина коробки не известна — схема на 2/3 окна, кадром позже
        // встаёт по мерке.
        let width = if width > 0. { width } else { vw * 0.66 };
        let box_h = box_height(cols, width, vh, self.board_h);
        let height = f32::from(measured.size.height) / zoom;
        let (board_w, board_h) = board_size(cols, width, if height > 0. { height } else { box_h });
        let geo = Geo::new(board_w, board_h, cols);
        let caps: Vec<AnyElement> = view
            .caps
            .iter()
            .map(|cap| self.cap(cap, geo, &theme, window, cx))
            .collect();
        let leave = self.session.clone();
        let board = div()
            .id("kb-board")
            .relative()
            .w(u(board_w))
            .h(u(board_h))
            .mx_auto()
            .role(Role::Group)
            .aria_label(self.t("#L_GS_S_KEYBOARD", &[]))
            .on_hover(move |on, _, _| {
                if !*on {
                    leave.kb_hover(None);
                }
            })
            .children(caps);
        let board_box = div()
            .relative()
            .w_full()
            .flex_shrink_1()
            .h(u(box_h))
            .min_h(u(BOX_MIN))
            .child(measure(self.board_box.clone()))
            .child(board);
        let legend = self.legend(&view, &theme, window, cx);
        let radius = theme.rem("l-radius-sm");
        let pane = div()
            .id("kb-pane")
            .flex()
            .flex_col()
            .flex_shrink_1()
            .min_h(u(208.))
            .pt(u(8.))
            .px(u(12.))
            .pb(u(10.))
            .border_1()
            .border_color(theme.color("l-border"))
            .rounded(radius)
            .bg(theme.color("l-modal-bar"))
            .aria_label(self.t("#L_GS_S_KEYBOARD", &[]))
            .child(legend)
            .child(board_box);
        let splitter = pane_splitter(
            "kb-split",
            Orientation::Horizontal,
            f64::from(BOX_MIN),
            self.board_h,
        )
        .panes(self.board_box.clone(), self.bottom.clone())
        .rest(f64::from(BOTTOM_MIN))
        .label(self.t("#L_SPLIT_KEYBOARD", &[]))
        .hint(self.t("#L_SPLIT_HINT", &[]))
        .on_input(cx.listener(|this, value: &Option<f64>, _, cx| {
            this.board_h = *value;
            cx.notify();
        }))
        .on_save(cx.listener(|this, value: &Option<f64>, _, cx| {
            if let Some(game) = this.pane_game.clone() {
                save_pane(&format!("keyboard:{game}"), *value, cx);
            }
        }));
        let info = ScrollArea::new("kb-info", &self.info_scroll)
            .contain()
            .flex_none()
            .w(relative(0.24))
            .min_w(u(208.))
            .h_full()
            .border_1()
            .border_color(theme.color("l-border"))
            .rounded(radius)
            .bg(theme.color("l-modal-bar"))
            .child(self.info(&view, &theme, window, cx));
        let actions = self.actions(&view, [&custom, &search], low, &theme, window, cx);
        let bottom = div()
            .relative()
            .flex()
            .flex_1()
            .min_h(u(bottom_min))
            .gap(u(8.))
            .child(measure(self.bottom.clone()))
            .child(info)
            .child(actions);
        ScrollArea::new("kb", &self.scroll)
            .contain()
            .size_full()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .h_full()
                    .min_h(u(208. + 8. + bottom_min))
                    .text_color(theme.color("l-text"))
                    .font_family(RUBIK)
                    .child(pane)
                    .child(splitter)
                    .child(bottom),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{board_size, box_height};

    /// Высота схемы как у CSS 0.4.9: по умолчанию от высоты окна (162…215), не выше ширины;
    /// с разделителя — его значение; схема вписывается в коробку.
    #[test]
    fn board_fits_its_box_like_css() {
        // Окно 900: 900 − 530 = 370 → потолок 37 px на клавишу (214,6).
        assert!((box_height(26., 1200., 900., None) - 214.6).abs() < 1e-3);
        // Full HD-окно 648: 118 → пол 28 px на клавишу (162,4).
        assert!((box_height(26., 1200., 648., None) - 162.4).abs() < 1e-3);
        // Узкая панель: не выше, чем влезает по ширине; не ниже 162.
        assert!((box_height(15., 400., 900., None) - 162.).abs() < 1e-3);
        assert!((box_height(15., 600., 900., Some(400.)) - 232.).abs() < 1e-3);
        assert_eq!(box_height(26., 1200., 900., Some(170.)), 170.);
        let (w, h) = board_size(26., 1200., 214.6);
        assert!((w - 962.).abs() < 0.1 && (h - 214.6).abs() < 0.1);
        let (w, h) = board_size(15., 400., 300.);
        assert!((w - 400.).abs() < 1e-3 && (h - 154.666).abs() < 0.01);
    }
}
