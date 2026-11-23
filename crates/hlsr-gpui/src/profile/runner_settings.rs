//! Вкладка «Настройки» своего профиля (`RunnerSettings.vue` 0.4.9): отображение (SteamID64 под
//! спойлером), приватность, «О себе» и ссылки одной кнопкой «Сохранить», группа «Подсветка ника
//! и рамка» ([`super::style_presets`] и Telegram по коду), speedrun.com (ключ, рекорды,
//! закреплённый забег), Discord по коду («Как привязать» — 5 шагов), «Данные»: выход, выход на
//! всех устройствах, удаление аккаунта с отсчётом. Логика — `controllers::runner_settings`.
//!
//! Строки, переключатели и кнопки — как настройки лаунчера (`.settings-row`, `.settings-switch`,
//! `.settings-btn`). Раздел живёт, пока окно показывает свой профиль ([`RunnerSettingsTab::attach`]
//! из окна): там же `created()` — список заготовок, и `beforeUnmount` — таймеры сняты.

use super::ProfileContext;
use super::overview::{LAMBDA, medal_color};
use super::style_presets::picker;
use crate::icons::{chevron, fa_glyph, icon};
use crate::kit::button::{
    child_id, faded, focus_handle, focus_visible, outline, white, window_under,
};
use crate::kit::input::FieldStyle;
use crate::kit::{
    CheckLook, Checked, FieldEvent, Item, Kind, TextInput, ToggleState, check, dropdown, link,
    toggle,
};
use crate::layer::TipExt;
use crate::motion::{Push, Timing, hover, mix, press, tween};
use crate::paint::ring;
use crate::scale::{u, viewport, zoom};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::shell::root::ShellContext;
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, Hsla, IntoElement, Render, Role,
    SharedString, Subscription, Task, Window, div, prelude::*, rgb, rgba, svg,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::runner_settings::{
    Code, Effect, Hint, LINKS, SettingsState, SettingsView, style_presets::PickerView,
};
use hlsr_ui::controllers::wall_clock::WallClock;
use hlsr_ui::services::Services;
use std::rc::Rc;
use std::time::Duration;

/// Поля ввода раздела (создаются с окном).
struct Inputs {
    status: Entity<TextInput>,
    links: [Entity<TextInput>; 3],
    key: Entity<TextInput>,
    _events: Vec<Subscription>,
}

pub struct RunnerSettingsTab {
    services: Services,
    state: SettingsState,
    wall: WallClock,
    /// Окно (поколение), в котором раздел смонтирован; `None` — не смонтирован.
    mounted: Option<u64>,
    /// Растёт при каждом монтировании: ответы прежнего раздела не применяются.
    generation: u64,
    inputs: Option<Inputs>,
    countdown: Option<Task<()>>,
    /// Сроки кодов Telegram и Discord.
    timers: [Option<Task<()>>; 2],
    scroll: Scroll,
    /// Кэш снимка настроек, заготовок и списков выбора (~200 стран): прокрутка не перестраивает их 240 раз/с.
    local_rev: u64,
    synced_modal_rev: Option<u64>,
    synced_inputs_key: Option<(Language, u64, u64)>,
    view_cache: Option<(
        (Language, u64, u64),
        Rc<SettingsView>,
        Option<Rc<PickerView>>,
        Vec<Item>,
        Vec<Item>,
    )>,
    _activation: Option<Subscription>,
}

/// Вид `.settings-btn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Btn {
    Plain,
    Ghost,
    Primary,
    Danger,
}

const ERROR: u32 = 0xff6b78;
const OK: u32 = 0x6fd38a;

impl RunnerSettingsTab {
    pub fn new(services: &Services, _cx: &mut Context<RunnerSettingsTab>) -> RunnerSettingsTab {
        RunnerSettingsTab {
            services: services.clone(),
            state: SettingsState::new(),
            wall: WallClock::system(),
            mounted: None,
            generation: 0,
            inputs: None,
            countdown: None,
            timers: [None, None],
            scroll: Scroll::new(),
            local_rev: 0,
            synced_modal_rev: None,
            synced_inputs_key: None,
            view_cache: None,
            _activation: None,
        }
    }

    fn bump(&mut self) {
        self.local_rev = self.local_rev.wrapping_add(1);
    }

    pub fn reset_scroll(&mut self) {
        self.scroll = Scroll::new();
    }

    /// Окно профиля на каждом кадре: `Some(поколение окна)` — свой профиль показан (раздел
    /// смонтирован: `v-if="!targetSteamId"`), `active` — открыт раздел «Настройки».
    pub fn attach(&mut self, key: Option<u64>, active: bool, cx: &mut Context<Self>) {
        if key != self.mounted {
            if self.mounted.is_some() {
                let effects = self.state.unmounted();
                self.run(effects, cx);
            }
            self.generation += 1;
            self.countdown = None;
            self.timers = [None, None];
            self.state = SettingsState::new();
            self.mounted = key;
            self.synced_modal_rev = None;
            self.synced_inputs_key = None;
            self.view_cache = None;
            self.bump();
            if key.is_some() {
                let effects = self.state.load_presets(false);
                self.run(effects, cx);
            }
        }
        self.state.set_active(active);
    }

    pub(super) fn state(&mut self) -> &mut SettingsState {
        self.bump();
        &mut self.state
    }

