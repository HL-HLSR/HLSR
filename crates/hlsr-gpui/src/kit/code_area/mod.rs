//! Многострочный редактор (F10): `.cf__area` 0.4.9 — колонка номеров `.cf__gutter` и
//! `textarea.cf__text` (GameCfgFiles.vue: Consolas → встроенный Cascadia Mono 12,5 px, строка
//! 1,55, `tab-size: 4`, `white-space: pre`, отступы 10/12, полосы `gx-scrollbar`).
//!
//! Ввод — [`EntityInputHandler`], как у поля F6 (IME, палитра символов). Отмена и повтор —
//! [`History`] из `hlsr_ui::controllers::cfg_history`: группы набора и удаления, вставка,
//! вырезание и правки программы ([`CodeArea::apply_edit`]) — свои шаги, как у Chromium.
//! Найденное показывается выделением (`::selection` — `--accent` 45 %), оно видно и без фокуса;
//! [`CodeArea::reveal`] и [`CodeArea::go_to_line`] прокручивают к нему по формуле `reveal` 0.4.9.
//!
//! Прокрутка — [`ScrollArea`] F5 (колесо и полоса Chromium); строки рисует один элемент, только
//! видимые. `uniform_list` не подходит: он берёт высоту строки из раскладки, а та привязана к
//! пикселям (19,375 → 19), и к 40-й строке набегает 15 px против Chromium. По горизонтали —
//! своя прокрутка (колесо с Shift, ползунок, каретка).

mod doc;
#[cfg(test)]
mod tests;

use crate::scale::{u, zoom, zpx};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::text::MONO;
use crate::theme::Theme;
use doc::Lines;
use gpui::{
    Action, App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font, Hitbox, HitboxBehavior,
    Hsla, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, Render, ScrollDelta, ScrollEvent, ShapedLine, SharedString,
    Styled, Subscription, Task, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window,
    actions, canvas, div, fill, font, hsla, point, prelude::*, px, size,
};
use hlsr_ui::controllers::cfg_history::{EditKind, History};
use hlsr_ui::controllers::game_cfg_files::{LINE_HEIGHT, byte_at, utf16_at};
use std::ops::Range;
use std::time::Duration;

const CONTEXT: &str = "CodeArea";
/// `.cf__text`: 12,5 px, `line-height: 1.55` ([`LINE_HEIGHT`]), `padding: 10px 12px`.
const SIZE: f32 = 12.5;
const PAD_X: f32 = 12.;
const PAD_Y: f32 = 10.;
/// `.cf__numbers`: `padding: 10px 8px 10px 10px`.
const GUTTER_LEFT: f32 = 10.;
const GUTTER_RIGHT: f32 = 8.;
/// Горизонтальная полоса `gx-scrollbar`: 10 px, прозрачная рамка ползунка 3 px, ползунок .14.
const HBAR: f32 = 10.;
const HBAR_BORDER: f32 = 3.;
const HBAR_THUMB: f32 = 0.14;
/// Мигание каретки: `GetCaretBlinkTime` Windows по умолчанию (как у поля F6).
const BLINK: Duration = Duration::from_millis(530);

/// Куда идёт каретка.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum To {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

/// Стрелки, Home/End, PageUp/PageDown; `select` — с Shift.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = code_area, no_json)]
pub struct Move {
    pub to: To,
    pub select: bool,
}

/// Backspace/Delete (`Left`/`Right`) и с Ctrl (`WordLeft`/`WordRight`).
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = code_area, no_json)]
pub struct Erase {
    pub to: To,
}

actions!(
    code_area,
    [SelectAll, Copy, Cut, Paste, Undo, Redo, Newline]
);

/// Клавиши редактора — один раз до первого окна (`crate::kit::init`). Tab не занят: в
/// `textarea` он уводит фокус, отступы ставит владелец редактора (GameCfgFiles `onKey`).
pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    let mut keys = Vec::new();
    for (key, shifted, to) in [
        ("left", "shift-left", To::Left),
        ("right", "shift-right", To::Right),
        ("ctrl-left", "ctrl-shift-left", To::WordLeft),
        ("ctrl-right", "ctrl-shift-right", To::WordRight),
        ("up", "shift-up", To::Up),
        ("down", "shift-down", To::Down),
        ("home", "shift-home", To::LineStart),
        ("end", "shift-end", To::LineEnd),
        ("ctrl-home", "ctrl-shift-home", To::DocStart),
        ("ctrl-end", "ctrl-shift-end", To::DocEnd),
        ("pageup", "shift-pageup", To::PageUp),
        ("pagedown", "shift-pagedown", To::PageDown),
    ] {
        keys.push(KeyBinding::new(key, Move { to, select: false }, c));
        keys.push(KeyBinding::new(shifted, Move { to, select: true }, c));
    }
    for (key, to) in [
        ("backspace", To::Left),
        ("shift-backspace", To::Left),
        ("delete", To::Right),
        ("ctrl-backspace", To::WordLeft),
        ("ctrl-delete", To::WordRight),
    ] {
        keys.push(KeyBinding::new(key, Erase { to }, c));
    }
    cx.bind_keys(keys);
    cx.bind_keys([
        KeyBinding::new("ctrl-a", SelectAll, c),
        KeyBinding::new("ctrl-c", Copy, c),
        KeyBinding::new("ctrl-insert", Copy, c),
        KeyBinding::new("ctrl-x", Cut, c),
        KeyBinding::new("shift-delete", Cut, c),
        KeyBinding::new("ctrl-v", Paste, c),
        KeyBinding::new("shift-insert", Paste, c),
        KeyBinding::new("ctrl-z", Undo, c),
        KeyBinding::new("ctrl-y", Redo, c),
        KeyBinding::new("ctrl-shift-z", Redo, c),
        KeyBinding::new("enter", Newline, c),
        KeyBinding::new("shift-enter", Newline, c),
    ]);
}

