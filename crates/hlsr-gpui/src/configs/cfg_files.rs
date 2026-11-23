//! Вкладка «Файлы cfg» (`GameCfgFiles.vue` 0.4.9): слева список cfg сборки с поиском
//! (основной файл игрока — первым, с меткой), справа редактор: строка пути («не сохранено»,
//! поиск, памятка клавиш), поиск и замена, переход к строке, текст на [`CodeArea`] (F10), подвал
//! (итог, позиция курсора, «Вернуть», «Сохранить»). Логика — `hlsr_ui::controllers::game_cfg_files`;
//! отмена и повтор ведёт сам [`CodeArea`] (правки программы — его шаги `apply_edit`).
//!
//! Несохранённый текст сообщается сессии настроек (`@dirty`), сохранение — `@saved`
//! ([`Session::on_cfg_file_saved`]); `openAt` — переход из «Проверки».

use crate::icons::fa_glyph;
use crate::kit::button::{Look, button};
use crate::kit::input::FieldStyle;
use crate::kit::{CodeArea, CodeEvent, FieldEvent, TextInput, gx};
use crate::layer::{TipExt, anchor_probe, pop_plain};
use crate::motion::{Timing, hover, mix};
use crate::scale::u;
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::text::{MONO, RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Anchor, AnyElement, App, Bounds, Context, ElementId, Entity, FocusHandle, Focusable,
    FontWeight, IntoElement, KeyDownEvent, ParentElement, Pixels, Render, Styled, Subscription,
    Window, anchored, deferred, div, hsla, point, prelude::*, relative,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::game_cfg_files::{
    self as cf, CfgFiles, Effect, Field, FindFlag, Focus, Key, OpenStep, Request, SHORTCUTS,
    base_name, byte_at, dir_name, utf16_at,
};
use hlsr_ui::controllers::game_settings::{FilesSlot, Session};
use hlsr_ui::services::Services;
use serde_json::Value;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

/// Редактор cfg одной сборки.
pub struct GameCfgFiles {
    services: Services,
    session: Rc<Session>,
    language: Language,
    cfg: CfgFiles,
    code: Entity<CodeArea>,
    search: Entity<TextInput>,
    find: Entity<TextInput>,
    replace: Entity<TextInput>,
    goto: Entity<TextInput>,
    list_scroll: Scroll,
    /// Окно видимости списка файлов (`crate::rows`).
    list_rows: crate::rows::Rows,
    keys_toggle: FocusHandle,
    keys_probe: Rc<Cell<Bounds<Pixels>>>,
    /// `@dirty`, сообщённый сессии последним.
    reported_dirty: bool,
    /// Рамка ошибки выражения у поиска и «только чтение» текста — как на прошлом кадре.
    find_invalid: bool,
    read_only: bool,
    /// `openAt(file, line)` ждёт список или чтение файла.
    open_at: Option<(String, usize)>,
    _subs: Vec<Subscription>,
}

/// Клавиша GPUI → `event.key` DOM для контроллера.
fn dom_key(event: &KeyDownEvent) -> Key {
    let stroke = &event.keystroke;
    let name = match stroke.key.as_str() {
        "tab" => "Tab".to_owned(),
        "escape" => "Escape".to_owned(),
        "enter" => "Enter".to_owned(),
        "f3" => "F3".to_owned(),
        "up" => "ArrowUp".to_owned(),
        "down" => "ArrowDown".to_owned(),
        "backspace" => "Backspace".to_owned(),
        "delete" => "Delete".to_owned(),
        other => stroke.key_char.clone().unwrap_or_else(|| other.to_owned()),
    };
    let m = stroke.modifiers;
    Key {
        key: name,
        code: if stroke.key == "/" {
            "Slash".to_owned()
        } else {
            String::new()
        },
        ctrl: m.control,
        shift: m.shift,
        alt: m.alt,
        meta: m.platform,
    }
}

/// Разница двух текстов одной правкой: участок старого (байты) и что вставить.
fn diff<'a>(old: &str, new: &'a str) -> (Range<usize>, &'a str) {
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
        start -= 1;
    }
    let tail = old[start..]
        .bytes()
        .rev()
        .zip(new[start..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (mut old_end, mut new_end) = (old.len() - tail, new.len() - tail);
    while !old.is_char_boundary(old_end) || !new.is_char_boundary(new_end) {
        old_end += 1;
        new_end += 1;
    }
    (start..old_end, &new[start..new_end])
}

fn white(alpha: f32) -> gpui::Hsla {
    hsla(0., 0., 1., alpha)
}

impl GameCfgFiles {
    pub fn new(
        services: &Services,
        session: Rc<Session>,
        language: Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let t = |key: &str| hlsr_ui::tr(language, key, &[]);
        let code = cx.new(|cx| CodeArea::new(window, cx));
        let search = cx.new(|cx| TextInput::new(window, cx).placeholder(t("#L_CF_SEARCH")));
        let find = cx.new(|cx| {
            TextInput::new(window, cx)
                .style(find_style)
                .placeholder(t("#L_CF_FIND"))
        });
        let replace = cx.new(|cx| {
            TextInput::new(window, cx)
                .style(FieldStyle::gx_mono)
                .placeholder(t("#L_CF_REPLACE_WITH"))
        });
        let goto = cx.new(|cx| {
            TextInput::new(window, cx)
                .style(goto_style)
                .kind(crate::kit::Kind::Number {
                    min: "1".into(),
                    max: "".into(),
                    step: "".into(),
                })
        });
        let subs = vec![
            cx.subscribe_in(
                &code,
                window,
                |this, code, event: &CodeEvent, window, cx| {
                    let code = code.read(cx);
                    let text = code.text();
                    let sel = code.selection();
                    let sel16 = (utf16_at(text, sel.start), utf16_at(text, sel.end));
                    match event {
                        CodeEvent::Input => {
                            let text = text.to_owned();
                            this.cfg.user_edit(&text, sel16);
                        }
                        CodeEvent::Select => this.cfg.user_select(sel16),
                    }
                    this.changed(window, cx);
                },
            ),
            cx.subscribe_in(&search, window, |this, input, event: &FieldEvent, _, cx| {
                if matches!(event, FieldEvent::Input) {
                    this.cfg.filter = input.read(cx).text().to_owned();
                    cx.notify();
                }
            }),
            cx.subscribe_in(
                &find,
                window,
                |this, input, event: &FieldEvent, window, cx| match event {
                    FieldEvent::Input => {
                        this.cfg.find.query = input.read(cx).text().to_owned();
                        cx.notify();
                    }
                    FieldEvent::Enter => this.key(Field::Find, Key::new("Enter"), window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &replace,
                window,
                |this, input, event: &FieldEvent, window, cx| match event {
                    FieldEvent::Input => {
                        this.cfg.find.replacement = input.read(cx).text().to_owned();
                    }
                    FieldEvent::Enter => this.key(Field::Replace, Key::new("Enter"), window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &goto,
                window,
                |this, input, event: &FieldEvent, window, cx| match event {
                    FieldEvent::Input => {
                        this.cfg.goto_line = input.read(cx).text().to_owned();
                    }
                    FieldEvent::Enter => this.key(Field::Goto, Key::new("Enter"), window, cx),
                    _ => {}
                },
            ),
        ];
        GameCfgFiles {
            services: services.clone(),
            session,
            language,
            cfg: CfgFiles::new(language),
            code,
            search,
            find,
            replace,
            goto,
            list_scroll: Scroll::new(),
            list_rows: Default::default(),
            keys_toggle: cx.focus_handle().tab_stop(true),
            keys_probe: Rc::default(),
            reported_dirty: false,
            find_invalid: false,
            read_only: false,
            open_at: None,
            _subs: subs,
        }
    }

    fn t(&self, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(self.language, key, args)
    }

    pub fn dirty(&self) -> bool {
        self.cfg.dirty()
    }

    /// Место редактора от сессии: игра и сборка (`props`), `None` — вкладка ушла (`unmount`:
    /// текст пропал, флаг несохранённого не залипает).
    pub fn set_target(
        &mut self,
        slot: Option<&FilesSlot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match slot {
            Some(slot) => {
                let build = slot
                    .build
                    .as_ref()
                    .map_or(Value::Null, |b| Value::String(b.clone()));
                if let Some(request) = self.cfg.set_target(&slot.game_id, build) {
                    self.code.update(cx, |code, cx| code.set_text("", cx));
                    self.run(request, window, cx);
                }
            }
            None => {
                self.cfg = CfgFiles::new(self.language);
                self.open_at = None;
                self.code.update(cx, |code, cx| code.set_text("", cx));
            }
        }
        self.changed(window, cx);
    }

    /// Вызов канала и ответ контроллеру.
    fn run(&mut self, request: Request, window: &mut Window, cx: &mut Context<Self>) {
        let (channel, args) = request.invoke();
        let reply = self.services.invoke(channel, args);
        let saved = match &request {
            Request::Write { content, .. } => Some(content.clone()),
            _ => None,
        };
        cx.spawn_in(window, async move |this, cx| {
            let reply = reply.await;
            this.update_in(cx, |this, window, cx| {
                match &request {
                    Request::List { .. } => {
                        this.cfg.finish_load(reply);
                        if let Some((file, line)) = this.open_at.clone() {
                            this.open_path(&file, Some(line), window, cx);
                        }
                    }
                    Request::Read { path, .. } => {
                        let effect = this.cfg.finish_read(reply);
                        this.apply(effect, window, cx);
                        if let Some((file, line)) = this.open_at.take()
                            && file == *path
                        {
                            this.reveal_line(line, window, cx);
                        }
                    }
                    Request::Write { .. } => {
                        this.cfg
                            .finish_save(reply, saved.as_deref().unwrap_or_default());
                        let ok = this.cfg.notice.as_ref().is_some_and(|n| !n.error);
                        this.changed(window, cx);
                        if ok {
                            this.session.on_cfg_file_saved();
                        }
                    }
                }
                this.changed(window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = self.cfg.begin_load();
        self.run(request, window, cx);
        cx.notify();
    }

    /// Нажат файл (`open`): правки открытого — сначала вопрос (`confirmLeave`).
    fn open_path(
        &mut self,
        path: &str,
        line: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.cfg.open(path) {
            OpenStep::Nothing => {
                if let Some(line) = line {
                    self.open_at = None;
                    self.reveal_line(line, window, cx);
                }
            }
            OpenStep::Read => {
                self.open_at = line.map(|line| (path.to_owned(), line));
                let request = self.cfg.read(path);
                self.run(request, window, cx);
            }
            OpenStep::Confirm => {
                let Some(shell) =
                    crate::shell::root::ShellContext::get(cx).map(|c| c.shell.clone())
                else {
                    return;
                };
                let answer = shell.ask_confirm(hlsr_ui::controllers::shell::Confirm {
                    text: self.t("#L_CF_CONFIRM_DISCARD", &[]),
                    ..Default::default()
                });
                let path = path.to_owned();
                cx.spawn_in(window, async move |this, cx| {
                    if !answer.await.accepted() {
                        return;
                    }
                    this.update_in(cx, |this, window, cx| {
                        this.open_at = line.map(|line| (path.clone(), line));
                        let request = this.cfg.read(&path);
                        this.run(request, window, cx);
                    })
                    .ok();
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// `openAt(file, line)` из «Проверки»: дождаться списка (редактор ещё не на месте или
    /// читает его), открыть файл, курсор на строке.
    pub fn open_at(
        &mut self,
        file: &str,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cfg.game_id().is_empty() || self.cfg.loading {
            self.open_at = Some((file.to_owned(), line));
            return;
        }
        self.open_path(file, Some(line), window, cx);
    }

    fn reveal_line(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((start, end)) = self.cfg.line_span(line) {
            let effect = self.cfg.reveal(start, end, &cf::View::default());
            self.apply(effect, window, cx);
        }
        let focus = self.code.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    /// Нажатие в редакторе: обработчик поля, затем `onEditorKey`.
    fn key(&mut self, field: Field, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let outcome = self.cfg.key(field, &key, &cf::View::default());
        if outcome.handled {
            window.prevent_default();
        }
        if outcome.stop || outcome.handled {
            cx.stop_propagation();
        }
        self.apply(outcome.effect, window, cx);
    }

    /// Что сделать с полем после действия контроллера.
    fn apply(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.cfg.text().to_owned();
        let (s16, e16) = self.cfg.selection();
        let sel = byte_at(&text, s16)..byte_at(&text, e16);
        if effect.apply {
            self.code.update(cx, |code, cx| {
                if effect.scroll_home || code.text().is_empty() && !text.is_empty() {
                    code.set_text(text.clone(), cx);
                    code.set_selection(sel.clone(), cx);
                } else if code.text() != text {
                    let (range, insert) = diff(code.text(), &text);
                    let insert = insert.to_owned();
                    code.apply_edit(range, &insert, sel.clone(), window, cx);
                } else {
                    code.set_selection(sel.clone(), cx);
                }
            });
        }
        if effect.scroll.is_some() {
            self.code
                .update(cx, |code, cx| code.reveal(sel.clone(), window, cx));
        }
        // Запрос из выделения и номер строки ставит только открытие поиска и перехода; иначе
        // поле ведёт свой текст само (нажатия могут прийти раньше его события `input`).
        if matches!(effect.focus, Focus::Find | Focus::Replace | Focus::Goto) {
            self.sync_inputs(cx);
        }
        match effect.focus {
            Focus::None => {}
            Focus::Text => {
                let focus = self.code.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }
            Focus::Find | Focus::Replace | Focus::Goto => {
                let input = match effect.focus {
                    Focus::Find => &self.find,
                    Focus::Replace => &self.replace,
                    _ => &self.goto,
                }
                .clone();
                let focus = input.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                if effect.select {
                    input.update(cx, |input, cx| {
                        let len = input.text().len();
                        input.set_selection(0..len, cx);
                    });
                }
            }
            Focus::KeysToggle => window.focus(&self.keys_toggle, cx),
        }
        if effect.save {
            self.save(window, cx);
        }
        self.changed(window, cx);
    }

    /// Поля поиска, замены и строки — за контроллером (запрос из выделения, номер строки).
    fn sync_inputs(&mut self, cx: &mut Context<Self>) {
        for (input, text) in [
            (&self.find, self.cfg.find.query.clone()),
            (&self.replace, self.cfg.find.replacement.clone()),
            (&self.goto, self.cfg.goto_line.clone()),
        ] {
            input.update(cx, |input, cx| {
                if input.text() != text {
                    input.set_text(text, cx);
                }
            });
        }
        let lines = self.cfg.lines().to_string();
        self.goto.update(cx, |input, cx| {
            input.set_placeholder(format!("1–{lines}"), cx)
        });
    }

    /// Флаг несохранённого — сессии; вид — заново.
    fn changed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let dirty = self.cfg.dirty();
        if dirty != self.reported_dirty {
            self.reported_dirty = dirty;
            self.session.set_cfg_files_dirty(dirty);
        }
        let reading = self.cfg.reading;
        if self.read_only != reading {
            self.read_only = reading;
            self.code
                .update(cx, |code, cx| code.set_read_only(reading, cx));
        }
        cx.notify();
    }

    /// «Сохранить», Ctrl+S; вопрос «Сохранить» на странице.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(request) = self.cfg.begin_save() {
            self.run(request, window, cx);
        }
        cx.notify();
    }

    /// «Вернуть»; «Отмена» и «Не сохранять» страницы.
    pub fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let effect = self.cfg.revert();
        if effect.apply {
            let text = self.cfg.text().to_owned();
            let end = text.len();
            self.code.update(cx, |code, cx| {
                if code.text() != text {
                    code.set_text(text, cx);
                }
                code.set_selection(end..end, cx);
            });
        }
        self.changed(window, cx);
    }

    fn file_row(
        &self,
        path: &str,
        active: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = ElementId::Name(format!("cf-file-{path}").into());
        let inner = id.clone();
        let (text, faint, accent) = (
            theme.color("l-text"),
            theme.color("l-text-faint"),
            theme.color("accent"),
        );
        let title = base_name(path).to_owned();
        let dir = dir_name(path).to_owned();
        let player = self.cfg.is_player_file(path);
        let (badge_text, badge_tip) = (
            self.t("#L_CF_PLAYER_FILE", &[]),
            self.t("#L_CF_PLAYER_FILE_HINT", &[]),
        );
        let badge = move || {
            player.then(|| {
                div()
                    .id("cf-badge")
                    .flex_none()
                    .px(u(6.))
                    .py(u(1.))
                    .rounded_full()
                    .bg(accent.opacity(0.22))
                    .border_1()
                    .border_color(accent.opacity(0.55))
                    .font_family(RUBIK)
                    .text_px(10., 14.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text)
                    .tip(badge_tip.clone())
                    .child(badge_text.clone())
            })
        };
        let weak = cx.weak_entity();
        let path = path.to_owned();
        hover(id, Timing::fast(), move |t| {
            let (weak, path_click) = (weak.clone(), path.clone());
            div()
                .id(inner.clone())
                .flex()
                .flex_col()
                .gap(u(1.))
                .w_full()
                .px(u(9.))
                .py(u(6.))
                .rounded(u(7.))
                .bg(if active {
                    accent.opacity(0.16)
                } else {
                    white(0.05 * t)
                })
                .text_color(text)
                .cursor_pointer()
                .tip(path.clone())
                .button_text()
                .on_click(move |_, window, cx| {
                    weak.update(cx, |this, cx| this.open_path(&path_click, None, window, cx))
                        .ok();
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(6.))
                        .min_w_0()
                        .font_family(MONO)
                        .text_size(u(12.5))
                        .line_height(u(20.))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(title.clone()),
                        )
                        .children(badge()),
                )
                .when(!dir.is_empty(), |el| {
                    el.child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_px(11., 18.)
                            .text_color(faint)
                            .child(dir.clone()),
                    )
                })
        })
        .into_any_element()
    }

    fn list(&self, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = self.cfg.list_state();
        let current = self.cfg.current.as_ref().map(|c| c.path.clone());
        let state_line = |error: bool, text: String| {
            div()
                .text_px(12., 12. * 1.4)
                .text_color(if error {
                    gpui::rgb(0xff8a96).into()
                } else {
                    theme.color("l-text-faint")
                })
                .child(text)
                .into_any_element()
        };
        let body = match state {
            Some((error, text)) => state_line(error, text),
            None => {
                // Окно видимости (`crate::rows`): в папке бывают сотни cfg.
                let files = self.cfg.shown_files();
                let keys = files.iter().map(|path| (*path).to_owned()).collect();
                let slots = self.list_rows.plan(&self.list_scroll, keys, 2., window, cx);
                let rows: Vec<AnyElement> = slots
                    .into_iter()
                    .map(|slot| match slot {
                        crate::rows::Slot::Row(index) => {
                            let path = files[index];
                            let active = current.as_deref() == Some(path);
                            crate::snap::target(
                                format!("cf-file-{index}"),
                                self.file_row(path, active, theme, cx),
                            )
                        }
                        crate::rows::Slot::Space(height) => {
                            div().flex_none().h(height).into_any_element()
                        }
                    })
                    .collect();
                ScrollArea::new("cf-files", &self.list_scroll)
                    .bar(Bar::Fields)
                    .contain()
                    .flex_1()
                    .min_h_0()
                    .child(self.list_rows.measure(
                        &self.list_scroll,
                        div().flex().flex_col().gap(u(2.)).children(rows),
                    ))
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(u(8.))
            .flex_none()
            .w(relative(0.38 / 1.38))
            .min_w(u(150.))
            .min_h_0()
            .p(u(10.))
            .border_r_1()
            .border_color(theme.color("l-border"))
            .child(self.search.clone())
            .child(body)
            .when(self.cfg.truncated, |el| {
                el.child(state_line(false, self.t("#L_CF_TRUNCATED", &[])))
            })
            .into_any_element()
    }

    /// `.cf__tool`: кнопка 30 × 30 строки пути; `on` — открытый поиск или памятка.
    fn tool(
        &self,
        (id, icon, label): (&'static str, &'static str, String),
        on: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
        act: fn(&mut Self, &mut Window, &mut Context<Self>),
    ) -> AnyElement {
        let weak = cx.weak_entity();
        let tool = button(id, Look::Gx)
            .label(label)
            .w(u(30.))
            .px_0()
            .when(on, |el| {
                el.border_color(mix(theme.color("accent"), theme.color("l-border"), 0.45))
                    .text_color(theme.color("l-text"))
            })
            .child(fa_glyph(&format!("fas {icon}"), 13.))
            .on_click(move |_, window, cx| {
                weak.update(cx, |this, cx| act(this, window, cx)).ok();
            });
        crate::snap::target(id, tool)
    }

    fn find_bar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.cfg.find.open {
            return None;
        }
        let state = self.cfg.find_state();
        let count = self.cfg.find_count_text();
        let none = state.matches.is_empty();
        let weak = cx.weak_entity();
        let act = |f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| f(this, window, cx)).ok();
            }
        };
        let icon_btn = |id: &'static str, icon: &'static str, label: String, disabled: bool| {
            button(id, Look::Gx)
                .label(label)
                .w(u(28.))
                .h(u(28.))
                .px_0()
                .disabled(disabled)
                .child(fa_glyph(&format!("fas {icon}"), 13.))
        };
        if self.find_invalid != state.error {
            self.find_invalid = state.error;
            self.find
                .update(cx, |input, cx| input.set_invalid(state.error, cx));
        }
        let flags = FindFlag::ALL.iter().enumerate().map(|(index, flag)| {
            let flag = *flag;
            let on = self.cfg.find.flag(flag);
            let weak = weak.clone();
            let accent = theme.color("accent");
            let flag_el = div()
                .id(ElementId::Name(format!("cf-flag-{index}").into()))
                .absolute()
                .top(u(3.))
                .right(u([56., 30., 4.][index]))
                .w(u(24.))
                .h(u(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(u(5.))
                .border_1()
                .border_color(if on { accent.opacity(0.6) } else { white(0.) })
                .bg(if on { accent.opacity(0.22) } else { white(0.) })
                .font_family(MONO)
                .text_size(u(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.color(if on { "l-text" } else { "l-text-faint" }))
                .cursor_pointer()
                .tip(self.t(flag.label(), &[]))
                .button_text()
                .on_click(move |_, _, cx| {
                    weak.update(cx, |this, cx| {
                        this.cfg.toggle_flag(flag);
                        cx.notify();
                    })
                    .ok();
                })
                .child(flag.text());
            crate::snap::target(format!("cf-flag-{index}"), flag_el)
        });
        let replace_row = self.cfg.find.replace.then(|| {
            let busy = none || self.cfg.reading;
            div()
                .flex()
                .items_center()
                .gap(u(6.))
                .pl(u(26.))
                .child(div().flex_1().min_w_0().child(self.replace.clone()))
                .child(
                    button("cf-replace", Look::Gx)
                        .disabled(busy)
                        .child(self.t("#L_CF_REPLACE", &[]))
                        .on_click(act(|this, window, cx| {
                            let effect = this.cfg.replace_current(&cf::View::default());
                            this.apply(effect, window, cx);
                        })),
                )
                .child(
                    button("cf-replace-all", Look::Gx)
                        .disabled(busy)
                        .child(self.t("#L_CF_REPLACE_ALL", &[]))
                        .on_click(act(|this, window, cx| {
                            let effect = this.cfg.replace_everything(&cf::View::default());
                            this.apply(effect, window, cx);
                        })),
                )
        });
        let replace_open = self.cfg.find.replace;
        let row = div()
            .flex()
            .items_center()
            .gap(u(6.))
            .child(
                div()
                    .id("cf-find-mode")
                    .flex_none()
                    .w(u(20.))
                    .h(u(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(u(6.))
                    .text_color(theme.color("l-text-dim"))
                    .hover(|s| s.text_color(theme.color("l-text")))
                    .cursor_pointer()
                    .tip(self.t("#L_CF_TOGGLE_REPLACE", &[]))
                    .button_text()
                    .on_click(act(|this, window, cx| {
                        let effect = this.cfg.toggle_replace();
                        this.apply(effect, window, cx);
                    }))
                    .child(fa_glyph(
                        if replace_open {
                            "fas fa-chevron-down"
                        } else {
                            "fas fa-chevron-right"
                        },
                        10.,
                    )),
            )
            .child(
                div()
                    .id("cf-find-field")
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        this.key(Field::Find, dom_key(event), window, cx);
                    }))
                    .child(self.find.clone())
                    .children(flags),
            )
            .child(
                div()
                    .flex_none()
                    .min_w(u(82.))
                    .flex()
                    .justify_center()
                    .whitespace_nowrap()
                    .text_px(11.5, 11.5 * 1.2)
                    .tnum()
                    .text_color(theme.color("l-text-faint"))
                    .child(count),
            )
            .child(
                icon_btn(
                    "cf-prev",
                    "fa-arrow-up",
                    self.t("#L_CF_FIND_PREV", &[]),
                    none,
                )
                .on_click(act(|this, window, cx| {
                    let effect = this.cfg.step(-1, &cf::View::default());
                    this.apply(effect, window, cx);
                })),
            )
            .child(
                icon_btn(
                    "cf-next",
                    "fa-arrow-down",
                    self.t("#L_CF_FIND_NEXT", &[]),
                    none,
                )
                .on_click(act(|this, window, cx| {
                    let effect = this.cfg.step(1, &cf::View::default());
                    this.apply(effect, window, cx);
                })),
            )
            .child(
                icon_btn(
                    "cf-close-find",
                    "fa-times",
                    self.t("#L_CF_CLOSE_FIND", &[]),
                    false,
                )
                .on_click(act(|this, window, cx| {
                    let effect = this.cfg.close_find();
                    this.apply(effect, window, cx);
                })),
            );
        Some(
            bar_box(theme)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    // Поле «Заменить на»: Ctrl+Alt+Enter — заменить всё (Enter — событие поля).
                    if this.replace.read(cx).focus_handle(cx).is_focused(window) {
                        this.key(Field::Replace, dom_key(event), window, cx);
                    }
                }))
                .child(row)
                .children(replace_row)
                .into_any_element(),
        )
    }

    fn goto_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.cfg.goto_open {
            return None;
        }
        let weak = cx.weak_entity();
        let act = move |f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| f(this, window, cx)).ok();
            }
        };
        Some(
            bar_box(theme)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(6.))
                        .child(
                            div()
                                .flex_none()
                                .text_px(12., 12. * 1.2)
                                .text_color(theme.color("l-text-dim"))
                                .child(self.t("#L_CF_GOTO", &[])),
                        )
                        .child(
                            div()
                                .id("cf-goto-field")
                                .w(u(110.))
                                .on_key_down(cx.listener(
                                    |this, event: &KeyDownEvent, window, cx| {
                                        this.key(Field::Goto, dom_key(event), window, cx);
                                    },
                                ))
                                .child(self.goto.clone()),
                        )
                        .child(
                            button("cf-goto-ok", Look::Gx)
                                .child(self.t("#L_CF_GOTO_OK", &[]))
                                .on_click(act(|this, window, cx| {
                                    let effect = this.cfg.go_to_line(&cf::View::default());
                                    this.apply(effect, window, cx);
                                })),
                        )
                        .child(
                            button("cf-goto-close", Look::Gx)
                                .label(self.t("#L_CF_CLOSE_FIND", &[]))
                                .w(u(28.))
                                .h(u(28.))
                                .px_0()
                                .child(fa_glyph("fas fa-times", 13.))
                                .on_click(act(|this, window, cx| {
                                    let effect = this.cfg.close_goto();
                                    this.apply(effect, window, cx);
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Памятка горячих клавиш поверх окна (`Teleport` к корню, место — `placeKeys`).
    fn keys_popup(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.cfg.keys_open {
            return None;
        }
        let toggle = self.keys_probe.get();
        let zoom = crate::scale::zoom(window);
        let rows = SHORTCUTS.iter().map(|item| {
            let keys = item.keys.iter().map(|key| {
                div()
                    .min_w(u(20.))
                    .px(u(6.))
                    .py(u(2.))
                    .flex()
                    .justify_center()
                    .border_1()
                    .border_color(theme.color("l-border-strong"))
                    .rounded(u(5.))
                    .bg(white(0.06))
                    .text_px(11., 11. * 1.4)
                    .text_color(theme.color("l-text"))
                    .child(*key)
            });
            div()
                .flex()
                .items_center()
                .gap(u(16.))
                .child(
                    div()
                        .flex()
                        .gap(u(3.))
                        .w(u(150.))
                        .flex_none()
                        .children(keys),
                )
                .child(
                    div()
                        .whitespace_nowrap()
                        .text_px(12., 12. * 1.2)
                        .text_color(theme.color("l-text-dim"))
                        .child(self.t(item.label, &[])),
                )
        });
        // `.cf__keys` нет в правилах стекла Liquid Glass 0.4.9.
        let card = pop_plain(theme, zoom)
            .id("cf-keys")
            .occlude()
            .px(u(14.))
            .py(u(12.))
            .flex()
            .flex_col()
            .gap(u(5.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.cfg.close_keys();
                cx.notify();
            }))
            .child(
                div()
                    .mb(u(3.))
                    .text_px(12.5, 12.5 * 1.2)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.color("l-text"))
                    .child(self.t("#L_CF_KEYS", &[])),
            )
            .children(rows);
        Some(
            deferred(
                anchored()
                    .anchor(Anchor::TopRight)
                    .position(point(
                        toggle.right(),
                        toggle.bottom() + crate::scale::zpx(zoom, 6.),
                    ))
                    .snap_to_window_with_margin(crate::scale::zpx(zoom, 12.))
                    .child(card),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    fn editor(&mut self, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(current) = self.cfg.current.clone() else {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p(u(20.))
                .text_px(13., 13. * 1.2)
                .text_color(theme.color("l-text-faint"))
                .child(self.t("#L_CF_PICK", &[]))
                .into_any_element();
        };
        let dirty = self.cfg.dirty();
        let keys_open = self.cfg.keys_open;
        let probe = self.keys_probe.clone();
        let bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(10.))
            .px(u(12.))
            .py(u(8.))
            .border_b_1()
            .border_color(theme.color("l-border"))
            .child(
                div()
                    .id("cf-path")
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(MONO)
                    .text_size(u(12.))
                    .text_color(theme.color("l-text-dim"))
                    .tip(current.path.clone())
                    .child(current.path.clone()),
            )
            .when(dirty, |el| {
                el.child(
                    div()
                        .flex_none()
                        .text_px(11.5, 11.5 * 1.2)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.color("accent"))
                        .child(self.t("#L_CF_UNSAVED", &[])),
                )
            })
            .child(self.tool(
                ("cf-find-btn", "fa-search", self.t("#L_CF_FIND_BTN", &[])),
                self.cfg.find.open,
                theme,
                cx,
                |this, window, cx| {
                    let effect = if this.cfg.find.open {
                        this.cfg.close_find()
                    } else {
                        this.cfg.open_find(false)
                    };
                    this.apply(effect, window, cx);
                },
            ))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .track_focus(&self.keys_toggle)
                    .child(anchor_probe(probe))
                    .child(self.tool(
                        ("cf-keys-btn", "fa-keyboard", self.t("#L_CF_KEYS", &[])),
                        keys_open,
                        theme,
                        cx,
                        |this, _, cx| {
                            this.cfg.toggle_keys();
                            cx.notify();
                        },
                    )),
            );
        let find = self.find_bar(theme, cx);
        let goto = self.goto_bar(theme, cx);
        let caret = self.cfg.caret();
        let note_error = self.cfg.notice.as_ref().is_some_and(|n| n.error);
        let disabled = !dirty || self.cfg.saving;
        let weak = cx.weak_entity();
        let accent = theme.color("accent");
        let foot = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(8.))
            .px(u(12.))
            .py(u(8.))
            .border_t_1()
            .border_color(theme.color("l-border"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_px(12., 12. * 1.35)
                    .text_color(if note_error {
                        gpui::rgb(0xff8a96).into()
                    } else {
                        theme.color("l-text-faint")
                    })
                    .child(self.cfg.foot_note()),
            )
            .child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_px(11.5, 11.5 * 1.2)
                    .tnum()
                    .text_color(theme.color("l-text-faint"))
                    .child(self.t(
                        "#L_CF_POS",
                        &[&caret.line.to_string(), &caret.col.to_string()],
                    )),
            )
            .child({
                let weak = weak.clone();
                button("cf-revert", Look::Gx)
                    .disabled(disabled)
                    .child(fa_glyph("fas fa-undo-alt", 11.))
                    .child(self.t("#L_CF_REVERT", &[]))
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |this, cx| this.revert(window, cx)).ok();
                    })
            })
            .child(crate::snap::target(
                "cf-save",
                button("cf-save", Look::Gx)
                    .disabled(disabled)
                    // `.cf__save:not(:disabled)`: заливка акцентом поверх рамки (фон и рамку кнопки
                    // GX красит она сама — подложка ребёнком).
                    .when(!disabled, |el| {
                        el.text_color(theme.color("accent-contrast-text")).child(
                            div()
                                .absolute()
                                .top(u(-1.))
                                .left(u(-1.))
                                .right(u(-1.))
                                .bottom(u(-1.))
                                .rounded(u(8.))
                                .bg(accent),
                        )
                    })
                    .child(fa_glyph("fas fa-save", 11.))
                    .child(self.t("#L_SAVE", &[]))
                    .button_text()
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |this, cx| this.save(window, cx)).ok();
                    }),
            ));
        let area = crate::snap::target(
            "cf-area",
            div()
                .id("cf-area")
                .flex_1()
                .min_h_0()
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if this.code.read(cx).focus_handle(cx).is_focused(window) {
                        this.key(Field::Text, dom_key(event), window, cx);
                    }
                }))
                // Tab окна ведёт фокус дальше; в тексте он — отступ (`onKey`).
                .on_action(
                    cx.listener(|this, _: &crate::layer::FocusNext, window, cx| {
                        this.key(Field::Text, Key::new("Tab"), window, cx);
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &crate::layer::FocusPrev, window, cx| {
                        this.key(Field::Text, Key::new("Tab").shift(), window, cx);
                    }),
                )
                .child(self.code.clone()),
        );
        let _ = window;
        div()
            .id("cf-editor")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // `onEditorKey` у кнопок и прочего внутри редактора.
                let key = dom_key(event);
                let outcome = this.cfg.key(Field::Other, &key, &cf::View::default());
                if outcome.handled {
                    window.prevent_default();
                    cx.stop_propagation();
                }
                this.apply(outcome.effect, window, cx);
            }))
            .child(bar)
            .children(find)
            .children(goto)
            .child(area)
            .child(foot)
            .into_any_element()
    }
}