    /// Выполнить побочные действия логики раздела.
    pub(super) fn run(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        if effects.is_empty() {
            return;
        }
        self.bump();
        for effect in effects {
            match effect {
                Effect::Request { tag, channel, args } => {
                    let reply = self.services.invoke(channel, args);
                    let generation = self.generation;
                    cx.spawn(async move |this, cx| {
                        let reply = reply.await;
                        this.update(cx, |this, cx| {
                            if this.generation != generation {
                                return;
                            }
                            let now = this.wall.now_ms();
                            let effects = this.state.answer(tag, reply, now);
                            this.bump();
                            this.run(effects, cx);
                            cx.notify();
                        })
                        .ok();
                    })
                    .detach();
                }
                Effect::Updated(snapshot) => {
                    with_modal(cx, move |modal, cx| modal.apply_snapshot(&snapshot, cx))
                }
                Effect::OpenLink(url) => with_modal(cx, move |modal, cx| {
                    let effects = modal.state.borrow().open_link(&url);
                    modal.run(effects, cx);
                }),
                Effect::Countdown(on) => {
                    self.countdown = on.then(|| {
                        cx.spawn(async move |this, cx| {
                            loop {
                                cx.background_executor()
                                    .timer(Duration::from_millis(
                                        hlsr_ui::controllers::runner_settings::COUNTDOWN_TICK_MS,
                                    ))
                                    .await;
                                let done = this
                                    .update(cx, |this, cx| {
                                        let now = this.wall.now_ms();
                                        let done = this.state.tick(now);
                                        this.bump();
                                        cx.notify();
                                        done
                                    })
                                    .unwrap_or(true);
                                if done {
                                    break;
                                }
                            }
                        })
                    });
                }
                Effect::CodeTimer(code, ms) => {
                    let slot = match code {
                        Code::Telegram => 0,
                        Code::Discord => 1,
                    };
                    self.timers[slot] = ms.map(|ms| {
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(Duration::from_millis(ms))
                                .await;
                            this.update(cx, |this, cx| {
                                this.state.code_expired(code);
                                this.bump();
                                cx.notify();
                            })
                            .ok();
                        })
                    });
                }
            }
        }
        cx.notify();
    }

    /// Действие логики по щелчку.
    fn act(
        &self,
        cx: &mut Context<Self>,
        f: impl Fn(&mut SettingsState, i64) -> Vec<Effect> + 'static,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _: &ClickEvent, _, cx| {
            let now = this.wall.now_ms();
            let effects = f(&mut this.state, now);
            this.bump();
            this.run(effects, cx);
            cx.notify();
        })
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inputs.is_some() {
            return;
        }
        let placeholder = hlsr_ui::tr(
            crate::shell::root::language_of(&self.services),
            "#L_RUNNER_STATUS_PLACEHOLDER",
            &[],
        );
        let field =
            |window: &mut Window, cx: &mut Context<Self>, max: usize, hint: SharedString| {
                cx.new(|cx| {
                    TextInput::new(window, cx)
                        .style(FieldStyle::runner)
                        .max_length(max)
                        .placeholder(hint)
                })
            };
        let status = field(window, cx, 160, placeholder.into());
        let links = LINKS.map(|spec| field(window, cx, spec.max as usize, spec.placeholder.into()));
        let key = cx.new(|cx| {
            TextInput::new(window, cx)
                .kind(Kind::Password)
                .style(FieldStyle::runner)
                .max_length(250)
        });
        let mut events = vec![
            cx.subscribe(&status, |this, input, event: &FieldEvent, cx| {
                if *event == FieldEvent::Input {
                    let text = input.read(cx).text().to_owned();
                    this.state.set_field("status", &text);
                    this.bump();
                    cx.notify();
                }
            }),
        ];
        for (index, input) in links.iter().enumerate() {
            let id = LINKS[index].id;
            events.push(
                cx.subscribe(input, move |this, input, event: &FieldEvent, cx| {
                    match event {
                        FieldEvent::Input => {
                            let text = input.read(cx).text().to_owned();
                            this.state.set_field(id, &text);
                        }
                        // После ввода поле показывает, что уйдёт на сервер, — полный адрес канала.
                        FieldEvent::Blur => {
                            this.state.tidy_link(id);
                        }
                        _ => return,
                    }
                    this.bump();
                    cx.notify();
                }),
            );
        }
        events.push(cx.subscribe(&key, |this, input, event: &FieldEvent, cx| {
            match event {
                FieldEvent::Input => {
                    let text = input.read(cx).text().to_owned();
                    this.state.set_speedrun_key(&text);
                }
                FieldEvent::Enter => {
                    let effects = this.state.link_speedrun();
                    this.run(effects, cx);
                }
                _ => return,
            }
            this.bump();
            cx.notify();
        }));
        self.inputs = Some(Inputs {
            status,
            links,
            key,
            _events: events,
        });
        // Открытый SteamID64 прячется, когда окно уходит на задний план; фокус вернулся, пока
        // код Discord на экране, — синхронизация (игрок мог уже ввести его).
        self._activation = Some(cx.observe_window_activation(window, |this, window, cx| {
            let effects = if window.is_window_active() {
                this.state.window_focused()
            } else {
                this.state.hide_id();
                Vec::new()
            };
            this.bump();
            this.run(effects, cx);
            cx.notify();
        }));
    }

    /// Поля показывают значения формы (сброс, «Вернуть», полный адрес, стёртый ключ).
    fn sync_inputs(&self, view: &SettingsView, cx: &mut Context<Self>) {
        let Some(inputs) = &self.inputs else {
            return;
        };
        let set =
            |input: &Entity<TextInput>, text: &str, disabled: bool, cx: &mut Context<Self>| {
                input.update(cx, |input, cx| {
                    if input.text() != text {
                        input.set_text(text, cx);
                    }
                    input.set_disabled(disabled, cx);
                });
            };
        set(&inputs.status, &view.status, view.status_muted, cx);
        for (input, row) in inputs.links.iter().zip(&view.links) {
            set(input, &row.value, false, cx);
            input.update(cx, |input, cx| input.set_invalid(row.invalid, cx));
        }
        set(&inputs.key, self.state.speedrun_key(), view.busy, cx);
    }
}

/// Действие над окном профиля (`$emit` наверх).
fn with_modal<T: 'static>(
    cx: &mut Context<T>,
    f: impl FnOnce(&mut super::modal::ProfileModal, &mut Context<super::modal::ProfileModal>) + 'static,
) {
    let Some(modal) = ProfileContext::get(cx).map(|c| c.modal.clone()) else {
        return;
    };
    modal.update(cx, f).ok();
}

/// `h4.settings-group`: 12/16, 600, разрядка .08em, заглавные, `--l-text-dim`.
fn group(text: &str, first: bool, theme: &Theme) -> AnyElement {
    div()
        .mt(u(if first { 16. } else { 32. }))
        .mb(u(4.))
        .child(spaced(
            text.to_uppercase(),
            12.,
            16.,
            FontWeight::SEMIBOLD,
            0.08,
            theme.color("l-text-dim"),
        ))
        .into_any_element()
}

/// `.settings-row__hint` (`is-error` — `#ff6b78`).
fn hint(text: impl Into<SharedString>, error: bool, theme: &Theme) -> gpui::Div {
    div()
        .text_px(12., 20.)
        .text_color(if error {
            rgb(ERROR).into()
        } else {
            theme.color("l-text-faint")
        })
        .child(text.into())
}

/// `.settings-row__label`: 14/24, 500.
fn label(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .text_px(14., 24.)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.color("l-text"))
        .child(text.into())
}

/// `.runner-settings__brand`: значок 16 по центру (`--l-text-dim`, 14 px) и подпись через 12.
fn brand_row(classes: &str, text: String, color: Option<Hsla>, theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(u(12.))
        .text_px(14., 24.)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.color("l-text"))
        .child(
            div()
                .w(u(16.))
                .flex()
                .justify_center()
                .text_color(color.unwrap_or_else(|| theme.color("l-text-dim")))
                .child(fa_glyph(classes, 14.)),
        )
        .child(text)
}