/// События редактора, как у DOM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeEvent {
    /// `input`: текст изменился от пользователя (набор, IME, вставка, удаление, отмена).
    Input,
    /// Каретка или выделение сдвинуты пользователем без правки (клавиши, мышь).
    Select,
}

/// Раскладка прошлого кадра, px окна: по ней мышь и IME находят символы, а прокрутка — край.
#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    /// Левый край текста (за колонкой номеров и её рамкой).
    text_left: f32,
    /// Ширина текста до вертикальной полосы.
    text_w: f32,
    /// Верх содержимого при scrollTop 0.
    top: f32,
    /// Видимая часть области.
    view: Bounds<Pixels>,
    /// Наибольший scrollLeft (0 — горизонтальной полосы нет).
    max_x: f32,
    /// Горизонтальный ползунок: начало от `text_left` и длина.
    thumb: Option<(f32, f32)>,
    /// Строк нарисовано в кадре.
    #[cfg(test)]
    painted: usize,
}

/// Ползунок горизонтальной полосы тянут: x мыши и scrollLeft при нажатии.
#[derive(Clone, Copy)]
struct Drag {
    pointer: f32,
    left: f32,
}

/// Редактор. Строится в `cx.new(|cx| CodeArea::new(window, cx))`, текст — [`CodeArea::set_text`];
/// размер задаёт родитель (`size_full`).
pub struct CodeArea {
    focus_handle: FocusHandle,
    read_only: bool,
    content: String,
    lines: Lines,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    /// Текст и выделение до составления IME: вся композиция — один шаг набора.
    composing: Option<(String, Range<usize>)>,
    history: History,
    /// x каретки для ↑/↓ (px от начала строки), пока она ходит по строкам.
    goal_x: Option<Pixels>,
    scroll: Scroll,
    /// scrollLeft, px окна.
    scroll_x: f32,
    frame: Frame,
    selecting: bool,
    hdrag: Option<Drag>,
    caret_on: bool,
    blink: Option<Task<()>>,
    _focus: [Subscription; 2],
}

impl EventEmitter<CodeEvent> for CodeArea {}

impl Focusable for CodeArea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Размеры шрифта и строки в px окна.
#[derive(Clone, Copy)]
struct Metrics {
    zoom: f32,
    size: Pixels,
    line: f32,
    pad_x: f32,
    pad_y: f32,
    /// Ширина «0» (`measureChar` 0.4.9): колонка табуляции, колонка номеров, `reveal`.
    char_w: f32,
    /// Высота каретки: ascent + descent.
    glyphs: f32,
}

fn mono() -> Font {
    font(MONO)
}

fn metrics(window: &Window) -> Metrics {
    let zoom = zoom(window);
    let size = zpx(zoom, SIZE);
    let text = window.text_system();
    let id = text.resolve_font(&mono());
    let char_w = text
        .ch_advance(id, size)
        .map_or(SIZE * 0.6 * zoom, f32::from);
    Metrics {
        zoom,
        size,
        line: LINE_HEIGHT * zoom,
        pad_x: PAD_X * zoom,
        pad_y: PAD_Y * zoom,
        char_w,
        glyphs: f32::from(text.ascent(id, size)) + f32::from(text.descent(id, size)).abs(),
    }
}

/// Строка одним шрифтом; у куска `marked` (составление IME) — подчёркивание.
fn shape(
    text: &str,
    color: Hsla,
    marked: Option<Range<usize>>,
    size: Pixels,
    window: &Window,
) -> Option<ShapedLine> {
    if text.is_empty() {
        return None;
    }
    let base = TextRun {
        len: text.len(),
        font: mono(),
        color,
        ..Default::default()
    };
    let runs = match marked.filter(|marked| !marked.is_empty()) {
        None => vec![base],
        Some(marked) => {
            let underline = UnderlineStyle {
                color: Some(color),
                thickness: px(1.),
                wavy: false,
            };
            [
                (0..marked.start, None),
                (marked.clone(), Some(underline)),
                (marked.end..text.len(), None),
            ]
            .into_iter()
            .filter(|(range, _)| !range.is_empty())
            .map(|(range, underline)| TextRun {
                len: range.len(),
                underline,
                ..base.clone()
            })
            .collect()
        }
    };
    Some(
        window
            .text_system()
            .shape_line(SharedString::from(text.to_owned()), size, &runs, None),
    )
}

fn x_at(line: Option<&ShapedLine>, index: usize) -> f32 {
    line.map_or(0., |line| f32::from(line.x_for_index(index)))
}

/// Шаг колеса на строку события GPUI, CSS px (как у прокрутки F5).
fn wheel_step() -> f32 {
    if cfg!(windows) {
        hlsr_ui::render::wheel_step_for_lines(1)
    } else {
        hlsr_ui::render::WHEEL_STEP
    }
}

