//! Однострочное поле ввода (F6): `<input>` 0.4.9 типов text, password, number и search.
//!
//! Основа — `examples/input.rs` из gpui-pre 0.3.13 (Apache-2.0, © Zed Industries, Inc.): строка
//! на [`EntityInputHandler`] (IME, палитра символов ОС), выделение мышью, буфер обмена. Сверху —
//! то, что делает Chromium у `<input>`: шаг по словам (Ctrl+стрелки, Ctrl+Backspace/Delete,
//! пробелы после слова пропускаются, как на Windows), двойной и тройной щелчок, отмена и повтор с
//! группами набора, прокрутка строки за кареткой, мигание каретки, `maxlength`, маска пароля,
//! фильтр и шаг числа (`hlsr_ui::fields`: стрелки ↑/↓, колесо в фокусе), поиск со значком и
//! крестиком, `placeholder`, фокус, `invalid`, `disabled`.
//!
//! Оформление — [`FieldStyle`] по CSS 0.4.9. Ширину задаёт родитель (поле — `w_full`). События —
//! [`FieldEvent`] (`input`, `change`, Enter, `blur`, очистка поиска); текст — [`TextInput::text`].

use crate::icons::fa_glyph;
use crate::paint::ring;
use crate::scale::{u, zoom, zpx};
use crate::text::{MONO, RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, KeyBinding, LayoutId, LineCap,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, PathStyle, Pixels,
    Point, Render, ScrollEvent, ShapedLine, SharedString, StrokeOptions, Style, Styled,
    Subscription, Task, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, actions,
    canvas, div, fill, hsla, point, prelude::*, px, relative, rgb, size,
};
use hlsr_ui::fields;
use std::ops::Range;
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation as _;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        Home,
        End,
        SelectHome,
        SelectEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Enter,
        StepUp,
        StepDown,
        Cancel,
    ]
);

const CONTEXT: &str = "TextInput";
/// Контекст числового поля: только в нём ↑/↓ — шаг, в остальных они уходят родителю (список
/// поиска бегунов).
const NUMBER_CONTEXT: &str = "NumberInput";
/// Контекст поиска: Esc очищает только его.
const SEARCH_CONTEXT: &str = "SearchInput";
/// Маска пароля: `-webkit-text-security: disc` — U+2022.
const BULLET: char = '•';
/// Мигание каретки: `GetCaretBlinkTime` Windows по умолчанию (Chromium берёт его).
const BLINK: Duration = Duration::from_millis(530);
/// Шагов отмены не больше (у Chromium своё ограничение; поле короткое).
const UNDO_LIMIT: usize = 100;
/// `::selection` (base.css 0.4.9): действует и внутри `<input>` (снято эталоном).
const SELECTION: u32 = 0xff5252;

/// Клавиши поля — один раз до первого окна (`crate::kit::init`).
pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    let n = Some(NUMBER_CONTEXT);
    let s = Some(SEARCH_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, c),
        KeyBinding::new("ctrl-delete", DeleteWordRight, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("ctrl-left", WordLeft, c),
        KeyBinding::new("ctrl-right", WordRight, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
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
        KeyBinding::new("enter", Enter, c),
        KeyBinding::new("up", StepUp, n),
        KeyBinding::new("down", StepDown, n),
        KeyBinding::new("escape", Cancel, s),
    ]);
}

/// Тип `<input>`.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Text,
    Password,
    /// `type="number"`: атрибуты `min`, `max`, `step` строками, как в разметке (пусто — нет).
    Number {
        min: SharedString,
        max: SharedString,
        step: SharedString,
    },
    /// `type="search"` поиска бегунов: лупа слева, свой крестик справа.
    Search,
}

/// События поля, как у DOM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldEvent {
    /// `input`: текст изменился от пользователя (набор, IME, вставка, удаление, отмена, шаг).
    Input,
    /// `change`: Enter, уход фокуса или шаг числа — и текст не тот, что при прошлом `change`.
    Change,
    /// Нажат Enter (после `change`).
    Enter,
    /// `blur` (после `change`).
    Blur,
    /// Нажат крестик поиска: поле очищено (перед этим — `input`).
    Cleared,
}

/// Оформление поля: CSS одного класса 0.4.9, длины — CSS px макета.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldStyle {
    /// Высота с рамкой; 0 — `2 × pad_y + line + рамка`.
    pub height: f32,
    pub pad_x: f32,
    /// `padding-right`, когда оно не `pad_x` (`.gx-num.has-step`: место под стрелки).
    pub pad_right: Option<f32>,
    pub pad_y: f32,
    pub family: &'static str,
    pub size: f32,
    /// `line-height`; 0 — normal (по метрикам шрифта).
    pub line: f32,
    pub radius: f32,
    pub bg: Hsla,
    pub hover_bg: Hsla,
    pub border: Hsla,
    pub hover_border: Hsla,
    pub focus_border: Hsla,
    pub invalid_border: Hsla,
    /// Рамка — `box-shadow: inset 0 0 0 1px` (места не занимает), а не `border`.
    pub ring: bool,
    /// `:focus { box-shadow: 0 0 0 <N>px <цвет> }`.
    pub glow: Option<(f32, Hsla)>,
    /// `:focus { outline: 2px solid <цвет>; outline-offset: 2px }`.
    pub outline: Option<Hsla>,
    /// `opacity` у `:disabled`.
    pub disabled_opacity: f32,
}

fn white(alpha: f32) -> Hsla {
    hsla(0., 0., 1., alpha)
}

impl FieldStyle {
    /// `.gx-input` (settings-fields.scss:155): 34 px, `padding: 0 12px`, 13 px; наведение —
    /// фон .07 и `--l-border-strong`; фокус — рамка `--accent` и кольцо 2 px `--accent-soft`;
    /// `.invalid` — рамка `#ff4d5e`.
    pub fn gx(theme: &Theme) -> FieldStyle {
        FieldStyle {
            height: 34.,
            pad_x: 12.,
            pad_right: None,
            pad_y: 0.,
            family: RUBIK,
            size: 13.,
            line: 0.,
            radius: theme.len("l-radius-sm"),
            bg: white(0.05),
            hover_bg: white(0.07),
            border: theme.color("l-border"),
            hover_border: theme.color("l-border-strong"),
            focus_border: theme.color("accent"),
            invalid_border: rgb(0xff4d5e).into(),
            ring: false,
            glow: Some((2., theme.color("accent-soft"))),
            outline: None,
            disabled_opacity: 1.,
        }
    }