/// `.settings-row`: текст слева, контрол справа (`column` — под текстом); линия снизу места не
/// занимает (`box-shadow: inset 0 -1px 0`).
fn row(text: impl IntoElement, control: Option<AnyElement>, column: bool) -> gpui::Div {
    div()
        .relative()
        .flex()
        .py(u(16.))
        .map(|el| {
            if column {
                el.flex_col().gap(u(12.))
            } else {
                el.items_center().justify_between().gap(u(20.))
            }
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(u(4.))
                .min_w_0()
                .when(!column, |el| el.flex_1())
                .child(text),
        )
        .children(control)
        .child(line())
}

fn line() -> gpui::Div {
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(u(1.))
        .bg(white(0.045))
}

/// Текст строки: подпись и подсказки.
fn texts(head: gpui::Div, hints: Vec<gpui::Div>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(u(4.))
        .min_w_0()
        .child(head)
        .children(hints)
}

/// `.runner-settings__message`: значок и текст (зелёный или `is-error`).
pub(super) fn message(text: &str, error: bool) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(u(8.))
        .text_px(12., 20.)
        .text_color(rgb(if error { ERROR } else { OK }))
        .child(fa_glyph(
            if error {
                "fas fa-exclamation-circle"
            } else {
                "fas fa-check"
            },
            12.,
        ))
        .child(text.to_owned())
}

/// Кнопка `.settings-btn`: высота от 40, поля 8/16, 13/24, 600, рамка inset `--l-border-strong`;
/// `ghost` — без заливки и рамки; `primary` — акцент; `danger` — красная рамка и текст; нажатие
/// `scale(.97)` `--l-fast`.
pub(super) fn btn(
    id: impl Into<SharedString>,
    look: Btn,
    disabled: bool,
    children: Vec<AnyElement>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let name: SharedString = id.into();
    let id = gpui::ElementId::Name(name.clone());
    let zoom = zoom(window);
    let focus = focus_handle(&id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let theme = cx.global::<Theme>();
    let radius = theme.len("l-radius-sm");
    let focus_color = theme.color("accent-focus");
    let (text, strong) = (theme.color("l-text"), theme.color("l-border-strong"));
    let danger: Hsla = rgba(0xe0455a73).into();
    let (bg, hover_bg, color, hover_color, frame) = match look {
        Btn::Plain => (white(0.05), white(0.1), text, text, Some(strong)),
        Btn::Ghost => (
            white(0.),
            white(0.06),
            theme.color("l-text-dim"),
            text,
            None,
        ),
        Btn::Primary => (
            theme.color("accent"),
            theme.color("accent-hover"),
            theme.color("accent-contrast-text"),
            theme.color("accent-contrast-text"),
            None,
        ),
        Btn::Danger => (
            white(0.05),
            rgba(0xe0455a1f).into(),
            rgb(0xff8796).into(),
            rgb(0xff8796).into(),
            Some(danger),
        ),
    };
    let pressed = (look == Btn::Primary).then(|| theme.color("accent-pressed"));
    let under = window_under(theme);
    let inner = id.clone();
    let press_id = child_id(&id, "press");
    let button = hover(id, Timing::fast(), move |t| {
        let t = if disabled { 0. } else { t };
        let el = div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(u(8.))
            .min_h(u(40.))
            .px(u(16.))
            .py(u(8.))
            .rounded(u(radius))
            .bg(mix(bg, hover_bg, t))
            .when_some(frame, |el, frame| el.shadow(vec![ring(zoom, 1., frame)]))
            .text_px(13., 24.)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(mix(color, hover_color, t))
            .whitespace_nowrap()
            .children(children)
            .when(disabled, |el| faded(el, 0.45, under))
            .when(!disabled, |el| {
                el.track_focus(&focus)
                    .cursor_pointer()
                    .when_some(pressed, |el, pressed| el.active(move |s| s.bg(pressed)))
                    .on_click(on_click)
            })
            .when(ring_on, |el| {
                el.child(outline(zoom, radius, 2., 2., focus_color))
            });
        press(press_id, Push::fast(0.97), el).live(!disabled)
    });
    // Цель стенда и тестов — имя кнопки.
    crate::snap::target(name, button)
}

/// Значок кнопки (12 px, .9) и подпись.
pub(super) fn icon_text(classes: &str, text: String) -> Vec<AnyElement> {
    let mut parts = Vec::new();
    if !classes.is_empty() {
        parts.push(fa_glyph(classes, 12.).opacity(0.9).into_any_element());
    }
    parts.push(div().child(text).into_any_element());
    parts
}

/// `.settings-select.runner-settings__select`: 280 px, подпись с многоточием и шеврон.
fn select_toggle(theme: &Theme, label: String, state: ToggleState) -> AnyElement {
    let open = state.open;
    let dim = theme.color("l-text-dim");
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(u(12.))
        .w(u(280.))
        .px(u(14.))
        .py(u(9.))
        .rounded(u(theme.len("l-radius-sm")))
        .border_1()
        .border_color(theme.color(if open { "l-border-strong" } else { "l-border" }))
        .bg(white(if open { 0.08 } else { 0.05 }))
        .hover(|s| s.bg(white(0.08)))
        .text_px(13., 20.)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.color("l-text"))
        .child(div().min_w_0().truncate().child(label))
        // `.settings-select.open i { transform: rotate(180deg) }` за `--l-fast`.
        .child(tween(
            "runner-chev",
            f32::from(u8::from(open)),
            Timing::fast(),
            move |t| chevron(10., 180. * t, dim),
        ))
        .into_any_element()
}

/// Поле `.runner-input` (280 px).
fn field(input: &Entity<TextInput>) -> AnyElement {
    div()
        .flex_none()
        .w(u(280.))
        .child(input.clone())
        .into_any_element()
}