impl CodeArea {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> CodeArea {
        let focus_handle = cx.focus_handle().tab_stop(true);
        let focus = cx.on_focus(&focus_handle, window, |this, window, cx| {
            this.restart_blink(window, cx);
            cx.notify();
        });
        let blur = cx.on_blur(&focus_handle, window, |this, window, cx| {
            this.blink = None;
            this.selecting = false;
            // Дальше IME пишет в новый фокус: составленное здесь остаётся текстом.
            this.finish_composition(window, cx);
            cx.notify();
        });
        CodeArea {
            focus_handle,
            read_only: false,
            content: String::new(),
            lines: Lines::new(""),
            selected: 0..0,
            reversed: false,
            marked: None,
            composing: None,
            history: History::new(),
            goal_x: None,
            scroll: Scroll::new(),
            scroll_x: 0.,
            frame: Frame::default(),
            selecting: false,
            hdrag: None,
            caret_on: true,
            blink: None,
            _focus: [focus, blur],
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// Строк в тексте (`textOps.lineCount`).
    pub fn lines(&self) -> usize {
        self.lines.count()
    }

    /// Новый текст не правкой (файл открыт, «Вернуть»): отмена забывается, каретка в начало,
    /// прокрутка в начало (`open` 0.4.9: `scrollTop = scrollLeft = 0`, `setSelectionRange(0, 0)`).
    /// Событий нет. Переводы строк `\r\n` и `\r` — `\n`, как значение `textarea`.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into().replace("\r\n", "\n").replace('\r', "\n");
        self.lines = Lines::new(&self.content);
        self.selected = 0..0;
        self.reversed = false;
        self.marked = None;
        self.composing = None;
        self.history.clear();
        self.goal_x = None;
        self.scroll_x = 0.;
        self.scroll.handle().set_offset(Point::default());
        cx.notify();
    }

    /// Выделение, байты UTF-8.
    pub fn selection(&self) -> Range<usize> {
        self.selected.clone()
    }

    /// `setSelectionRange` в байтах (обрезается по тексту и границам символов); набор закрыт.
    pub fn set_selection(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.selected = self.clamp(range.start)..self.clamp(range.end.max(range.start));
        self.reversed = false;
        self.goal_x = None;
        self.history.close_typing();
        cx.notify();
    }

    /// `readonly`: выделение и копирование есть, правок, каретки и IME нет. Правки программы
    /// ([`CodeArea::apply_edit`]) проходят.
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.read_only = read_only;
        cx.notify();
    }

    /// `applyEdit` 0.4.9: правка программы как `execCommand('insertText'|'delete')` — свой шаг
    /// отмены; затем выделение `selection` и прокрутка к каретке. Байты UTF-8.
    pub fn apply_edit(
        &mut self,
        range: Range<usize>,
        insert: &str,
        selection: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Составление IME — текстом до правки: участок программа считала по тексту с ним.
        self.finish_composition(window, cx);
        let range = self.clamp(range.start)..self.clamp(range.end.max(range.start));
        if range.is_empty() && insert.is_empty() {
            self.set_selection(selection, cx);
            return;
        }
        let len = self.content.len() - range.len() + insert.len();
        let at = |offset: usize| offset.min(len);
        self.replace(
            range,
            insert,
            EditKind::Command,
            at(selection.start)..at(selection.end.max(selection.start)),
            window,
            cx,
        );
        // Выделение после правки — на границах символов нового текста.
        self.selected = self.clamp(self.selected.start)..self.clamp(self.selected.end);
    }

    /// `reveal(start, end)` 0.4.9: выделить и прокрутить к участку, не забирая фокус. Строка
    /// конца вне видимого — она по центру; начало левее или правее видимого — треть ширины.
    pub fn reveal(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.set_selection(range, cx);
        let m = metrics(window);
        let (start, end) = (self.selected.start, self.selected.end);
        let client_h = (f32::from(self.frame.view.size.height) - self.hbar(m.zoom)) / m.zoom;
        let top = self.lines.of(end) as f32 * LINE_HEIGHT;
        let scroll_top = f32::from(self.scroll.top()) / m.zoom;
        if top < scroll_top || top + LINE_HEIGHT > scroll_top + client_h {
            let to = (top - client_h / 2.).max(0.) * m.zoom;
            self.scroll.scroll_to(Pixels::from(to), false, window, cx);
        }
        let line_start = self.lines.range(self.lines.of(start)).start;
        // Колонки с раскрытой табуляцией (у 0.4.9 — символы, найденное после `\t` не доезжало).
        let from = doc::columns(&self.content[line_start..start]) as f32;
        let left = from * m.char_w / m.zoom;
        let (scroll_left, client_w) = (self.scroll_x / m.zoom, self.frame.text_w / m.zoom);
        if left < scroll_left || left + PAD_X * 2. > scroll_left + client_w {
            self.scroll_x = (left - client_w / 3.).max(0.) * m.zoom;
        }
        window.notify_text_selection_changed(&self.focus_handle);
    }

    /// Переход к строке (`goToLine` 0.4.9): номер с 1, в пределах текста; каретка в начале строки.
    pub fn go_to_line(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let ix = line.clamp(1, self.lines.count()) - 1;
        let at = self.lines.range(ix).start;
        self.reveal(at..at, window, cx);
    }

    // ---- Каретка -----------------------------------------------------------------------

    /// Высота горизонтальной полосы прошлого кадра, px окна (0 — полосы нет).
    fn hbar(&self, zoom: f32) -> f32 {
        if self.frame.max_x > 0. {
            HBAR * zoom
        } else {
            0.
        }
    }