    /// `.gx-input.cf__find-input` (GameCfgFiles.vue:1006): 28 px, моноширинный 12,5 px
    /// (Consolas → встроенный Cascadia Mono, как в slint).
    pub fn gx_mono(theme: &Theme) -> FieldStyle {
        FieldStyle {
            height: 28.,
            family: MONO,
            size: 12.5,
            ..FieldStyle::gx(theme)
        }
    }

    /// `.runner-input` (RunnerSettings.vue:1161): `padding: 9px 14px`, строка 20 px; фокус —
    /// только рамка `--accent`.
    pub fn runner(theme: &Theme) -> FieldStyle {
        let border = theme.color("l-border");
        FieldStyle {
            height: 0.,
            pad_x: 14.,
            pad_y: 9.,
            line: 20.,
            hover_border: border,
            invalid_border: border,
            glow: None,
            disabled_opacity: 0.45,
            ..FieldStyle::gx(theme)
        }
    }

    /// `.runner-search input` (RunnerSearch.vue): высота `--l-header-control`, `padding: 0 32px`
    /// (справа 12 px, пока пусто), 12,5 px, строка 20 px, рамка inset 1 px; фокус — контур 2 px
    /// `--accent` со смещением 2 px; `:disabled` — .6.
    pub fn search(theme: &Theme) -> FieldStyle {
        let border = theme.color("l-border");
        FieldStyle {
            height: theme.len("l-header-control"),
            pad_x: 32.,
            size: 12.5,
            line: 20.,
            bg: white(0.04),
            hover_bg: white(0.06),
            hover_border: border,
            focus_border: border,
            invalid_border: border,
            ring: true,
            glow: None,
            outline: Some(theme.color("accent")),
            disabled_opacity: 0.6,
            ..FieldStyle::gx(theme)
        }
    }
}

struct Snapshot {
    text: String,
    selection: Range<usize>,
}

/// Поле ввода. Строится в `cx.new(|cx| TextInput::new(window, cx).placeholder(…))`.
pub struct TextInput {
    focus_handle: FocusHandle,
    kind: Kind,
    /// Пусто — по типу: `search` у поиска, `gx` у остальных.
    style: Option<fn(&Theme) -> FieldStyle>,
    placeholder: SharedString,
    max_length: usize,
    align_right: bool,
    align_center: bool,
    disabled: bool,
    invalid: bool,
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    /// Левый край строки на экране (с выравниванием и прокруткой) в прошлом кадре.
    last_x0: Pixels,
    scroll_x: Pixels,
    is_selecting: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Идёт набор: следующие символы и удаления — в ту же группу отмены (TypingCommand Blink).
    typing: bool,
    /// Текст при прошлом `change`.
    committed: String,
    caret_on: bool,
    blink: Option<Task<()>>,
    _focus: [Subscription; 2],
}

impl EventEmitter<FieldEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextInput {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> TextInput {
        // Поле — точка остановки Tab, как <input> (порядок — порядок отрисовки).
        let focus_handle = cx.focus_handle().tab_stop(true);
        let focus = cx.on_focus(&focus_handle, window, |this, window, cx| {
            this.committed = this.content.clone();
            this.restart_blink(window, cx);
            cx.notify();
        });
        let blur = cx.on_blur(&focus_handle, window, |this, window, cx| {
            // Отключённое поле в фокусе теряет фокус насовсем, как в Chromium: после
            // `set_disabled(false)` он не возвращается.
            if this.disabled && this.focus_handle.is_focused(window) {
                window.blur(cx);
            }
            this.blink = None;
            this.is_selecting = false;
            this.commit(cx);
            cx.emit(FieldEvent::Blur);
            cx.notify();
        });
        TextInput {
            focus_handle,
            kind: Kind::Text,
            style: None,
            placeholder: SharedString::default(),
            max_length: 0,
            align_right: false,
            align_center: false,
            disabled: false,
            invalid: false,
            content: String::new(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            last_x0: Pixels::ZERO,
            scroll_x: Pixels::ZERO,
            is_selecting: false,
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            committed: String::new(),
            caret_on: true,
            blink: None,
            _focus: [focus, blur],
        }
    }

    pub fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    pub fn style(mut self, style: fn(&Theme) -> FieldStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// `maxlength` в единицах UTF-16; 0 — без предела.
    pub fn max_length(mut self, max: usize) -> Self {
        self.max_length = max;
        self
    }

    /// `text-align: right` (`.gx-input--num`).
    pub fn align_right(mut self) -> Self {
        self.align_right = true;
        self
    }

    /// `text-align: center` (`.dex__time`).
    pub fn align_center(mut self) -> Self {
        self.align_center = true;
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.content = text.into();
        self.committed = self.content.clone();
        let end = self.content.len();
        self.selected_range = end..end;
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// `input.value`: у числового поля — число или пустая строка (badInput), иначе — текст.
    pub fn value(&self) -> String {
        match self.kind {
            Kind::Number { .. } => fields::number_value(&self.content),
            _ => self.content.clone(),
        }
    }

    /// `input.value = text`: каретка в конец, отмена забывается, событий нет (как у DOM).
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.committed = self.content.clone();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_x = Pixels::ZERO;
        self.undo.clear();
        self.redo.clear();
        self.typing = false;
        cx.notify();
    }

    /// `setSelectionRange` в байтах UTF-8 (обрезается по тексту и границам символов).
    pub fn set_selection(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let clamp = |at: usize| {
            let mut at = at.min(self.content.len());
            while !self.content.is_char_boundary(at) {
                at -= 1;
            }
            at
        };
        self.selected_range = clamp(range.start)..clamp(range.end.max(range.start));
        self.selection_reversed = false;
        self.typing = false;
        cx.notify();
    }

    pub fn selection(&self) -> Range<usize> {
        self.selected_range.clone()
    }

    /// То же значение — без кадра: места зовут это из своей отрисовки.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled != disabled {
            self.disabled = disabled;
            cx.notify();
        }
    }

    pub fn set_invalid(&mut self, invalid: bool, cx: &mut Context<Self>) {
        if self.invalid != invalid {
            self.invalid = invalid;
            cx.notify();
        }
    }

    pub fn set_placeholder(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = text.into();
        cx.notify();
    }

