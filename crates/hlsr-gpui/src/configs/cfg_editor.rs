//! Раздел «Конфиг» страницы «Персонализация» (`GameSettingsModal.vue` 0.4.9,
//! `activeSection === 'configs'`): строка над редактором (сборка у HL2, путь к папке cfg),
//! предупреждение (ошибка записи, «игра запущена») и `GameConfigEditor.vue` — вкладки строкой
//! «Файлы cfg» ([`GameCfgFiles`]), «Проверка» (HL1, значок итога — [`super::cfg_check`]) и
//! «Игра» (все разделы каталога одной прокруткой, якоря к ним, сброс раздела, имя своего cfg у
//! значения, «перекрыто ниже»). У игры без понятных настроек — только файлы; без папки — пусто.
//! Логика — `Session` (`hlsr_ui::controllers::game_settings`).

use super::cfg_check::{CheckActions, check_pane, level_color};
use super::cfg_files::GameCfgFiles;
use super::sections::SectionHost;
use crate::icons::fa_glyph;
use crate::kit::button::{Look, button};
use crate::kit::num_step::{field as num_field, field_unit};
use crate::kit::{
    FieldEvent, Item, Kind as InputKind, TextInput, dropdown, gx, num_step, slider, toggle,
};
use crate::layer::{TipExt, anchor_probe};
use crate::motion::{Timing, hover, mix};
use crate::scale::{u, viewport};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::text::{MONO, TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext, Bounds, Context, ElementId, Entity, Focusable, FontWeight, IntoElement,
    ParentElement, Pixels, Render, Role, SharedString, Styled, Subscription, Window, div, hsla,
    prelude::*, px,
};
use hlsr_ui::controllers::cfg_check::{self, PlaceView};
use hlsr_ui::controllers::game_settings::FilesSlot;
use hlsr_ui::controllers::game_settings::config::{CHECK_ID, FILES_ID};
use hlsr_ui::controllers::game_settings::{
    ConfigMode, EditorView, FieldView, Kind, SectionView, Session, View,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Раздел «Конфиг».
pub struct GameConfigEditor {
    host: SectionHost,
    /// Редактор файлов: создаётся с первым кадром (ему нужно окно).
    files: Option<Entity<GameCfgFiles>>,
    /// Поля чисел по переменной.
    numbers: HashMap<String, Entity<TextInput>>,
    /// Поля чисел, выключенные сейчас (`<fieldset disabled>`).
    numbers_off: HashSet<String>,
    /// Место редактора от сессии, ещё не отданное ему (`Some(None)` — вкладка ушла).
    slot: Rc<RefCell<Option<Option<FilesSlot>>>>,
    /// «Отмена» страницы: откатить текст редактора на следующем кадре.
    revert: Rc<Cell<bool>>,
    scroll: Scroll,
    check_scroll: Scroll,
    /// Раскрытые группы «Проверки».
    open_groups: HashSet<String>,
    /// Рамки заголовков разделов «Игры» (прошлый кадр) — для якорей.
    heads: HashMap<&'static str, Rc<Cell<Bounds<Pixels>>>>,
    /// Верх содержимого прокрутки «Игры».
    top: Rc<Cell<Bounds<Pixels>>>,
    /// Вкладка прошлого кадра: смена — прокрутка «Игры» в начало.
    seen_tab: Option<&'static str>,
    _subs: Vec<Subscription>,
}

fn white(alpha: f32) -> gpui::Hsla {
    hsla(0., 0., 1., alpha)
}

impl GameConfigEditor {
    pub fn new(host: &SectionHost, _cx: &mut Context<GameConfigEditor>) -> Self {
        let category = crate::shell::root::ShellContext::get(_cx)
            .and_then(|context| context.root.upgrade())
            .map(|root| {
                let prefs = root.read(_cx).prefs();
                let prefs = prefs.borrow();
                let hl1 = prefs.get("ruleCategory").and_then(|v| v.get("hl1-won"));
                if hl1.and_then(|v| v.as_str()) == Some("scripted") {
                    "scripted"
                } else {
                    "scriptless"
                }
            })
            .unwrap_or("scriptless");
        host.session.init_rule_category(category);
        // Сессия зовёт хуки посреди своих кругов: здесь только запомнить и попросить кадр.
        let poke = {
            let weak = _cx.weak_entity();
            let app = _cx.to_async();
            let executor = _cx.foreground_executor().clone();
            Rc::new(move || {
                let (weak, app) = (weak.clone(), app.clone());
                executor
                    .spawn(async move {
                        weak.update(&mut app.clone(), |_, cx| cx.notify()).ok();
                    })
                    .detach();
            })
        };
        let slot: Rc<RefCell<Option<Option<FilesSlot>>>> = Rc::default();
        let revert = Rc::new(Cell::new(false));
        {
            let (slot, poke) = (slot.clone(), poke.clone());
            host.session.set_files_hook(Rc::new(move |wanted| {
                *slot.borrow_mut() = Some(wanted.cloned());
                poke();
            }));
        }
        {
            let revert = revert.clone();
            host.session.set_cfg_files_revert(Rc::new(move || {
                revert.set(true);
                poke();
            }));
        }
        GameConfigEditor {
            host: host.clone(),
            files: None,
            numbers: HashMap::new(),
            numbers_off: HashSet::new(),
            slot,
            revert,
            scroll: Scroll::new(),
            check_scroll: Scroll::new(),
            open_groups: HashSet::new(),
            heads: HashMap::new(),
            top: Rc::default(),
            seen_tab: None,
            _subs: Vec::new(),
        }
    }

    fn session(&self) -> &Rc<Session> {
        &self.host.session
    }

    fn t(&self, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(self.host.language, key, args)
    }

    /// Редактор файлов и его связь с сессией: место (игра, сборка), откат правок.
    fn ensure_files(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<GameCfgFiles> {
        if let Some(files) = &self.files {
            return files.clone();
        }
        let (services, session, language) = (
            self.host.services.clone(),
            self.host.session.clone(),
            self.host.language,
        );
        let files = cx.new(|cx| GameCfgFiles::new(&services, session, language, window, cx));
        self.files = Some(files.clone());
        files
    }

    /// Место и откат от сессии — редактору (в кадре: ему нужно окно).
    fn sync_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let slot = self.slot.borrow_mut().take();
        let revert = self.revert.replace(false);
        if slot.is_none() && !revert {
            return;
        }
        let files = self.ensure_files(window, cx);
        files.update(cx, |files, cx| {
            if revert {
                files.revert(window, cx);
            }
            if let Some(slot) = slot {
                files.set_target(slot.as_ref(), window, cx);
            }
        });
    }

    /// Несохранённый текст cfg сохранить (вопрос «Сохранить» страницы).
    pub fn save_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(files) = &self.files {
            files.update(cx, |files, cx| {
                if files.dirty() {
                    files.save(window, cx);
                }
            });
        }
    }

    /// Вкладка (`selectSection`): уход из «Файлов cfg» с несохранённым текстом — сначала вопрос
    /// страницы (`canLeave` → `askUnsaved`).
    fn select_tab(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let session = self.session().clone();
        let leaving_files = session
            .config_view()
            .editor
            .as_ref()
            .is_some_and(|editor| editor.active == FILES_ID && id != FILES_ID);
        if leaving_files && session.cfg_files_dirty() {
            let page = self.host.page.clone();
            window.defer(cx, move |window, cx| {
                page.update(cx, |page, cx| {
                    page.settle_unsaved(
                        None,
                        Box::new(move |_, _, _| session.config_select_section(id)),
                        window,
                        cx,
                    )
                })
                .ok();
            });
            return;
        }
        session.config_select_section(id);
        cx.notify();
    }

    /// «Проверка»: место → «Файлы cfg», нужный файл, курсор на строке (`showCheckPlace`).
    fn open_place(&mut self, place: &PlaceView, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = place.path.clone() else {
            return;
        };
        let line = place.line;
        self.session().config_select_section(FILES_ID);
        // Редактор ещё не на месте: строку он откроет, когда сессия отдаст место и придёт список.
        let files = self.ensure_files(window, cx);
        files.update(cx, |files, cx| files.open_at(&path, line, window, cx));
    }

    /// Якорь: раздел к верху прокрутки (`jumpTo`).
    fn jump(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(head) = self.heads.get(id).map(|cell| cell.get()) else {
            return;
        };
        let top = self.top.get().top();
        let to = head.top() - top;
        // `scroll-behavior: smooth` у `.gc-scroll` (без него — при reduced-motion).
        self.scroll.scroll_to(to, true, window, cx);
        cx.notify();
    }

    /// Поле числа переменной: текст — за значением, пока поле не в фокусе.
    fn number(
        &mut self,
        field: &FieldView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(input) = self.numbers.get(&field.cvar) {
            let input = input.clone();
            let focused = input.read(cx).focus_handle(cx).is_focused(window);
            if !focused && input.read(cx).text() != field.text {
                let text = field.text.clone();
                input.update(cx, |input, cx| input.set_text(text, cx));
            }
            return input;
        }
        let percent = field.kind == Kind::Percent;
        let num = |v: f64| SharedString::from(hlsr_core::js::number::number_to_string(v));
        let (min, max, step) = (num(field.minimum), num(field.maximum), num(field.step));
        let text = field.text.clone();
        let input = cx.new(|cx| {
            TextInput::new(window, cx)
                .kind(InputKind::Number { min, max, step })
                .style(if percent { field_unit } else { num_field })
                .align_right()
                .with_text(text)
        });
        let cvar = field.cvar.clone();
        let sub = cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &FieldEvent, _, cx| {
                if matches!(event, FieldEvent::Change) {
                    let text = input.read(cx).value();
                    let shown = this.session().config_set_number(&cvar, &text);
                    input.update(cx, |input, cx| {
                        if input.text() != shown {
                            input.set_text(shown, cx);
                        }
                    });
                }
            },
        );
        self._subs.push(sub);
        self.numbers.insert(field.cvar.clone(), input.clone());
        input
    }

    fn row(
        &mut self,
        field: &FieldView,
        last: bool,
        disabled: bool,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let session = self.session().clone();
        let cvar = field.cvar.clone();
        let locked = disabled || field.off;
        let control: AnyElement = match field.kind {
            Kind::Switch => {
                let mut sw = toggle(format!("gc-switch-{cvar}"), field.checked)
                    .label(field.label.clone())
                    .disabled(disabled);
                if !locked {
                    let (session, cvar) = (session.clone(), cvar.clone());
                    sw = sw.on_change(move |on, _, _| session.config_set_switch(&cvar, *on));
                }
                sw.into_any_element()
            }
            Kind::Range | Kind::Percent => {
                let input = self.number(field, window, cx);
                if self.numbers_off.contains(&cvar) != locked {
                    if locked {
                        self.numbers_off.insert(cvar.clone());
                    } else {
                        self.numbers_off.remove(&cvar);
                    }
                    input.update(cx, |input, cx| input.set_disabled(locked, cx));
                }
                let mut step = num_step(format!("gc-num-{cvar}"), input).disabled(locked);
                if field.kind == Kind::Percent {
                    step = step.unit("%");
                }
                div().w(u(92.)).child(step).into_any_element()
            }
            Kind::Select => {
                let items: Vec<Item> = field
                    .options
                    .iter()
                    .map(|item| Item::new(item.id.clone(), item.label.clone()))
                    .collect();
                let label = field.select_label.clone();
                let id = format!("gc-select-{cvar}");
                let inner = id.clone();
                let mut menu = dropdown(id, items, Some(field.select_value.clone().into()))
                    .align_right()
                    .label(field.label.clone())
                    .toggle(move |state| {
                        div().w(u(164.)).child(gx::select(
                            format!("{inner}-box"),
                            label,
                            state.open,
                            state.focus,
                        ))
                    });
                if !locked {
                    let (session, cvar) = (session.clone(), cvar.clone());
                    menu = menu
                        .on_select(move |item, _, _| session.config_select_option(&cvar, &item.id));
                }
                div()
                    .flex_shrink(1.)
                    .min_w_0()
                    .child(menu)
                    .into_any_element()
            }
        };
        let mut row = gx::row(
            ElementId::Name(format!("gc-row-{cvar}").into()),
            field.label.clone(),
        )
        .card(last)
        .changed(field.edited)
        .off(field.off)
        .control(control);
        if field.kind == Kind::Switch && !locked {
            let (session, cvar, on) = (session.clone(), cvar.clone(), field.checked);
            row = row.on_label(move |_, _, _| session.config_set_switch(&cvar, !on));
        }
        if !field.hint.is_empty() || !field.source.is_empty() || field.shadowed {
            // `.gc-source` — inline-block в строке подсказки: отступ слева 4 и в начале строки,
            // между словами — пробел (~3), переносы без зазора.
            let chip = |text: String| {
                div()
                    .min_w_0()
                    .ml(u(4.))
                    .px(u(6.))
                    .rounded(u(4.))
                    .bg(white(0.06))
                    .text_color(theme.color("l-text-dim"))
                    .font_family(MONO)
                    .text_px(11., 17.)
                    .child(text)
            };
            let hint = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(u(3.))
                .text_px(12., 12. * 1.4)
                .text_color(theme.color("l-text-faint"))
                .when(!field.hint.is_empty(), |el| el.child(field.hint.clone()))
                .when(field.shadowed, |el| {
                    el.child(chip(self.t("#L_GS_CFG_SHADOWED", &[])))
                })
                .when(!field.source.is_empty(), |el| {
                    el.child(
                        chip(field.source.clone())
                            .id(ElementId::Name(format!("gc-src-{cvar}").into()))
                            .tip(field.source_title.clone()),
                    )
                });
            row = row.below(hint);
        }
        if matches!(field.kind, Kind::Range | Kind::Percent) {
            let mut range = slider(
                format!("gc-range-{cvar}"),
                field.value,
                field.minimum,
                field.maximum,
            )
            .default_value(field.default_value)
            .label(field.label.clone())
            .disabled(disabled);
            if !locked {
                let (s1, c1) = (session.clone(), cvar.clone());
                let (s2, c2) = (session.clone(), cvar.clone());
                range = range
                    .on_input(move |raw, _, _| s1.config_slide(&c1, *raw, 0.))
                    .on_change(move |raw, _, _| s2.config_end_slide(&c2, *raw, 0.));
            }
            let range = range.into_any_element();
            row = row.below(if field.has_scale {
                gx::slider_ends(
                    field.scale_start.clone(),
                    range,
                    field.scale_end.clone(),
                    theme,
                )
                .into_any_element()
            } else {
                div().mt(u(2.)).child(range).into_any_element()
            });
        }
        if !field.needs.is_empty() {
            row = row.needs(field.needs.clone());
        }
        row.into_any_element()
    }

    fn section(
        &mut self,
        index: usize,
        section: &SectionView,
        disabled: bool,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tight = viewport(window).1 <= 700.;
        let probe = self.heads.entry(section.id).or_default().clone();
        let session = self.session().clone();
        let id = section.id;
        let title = div()
            .flex()
            .items_center()
            .gap(u(8.))
            .child(
                div()
                    .w(u(14.))
                    .flex()
                    .justify_center()
                    .text_color(theme.color("l-text-faint"))
                    .child(fa_glyph(&format!("fas {}", section.icon), 12.)),
            )
            .child(spaced(
                section.title.to_uppercase(),
                12.,
                16.,
                FontWeight::SEMIBOLD,
                0.08,
                theme.color("l-text-dim"),
            ));
        let reset = button(ElementId::Name(format!("gc-reset-{id}").into()), Look::Gx)
            .label(self.t("#L_GS_CFG_RESET_HINT", &[]))
            .disabled(!section.reset_enabled)
            .child(fa_glyph("fas fa-undo-alt", 11.))
            .on_click(move |_, _, _| session.config_reset_section(id));
        let head = div()
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .gap(u(12.))
            .px(u(14.))
            .pt(u(if tight { 8. } else { 16. }))
            .pb(u(2.))
            .child(anchor_probe(probe))
            .child(title)
            .child(
                div()
                    .id(ElementId::Name(format!("gc-reset-tip-{id}").into()))
                    .tip(self.t("#L_GS_CFG_RESET_HINT", &[]))
                    .child(reset),
            );
        let columns: Vec<AnyElement> = section
            .columns
            .iter()
            .map(|column| {
                let count = column.len();
                let rows: Vec<AnyElement> = column
                    .iter()
                    .enumerate()
                    .map(|(i, field)| self.row(field, i + 1 == count, disabled, theme, window, cx))
                    .collect();
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .children(rows)
                    .into_any_element()
            })
            .collect();
        div()
            .when(index > 0, |el| {
                el.child(
                    div()
                        .mt(u(10.))
                        .mx(u(14.))
                        .h(px(1.))
                        .bg(theme.color("l-border")),
                )
            })
            .child(head)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(u(4.))
                    .px(u(2.))
                    .py(u(6.))
                    .children(columns),
            )
            .into_any_element()
    }

    /// Вкладка строкой (`.gc--top .gx-nav__item`): значок с точкой правок, подпись, у
    /// «Проверки» — значок итога.
    fn tab(
        &self,
        item: &hlsr_ui::controllers::game_settings::TabItem,
        view: &EditorView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = view.selected == Some(item.id);
        let id = ElementId::Name(format!("gc-tab-{}", item.id).into());
        let inner = id.clone();
        let (dim, text, soft, accent) = (
            theme.color("l-text-dim"),
            theme.color("l-text"),
            theme.color("accent-soft"),
            theme.color("accent"),
        );
        let badge = (item.id == CHECK_ID).then(|| view.badge.clone()).flatten();
        let badge_theme = theme.clone();
        let badge = move || {
            badge.clone().map(|badge| {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(4.))
                    .px(u(6.))
                    .rounded(u(9.))
                    .bg(white(0.08))
                    .text_px(11., 18.)
                    .tnum()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(level_color(badge.level, &badge_theme))
                    // `.gx-nav__item.active i` красит акцентом и значок итога.
                    .child(
                        fa_glyph(&format!("fas {}", badge.icon), 10.)
                            .when(active, |i| i.text_color(accent)),
                    )
                    .when(badge.count > 0, |el| el.child(badge.count.to_string()))
            })
        };
        let (icon, title, dot, tab_id) = (
            format!("fas {}", item.icon),
            item.title.clone(),
            item.edits > 0,
            item.id,
        );
        let weak = cx.weak_entity();
        let tab = hover(id, Timing::fast(), move |t| {
            let weak = weak.clone();
            let (bg, color) = if active {
                (soft, text)
            } else {
                (white(0.06 * t), mix(dim, text, t))
            };
            div()
                .id(inner.clone())
                .role(Role::Tab)
                .button_text()
                .aria_selected(active)
                .aria_label(title.clone())
                .flex()
                .flex_none()
                .items_center()
                .gap(u(9.))
                .min_h(u(38.))
                .px(u(8.))
                .py(u(7.))
                .rounded(u(8.))
                .bg(bg)
                .text_color(color)
                .text_px(13., 13. * 1.25)
                .whitespace_nowrap()
                .cursor_pointer()
                .on_click(move |_, window, cx| {
                    weak.update(cx, |this, cx| this.select_tab(tab_id, window, cx))
                        .ok();
                })
                .child(
                    div()
                        .relative()
                        .w(u(16.))
                        .flex()
                        .justify_center()
                        .child(fa_glyph(&icon, 13.).when(active, |i| i.text_color(accent)))
                        .when(dot, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .top(u(-4.))
                                    .right(u(-5.))
                                    .size(u(8.))
                                    .rounded_full()
                                    .bg(accent),
                            )
                        }),
                )
                .child(title.clone())
                .children(badge())
        });
        crate::snap::target(format!("gc-tab-{}", item.id), tab)
    }

    fn game_pane(
        &mut self,
        view: &EditorView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tight = viewport(window).1 <= 700.;
        let anchors = (view.sections.len() > 1).then(|| {
            let weak = cx.weak_entity();
            div()
                .flex()
                .flex_wrap()
                .gap(u(2.))
                .ml(u(-8.))
                .children(view.sections.iter().map(|section| {
                    let id = ElementId::Name(format!("gc-jump-{}", section.id).into());
                    let inner = id.clone();
                    let (sid, icon, title, dot) = (
                        section.id,
                        format!("fas {}", section.icon),
                        section.title.clone(),
                        section.edits > 0,
                    );
                    let (dim, text, hover_bg, accent, faint) = (
                        theme.color("l-text-dim"),
                        theme.color("l-text"),
                        theme.color("l-chip-hover"),
                        theme.color("accent"),
                        theme.color("l-text-faint"),
                    );
                    let weak = weak.clone();
                    hover(id, Timing::fast(), move |t| {
                        let weak = weak.clone();
                        div()
                            .id(inner.clone())
                            .flex()
                            .items_center()
                            .gap(u(8.))
                            .h(u(28.))
                            .px(u(8.))
                            .rounded(u(8.))
                            .bg(hover_bg.opacity(t))
                            .text_color(mix(dim, text, t))
                            .text_px(12.5, 12.5 * 1.2)
                            .cursor_pointer()
                            .button_text()
                            .on_click(move |_, window, cx| {
                                weak.update(cx, |this, cx| this.jump(sid, window, cx)).ok();
                            })
                            .child(
                                div()
                                    .relative()
                                    .w(u(16.))
                                    .flex()
                                    .justify_center()
                                    .text_color(faint)
                                    .child(fa_glyph(&icon, 13.))
                                    .when(dot, |el| {
                                        el.child(
                                            div()
                                                .absolute()
                                                .top(u(-4.))
                                                .right(u(-5.))
                                                .size(u(8.))
                                                .rounded_full()
                                                .bg(accent),
                                        )
                                    }),
                            )
                            .child(title.clone())
                    })
                }))
        });
        let note = div()
            .id("gc-note")
            .text_px(12., 12. * 1.4)
            .text_color(theme.color("l-text-faint"))
            .when(tight, |el| el.clamp(1))
            .tip(view.note.clone())
            .child(view.note.clone());
        let head = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(u(3.))
            .px(u(14.))
            .py(u(if tight { 6. } else { 10. }))
            .border_b_1()
            .border_color(theme.color("l-border"))
            .children(anchors)
            .child(note);
        let sections: Vec<AnyElement> = view
            .sections
            .iter()
            .enumerate()
            .map(|(i, section)| self.section(i, section, view.disabled, theme, window, cx))
            .collect();
        gx::pane(theme)
            .size_full()
            .child(head)
            .child(
                ScrollArea::new("gc-scroll", &self.scroll)
                    .bar(Bar::Fields)
                    .contain()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .relative()
                            .pb(u(8.))
                            .child(anchor_probe(self.top.clone()))
                            .children(sections),
                    ),
            )
            .into_any_element()
    }

    fn editor(
        &mut self,
        view: &EditorView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.seen_tab.replace(view.active) != Some(view.active) {
            self.scroll.scroll_to(Pixels::ZERO, false, window, cx);
        }
        let tabs: Vec<AnyElement> = view
            .tabs
            .iter()
            .map(|item| self.tab(item, view, theme, cx))
            .collect();
        let body: AnyElement = match view.active {
            FILES_ID => {
                let files = self.ensure_files(window, cx);
                div().size_full().child(files).into_any_element()
            }
            CHECK_ID => {
                let (check, loading, failed) = self.session().rule_check();
                let tf = |key: &str, args: &[&str]| self.t(key, args);
                let check_view = cfg_check::check_view(check.as_ref(), loading, failed, &tf);
                let weak = cx.weak_entity();
                let session = self.session().clone();
                let (s1, s2) = (session.clone(), session.clone());
                let (w1, w2) = (weak.clone(), weak.clone());
                let actions = CheckActions {
                    refresh: Rc::new(move |(), _, _| s1.load_rule_check()),
                    category: Rc::new(move |category, _, cx| {
                        s2.set_rule_category(category);
                        if let Some(root) =
                            crate::shell::root::ShellContext::get(cx).and_then(|c| c.root.upgrade())
                        {
                            let prefs = root.read(cx).prefs();
                            let mut prefs = prefs.borrow_mut();
                            let mut all = prefs
                                .get("ruleCategory")
                                .and_then(|v| v.as_object().cloned())
                                .unwrap_or_default();
                            all.insert("hl1-won".into(), category.into());
                            let mut changes = serde_json::Map::new();
                            changes.insert("ruleCategory".into(), serde_json::Value::Object(all));
                            prefs.save(&changes);
                        }
                    }),
                    toggle: Rc::new(move |key: String, _, cx| {
                        w1.update(cx, |this, cx| {
                            if !this.open_groups.remove(&key) {
                                this.open_groups.insert(key);
                            }
                            cx.notify();
                        })
                        .ok();
                    }),
                    open: Rc::new(move |place: PlaceView, window, cx| {
                        w2.update(cx, |this, cx| this.open_place(&place, window, cx))
                            .ok();
                    }),
                };
                let t = |key: &str| self.t(key, &[]);
                check_pane(
                    &check_view,
                    &session.rule_category(),
                    &self.open_groups,
                    &self.check_scroll,
                    actions,
                    theme,
                    &t,
                    &tf,
                )
            }
            _ => self.game_pane(view, theme, window, cx),
        };
        div()
            .flex()
            .flex_col()
            .gap(u(12.))
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("gc-tabs")
                    .role(Role::TabList)
                    .aria_label(self.t("#L_GS_X_CATEGORIES", &[]))
                    .flex()
                    .flex_wrap()
                    .gap(u(4.))
                    .children(tabs),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// Строка над редактором: сборка (у HL2 их несколько) и путь к папке cfg.
    fn bar(&self, view: &View, theme: &Theme) -> AnyElement {
        let session = self.session().clone();
        let build = view.build_select.then(|| {
            let items: Vec<Item> = view
                .build
                .items
                .iter()
                .map(|item| Item::new(item.id.clone(), item.label.clone()))
                .collect();
            let label = format!("{}: {}", self.t("#L_GS_CFG_BUILD", &[]), view.build.label);
            let hint = self.t("#L_GS_CFG_BUILD_HINT", &[]);
            dropdown("gc-build", items, Some(view.build.value.clone().into()))
                .toggle(move |state| {
                    div().id("gc-build-box").tip(hint.clone()).child(gx::select(
                        "gc-build-sel",
                        label,
                        state.open,
                        state.focus,
                    ))
                })
                .on_select(move |item, _, _| session.select_build(&item.id))
        });
        div()
            .flex()
            .flex_none()
            .items_stretch()
            .gap(u(8.))
            .min_w_0()
            .children(build)
            .child(
                div()
                    .id("gc-path")
                    .flex_1()
                    .min_w_0()
                    .min_h(u(36.))
                    // Рамка 0.4.9 — тень внутрь, места не занимает: поля 8 − 1 под `border_1`.
                    .px(u(11.))
                    .py(u(7.))
                    .flex()
                    .items_center()
                    .rounded(theme.rem("l-radius-sm"))
                    .bg(theme.color("l-field-bg"))
                    .border_1()
                    .border_color(theme.color("l-border"))
                    .text_color(theme.color("l-text-dim"))
                    .font_family(MONO)
                    .text_px(12., 20.)
                    .tip(view.path_text.clone())
                    .child(div().min_w_0().clamp(2).child(view.path_text.clone())),
            )
            .into_any_element()
    }
}