    fn clamp(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.content.len());
        while !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    fn anchor(&self) -> usize {
        if self.reversed {
            self.selected.end
        } else {
            self.selected.start
        }
    }

    fn move_to(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = offset..offset;
        self.reversed = false;
        self.moved(window, cx);
    }

    fn select_to(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = self.anchor();
        self.reversed = offset < anchor;
        self.selected = anchor.min(offset)..anchor.max(offset);
        self.moved(window, cx);
    }

    /// Выделение сменилось не правкой: набор закрыт (`TypingCommand::CloseTyping`).
    fn moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.history.close_typing();
        window.notify_text_selection_changed(&self.focus_handle);
        self.restart_blink(window, cx);
        cx.emit(CodeEvent::Select);
        cx.notify();
    }

    /// Каретка видна сразу после действия и мигает, пока поле в фокусе (у «только чтения» её нет).
    fn restart_blink(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.caret_on = true;
        if self.read_only || !self.focus_handle.is_focused(window) {
            self.blink = None;
            return;
        }
        self.blink = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK).await;
                let alive = this.update(cx, |this, cx| {
                    this.caret_on = !this.caret_on;
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    /// Строка на экране (табуляция раскрыта).
    fn shape_line(&self, ix: usize, window: &Window) -> Option<ShapedLine> {
        let line = &self.content[self.lines.range(ix)];
        let size = zpx(zoom(window), SIZE);
        shape(&doc::expand(line), gpui::black(), None, size, window)
    }

    /// x символа `offset` от начала его строки.
    fn x_of(&self, offset: usize, window: &Window) -> f32 {
        let ix = self.lines.of(offset);
        let range = self.lines.range(ix);
        let shown = doc::to_display(&self.content[range.clone()], offset - range.start);
        x_at(self.shape_line(ix, window).as_ref(), shown)
    }

    /// Символ под точкой окна: выше первой строки — первая, ниже последней — последняя.
    fn offset_at(&self, position: Point<Pixels>, window: &Window) -> usize {
        let m = metrics(window);
        let top = self.frame.top - f32::from(self.scroll.top()) + m.pad_y;
        let row = ((f32::from(position.y) - top) / m.line).floor();
        let ix = (row.max(0.) as usize).min(self.lines.count() - 1);
        let x = f32::from(position.x) - (self.frame.text_left + m.pad_x - self.scroll_x);
        let range = self.lines.range(ix);
        let shown = self
            .shape_line(ix, window)
            .map_or(0, |line| line.closest_index_for_x(Pixels::from(x)));
        range.start + doc::from_display(&self.content[range], shown)
    }

    /// Строка выше или ниже на `delta` с той же x (за первой — начало текста, за последней —
    /// конец).
    fn vertical(&mut self, from: usize, delta: isize, window: &Window) -> usize {
        let goal = match self.goal_x {
            Some(goal) => goal,
            None => Pixels::from(self.x_of(from, window)),
        };
        self.goal_x = Some(goal);
        let target = self.lines.of(from) as isize + delta;
        if target < 0 {
            return 0;
        }
        let ix = target as usize;
        if ix >= self.lines.count() {
            return self.content.len();
        }
        let range = self.lines.range(ix);
        let shown = self
            .shape_line(ix, window)
            .map_or(0, |line| line.closest_index_for_x(goal));
        range.start + doc::from_display(&self.content[range], shown)
    }

    /// Каретку — в видимое, как Chromium после правки и стрелок: на край, ближайший к ней; у
    /// первой и последней строк — вместе с отступом.
    fn follow(&mut self, window: &Window, cx: &App) {
        let view_h = f32::from(self.frame.view.size.height);
        if view_h <= 0. {
            return;
        }
        let m = metrics(window);
        let cursor = self.cursor();
        let ix = self.lines.of(cursor);
        let line_top = m.pad_y + ix as f32 * m.line;
        let lo = if ix == 0 { 0. } else { line_top };
        let hi = if ix + 1 == self.lines.count() {
            line_top + m.line + m.pad_y
        } else {
            line_top + m.line
        };
        let client_h = view_h - self.hbar(m.zoom);
        let top = f32::from(self.scroll.top());
        let to = if lo < top {
            lo
        } else if hi > top + client_h {
            hi - client_h
        } else {
            top
        };
        if to != top {
            self.scroll.scroll_to(Pixels::from(to), false, window, cx);
        }
        let x = m.pad_x + self.x_of(cursor, window);
        let lo = if cursor == self.lines.range(ix).start {
            0.
        } else {
            x
        };
        let hi = x + 1.;
        let width = self.frame.text_w;
        if lo < self.scroll_x {
            self.scroll_x = lo;
        } else if width > 0. && hi > self.scroll_x + width {
            self.scroll_x = hi - width;
        }
    }

    // ---- Правки ------------------------------------------------------------------------

    /// Правка пользователя: каретка за вставкой; у «только чтения» — ничего.
    fn edit(
        &mut self,
        range: Range<usize>,
        text: &str,
        kind: EditKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only || (range.is_empty() && text.is_empty()) {
            return;
        }
        let end = range.start + text.len();
        self.replace(range, text, kind, end..end, window, cx);
    }

    /// Замена куска с шагом отмены; выделение после — `selection`.
    fn replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        kind: EditKind,
        selection: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // shortcut: копия текста на правку (шаг отмены — разница двух текстов); 5000 строк —
        // ~200 КБ за микросекунды, менять, если файлы станут в разы больше.
        let before = self.content.clone();
        let was = (self.selected.start, self.selected.end);
        self.content.replace_range(range, text);
        // Выделение в шаге — байты: History его только хранит и возвращает.
        self.history.record(
            kind,
            &before,
            &self.content,
            was,
            (selection.start, selection.end),
        );
        self.changed(selection, window, cx);
    }

    /// Текст сменился: строки, выделение, IME, прокрутка к каретке, `input`.
    fn changed(&mut self, selection: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.lines = Lines::new(&self.content);
        self.selected = selection;
        self.reversed = false;
        self.marked = None;
        self.goal_x = None;
        window.notify_text_input_changed(&self.focus_handle);
        self.restart_blink(window, cx);
        self.follow(window, cx);
        cx.emit(CodeEvent::Input);
        cx.notify();
    }

    /// Конец составления IME: весь ввод от его начала — один шаг набора.
    fn finish_composition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((before, was)) = self.composing.take() else {
            return;
        };
        if before != self.content {
            self.history.record(
                EditKind::Insert,
                &before,
                &self.content,
                (was.start, was.end),
                (self.selected.start, self.selected.end),
            );
        }
        let selection = self.selected.clone();
        self.changed(selection, window, cx);
    }

    /// Клавиша правки посреди составления IME: составление снято (текст и выделение до него).
    /// Иначе шаги отмены разойдутся с текстом, а IME потом вставит составленное ещё раз.
    fn drop_composition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((before, was)) = self.composing.take() {
            self.content = before;
            self.changed(was, window, cx);
        }
    }

    // ---- Действия ----------------------------------------------------------------------

    fn move_caret(&mut self, action: &Move, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor();
        let collapse = !action.select && !self.selected.is_empty();
        let page = || {
            let m = metrics(window);
            (f32::from(self.frame.view.size.height) / m.line)
                .floor()
                .max(1.) as isize
        };
        let page = page();
        let vertical = matches!(action.to, To::Up | To::Down | To::PageUp | To::PageDown);
        if !vertical {
            self.goal_x = None;
        }
        let target = match action.to {
            To::Left if collapse => self.selected.start,
            To::Right if collapse => self.selected.end,
            To::Left => doc::previous_boundary(&self.content, &self.lines, cursor),
            To::Right => doc::next_boundary(&self.content, &self.lines, cursor),
            To::WordLeft => doc::previous_word(&self.content, &self.lines, cursor),
            To::WordRight => doc::next_word(&self.content, &self.lines, cursor),
            To::LineStart => self.lines.range(self.lines.of(cursor)).start,
            To::LineEnd => self.lines.range(self.lines.of(cursor)).end,
            To::DocStart => 0,
            To::DocEnd => self.content.len(),
            To::Up => self.vertical(cursor, -1, window),
            To::Down => self.vertical(cursor, 1, window),
            To::PageUp => self.vertical(cursor, -page, window),
            To::PageDown => self.vertical(cursor, page, window),
        };
        if action.select {
            self.select_to(target, window, cx);
        } else {
            self.move_to(target, window, cx);
        }
        self.follow(window, cx);
    }

    fn erase(&mut self, action: &Erase, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_composition(window, cx);
        let cursor = self.cursor();
        let range = if !self.selected.is_empty() {
            self.selected.clone()
        } else {
            match action.to {
                To::Left => doc::previous_boundary(&self.content, &self.lines, cursor)..cursor,
                To::Right => cursor..doc::next_boundary(&self.content, &self.lines, cursor),
                To::WordLeft => doc::previous_word(&self.content, &self.lines, cursor)..cursor,
                To::WordRight => cursor..doc::next_word(&self.content, &self.lines, cursor),
                _ => return,
            }
        };
        let kind = match action.to {
            To::Left | To::WordLeft => EditKind::DeleteBackward,
            _ => EditKind::DeleteForward,
        };
        self.edit(range, "", kind, window, cx);
    }

    /// Ctrl+A: всё, без прокрутки.
    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0..self.content.len();
        self.reversed = false;
        self.goal_x = None;
        self.moved(window, cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            let text = self.content[self.selected.clone()].to_owned();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_composition(window, cx);
        if self.read_only || self.selected.is_empty() {
            return;
        }
        self.copy(&Copy, window, cx);
        self.edit(self.selected.clone(), "", EditKind::Cut, window, cx);
    }

    /// Вставка: переводы строк `\r\n` и `\r` — `\n`, как `textarea`; табуляция остаётся.
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.drop_composition(window, cx);
        self.edit(
            self.selected.clone(),
            &clean(&text, true),
            EditKind::Paste,
            window,
            cx,
        );
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_composition(window, cx);
        if self.read_only {
            return;
        }
        if let Some(restored) = self.history.undo(&self.content) {
            self.content = restored.text;
            self.changed(restored.selection.0..restored.selection.1, window, cx);
        }
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_composition(window, cx);
        if self.read_only {
            return;
        }
        if let Some(restored) = self.history.redo(&self.content) {
            self.content = restored.text;
            self.changed(restored.selection.0..restored.selection.1, window, cx);
        }
    }

    /// Enter: перевод строки — набор (`InsertLineBreak`), в группе с символами.
    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_composition(window, cx);
        self.edit(self.selected.clone(), "\n", EditKind::Insert, window, cx);
    }

    // ---- Мышь --------------------------------------------------------------------------

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (x, y) = (f32::from(event.position.x), f32::from(event.position.y));
        // Колонка номеров — отдельный блок без фокуса (`aria-hidden`, `user-select: none`).
        if x < self.frame.text_left {
            window.prevent_default();
            return;
        }
        let zoom = zoom(window);
        if let Some((start, length)) = self.frame.thumb
            && y >= f32::from(self.frame.view.bottom()) - HBAR * zoom
        {
            window.prevent_default();
            let local = x - self.frame.text_left;
            if local >= start && local < start + length {
                self.hdrag = Some(Drag {
                    pointer: x,
                    left: self.scroll_x,
                });
            } else {
                // Щелчок по дорожке — страница Chromium: 87,5 % видимого.
                let page = (self.frame.text_w * 0.875).max(1.);
                let step = if local < start { -page } else { page };
                self.scroll_x = (self.scroll_x + step).clamp(0., self.frame.max_x);
                cx.notify();
            }
            return;
        }
        let offset = self.offset_at(event.position, window);
        self.goal_x = None;
        match event.click_count {
            2 => {
                let word = doc::word_at(&self.content, &self.lines, offset);
                self.move_to(word.start, window, cx);
                self.select_to(word.end, window, cx);
            }
            // Тройной — строка вместе с переводом строки (абзац Chromium).
            n if n >= 3 => {
                let line = self.lines.range(self.lines.of(offset));
                let end = (line.end + 1).min(self.content.len());
                self.move_to(line.start, window, cx);
                self.select_to(end, window, cx);
            }
            _ => {
                self.selecting = true;
                if event.modifiers.shift {
                    self.select_to(offset, window, cx);
                } else {
                    self.move_to(offset, window, cx);
                }
            }
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        if self.hdrag.take().is_some() {
            cx.notify();
        }
    }

    /// Мышь с зажатой кнопкой (и за краем области): выделение с прокруткой или ползунок.
    fn drag(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            // Отпускание не дошло (захват мыши потерян) — тянуть нечего.
            self.selecting = false;
            if self.hdrag.take().is_some() {
                cx.notify();
            }
            return;
        }
        if let Some(drag) = self.hdrag {
            if let Some((_, length)) = self.frame.thumb {
                let track = self.frame.text_w - length;
                if track > 0. {
                    let dx = f32::from(event.position.x) - drag.pointer;
                    self.scroll_x =
                        (drag.left + dx * self.frame.max_x / track).clamp(0., self.frame.max_x);
                    cx.notify();
                }
            }
            return;
        }
        if self.selecting {
            let offset = self.offset_at(event.position, window);
            self.select_to(offset, window, cx);
            self.follow(window, cx);
        }
    }

    /// Колесо по горизонтали (Shift + колесо GPUI отдаёт как x); вертикаль — у [`ScrollArea`].
    fn on_wheel(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let dx = match event.delta {
            ScrollDelta::Lines(lines) => -lines.x * wheel_step() * zoom(window),
            ScrollDelta::Pixels(pixels) => -f32::from(pixels.x),
        };
        let to = (self.scroll_x + dx).clamp(0., self.frame.max_x.max(0.));
        if dx != 0. && to != self.scroll_x {
            self.scroll_x = to;
            cx.stop_propagation();
            cx.notify();
        }
    }

    // ---- Кадр --------------------------------------------------------------------------

    /// Видимые строки кадра: номера, текст, выделение, каретка; горизонтальный ползунок.
    fn layout(&self, bounds: Bounds<Pixels>, colors: Colors, window: &mut Window) -> Layout {
        let m = metrics(window);
        // Видимое — вся область прокрутки (маска), даже когда текст короче её: колонка номеров,
        // горизонтальная полоса и I-beam — во всю высоту, как у `textarea`.
        let mask = window.content_mask().bounds;
        let view = Bounds::from_corners(
            point(bounds.left(), mask.top()),
            point(bounds.right(), mask.bottom()),
        );
        let digits = self.lines.count().to_string().len() as f32;
        let gutter = (GUTTER_LEFT + GUTTER_RIGHT) * m.zoom + digits * m.char_w;
        let left = f32::from(bounds.left());
        let text_left = left + gutter + 1.;
        let text_w = (f32::from(bounds.right()) - text_left).max(0.);
        let content_w = self.lines.widest() as f32 * m.char_w + 2. * m.pad_x + 1.;
        let max_x = (content_w - text_w).max(0.);
        let scroll_x = self.scroll_x.clamp(0., max_x);
        let x0 = text_left + m.pad_x - scroll_x;
        let top = f32::from(bounds.top()) + m.pad_y;
        let visible = |y: Pixels| (f32::from(y) - top) / m.line;
        let first = visible(view.top()).floor().max(0.) as usize;
        let last = (visible(view.bottom()).ceil().max(0.) as usize).min(self.lines.count());
        let caret = !self.read_only
            && self.caret_on
            && self.selected.is_empty()
            && self.focus_handle.is_focused(window);
        let cursor = self.cursor();
        let selection = &self.selected;
        let rows = (first..last.max(first))
            .map(|ix| {
                let range = self.lines.range(ix);
                let line = &self.content[range.clone()];
                let shown = |offset: usize| doc::to_display(line, offset - range.start);
                let marked = self
                    .marked
                    .as_ref()
                    .filter(|marked| marked.start >= range.start && marked.end <= range.end)
                    .map(|marked| shown(marked.start)..shown(marked.end));
                let text = shape(&doc::expand(line), colors.text, marked, m.size, window);
                let y = top + ix as f32 * m.line;
                let x = |offset: usize| x0 + x_at(text.as_ref(), shown(offset));
                // Выделение на строке; дальше конца — ещё ширина символа за перевод строки.
                let newline = selection.start <= range.end && selection.end > range.end;
                let from = selection.start.max(range.start);
                let to = selection.end.min(range.end);
                let selection = (!selection.is_empty() && (from < to || newline)).then(|| {
                    let (x1, x2) = (x(from), x(to) + if newline { m.char_w } else { 0. });
                    bounds_of(x1, y, x2 - x1, m.line)
                });
                let caret = (caret && self.lines.of(cursor) == ix)
                    .then(|| bounds_of(x(cursor), y + (m.line - m.glyphs) / 2., 1., m.glyphs));
                // `.cf__numbers { text-align: right }`.
                let number = shape(&(ix + 1).to_string(), colors.faint, None, m.size, window);
                let width = number.as_ref().map_or(0., |number| f32::from(number.width));
                let number_x = left + gutter - GUTTER_RIGHT * m.zoom - width;
                Row {
                    y,
                    number,
                    number_x,
                    text,
                    x0,
                    selection,
                    caret,
                }
            })
            .collect::<Vec<_>>();
        // Ползунок Chromium: длина — видимое/всё × дорожка, место — по прокрутке.
        let thumb = (max_x > 0.).then(|| {
            let length = (text_w / content_w * text_w)
                .round()
                .max(2. * HBAR_BORDER * m.zoom);
            let start = if max_x > 0. {
                (scroll_x / max_x * (text_w - length)).round()
            } else {
                0.
            };
            (start, length)
        });
        let clip = Bounds::from_corners(
            point(Pixels::from(text_left), view.top()),
            point(view.right(), view.bottom()),
        );
        Layout {
            m,
            gutter_x: left + gutter,
            rows,
            view,
            clip,
            frame: Frame {
                text_left,
                text_w,
                top: f32::from(bounds.top()) + f32::from(self.scroll.top()),
                view,
                max_x,
                thumb,
                #[cfg(test)]
                painted: last.saturating_sub(first),
            },
            scroll_x,
        }
    }
}