    /// Крестик поиска: пусто, `input`, `Cleared`, фокус в поле (`clear()` RunnerSearch.vue).
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.content.is_empty() {
            self.edit(0..self.content.len(), "", false, window, cx);
        }
        cx.emit(FieldEvent::Cleared);
        window.focus(&self.focus_handle, cx);
    }

    fn password(&self) -> bool {
        self.kind == Kind::Password
    }

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), window, cx);
        } else {
            self.move_to(self.selected_range.start, window, cx)
        }
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), window, cx);
        } else {
            self.move_to(self.selected_range.end, window, cx)
        }
    }

    fn word_left(&mut self, _: &WordLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.cursor_offset()), window, cx);
    }

    fn word_right(&mut self, _: &WordRight, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor_offset()), window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), window, cx);
    }

    fn select_word_left(
        &mut self,
        _: &SelectWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.previous_word(self.cursor_offset()), window, cx);
    }

    fn select_word_right(
        &mut self,
        _: &SelectWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.next_word(self.cursor_offset()), window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
        self.select_to(self.content.len(), window, cx)
    }

    fn home(&mut self, _: &Home, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
    }

    fn end(&mut self, _: &End, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), window, cx);
    }

    fn select_home(&mut self, _: &SelectHome, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, window, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), window, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selected_range.is_empty() {
            self.previous_boundary(self.cursor_offset())..self.cursor_offset()
        } else {
            self.selected_range.clone()
        };
        self.edit(range, "", true, window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selected_range.is_empty() {
            self.cursor_offset()..self.next_boundary(self.cursor_offset())
        } else {
            self.selected_range.clone()
        };
        self.edit(range, "", true, window, cx);
    }

    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = if self.selected_range.is_empty() {
            self.previous_word(self.cursor_offset())..self.cursor_offset()
        } else {
            self.selected_range.clone()
        };
        self.edit(range, "", true, window, cx);
    }

    fn delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = if self.selected_range.is_empty() {
            self.cursor_offset()..self.next_word(self.cursor_offset())
        } else {
            self.selected_range.clone()
        };
        self.edit(range, "", true, window, cx);
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.index_for_mouse_position(event.position);
        match event.click_count {
            2 => {
                let word = self.word_at(offset);
                self.move_to(word.start, window, cx);
                self.select_to(word.end, window, cx);
            }
            n if n >= 3 => self.select_all(&SelectAll, window, cx),
            _ => {
                self.is_selecting = true;
                if event.modifiers.shift {
                    self.select_to(offset, window, cx);
                } else {
                    self.move_to(offset, window, cx)
                }
            }
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), window, cx);
        }
    }

    /// Колесо над числовым полем в фокусе — шаг (NumberInputType::HandleWheelEvent), вверх —
    /// больше; иначе колесо уходит родителю.
    fn on_scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.kind, Kind::Number { .. }) || !self.focus_handle.is_focused(window) {
            return;
        }
        let dy = event.delta.pixel_delta(window.line_height()).y;
        if dy != Pixels::ZERO {
            self.step(if dy > Pixels::ZERO { 1 } else { -1 }, window, cx);
            cx.stop_propagation();
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let range = self
            .marked_range
            .clone()
            .unwrap_or(self.selected_range.clone());
        let text = self.sanitize(&range, &text);
        self.edit(range, &text, false, window, cx);
    }

    /// Из пароля не копируется и не вырезается (Chromium).
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.password() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.password() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.edit(self.selected_range.clone(), "", false, window, cx)
        }
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.undo.pop() {
            let current = self.restore(snapshot);
            self.redo.push(current);
            self.after_history(window, cx);
        }
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.redo.pop() {
            let current = self.restore(snapshot);
            self.undo.push(current);
            self.after_history(window, cx);
        }
    }

    fn restore(&mut self, snapshot: Snapshot) -> Snapshot {
        Snapshot {
            text: std::mem::replace(&mut self.content, snapshot.text),
            selection: std::mem::replace(&mut self.selected_range, snapshot.selection),
        }
    }

    fn after_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.typing = false;
        self.marked_range = None;
        self.selection_reversed = false;
        window.notify_text_input_changed(&self.focus_handle);
        self.restart_blink(window, cx);
        cx.emit(FieldEvent::Input);
        cx.notify();
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        cx.emit(FieldEvent::Enter);
    }

    fn step_up(&mut self, _: &StepUp, window: &mut Window, cx: &mut Context<Self>) {
        self.step(1, window, cx);
    }

    fn step_down(&mut self, _: &StepDown, window: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, window, cx);
    }

    /// Esc в поиске — пусто и `input`, без `Cleared` (SearchInputType::HandleKeydownEvent);
    /// Esc идёт и дальше: RunnerSearch по нему закрывает список.
    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.edit(0..self.content.len(), "", false, window, cx);
        cx.propagate();
    }

    /// stepUp/stepDown по `min`, `max`, `step`; Chromium сразу шлёт `input` и `change`.
    fn step(&mut self, direction: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Kind::Number { min, max, step } = &self.kind else {
            return;
        };
        if let Some(next) = fields::number_step(&self.content, min, max, step, direction) {
            self.edit(0..self.content.len(), &next, false, window, cx);
            self.commit(cx);
        }
    }

    /// `change`, если текст не тот, что при прошлом.
    fn commit(&mut self, cx: &mut Context<Self>) {
        if self.content != self.committed {
            self.committed = self.content.clone();
            cx.emit(FieldEvent::Change);
        }
    }

    /// Вставляемый текст по правилам `<input>`: перевод строки — пробел, у числа — только
    /// допустимые символы, `maxlength` — сколько осталось места (по UTF-16).
    fn sanitize(&self, range: &Range<usize>, text: &str) -> String {
        let mut text = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
        if matches!(self.kind, Kind::Number { .. }) {
            text = fields::filter_number_input(&text);
        }
        if self.max_length > 0 {
            let kept = self.content.encode_utf16().count()
                - self.content[range.clone()].encode_utf16().count();
            let room = self.max_length.saturating_sub(kept);
            text = if room == 0 {
                String::new()
            } else {
                fields::truncate_utf16(&text, room)
            };
        }
        text
    }

    /// Запомнить состояние для отмены: набор (`typing`) продолжает текущую группу.
    fn remember(&mut self, typing: bool) {
        if !(typing && self.typing) {
            self.undo.push(Snapshot {
                text: self.content.clone(),
                selection: self.selected_range.clone(),
            });
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.typing = typing;
    }

    /// Правка текста от пользователя: отмена, каретка за вставкой, `input`.
    fn edit(
        &mut self,
        range: Range<usize>,
        text: &str,
        typing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled || (range.is_empty() && text.is_empty()) {
            return;
        }
        self.remember(typing);
        self.content.replace_range(range.clone(), text);
        let end = range.start + text.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        self.restart_blink(window, cx);
        cx.emit(FieldEvent::Input);
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.caret_moved(window, cx);
    }

    fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.caret_moved(window, cx);
    }

    fn caret_moved(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.typing = false;
        window.notify_text_selection_changed(&self.focus_handle);
        self.restart_blink(window, cx);
        cx.notify()
    }

    /// Каретка видна сразу после действия и мигает, пока поле в фокусе.
    fn restart_blink(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.caret_on = true;
        if !self.focus_handle.is_focused(window) {
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

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if self.content.is_empty() {
            return 0;
        }
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        self.content_offset(line.closest_index_for_x(position.x - self.last_x0))
    }

    /// Текст на экране: у пароля — точки.
    fn display_text(&self) -> SharedString {
        if self.password() {
            BULLET
                .to_string()
                .repeat(self.content.chars().count())
                .into()
        } else {
            self.content.clone().into()
        }
    }

    /// Смещение в тексте → смещение в строке на экране.
    fn display_offset(&self, offset: usize) -> usize {
        if self.password() {
            self.content[..offset].chars().count() * BULLET.len_utf8()
        } else {
            offset
        }
    }

    fn content_offset(&self, index: usize) -> usize {
        if self.password() {
            self.content
                .char_indices()
                .nth(index / BULLET.len_utf8())
                .map_or(self.content.len(), |(at, _)| at)
        } else {
            index
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    /// Начало слова слева (у пароля слов нет — начало поля).
    fn previous_word(&self, offset: usize) -> usize {
        if self.password() {
            return 0;
        }
        self.content
            .split_word_bound_indices()
            .rev()
            .find(|(at, part)| *at < offset && is_word(part))
            .map_or(0, |(at, _)| at)
    }

    /// Начало следующего слова (Windows: пробелы после слова пропускаются), иначе конец.
    fn next_word(&self, offset: usize) -> usize {
        if self.password() {
            return self.content.len();
        }
        self.content
            .split_word_bound_indices()
            .find(|(at, part)| *at > offset && is_word(part))
            .map_or(self.content.len(), |(at, _)| at)
    }

    /// Слово под двойным щелчком: кусок между границами слов (за концом — последний).
    fn word_at(&self, offset: usize) -> Range<usize> {
        if self.password() {
            return 0..self.content.len();
        }
        let mut last = offset..offset;
        for (at, part) in self.content.split_word_bound_indices() {
            last = at..at + part.len();
            if last.contains(&offset) {
                break;
            }
        }
        last
    }
}

fn is_word(part: &str) -> bool {
    part.chars().any(char::is_alphanumeric)
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = self.range_from_utf16(&range_utf16);
        self.selection_reversed = false;
        self.caret_moved(window, cx);
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    /// Набор и конец составления IME: фильтры `<input>`, группа набора в отмене.
    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let text = self.sanitize(&range, new_text);
        if range.is_empty() && text.is_empty() {
            self.unmark_text(window, cx);
            return;
        }
        self.edit(range, &text, true, window, cx);
    }

    /// Составление IME: текст с подчёркиванием, вся композиция — в группе набора.
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        if self.marked_range.is_none() {
            self.remember(true);
        }
        self.content.replace_range(range.clone(), new_text);
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        let insertion_start_utf16 = self.offset_to_utf16(range.start);
        let inserted_length_utf16 = new_text.encode_utf16().count();
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                self.range_from_utf16(
                    &(insertion_start_utf16 + selected.start.min(inserted_length_utf16)
                        ..insertion_start_utf16 + selected.end.min(inserted_length_utf16)),
                )
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        window.notify_text_input_changed(&self.focus_handle);
        self.restart_blink(window, cx);
        cx.emit(FieldEvent::Input);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let (start, end) = match self
            .last_layout
            .as_ref()
            .filter(|_| !self.content.is_empty())
        {
            Some(line) => (
                line.x_for_index(self.display_offset(range.start)),
                line.x_for_index(self.display_offset(range.end)),
            ),
            None => (Pixels::ZERO, Pixels::ZERO),
        };
        Some(Bounds::from_corners(
            point(self.last_x0 + start, bounds.top()),
            point(self.last_x0 + end, bounds.bottom()),
        ))
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
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.last_bounds?.localize(&point)?;
        Some(self.offset_to_utf16(self.index_for_mouse_position(point)))
    }
}