/// `.cf__find`: строка поиска или перехода (поля 7/12, заливка .02, линия снизу).
fn bar_box(theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id("cf-find")
        .flex()
        .flex_none()
        .flex_col()
        .gap(u(6.))
        .px(u(12.))
        .py(u(7.))
        .border_b_1()
        .border_color(theme.color("l-border"))
        .bg(white(0.02))
}

/// `.cf__find-input` с местом под флаги справа (`padding-right: 84px`).
fn find_style(theme: &Theme) -> FieldStyle {
    FieldStyle {
        pad_right: Some(84.),
        ..FieldStyle::gx_mono(theme)
    }
}

/// `.cf__goto-input`: 110 × 28, цифры одной ширины.
fn goto_style(theme: &Theme) -> FieldStyle {
    FieldStyle {
        height: 28.,
        ..FieldStyle::gx(theme)
    }
}

impl Render for GameCfgFiles {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let loading = self.cfg.loading;
        let weak = cx.weak_entity();
        let head = gx::pane_head(self.t("#L_CF_TITLE", &[]))
            .reset(
                button("cf-refresh", Look::GxIcon)
                    .label(self.t("#L_CF_REFRESH", &[]))
                    .disabled(loading)
                    .child(fa_glyph("fas fa-sync-alt", 13.))
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |this, cx| this.load(window, cx)).ok();
                    }),
            )
            .note(self.t("#L_CF_NOTE", &[]));
        let list = self.list(&theme, window, cx);
        let editor = self.editor(&theme, window, cx);
        let keys = self.keys_popup(&theme, window, cx);
        gx::pane(&theme)
            .size_full()
            .child(head)
            .child(div().flex().flex_1().min_h_0().child(list).child(editor))
            .children(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::diff;

    #[test]
    fn diff_is_one_edit_on_char_boundaries() {
        assert_eq!(diff("bind w +jump", "bind w +duck"), (8..12, "duck"));
        assert_eq!(diff("abc", "abc"), (3..3, ""));
        assert_eq!(diff("\tx", "x"), (0..1, ""));
        // Общий первый байт у «я» и «ю» — участок по границам символов.
        assert_eq!(diff("я", "ю"), (0..2, "ю"));
        assert_eq!(diff("aa", "aaa"), (2..2, "a"));
    }
}