impl Render for RunnerSettingsTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let Some(context) = ProfileContext::get(cx) else {
            return div().into_any_element();
        };
        let profile = context.state.clone();
        let Some(modal) = context.modal.upgrade() else {
            return div().into_any_element();
        };
        let (language, wall, tr, modal_rev) = {
            let modal = modal.read(cx);
            (modal.language, modal.wall.clone(), modal.tr.clone(), modal.view_rev())
        };
        if self.synced_modal_rev != Some(modal_rev) {
            let effects = {
                let p = profile.borrow();
                self.state
                    .set_props(p.account(), p.details(), p.deletion_at(), p.mute());
                self.state.set_hide_steam_id(p.hide_steam_id());
                self.state.set_support(
                    p.perks(),
                    p.subscriber(),
                    p.style_preset(),
                    p.telegram(),
                    p.discord_link(),
                )
            };
            self.synced_modal_rev = Some(modal_rev);
            self.run(effects, cx);
        }
        self.ensure_inputs(window, cx);
        let cache_key = (language, modal_rev, self.local_rev);
        let (view, picked_view, pronouns_items, country_items) = match &self.view_cache {
            Some((k, view, picked, pronouns, countries)) if *k == cache_key => (
                view.clone(),
                picked.clone(),
                pronouns.clone(),
                countries.clone(),
            ),
            _ => {
                let view = Rc::new(self.state.view(language, &wall, &tr));
                let picked = view
                    .style_group
                    .then(|| Rc::new(self.state.picker_view(&wall, &tr)));
                let pronouns: Vec<Item> = view
                    .pronouns_items
                    .iter()
                    .map(|(id, label)| Item::new(id.clone(), label.clone()))
                    .collect();
                let countries: Vec<Item> = view
                    .country_items
                    .iter()
                    .map(|(id, label)| Item::new(id.clone(), label.clone()))
                    .collect();
                self.view_cache = Some((
                    cache_key,
                    view.clone(),
                    picked.clone(),
                    pronouns.clone(),
                    countries.clone(),
                ));
                (view, picked, pronouns, countries)
            }
        };
        if self.synced_inputs_key != Some(cache_key) {
            self.sync_inputs(&view, cx);
            self.synced_inputs_key = Some(cache_key);
        }
        let theme = cx.global::<Theme>().clone();
        let t = |key: &str| tr(key, &[]);
        let mut body: Vec<AnyElement> = Vec::new();

        // ================= Отображение =================
        body.push(group(&t("#L_PROFILE_VIEW"), true, &theme));
        body.push(
            row(
                texts(
                    label(t("#L_RUNNER_SHOW_ID"), &theme),
                    vec![hint(t("#L_RUNNER_SHOW_ID_HINT"), false, &theme)],
                ),
                Some(
                    toggle("rs-show-id", view.show_id)
                        .label(t("#L_RUNNER_SHOW_ID"))
                        .on_change(|_, _, cx| {
                            if let Some(shell) = ShellContext::get(cx) {
                                shell.flows.clone().on_toggle_steam_id();
                            }
                        })
                        .into_any_element(),
                ),
                false,
            )
            .into_any_element(),
        );
        if !view.steam_id.is_empty() {
            let id_hint = div()
                .flex()
                .items_center()
                .text_px(12., 20.)
                .text_color(theme.color("l-text-faint"))
                .child("SteamID64:")
                .child(self.steam_id(&view, &tr, &theme, window, cx));
            let steam_id = view.steam_id.clone();
            let copy = btn(
                "rs-copy-id",
                Btn::Plain,
                false,
                icon_text("fas fa-copy", t("#L_RUNNER_COPY")),
                move |_, _, cx| {
                    let id = steam_id.clone();
                    if let Some(modal) = ProfileContext::get(cx).map(|c| c.modal.clone()) {
                        modal
                            .update(cx, |modal, cx| {
                                let effects = modal.state.borrow().copy(&id);
                                modal.run(effects, cx);
                            })
                            .ok();
                    }
                },
                window,
                cx,
            );
            let steam = btn(
                "rs-open-steam",
                Btn::Plain,
                false,
                icon_text("fab fa-steam", t("#L_PROFILE_OPEN_STEAM")),
                |_, _, cx| {
                    if let Some(modal) = ProfileContext::get(cx).map(|c| c.modal.clone()) {
                        modal
                            .update(cx, |modal, cx| {
                                let effects = modal.state.borrow().open_steam();
                                modal.run(effects, cx);
                            })
                            .ok();
                    }
                },
                window,
                cx,
            );
            body.push(
                row(
                    texts(label(t("#L_RUNNER_STEAM_ACCOUNT"), &theme), vec![id_hint]),
                    Some(buttons(vec![copy, steam], false)),
                    false,
                )
                .into_any_element(),
            );
        }

        // ================= Приватность =================
        body.push(group(&t("#L_RUNNER_PRIVACY_GROUP"), false, &theme));
        for item in &view.privacy {
            let flag = item.flag;
            let weak = cx.weak_entity();
            body.push(
                row(
                    texts(
                        label(item.label.clone(), &theme),
                        vec![hint(item.hint.clone(), item.error, &theme)],
                    ),
                    Some(
                        toggle(SharedString::from(format!("rs-privacy-{flag}")), item.on)
                            .disabled(item.disabled)
                            .label(item.label.clone())
                            .on_change(move |on, _, cx| {
                                weak.update(cx, |this, cx| {
                                    let effects = this.state.set_privacy(flag, !*on);
                                    this.run(effects, cx);
                                })
                                .ok();
                            })
                            .into_any_element(),
                    ),
                    false,
                )
                .into_any_element(),
            );
        }

        // ================= О себе =================
        let inputs = self.inputs.as_ref().expect("поля созданы");
        let (status_input, link_inputs, key_input) = (
            inputs.status.clone(),
            inputs.links.clone(),
            inputs.key.clone(),
        );
        body.push(group(&t("#L_RUNNER_ABOUT"), false, &theme));
        body.push(
            row(
                texts(
                    label(t("#L_RUNNER_STATUS"), &theme),
                    vec![hint(view.status_hint.clone(), view.status_error, &theme)],
                ),
                Some(field(&status_input)),
                false,
            )
            .into_any_element(),
        );
        let select = |id: &'static str,
                      items: Vec<Item>,
                      value: &str,
                      shown: String,
                      pick: fn(&mut SettingsState, &str),
                      cx: &mut Context<Self>| {
            let theme = theme.clone();
            let weak = cx.weak_entity();
            dropdown(id, items, Some(SharedString::from(value.to_owned())))
                .align_right()
                .toggle(move |state| select_toggle(&theme, shown, state))
                .on_select(move |item, _, cx| {
                    weak.update(cx, |this, cx| {
                        pick(&mut this.state, &item.id);
                        this.bump();
                        cx.notify();
                    })
                    .ok();
                })
                .into_any_element()
        };
        body.push(
            row(
                texts(
                    label(t("#L_RUNNER_PRONOUNS"), &theme),
                    vec![hint(t("#L_RUNNER_PRONOUNS_HINT"), false, &theme)],
                ),
                Some(select(
                    "rs-pronouns",
                    pronouns_items,
                    &view.pronouns,
                    view.pronouns_label.clone(),
                    SettingsState::set_pronouns,
                    cx,
                )),
                false,
            )
            .into_any_element(),
        );
        body.push(
            row(
                texts(
                    label(t("#L_RUNNER_COUNTRY"), &theme),
                    vec![hint(t("#L_RUNNER_COUNTRY_HINT"), false, &theme)],
                ),
                Some(select(
                    "rs-country",
                    country_items,
                    &view.country,
                    view.country_label.clone(),
                    SettingsState::set_country,
                    cx,
                )),
                false,
            )
            .into_any_element(),
        );
        body.push(group(&t("#L_RUNNER_LINKS"), false, &theme));
        for (index, link_row) in view.links.iter().enumerate() {
            let hints = (!link_row.hint.is_empty())
                .then(|| hint(link_row.hint.clone(), link_row.invalid, &theme))
                .into_iter()
                .collect();
            body.push(
                row(
                    texts(
                        brand_row(link_row.icon, link_row.label.clone(), None, &theme),
                        hints,
                    ),
                    Some(field(&link_inputs[index])),
                    false,
                )
                .into_any_element(),
            );
        }
        let revert = btn(
            "rs-revert",
            Btn::Ghost,
            view.busy || !view.dirty,
            icon_text("", t("#L_RUNNER_REVERT")),
            self.act(cx, |state, _| {
                state.revert();
                Vec::new()
            }),
            window,
            cx,
        );
        let save = btn(
            "rs-save",
            Btn::Primary,
            !view.can_save,
            icon_text("", t("#L_RUNNER_SAVE")),
            self.act(cx, |state, _| state.save()),
            window,
            cx,
        );
        body.push(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(u(8.))
                .pt(u(16.))
                .when(!view.message.is_empty(), |el| {
                    el.child(message(&view.message, !view.message_ok).mr_auto())
                })
                .child(revert)
                .child(save)
                .into_any_element(),
        );

        // ================= Подсветка ника и рамка =================
        if view.style_group {
            body.push(group(&t("#L_RUNNER_STYLE_GROUP"), false, &theme));
            let (vw, _) = viewport(window);
            let width = (vw - 64.).min(1040.) - 64.;
            if let Some(picked) = &picked_view
                && picked.visible
            {
                body.push(picker(picked, width, &tr, window, cx));
            }
            // Привязка Telegram — только в официальной сборке.
            if crate::shell::root::official(cx) {
                body.push(self.telegram(&view, &tr, &theme, window, cx));
            }
        }

        // ================= speedrun.com =================
        body.push(group("speedrun.com", false, &theme));
        body.extend(self.speedrun(&view, key_input, &tr, &theme, window, cx));
        if !view.speedrun_message.is_empty() {
            body.push(
                message(&view.speedrun_message, true)
                    .mt(u(12.))
                    .into_any_element(),
            );
        }

        // ================= Discord =================
        body.push(group("Discord", false, &theme));
        body.extend(self.discord(&view, &tr, &theme, window, cx));

        // ================= Данные =================
        body.push(group(&t("#L_RUNNER_DATA"), false, &theme));
        body.push(
            row(
                texts(
                    label(t("#L_RUNNER_TELEMETRY"), &theme),
                    vec![
                        hint(t("#L_RUNNER_TELEMETRY_HINT"), false, &theme),
                        div().mt(u(2.)).flex().child(
                            link("rs-privacy-link", t("#L_RUNNER_PRIVACY")).on_click(|_, _, cx| {
                                with_modal_app(cx, |modal, cx| {
                                    let effects = modal.state.borrow().open_external("privacy");
                                    modal.run(effects, cx);
                                })
                            }),
                        ),
                    ],
                ),
                None,
                false,
            )
            .into_any_element(),
        );
        for (id, title, hint_key, short, f) in [
            (
                "rs-logout",
                "#L_RUNNER_LOGOUT",
                "#L_RUNNER_LOGOUT_HINT",
                "#L_RUNNER_LOGOUT_SHORT",
                SettingsState::logout as fn(&mut SettingsState) -> Vec<Effect>,
            ),
            (
                "rs-logout-all",
                "#L_RUNNER_LOGOUT_ALL",
                "#L_RUNNER_LOGOUT_ALL_HINT",
                "#L_RUNNER_LOGOUT_ALL_SHORT",
                SettingsState::logout_all,
            ),
        ] {
            let button = btn(
                id,
                Btn::Plain,
                view.busy,
                icon_text("fas fa-sign-out-alt", t(short)),
                self.act(cx, move |state, _| f(state)),
                window,
                cx,
            );
            body.push(
                row(
                    texts(
                        label(t(title), &theme),
                        vec![hint(t(hint_key), false, &theme)],
                    ),
                    Some(button),
                    false,
                )
                .into_any_element(),
            );
        }
        body.extend(self.deletion(&view, &tr, &theme, window, cx));
        if !view.account_message.is_empty() {
            body.push(
                message(&view.account_message, true)
                    .mt(u(12.))
                    .into_any_element(),
            );
        }
        let inner = div()
            .id("runner-settings")
            .flex()
            .flex_col()
            .pt(u(28.))
            .px(u(32.))
            .pb(u(56.))
            .children(body);
        crate::snap::target(
            "profile-body",
            ScrollArea::new(("profile-scroll", 2usize), &self.scroll)
                .bar(Bar::Thin)
                .size_full()
                .child(inner),
        )
        .into_any_element()
    }
}