/// Ввод как в `textarea`: `\r\n` и `\r` — `\n`, прочие управляющие символы выброшены; табуляция —
/// только при `tab` (вставка её хранит, клавиша Tab в `textarea` не символ).
fn clean(text: &str, tab: bool) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|&c| c == '\n' || (tab && c == '\t') || !c.is_control())
        .collect()
}

fn bounds_of(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(
        point(Pixels::from(x), Pixels::from(y)),
        size(Pixels::from(w), Pixels::from(h)),
    )
}

/// Цвета темы кадра.
#[derive(Clone, Copy)]
struct Colors {
    text: Hsla,
    faint: Hsla,
    border: Hsla,
    /// `::selection`: `color-mix(in srgb, var(--accent) 45%, transparent)`.
    selection: Hsla,
}

struct Row {
    y: f32,
    number: Option<ShapedLine>,
    number_x: f32,
    text: Option<ShapedLine>,
    x0: f32,
    selection: Option<Bounds<Pixels>>,
    caret: Option<Bounds<Pixels>>,
}

struct Layout {
    m: Metrics,
    gutter_x: f32,
    rows: Vec<Row>,
    view: Bounds<Pixels>,
    clip: Bounds<Pixels>,
    frame: Frame,
    scroll_x: f32,
}

/// Рисует кадр, ставит обработчик ввода окна, курсор и мышь с зажатой кнопкой.
fn paint(
    area: Entity<CodeArea>,
    layout: Layout,
    hitbox: Hitbox,
    colors: Colors,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let m = layout.m;
    let line = Pixels::from(m.line);
    // Рамка колонки номеров (`border-right: 1px solid var(--l-border)`) — во всю высоту.
    window.paint_quad(fill(
        Bounds::new(
            point(Pixels::from(layout.gutter_x), layout.view.top()),
            size(px(1.), layout.view.size.height),
        ),
        colors.border,
    ));
    for row in &layout.rows {
        if let Some(number) = &row.number {
            let at = point(Pixels::from(row.number_x), Pixels::from(row.y));
            number
                .paint(at, line, TextAlign::Left, None, window, cx)
                .ok();
        }
    }
    window.with_content_mask(
        Some(ContentMask {
            bounds: layout.clip,
        }),
        |window| {
            for row in &layout.rows {
                if let Some(selection) = row.selection {
                    window.paint_quad(fill(selection, colors.selection));
                }
                if let Some(text) = &row.text {
                    let at = point(Pixels::from(row.x0), Pixels::from(row.y));
                    text.paint(at, line, TextAlign::Left, None, window, cx).ok();
                }
                if let Some(caret) = row.caret {
                    window.paint_quad(fill(caret, colors.text));
                }
            }
            if let Some((start, length)) = layout.frame.thumb {
                let inset = HBAR_BORDER * m.zoom;
                let thumb = bounds_of(
                    layout.frame.text_left + start + inset,
                    f32::from(layout.view.bottom()) - HBAR * m.zoom + inset,
                    (length - 2. * inset).max(0.),
                    (HBAR - 2. * HBAR_BORDER) * m.zoom,
                );
                window.paint_quad(
                    fill(thumb, hsla(0., 0., 1., HBAR_THUMB)).corner_radii(zpx(m.zoom, 2.)),
                );
            }
        },
    );
    window.set_cursor_style(CursorStyle::IBeam, &hitbox);
    let (focus, read_only) = {
        let area = area.read(cx);
        (area.focus_handle.clone(), area.read_only)
    };
    if !read_only {
        window.handle_input(&focus, ElementInputHandler::new(bounds, area.clone()), cx);
    }
    let dragged = area.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase == gpui::DispatchPhase::Bubble {
            dragged.update(cx, |area, cx| area.drag(event, window, cx));
        }
    });
    let (frame, scroll_x) = (layout.frame, layout.scroll_x);
    area.update(cx, |area, cx| {
        // Горизонтальная полоса появилась или ушла — высота содержимого другая (`overflow: auto`).
        let bar = (area.frame.max_x > 0.) != (frame.max_x > 0.);
        area.frame = frame;
        area.scroll_x = scroll_x;
        if bar {
            cx.notify();
        }
    });
}