impl Render for GameConfigEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let theme = cx.global::<Theme>().clone();
        self.sync_files(window, cx);
        let view = self.session().config_view();
        let tight = viewport(window).1 <= 700.;
        let alert = (!view.alert.is_empty()).then(|| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(9.))
                .px(u(14.))
                .py(u(9.))
                .rounded(theme.rem("l-radius-sm"))
                .bg(gpui::rgba(0xff7a1c1a))
                .border_1()
                .border_color(gpui::rgba(0xff7a1c59))
                .text_color(gpui::rgb(0xffb37a))
                .text_px(12.5, 12.5 * 1.4)
                .child(fa_glyph(
                    if view.alert_error {
                        "fas fa-exclamation-triangle"
                    } else {
                        "fas fa-info-circle"
                    },
                    12.5,
                ))
                .child(div().flex_1().min_w_0().child(view.alert.clone()))
        });
        let body: AnyElement = match (&view.config_mode, &view.editor) {
            (ConfigMode::Editor, Some(editor)) => self.editor(editor, &theme, window, cx),
            (ConfigMode::FilesOnly, _) => {
                let files = self.ensure_files(window, cx);
                div().flex_1().min_h_0().child(files).into_any_element()
            }
            _ => div()
                .px(u(16.))
                .py(u(32.))
                .rounded(theme.rem("l-radius-sm"))
                .bg(white(0.02))
                .border_1()
                .border_color(theme.color("l-border"))
                .text_px(13., 13. * 1.2)
                .text_color(theme.color("l-text-faint"))
                .flex()
                .justify_center()
                .child(view.empty_text.clone())
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap(u(if tight { 8. } else { 10. }))
            .size_full()
            .min_h_0()
            .child(self.bar(&view, &theme))
            .children(alert)
            .child(body)
    }
}