/// Куски строки для `TextRun`: выделенное — белым, составляемое IME — подчёркнуто.
fn runs(
    base: &TextRun,
    selection: &Range<usize>,
    marked: Option<&Range<usize>>,
    selected: Hsla,
) -> Vec<TextRun> {
    let len = base.len;
    let mut cuts = vec![0, len, selection.start.min(len), selection.end.min(len)];
    if let Some(marked) = marked {
        cuts.extend([marked.start.min(len), marked.end.min(len)]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let inside =
        |range: &Range<usize>, piece: &[usize]| range.start <= piece[0] && piece[1] <= range.end;
    cuts.windows(2)
        .map(|piece| TextRun {
            len: piece[1] - piece[0],
            color: if inside(selection, piece) {
                selected
            } else {
                base.color
            },
            underline: marked
                .filter(|marked| inside(marked, piece))
                .map(|_| UnderlineStyle {
                    color: Some(base.color),
                    thickness: px(1.),
                    wavy: false,
                }),
            ..base.clone()
        })
        .collect()
}

/// Строка поля: текст (или подсказка), выделение, каретка; ставит обработчик ввода окна.
struct TextElement {
    input: Entity<TextInput>,
    color: Hsla,
    faint: Hsla,
    /// `line-height` поля (выделение во всю строку); 0 — по шрифту.
    line: f32,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    x0: Pixels,
    scroll: Pixels,
    caret: Option<Bounds<Pixels>>,
    selection: Option<Bounds<Pixels>>,
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        style.min_size.width = Pixels::ZERO.into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let style = window.text_style();
        let font = style.font();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let empty = input.content.is_empty();
        let focused = input.focus_handle.is_focused(window);
        let display = if empty {
            input.placeholder.clone()
        } else {
            input.display_text()
        };
        let selection = if focused && !empty {
            input.display_offset(input.selected_range.start)
                ..input.display_offset(input.selected_range.end)
        } else {
            0..0
        };
        let marked = input
            .marked_range
            .as_ref()
            .map(|range| input.display_offset(range.start)..input.display_offset(range.end));
        let base = TextRun {
            len: display.len(),
            font: font.clone(),
            color: if empty { self.faint } else { self.color },
            ..Default::default()
        };
        let runs = runs(&base, &selection, marked.as_ref(), gpui::white());
        let line = (!display.is_empty()).then(|| {
            window
                .text_system()
                .shape_line(display, font_size, &runs, None)
        });

        // Строка шире поля — прокрутка за кареткой (как внутренний редактор Chromium).
        let caret_width = px(1.);
        let width = line.as_ref().map_or(Pixels::ZERO, |line| line.width);
        let cursor = match line.as_ref().filter(|_| !empty) {
            Some(line) => line.x_for_index(input.display_offset(input.cursor_offset())),
            None => Pixels::ZERO,
        };
        let avail = bounds.size.width;
        let (shift, scroll) = if width + caret_width <= avail {
            let shift = if input.align_right {
                avail - width - caret_width
            } else if input.align_center {
                (avail - width - caret_width) / 2.
            } else {
                Pixels::ZERO
            };
            (shift, Pixels::ZERO)
        } else {
            let mut scroll = input.scroll_x;
            if cursor < scroll {
                scroll = cursor;
            } else if cursor + caret_width > scroll + avail {
                scroll = cursor + caret_width - avail;
            }
            let scroll = scroll.clamp(Pixels::ZERO, width + caret_width - avail);
            (-scroll, scroll)
        };
        let x0 = bounds.left() + shift;

        // Каретка — высота шрифта, выделение — строка `line-height` (или шрифт), по центру.
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        let glyphs =
            text_system.ascent(font_id, font_size) + text_system.descent(font_id, font_size).abs();
        let band = if self.line > 0. {
            zpx(zoom(window), self.line)
        } else {
            glyphs
        };
        let centered = |height: Pixels| bounds.top() + (bounds.size.height - height) / 2.;
        let selection = (!selection.is_empty()).then(|| {
            let line = line.as_ref().expect("выделение у пустой строки");
            Bounds::new(
                point(x0 + line.x_for_index(selection.start), centered(band)),
                size(
                    line.x_for_index(selection.end) - line.x_for_index(selection.start),
                    band,
                ),
            )
        });
        let caret_x = if empty {
            if input.align_right {
                bounds.right() - caret_width
            } else if input.align_center {
                bounds.left() + (avail - caret_width) / 2.
            } else {
                bounds.left()
            }
        } else {
            x0 + cursor
        };
        let caret =
            (focused && input.caret_on && input.selected_range.is_empty() && !input.disabled)
                .then(|| Bounds::new(point(caret_x, centered(glyphs)), size(caret_width, glyphs)));
        PrepaintState {
            line,
            x0,
            scroll,
            caret,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let input = self.input.read(cx);
        let (focus_handle, disabled) = (input.focus_handle.clone(), input.disabled);
        if !disabled {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.input.clone()),
                cx,
            );
        }
        let line = prepaint.line.take();
        // Внутренний редактор режет строку по коробке содержимого.
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(fill(selection, rgb(SELECTION)));
            }
            if let Some(line) = line.as_ref() {
                line.paint(
                    point(prepaint.x0, bounds.top()),
                    bounds.size.height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            }
            if let Some(caret) = prepaint.caret.take() {
                window.paint_quad(fill(caret, self.color));
            }
        });
        let (x0, scroll) = (prepaint.x0, prepaint.scroll);
        self.input.update(cx, |input, _| {
            input.last_layout = line;
            input.last_bounds = Some(bounds);
            input.last_x0 = x0;
            input.scroll_x = scroll;
        });
    }
}