/// Действие над окном профиля из обработчика без `Context`.
fn with_modal_app(
    cx: &mut App,
    f: impl FnOnce(&mut super::modal::ProfileModal, &mut Context<super::modal::ProfileModal>),
) {
    if let Some(modal) = ProfileContext::get(cx).map(|c| c.modal.clone()) {
        modal.update(cx, f).ok();
    }
}

/// `.runner-settings__buttons` (`is-row` — в строку без переноса).
fn buttons(children: Vec<AnyElement>, nowrap: bool) -> AnyElement {
    div()
        .flex()
        .justify_end()
        .gap(u(8.))
        .map(|el| {
            if nowrap {
                el.flex_none()
            } else {
                el.flex_wrap()
            }
        })
        .children(children)
        .into_any_element()
}

impl RunnerSettingsTab {
    /// Капсула SteamID64: глаз слева, номер или точки той же длины на одном месте.
    fn steam_id(
        &self,
        view: &SettingsView,
        tr: &hlsr_ui::controllers::bridge::Tr,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let revealed = view.id_revealed;
        let title = tr(
            if revealed {
                "#L_RUNNER_ID_HIDE"
            } else {
                "#L_RUNNER_ID_REVEAL"
            },
            &[],
        );
        let (faint, text) = (theme.color("l-text-faint"), theme.color("l-text"));
        let zoom = zoom(window);
        let focus = focus_handle(&"rs-id-toggle".into(), window, cx);
        let ring_on = focus_visible(&focus, window);
        let accent = theme.color("accent");
        let toggle = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.state.toggle_id_revealed();
            this.bump();
            cx.notify();
        });
        let eye = hover("rs-id-eye", Timing::fast(), move |t| {
            div()
                .id("rs-id-toggle")
                .role(Role::Button)
                .button_text()
                .aria_label(title.clone())
                .tip(title)
                .track_focus(&focus)
                .relative()
                .w(u(22.))
                .h(u(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(white(0.08 * t))
                .text_color(mix(faint, text, t))
                .cursor_pointer()
                .on_click(toggle)
                .child(fa_glyph(
                    if revealed {
                        "fas fa-eye"
                    } else {
                        "fas fa-eye-slash"
                    },
                    10.,
                ))
                .when(ring_on, |el| el.child(outline(zoom, 999., 1., 2., accent)))
        });
        // Ширину задаёт невидимый образец из цифр той же длины.
        let length = view.id_length.max(0) as usize;
        let value = if revealed {
            div()
                .absolute()
                .inset_0()
                .text_color(text)
                .child(view.steam_id.clone())
        } else {
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_between()
                .text_color(faint)
                .children((0..length).map(|_| div().child("•")))
        };
        div()
            .flex()
            .items_center()
            .gap(u(2.))
            .ml(u(4.))
            .pl(u(1.))
            .pr(u(9.))
            .py(u(1.))
            .border_1()
            .border_color(white(0.08))
            .rounded_full()
            .bg(white(0.04))
            .child(eye)
            .child(
                div()
                    .relative()
                    .tnum()
                    .child(div().invisible().child("0".repeat(length)))
                    .child(value),
            )
            .into_any_element()
    }

    /// Строка Telegram: подпись, подсказки, «Обновить» и «Привязать»/«Отвязать».
    fn telegram(
        &self,
        view: &SettingsView,
        tr: &hlsr_ui::controllers::bridge::Tr,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = |key: &str| tr(key, &[]);
        let tg = &view.telegram;
        let dim = theme.color("l-text-dim");
        let refresh_icon = if tg.busy {
            icon("fas fa-sync-alt fa-spin", 12., dim)
        } else {
            fa_glyph("fas fa-sync-alt", 12.)
                .opacity(0.9)
                .into_any_element()
        };
        let refresh = btn(
            "rs-telegram-refresh",
            Btn::Ghost,
            tg.busy,
            vec![
                refresh_icon,
                div()
                    .child(t("#L_RUNNER_TELEGRAM_REFRESH"))
                    .into_any_element(),
            ],
            self.act(cx, |state, _| state.refresh_telegram()),
            window,
            cx,
        );
        let action = if tg.linked {
            btn(
                "rs-telegram-unlink",
                Btn::Plain,
                tg.busy,
                icon_text("", t("#L_RUNNER_TELEGRAM_UNLINK")),
                self.act(cx, |state, _| state.unlink_telegram()),
                window,
                cx,
            )
        } else {
            btn(
                "rs-telegram-link",
                Btn::Plain,
                tg.busy,
                icon_text("fab fa-telegram", t("#L_RUNNER_TELEGRAM_LINK")),
                self.act(cx, |state, now| state.link_telegram(now)),
                window,
                cx,
            )
        };
        let hints = tg
            .hints
            .iter()
            .map(|(text, error): &Hint| hint(text.clone(), *error, theme))
            .collect();
        crate::snap::target(
            "rs-telegram",
            row(
                texts(
                    brand_row("fab fa-telegram", tg.label.clone(), None, theme),
                    hints,
                ),
                Some(buttons(vec![refresh, action], true)),
                false,
            ),
        )
    }

    /// speedrun.com: ключ или привязка с рекордами.
    fn speedrun(
        &self,
        view: &SettingsView,
        key: Entity<TextInput>,
        tr: &hlsr_ui::controllers::bridge::Tr,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = |key: &str| tr(key, &[]);
        if !view.speedrun_linked {
            let link_btn = btn(
                "rs-sr-link",
                Btn::Primary,
                view.busy || self.state.speedrun_key().trim().is_empty(),
                icon_text("", t("#L_RUNNER_SR_LINK")),
                self.act(cx, |state, _| state.link_speedrun()),
                window,
                cx,
            );
            let auth =
                div()
                    .mt(u(2.))
                    .flex()
                    .child(
                        link("rs-sr-auth", t("#L_RUNNER_SR_AUTH")).on_click(|_, _, cx| {
                            with_modal_app(cx, |modal, cx| {
                                let effects = modal
                                    .state
                                    .borrow()
                                    .open_link("https://www.speedrun.com/settings/api");
                                modal.run(effects, cx);
                            })
                        }),
                    );
            return vec![
                row(
                    texts(
                        label(t("#L_RUNNER_SR_KEY"), theme),
                        vec![hint(t("#L_RUNNER_SR_HELP"), false, theme), auth],
                    ),
                    Some(
                        div()
                            .flex()
                            .gap(u(8.))
                            .child(div().flex_1().min_w_0().child(key))
                            .child(link_btn)
                            .into_any_element(),
                    ),
                    true,
                )
                .into_any_element(),
            ];
        }
        let bests = btn(
            "rs-sr-bests",
            Btn::Plain,
            view.busy,
            icon_text("fas fa-stopwatch", t("#L_RUNNER_SR_BESTS")),
            self.act(cx, |state, _| state.load_runs()),
            window,
            cx,
        );
        let unpin = view.pinned.then(|| {
            btn(
                "rs-sr-unpin",
                Btn::Ghost,
                view.busy,
                icon_text("fas fa-thumbtack", t("#L_RUNNER_SR_UNPIN")),
                self.act(cx, |state, _| state.unpin_run()),
                window,
                cx,
            )
        });
        let unlink = btn(
            "rs-sr-unlink",
            Btn::Ghost,
            view.busy,
            icon_text("", t("#L_RUNNER_SR_UNLINK")),
            self.act(cx, |state, _| state.unlink_speedrun()),
            window,
            cx,
        );
        let mut controls = vec![bests];
        controls.extend(unpin);
        controls.push(unlink);
        let mut out = vec![
            row(
                texts(
                    brand_row(
                        "fas fa-check-circle",
                        view.speedrun_name.clone(),
                        Some(rgb(0x4ad07a).into()),
                        theme,
                    ),
                    vec![hint(view.speedrun_hint.clone(), false, theme)],
                ),
                Some(buttons(controls, false)),
                false,
            )
            .into_any_element(),
        ];
        if !view.runs.is_empty() {
            let zoom = zoom(window);
            let mut list = div().flex().flex_col().gap(u(8.)).pt(u(8.)).pb(u(16.));
            for (index, run) in view.runs.iter().enumerate() {
                let medal = medal_color(run.medal);
                let place_color = medal.unwrap_or_else(|| theme.color("l-text-dim"));
                let place = (!run.place_label.is_empty()).then(|| {
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(6.))
                        .text_px(12., 16.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(place_color)
                        .tnum()
                        .whitespace_nowrap()
                        .when(medal.is_some(), |el| {
                            el.child(
                                svg()
                                    .data(LAMBDA.as_bytes())
                                    .size(u(14.))
                                    .text_color(place_color),
                            )
                        })
                        .child(run.place_label.clone())
                });
                let action = if run.pinned {
                    self.pinned_button(index, view.busy, tr, theme, window, cx)
                } else {
                    let id = run.id.clone();
                    btn(
                        format!("rs-pin-{index}"),
                        Btn::Plain,
                        view.busy,
                        icon_text("", t("#L_RUNNER_SR_PIN")),
                        self.act(cx, move |state, _| state.pin_run(&id)),
                        window,
                        cx,
                    )
                };
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(16.))
                        .min_h(u(56.))
                        .pl(u(16.))
                        .pr(u(8.))
                        .py(u(8.))
                        .rounded(u(theme.len("l-radius-sm")))
                        .bg(white(0.03))
                        .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .text_px(13., 20.)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.color("l-text"))
                                        .child(run.game.clone()),
                                )
                                .child(hint(run.category.clone(), false, theme)),
                        )
                        .children(place)
                        .child(
                            div()
                                .text_px(14., 20.)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.color("l-text"))
                                .tnum()
                                .child(run.time.clone()),
                        )
                        .child(action),
                );
            }
            out.push(list.into_any_element());
        } else if view.runs_loaded {
            out.push(
                div()
                    .pt(u(12.))
                    .pb(u(16.))
                    .text_px(12., 20.)
                    .text_color(theme.color("l-text-faint"))
                    .child(t("#L_RUNNER_SR_EMPTY"))
                    .into_any_element(),
            );
        }
        out
    }

    /// Закреплённый забег: «В профиле», при наведении и фокусе — «Открепить» цветом опасности.
    fn pinned_button(
        &self,
        index: usize,
        busy: bool,
        tr: &hlsr_ui::controllers::bridge::Tr,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = gpui::ElementId::from(("rs-unpin-run", index));
        let focus = focus_handle(&id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let zoom = zoom(window);
        let radius = theme.len("l-radius-sm");
        let (accent, danger) = (theme.color("l-accent"), rgb(0xff6b6b).into());
        let focus_color = theme.color("accent-focus");
        let on = tr("#L_RUNNER_SR_PINNED_BADGE", &[]);
        let off = tr("#L_RUNNER_SR_UNPIN", &[]);
        let click = self.act(cx, |state, _| state.unpin_run());
        let inner = id.clone();
        hover(id, Timing::fast(), move |t| {
            let over = !busy && (t > 0.5 || ring_on);
            div()
                .id(inner)
                .role(Role::Button)
                .button_text()
                .tip(off.clone())
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(8.))
                .min_h(u(40.))
                .px(u(16.))
                .py(u(8.))
                .rounded(u(radius))
                .bg(white(if busy { 0. } else { 0.06 * t }))
                .text_px(13., 24.)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if over { danger } else { accent })
                .whitespace_nowrap()
                .child(fa_glyph("fas fa-thumbtack", 12.).opacity(0.9))
                .child(if over { off.clone() } else { on.clone() })
                .when(busy, |el| el.opacity(0.45))
                .when(!busy, |el| {
                    el.track_focus(&focus).cursor_pointer().on_click(click)
                })
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., focus_color))
                })
        })
        .into_any_element()
    }

    /// Группа Discord: строка, «Как привязать», сообщение.
    fn discord(
        &self,
        view: &SettingsView,
        tr: &hlsr_ui::controllers::bridge::Tr,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = |key: &str| tr(key, &[]);
        let dc = &view.discord;
        let action = if dc.linked {
            btn(
                "rs-discord-unlink",
                Btn::Plain,
                dc.busy,
                icon_text("", t("#L_RUNNER_DISCORD_UNLINK")),
                self.act(cx, |state, _| state.unlink_discord()),
                window,
                cx,
            )
        } else if dc.code {
            let command = dc.command.clone();
            btn(
                "rs-discord-copy",
                Btn::Plain,
                false,
                icon_text("", t("#L_RUNNER_DISCORD_COPY")),
                move |_, _, cx| {
                    let command = command.clone();
                    with_modal_app(cx, move |modal, cx| {
                        let effects = modal.state.borrow().copy(&command);
                        modal.run(effects, cx);
                    })
                },
                window,
                cx,
            )
        } else {
            btn(
                "rs-discord-link",
                Btn::Plain,
                dc.busy,
                icon_text("fab fa-discord", t("#L_RUNNER_DISCORD_LINK")),
                self.act(cx, |state, _| state.request_discord_code()),
                window,
                cx,
            )
        };
        let hints = (!dc.hint.is_empty())
            .then(|| hint(dc.hint.clone(), false, theme))
            .into_iter()
            .collect();
        let mut out = vec![crate::snap::target(
            "rs-discord",
            row(
                texts(
                    brand_row("fab fa-discord", dc.label.clone(), None, theme),
                    hints,
                ),
                Some(buttons(vec![action], true)),
                false,
            ),
        )];
        if !dc.linked {
            out.push(self.steps(view, tr, theme, window, cx));
        }
        if !dc.message.is_empty() {
            out.push(message(&dc.message, true).mt(u(12.)).into_any_element());
        }
        out
    }

    /// «Как привязать»: заголовок-ссылка с шевроном и нумерованные шаги по щелчку.
    fn steps(
        &self,
        view: &SettingsView,
        tr: &hlsr_ui::controllers::bridge::Tr,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dc = &view.discord;
        let accent = theme.color("l-accent");
        let zoom = zoom(window);
        let focus = focus_handle(&"rs-discord-steps".into(), window, cx);
        let ring_on = focus_visible(&focus, window);
        let focus_color = theme.color("l-accent");
        let open = dc.help;
        let title = div()
            .id("rs-discord-steps")
            .role(Role::Button)
            .button_text()
            .aria_expanded(open)
            .track_focus(&focus)
            .relative()
            .flex()
            .items_center()
            .text_px(12., 20.)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(accent)
            .cursor_pointer()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.state.toggle_discord_help();
                this.bump();
                cx.notify();
            }))
            .child(div().w(u(12.)).child(fa_glyph(
                if open {
                    "fas fa-chevron-down"
                } else {
                    "fas fa-chevron-right"
                },
                10.,
            )))
            .child(
                div()
                    .pl(u(4.))
                    .underline()
                    .child(tr("#L_RUNNER_DISCORD_STEPS", &[])),
            )
            .when(ring_on, |el| {
                el.child(outline(zoom, 0., 2., 2., focus_color))
            });
        let steps =
            open.then(|| {
                let invite = dc.invite;
                div().mt(u(6.)).flex().flex_col().gap(u(4.)).children(
                    dc.steps
                        .iter()
                        .enumerate()
                        .map(|(index, (text, with_link))| {
                            div()
                                .flex()
                                .child(
                                    div()
                                        .flex_none()
                                        .w(u(20.))
                                        .pr(u(5.))
                                        .flex()
                                        .justify_end()
                                        .child(format!("{}.", index + 1)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(text.clone())
                                        // Ссылка — следом за коротким текстом шага, как inline.
                                        .when(*with_link, |el| {
                                            el.flex().flex_wrap().gap_x(u(4.)).child(
                                                link(
                                                    "rs-discord-open",
                                                    tr("#L_RUNNER_DISCORD_OPEN", &[]),
                                                )
                                                .on_click(move |_, _, cx| {
                                                    with_modal_app(cx, move |modal, cx| {
                                                        let effects =
                                                            modal.state.borrow().open_link(invite);
                                                        modal.run(effects, cx);
                                                    })
                                                }),
                                            )
                                        }),
                                )
                        }),
                )
            });
        div()
            .mt(u(12.))
            .flex()
            .flex_col()
            .items_start()
            .text_px(13., 20.)
            .text_color(theme.color("l-text-dim"))
            .child(crate::snap::target("rs-discord-steps", title))
            .children(steps)
            .into_any_element()
    }

    /// Удаление: запланировано, строка или плашка с отметкой и отсчётом.
    fn deletion(
        &self,
        view: &SettingsView,
        tr: &hlsr_ui::controllers::bridge::Tr,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = |key: &str| tr(key, &[]);
        if view.deletion_scheduled {
            let cancel = btn(
                "rs-delete-cancel",
                Btn::Plain,
                view.busy,
                icon_text("", t("#L_RUNNER_DELETE_CANCEL")),
                self.act(cx, |state, _| state.cancel_delete()),
                window,
                cx,
            );
            return vec![
                row(
                    texts(
                        label(t("#L_RUNNER_DELETE_SCHEDULED"), theme).text_color(rgb(0xff8796)),
                        vec![hint(view.deletion_hint.clone(), false, theme)],
                    ),
                    Some(cancel),
                    false,
                )
                .into_any_element(),
            ];
        }
        if !view.confirming {
            let delete = btn(
                "rs-delete",
                Btn::Danger,
                view.busy,
                icon_text("fas fa-trash-alt", t("#L_RUNNER_DELETE_SHORT")),
                self.act(cx, |state, _| state.prepare_delete()),
                window,
                cx,
            );
            return vec![
                row(
                    texts(
                        label(t("#L_RUNNER_DELETE"), theme),
                        vec![hint(t("#L_RUNNER_DELETE_HINT"), false, theme)],
                    ),
                    Some(delete),
                    false,
                )
                .into_any_element(),
            ];
        }
        let zoom = zoom(window);
        let close = btn(
            "rs-delete-close",
            Btn::Ghost,
            view.busy,
            icon_text("", t("#L_RUNNER_CANCEL")),
            self.act(cx, |state, _| state.close_delete()),
            window,
            cx,
        );
        let mut confirm_parts = icon_text("", t("#L_RUNNER_DELETE_CONFIRM"));
        if view.countdown > 0 {
            confirm_parts.push(
                div()
                    .min_w(u(20.))
                    .px(u(6.))
                    .rounded(u(4.))
                    .bg(white(0.08))
                    .flex()
                    .justify_center()
                    .tnum()
                    .child(view.countdown.to_string())
                    .into_any_element(),
            );
        }
        let confirm = btn(
            "rs-delete-confirm",
            Btn::Danger,
            view.busy || !view.agreed || view.countdown > 0,
            confirm_parts,
            self.act(cx, |state, _| state.confirm_delete()),
            window,
            cx,
        );
        let weak = cx.weak_entity();
        let agree = check(
            "rs-delete-agree",
            if view.agreed {
                Checked::On
            } else {
                Checked::Off
            },
            CheckLook::Native,
        )
        .side(16.)
        .items_start()
        .gap(u(12.))
        .text_px(13., 20.)
        .text_color(theme.color("l-text"))
        .on_change(move |on, _, cx| {
            weak.update(cx, |this, cx| {
                this.state.set_delete_agreed(*on);
                this.bump();
                cx.notify();
            })
            .ok();
        })
        .child(div().flex_1().min_w_0().child(t("#L_RUNNER_DELETE_AGREE")));
        let items = [
            "#L_RUNNER_DELETE_ITEM_DATA",
            "#L_RUNNER_DELETE_ITEM_WAIT",
            "#L_RUNNER_DELETE_ITEM_FINAL",
        ];
        vec![
            div()
                .mt(u(16.))
                .pt(u(16.))
                .px(u(20.))
                .pb(u(20.))
                .rounded(u(theme.len("l-radius-sm")))
                .bg(rgba(0xe0455a0f))
                .shadow(vec![ring(zoom, 1., rgba(0xe0455a59).into())])
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(12.))
                        .text_px(14., 24.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.color("l-text"))
                        .child(
                            fa_glyph("fas fa-exclamation-triangle", 13.).text_color(rgb(0xff8796)),
                        )
                        .child(t("#L_RUNNER_DELETE")),
                )
                .child(
                    div()
                        .mt(u(8.))
                        .mb(u(16.))
                        .flex()
                        .flex_col()
                        .gap(u(4.))
                        .text_px(12.5, 20.)
                        .text_color(theme.color("l-text-dim"))
                        .children(items.iter().map(|key| {
                            div()
                                .flex()
                                .child(div().flex_none().w(u(20.)).pl(u(6.)).child("•"))
                                .child(div().flex_1().min_w_0().child(t(key)))
                        })),
                )
                .child(agree)
                .child(
                    div()
                        .mt(u(16.))
                        .flex()
                        .justify_end()
                        .gap(u(8.))
                        .child(close)
                        .child(confirm),
                )
                .into_any_element(),
        ]
    }
}