impl Render for CodeArea {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let colors = Colors {
            text: theme.color("l-text"),
            faint: theme.color("l-text-faint"),
            border: theme.color("l-border"),
            selection: theme.color("accent").opacity(0.45),
        };
        let height = 2. * PAD_Y + self.lines.count() as f32 * LINE_HEIGHT + self.hbar(1.);
        let (laid, painted) = (cx.entity(), cx.entity());
        let body = canvas(
            move |bounds, window, cx| {
                let layout = laid.read(cx).layout(bounds, colors, window);
                let hitbox = window.insert_hitbox(layout.clip, HitboxBehavior::Normal);
                (layout, hitbox)
            },
            move |bounds, (layout, hitbox), window, cx| {
                paint(painted, layout, hitbox, colors, bounds, window, cx)
            },
        )
        .w_full()
        .h(u(height));
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            // `.cf__area { background: rgba(0, 0, 0, 0.18) }`.
            .bg(hsla(0., 0., 0., 0.18))
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::move_caret))
            .on_action(cx.listener(Self::erase))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::newline))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll(cx.listener(Self::on_wheel))
            .child(
                ScrollArea::new("code", &self.scroll)
                    .bar(Bar::Fields)
                    .size_full()
                    .child(body),
            )
    }
}

impl EntityInputHandler for CodeArea {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.from16(&range_utf16);
        actual_range.replace(self.to16(&range));
        Some(self.content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.to16(&self.selected),
            reversed: self.reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = self.from16(&range_utf16);
        self.reversed = false;
        self.moved(window, cx);
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.to16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        self.finish_composition(window, cx);
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    /// Набор и конец составления IME. Управляющие символы (Tab, Esc) в `textarea` не вводятся.
    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let text = clean(new_text, false);
        let range = range_utf16
            .map(|range| self.from16(&range))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        if self.composing.is_some() {
            self.content.replace_range(range.clone(), &text);
            let end = range.start + text.len();
            self.selected = end..end;
            self.finish_composition(window, cx);
            return;
        }
        self.edit(range, &text, EditKind::Insert, window, cx);
    }