/// Лупа поиска бегунов: свой SVG 0.4.9 (viewBox 16: круг r 4.75 в (7, 7) и хвост
/// 10.5→14, линия 1.6, концы круглые) — путём, без файла.
fn magnifier(color: Hsla) -> impl IntoElement + Styled {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let unit = bounds.size.width / 16.;
            let at = |x: f32, y: f32| point(bounds.left() + unit * x, bounds.top() + unit * y);
            let radius = point(unit * 4.75, unit * 4.75);
            let mut path = PathBuilder::default().with_style(PathStyle::Stroke(StrokeOptions {
                line_width: f32::from(unit * 1.6),
                line_cap: LineCap::Round,
                ..StrokeOptions::default()
            }));
            path.move_to(at(11.75, 7.));
            path.arc_to(radius, Pixels::ZERO, false, true, at(2.25, 7.));
            path.arc_to(radius, Pixels::ZERO, false, true, at(11.75, 7.));
            path.close();
            path.move_to(at(10.5, 10.5));
            path.line_to(at(14., 14.));
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let default = if self.kind == Kind::Search {
            FieldStyle::search
        } else {
            FieldStyle::gx
        };
        let style = self.style.unwrap_or(default)(theme);
        let text = theme.color("l-text");
        let faint = theme.color("l-text-faint");
        let accent = theme.color("accent");
        let zoom = zoom(window);
        let focused = self.focus_handle.is_focused(window);
        let search = self.kind == Kind::Search;
        let number = matches!(self.kind, Kind::Number { .. });
        let empty = self.content.is_empty();
        // Порядок правил CSS: :hover < :focus < .invalid.
        let border = if self.invalid {
            style.invalid_border
        } else if focused {
            style.focus_border
        } else {
            style.border
        };
        let hover_border = (!focused && !self.invalid).then_some(style.hover_border);
        let frame = if style.ring { 0. } else { 1. };
        let height = if style.height > 0. {
            style.height
        } else {
            2. * style.pad_y + style.line + 2. * frame
        };
        // `:placeholder-shown { padding-right: 12px }` у поиска: крестика ещё нет.
        let pad_right = if search && empty {
            12.
        } else {
            style.pad_right.unwrap_or(style.pad_x)
        };
        let shadows = if style.ring {
            vec![ring(zoom, 1., border)]
        } else {
            Vec::new()
        };
        // Кольцо фокуса и контур — рамка inset-тенью у блока вокруг поля: тень GPUI ложится и
        // под сам блок, а фон поля полупрозрачный (в CSS под блоком тени нет).
        let halo = move |inset: f32, width: f32, color: Hsla| {
            div()
                .absolute()
                .top(u(-inset))
                .left(u(-inset))
                .right(u(-inset))
                .bottom(u(-inset))
                .rounded(u(style.radius + inset))
                .shadow(vec![ring(zoom, width, color)])
        };
        let element = TextElement {
            input: cx.entity(),
            color: text,
            faint,
            line: style.line,
        };
        let meta = crate::live::Meta::new(
            "input",
            ElementId::NamedInteger("input".into(), cx.entity_id().as_u64()),
        )
        .label((!self.placeholder.is_empty()).then(|| self.placeholder.clone()))
        .disabled(self.disabled)
        .focused(focused);
        // Текст — только инспектору (копия строки на кадр); пароль — точками.
        #[cfg(any(test, feature = "dev-hooks"))]
        let meta = meta.value(if self.kind == Kind::Password {
            "•".repeat(self.content.chars().count())
        } else {
            self.content.clone()
        });
        let field = div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(u(height))
            .pl(u(style.pad_x))
            .pr(u(pad_right))
            .py(u(style.pad_y))
            .rounded(u(style.radius))
            .bg(style.bg)
            .when(!style.ring, |el| el.border_1().border_color(border))
            .shadow(shadows)
            .font_family(style.family)
            .text_size(u(style.size))
            .text_color(text)
            .when(self.disabled, |el| el.opacity(style.disabled_opacity))
            .when(!self.disabled, |el| {
                el.key_context(if number {
                    "TextInput NumberInput"
                } else if search {
                    "TextInput SearchInput"
                } else {
                    CONTEXT
                })
                .track_focus(&self.focus_handle)
                .cursor(CursorStyle::IBeam)
                .hover(move |s| {
                    let s = s.bg(style.hover_bg);
                    match hover_border {
                        Some(color) => s.border_color(color),
                        None => s,
                    }
                })
                .on_action(cx.listener(Self::backspace))
                .on_action(cx.listener(Self::delete))
                .on_action(cx.listener(Self::delete_word_left))
                .on_action(cx.listener(Self::delete_word_right))
                .on_action(cx.listener(Self::left))
                .on_action(cx.listener(Self::right))
                .on_action(cx.listener(Self::word_left))
                .on_action(cx.listener(Self::word_right))
                .on_action(cx.listener(Self::select_left))
                .on_action(cx.listener(Self::select_right))
                .on_action(cx.listener(Self::select_word_left))
                .on_action(cx.listener(Self::select_word_right))
                .on_action(cx.listener(Self::home))
                .on_action(cx.listener(Self::end))
                .on_action(cx.listener(Self::select_home))
                .on_action(cx.listener(Self::select_end))
                .on_action(cx.listener(Self::select_all))
                .on_action(cx.listener(Self::copy))
                .on_action(cx.listener(Self::cut))
                .on_action(cx.listener(Self::paste))
                .on_action(cx.listener(Self::undo))
                .on_action(cx.listener(Self::redo))
                .on_action(cx.listener(Self::enter))
                .on_action(cx.listener(Self::step_up))
                .on_action(cx.listener(Self::step_down))
                .on_action(cx.listener(Self::cancel))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                .on_mouse_move(cx.listener(Self::on_mouse_move))
                .on_scroll(cx.listener(Self::on_scroll))
            })
            .child(element)
            .when(search, |el| {
                el.child(
                    magnifier(if focused { accent } else { faint })
                        .absolute()
                        .left(u(11.))
                        .top(u((height - 14.) / 2.))
                        .size(u(14.)),
                )
                .when(!empty && !self.disabled, |el| {
                    el.child(
                        div()
                            .id("clear")
                            .absolute()
                            .right(u(6.))
                            .top(u((height - 24.) / 2.))
                            .size(u(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(u(style.radius))
                            .text_color(faint)
                            .cursor(CursorStyle::PointingHand)
                            .hover(move |s| s.text_color(text).bg(white(0.06)))
                            .child(fa_glyph("fas fa-times", 12.))
                            .button_text()
                            .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx))),
                    )
                })
            })
            .when_some(style.glow.filter(|_| focused), |el, (width, color)| {
                el.child(halo(width, width, color))
            })
            // outline-offset 2px: контур 2 px в 4 px от коробки.
            .when_some(style.outline.filter(|_| focused), |el, color| {
                el.child(halo(4., 2., color))
            });
        crate::live::mark(meta, field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, ScrollDelta, TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::rc::Rc;

    type Events = Rc<RefCell<Vec<FieldEvent>>>;

    /// Окно с одним полем в фокусе; события поля копятся в `Events`.
    fn field(
        cx: &mut TestAppContext,
        build: impl FnOnce(TextInput) -> TextInput + 'static,
    ) -> (Entity<TextInput>, &mut VisualTestContext, Events) {
        cx.update(crate::init);
        let (input, cx) = cx.add_window_view(|window, cx| build(TextInput::new(window, cx)));
        let events = Events::default();
        let sink = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&input, move |_, event: &FieldEvent, _| {
                sink.borrow_mut().push(event.clone())
            })
            .detach()
        });
        input.update_in(cx, |input, window, cx| {
            window.focus(&input.focus_handle, cx)
        });
        cx.run_until_parked();
        (input, cx, events)
    }

    fn text(input: &Entity<TextInput>, cx: &mut VisualTestContext) -> String {
        input.read_with(cx, |input, _| input.text().to_owned())
    }

    fn selection(input: &Entity<TextInput>, cx: &mut VisualTestContext) -> Range<usize> {
        input.read_with(cx, |input, _| input.selection())
    }

    /// Точка на экране перед символом `index` (по раскладке прошлого кадра).
    fn at(input: &Entity<TextInput>, cx: &mut VisualTestContext, index: usize) -> Point<Pixels> {
        input.read_with(cx, |input, _| {
            let line = input.last_layout.as_ref().expect("строка нарисована");
            let bounds = input.last_bounds.expect("поле нарисовано");
            point(
                input.last_x0 + line.x_for_index(input.display_offset(index)),
                bounds.center().y,
            )
        })
    }

    #[gpui::test]
    fn typing_goes_through_the_window_input_handler(cx: &mut TestAppContext) {
        let (input, cx, events) = field(cx, |input| input);
        cx.simulate_input("abc");
        assert_eq!(text(&input, cx), "abc");
        assert_eq!(*events.borrow(), vec![FieldEvent::Input; 3]);
        cx.simulate_keystrokes("enter");
        assert_eq!(
            events.borrow()[3..],
            [FieldEvent::Change, FieldEvent::Enter]
        );
        // Без правки Enter не шлёт change.
        cx.simulate_keystrokes("enter");
        assert_eq!(events.borrow()[5..], [FieldEvent::Enter]);
    }

    #[gpui::test]
    fn keys_move_and_select_by_characters_and_words(cx: &mut TestAppContext) {
        let (input, cx, _) = field(cx, |input| input.with_text("hello world"));
        cx.simulate_keystrokes("shift-left shift-left");
        assert_eq!(selection(&input, cx), 9..11);
        cx.simulate_keystrokes("ctrl-shift-left");
        assert_eq!(selection(&input, cx), 6..11);
        cx.simulate_keystrokes("ctrl-left");
        assert_eq!(selection(&input, cx), 0..0);
        cx.simulate_keystrokes("ctrl-right");
        assert_eq!(selection(&input, cx), 6..6);
        cx.simulate_keystrokes("shift-end");
        assert_eq!(selection(&input, cx), 6..11);
        cx.simulate_keystrokes("shift-home");
        assert_eq!(selection(&input, cx), 0..6);
        cx.simulate_keystrokes("ctrl-a");
        assert_eq!(selection(&input, cx), 0..11);
        cx.simulate_keystrokes("end ctrl-backspace");
        assert_eq!(text(&input, cx), "hello ");
        cx.simulate_keystrokes("home ctrl-delete");
        assert_eq!(text(&input, cx), "");
    }

    #[gpui::test]
    fn mouse_drag_double_and_triple_click_select(cx: &mut TestAppContext) {
        let (input, cx, _) = field(cx, |input| input.with_text("ab cd ef"));
        let (from, to) = (at(&input, cx, 1), at(&input, cx, 4));
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        assert_eq!(selection(&input, cx), 1..4);
        // Мышь отпущена: движение больше не выделяет.
        let past = at(&input, cx, 6);
        cx.simulate_mouse_move(past, None, Modifiers::default());
        assert_eq!(selection(&input, cx), 1..4);

        let word = at(&input, cx, 4);
        for click_count in [1, 2] {
            cx.simulate_event(MouseDownEvent {
                button: MouseButton::Left,
                position: word,
                modifiers: Modifiers::default(),
                click_count,
                first_mouse: false,
            });
            cx.simulate_mouse_up(word, MouseButton::Left, Modifiers::default());
        }
        assert_eq!(selection(&input, cx), 3..5);
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: word,
            modifiers: Modifiers::default(),
            click_count: 3,
            first_mouse: false,
        });
        assert_eq!(selection(&input, cx), 0..8);
    }

    #[gpui::test]
    fn clipboard_copy_cut_paste_and_line_breaks(cx: &mut TestAppContext) {
        let (input, cx, _) = field(cx, |input| input.with_text("copy me"));
        cx.simulate_keystrokes("ctrl-a ctrl-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("copy me".to_owned())
        );
        cx.simulate_keystrokes("ctrl-x");
        assert_eq!(text(&input, cx), "");
        cx.simulate_keystrokes("ctrl-v ctrl-v");
        assert_eq!(text(&input, cx), "copy mecopy me");
        // Перевод строки в однострочном поле — пробел.
        cx.write_to_clipboard(ClipboardItem::new_string("a\r\nb\nc".to_owned()));
        cx.simulate_keystrokes("ctrl-a ctrl-v");
        assert_eq!(text(&input, cx), "a b c");
    }

    #[gpui::test]
    fn undo_and_redo_group_typing_like_chromium(cx: &mut TestAppContext) {
        let (input, cx, _) = field(cx, |input| input);
        cx.simulate_input("ab");
        cx.simulate_keystrokes("left");
        cx.simulate_input("c");
        assert_eq!(text(&input, cx), "acb");
        cx.simulate_keystrokes("backspace backspace");
        assert_eq!(text(&input, cx), "b");
        // Набор и удаления после перемещения каретки — одна группа.
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text(&input, cx), "ab");
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text(&input, cx), "");
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text(&input, cx), "");
        cx.simulate_keystrokes("ctrl-y");
        assert_eq!(text(&input, cx), "ab");
        cx.simulate_keystrokes("ctrl-shift-z");
        assert_eq!(text(&input, cx), "b");
        // Отмена возвращает и каретку; вставка — отдельный шаг, после неё повтора нет.
        cx.write_to_clipboard(ClipboardItem::new_string("x".to_owned()));
        cx.simulate_keystrokes("ctrl-z ctrl-v");
        assert_eq!(text(&input, cx), "axb");
        cx.simulate_keystrokes("ctrl-y");
        assert_eq!(text(&input, cx), "axb");
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text(&input, cx), "ab");
    }

    #[gpui::test]
    fn ime_composition_is_marked_and_undone_as_one_step(cx: &mut TestAppContext) {
        let (input, cx, events) = field(cx, |input| input.with_text("A"));
        input.update_in(cx, |input, window, cx| {
            input.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
            assert_eq!(input.text(), "Ani");
            assert_eq!(input.marked_range, Some(1..3));
            assert_eq!(input.marked_text_range(window, cx), Some(1..3));
            input.replace_and_mark_text_in_range(None, "nih", Some(3..3), window, cx);
            assert_eq!(input.marked_range, Some(1..4));
            assert_eq!(input.selection(), 4..4);
            input.replace_text_in_range(None, "你好", window, cx);
            assert_eq!(input.text(), "A你好");
            assert_eq!(input.marked_range, None);
            assert_eq!(input.selection(), 7..7);
            // UTF-16 для IME: «你好» — две единицы.
            assert_eq!(input.text_length_utf16(window, cx), Some(3));
        });
        assert_eq!(*events.borrow(), vec![FieldEvent::Input; 3]);
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text(&input, cx), "A");
    }

    #[gpui::test]
    fn number_filters_input_and_steps_within_limits(cx: &mut TestAppContext) {
        let (input, cx, events) = field(cx, |input| {
            input.kind(Kind::Number {
                min: "0".into(),
                max: "10".into(),
                step: "0.5".into(),
            })
        });
        cx.simulate_input("2a.5x");
        assert_eq!(text(&input, cx), "2.5");
        events.borrow_mut().clear();
        cx.simulate_keystrokes("up");
        assert_eq!(text(&input, cx), "3");
        assert_eq!(*events.borrow(), [FieldEvent::Input, FieldEvent::Change]);
        cx.simulate_keystrokes("down down");
        assert_eq!(input.read_with(cx, |input, _| input.value()), "2");
        // Колесо над полем в фокусе: вверх — больше.
        let center = at(&input, cx, 0);
        cx.simulate_event(ScrollEvent {
            position: center,
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(text(&input, cx), "2.5");
        input.update(cx, |input, cx| input.set_text("9.5", cx));
        cx.simulate_keystrokes("up up");
        assert_eq!(text(&input, cx), "10");
    }

    #[gpui::test]
    fn up_and_down_leave_text_fields(cx: &mut TestAppContext) {
        let (input, cx, events) = field(cx, |input| input.with_text("7"));
        cx.simulate_keystrokes("up down");
        assert_eq!(text(&input, cx), "7");
        assert!(events.borrow().is_empty());
    }

    #[gpui::test]
    fn max_length_counts_utf16_and_cuts_paste(cx: &mut TestAppContext) {
        let (input, cx, _) = field(cx, |input| input.max_length(5));
        cx.simulate_input("abcdefg");
        assert_eq!(text(&input, cx), "abcde");
        cx.simulate_keystrokes("left left shift-left");
        cx.write_to_clipboard(ClipboardItem::new_string("XYZ".to_owned()));
        cx.simulate_keystrokes("ctrl-v");
        assert_eq!(text(&input, cx), "abXde");
        input.update(cx, |input, cx| input.set_text("abc", cx));
        cx.write_to_clipboard(ClipboardItem::new_string("🙂🙂".to_owned()));
        cx.simulate_keystrokes("ctrl-v");
        assert_eq!(text(&input, cx), "abc🙂");
    }

    #[gpui::test]
    fn password_is_masked_and_not_copied(cx: &mut TestAppContext) {
        let (input, cx, _) = field(cx, |input| input.kind(Kind::Password).with_text("pa🙂s"));
        input.read_with(cx, |input, _| {
            assert_eq!(input.display_text(), "••••");
            assert_eq!(
                input.display_offset(input.text().len()),
                4 * BULLET.len_utf8()
            );
            assert_eq!(input.content_offset(2 * BULLET.len_utf8()), 2);
        });
        cx.write_to_clipboard(ClipboardItem::new_string("old".to_owned()));
        cx.simulate_keystrokes("ctrl-a ctrl-c ctrl-x");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("old".to_owned())
        );
        assert_eq!(text(&input, cx), "pa🙂s");
        // Щелчок по третьей точке — перед третьим символом.
        let third = at(&input, cx, 2);
        cx.simulate_click(third, Modifiers::default());
        assert_eq!(selection(&input, cx), 2..2);
        cx.simulate_keystrokes("ctrl-right");
        assert_eq!(selection(&input, cx), 7..7);
    }

    #[gpui::test]
    fn search_clear_button_empties_and_reports(cx: &mut TestAppContext) {
        let (input, cx, events) = field(cx, |input| input.kind(Kind::Search).with_text("Fenix"));
        // Крестик: 24 px у правого края, 6 px от него (коробка текста кончается за 32 px).
        let cross = input.read_with(cx, |input, _| {
            let bounds = input.last_bounds.expect("поле нарисовано");
            point(bounds.right() + zpx(1., 14.), bounds.center().y)
        });
        cx.simulate_click(cross, Modifiers::default());
        assert_eq!(text(&input, cx), "");
        assert_eq!(*events.borrow(), [FieldEvent::Input, FieldEvent::Cleared]);
    }

    #[gpui::test]
    fn disabled_field_ignores_typing(cx: &mut TestAppContext) {
        let (input, cx, events) = field(cx, |input| input.with_text("x"));
        input.update(cx, |input, cx| input.set_disabled(true, cx));
        cx.simulate_input("a");
        input.update_in(cx, |input, window, cx| {
            input.replace_text_in_range(None, "b", window, cx);
            input.replace_and_mark_text_in_range(None, "c", None, window, cx);
        });
        assert_eq!(text(&input, cx), "x");
        assert!(events.borrow().is_empty());
    }

    #[gpui::test]
    fn escape_clears_search(cx: &mut TestAppContext) {
        let (input, cx, events) = field(cx, |input| input.kind(Kind::Search).with_text("Fenix"));
        cx.simulate_keystrokes("escape");
        assert_eq!(text(&input, cx), "");
        assert_eq!(*events.borrow(), [FieldEvent::Input]);
    }

    #[gpui::test]
    fn escape_leaves_text_field(cx: &mut TestAppContext) {
        let (input, cx, events) = field(cx, |input| input.with_text("Fenix"));
        cx.simulate_keystrokes("escape");
        assert_eq!(text(&input, cx), "Fenix");
        assert!(events.borrow().is_empty());
    }

    #[gpui::test]
    fn disabling_focused_field_blurs_for_good(cx: &mut TestAppContext) {
        let (input, cx, events) = field(cx, |input| input);
        // Без активного окна GPUI не шлёт фокус и blur.
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.simulate_input("z");
        input.update(cx, |input, cx| input.set_disabled(true, cx));
        cx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            [FieldEvent::Input, FieldEvent::Change, FieldEvent::Blur]
        );
        input.update(cx, |input, cx| input.set_disabled(false, cx));
        cx.run_until_parked();
        let focused = input.update_in(cx, |input, window, _| input.focus_handle.is_focused(window));
        assert!(!focused);
        assert_eq!(events.borrow().len(), 3);
    }

    #[test]
    fn runs_split_selection_and_composition() {
        let base = TextRun {
            len: 6,
            font: gpui::font(RUBIK),
            color: gpui::black(),
            ..Default::default()
        };
        let white = gpui::white();
        let pieces = runs(&base, &(1..3), Some(&(2..5)), white);
        let summary: Vec<_> = pieces
            .iter()
            .map(|run| (run.len, run.color == white, run.underline.is_some()))
            .collect();
        assert_eq!(
            summary,
            [
                (1, false, false),
                (1, true, false),
                (1, true, true),
                (2, false, true),
                (1, false, false)
            ]
        );
        assert_eq!(runs(&base, &(0..0), None, white).len(), 1);
    }
}