    /// Составление IME: текст с подчёркиванием; шаг отмены — по его концу.
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        // Как у конца составления, иначе текст сменит вид при нём.
        let new_text = &clean(new_text, false);
        let range = range_utf16
            .map(|range| self.from16(&range))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        if self.composing.is_none() {
            self.composing = Some((self.content.clone(), self.selected.clone()));
        }
        self.content.replace_range(range.clone(), new_text);
        self.lines = Lines::new(&self.content);
        self.marked = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        let start16 = utf16_at(&self.content, range.start);
        let len16 = new_text.encode_utf16().count();
        self.selected = match new_selected_range_utf16 {
            Some(selected) => self
                .from16(&(start16 + selected.start.min(len16)..start16 + selected.end.min(len16))),
            None => range.start + new_text.len()..range.start + new_text.len(),
        };
        self.reversed = false;
        window.notify_text_input_changed(&self.focus_handle);
        self.restart_blink(window, cx);
        cx.emit(CodeEvent::Input);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.from16(&range_utf16);
        let m = metrics(window);
        let ix = self.lines.of(range.start);
        let y = self.frame.top - f32::from(self.scroll.top()) + m.pad_y + ix as f32 * m.line;
        let x0 = self.frame.text_left + m.pad_x - self.scroll_x;
        let end = if self.lines.of(range.end) == ix {
            self.x_of(range.end, window)
        } else {
            self.x_of(self.lines.range(ix).end, window)
        };
        let start = self.x_of(range.start, window);
        Some(bounds_of(x0 + start, y, end - start, m.line))
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.bounds_for_range(range_utf16, bounds, window, cx)
            .into_iter()
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.frame.view.contains(&point).then(|| {
            let offset = self.offset_at(point, window);
            utf16_at(&self.content, offset)
        })
    }
}

impl CodeArea {
    fn from16(&self, range: &Range<usize>) -> Range<usize> {
        byte_at(&self.content, range.start)..byte_at(&self.content, range.end)
    }

    fn to16(&self, range: &Range<usize>) -> Range<usize> {
        utf16_at(&self.content, range.start)..utf16_at(&self.content, range.end)
    }
}
