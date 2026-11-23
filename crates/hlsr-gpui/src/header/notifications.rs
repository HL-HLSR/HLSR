//! Колокольчик и панель уведомлений (`NotificationCenter.vue` 0.4.9): кнопка со счётчиком,
//! панель (`notif-flow`: растёт из колокольчика со скруглением 48 px, `blur(6px)` нет — у блоков
//! GPUI нет `filter`), шапка без рамки, пункты без рамки и фона (наведение .05, непрочитанный —
//! точкой), кнопки версий и сбоя, всплывающее превью (4,8 с, пауза под указателем, «возврат в колокольчик» —
//! `notif-receive`). Под окнами превью ждёт (`suspended` = `overlayOpen`). Логика —
//! [`NotifMachine`] (`hlsr_ui::controllers::notifications`), события — сценарии оболочки.

use crate::icons::{fa_glyph, icon};
use crate::kit::button::{child_id, focus_handle, focus_visible, outline, white};
use crate::layer::{TipExt, anchor_probe, pop_surface_rounded};
use crate::motion::{
    AnimGate, Frame, Origin, Timing, apply, hover, keyframes, mix, presence_with, registry,
};
use crate::paint::{Radial, ring, shadow};
use crate::scale::{u, zoom, zpx};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::shell::root::{ShellContext, ShellRoot};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{
    Anchor, AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context, ElementId, FocusHandle,
    FontWeight, Hsla, IntoElement, MouseDownEvent, Pixels, Rems, Render, Subscription, Task,
    WeakEntity, Window, anchored, canvas, deferred, div, point, prelude::*, rgb,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::bridge::Tr;
use hlsr_ui::controllers::notifications::{Effect, Event, NotifMachine, NotifRow, Tag, Target};
use hlsr_ui::controllers::wall_clock::WallClock;
use hlsr_ui::services::Services;
use serde_json::json;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// z-index 0.4.9: панель 40, превью 41 (под окнами 50).
const Z_PANEL: usize = 40;
const Z_PEEK: usize = 41;
const FLOW: &str = "notif-flow";
/// `.notif-flow-enter-from { border-radius: 48px }`.
const FLOW_RADIUS: f32 = 48.;

/// `transform-origin: calc(100% - 12px) calc(var(--l-header-control) * -0.5 - 8px)` панели и
/// превью: середина колокольчика над ними.
fn flow_origin(theme: &Theme) -> Origin {
    Origin::new((1., -12.), (0., -theme.len("l-header-control") / 2. - 8.))
}

/// Скругление `notif-flow` в кадре `frame`: от 48 px к `radius` с той же кривой и временем, что
/// у `transform` (`--l-dur-slow` `--l-ease`) — по доле пути масштаба.
fn flow_radius(frame: &Frame, radius: f32) -> f32 {
    // Путь отрисовки: без записи в реестре — сразу конечное скругление, не паника.
    let Some(&from) = registry()
        .transitions
        .get(FLOW)
        .and_then(|flow| flow.hidden.get(&hlsr_ui::motion::Prop::Scale))
    else {
        return radius;
    };
    let done = ((frame.scale - from) / (1. - from)).clamp(0., 1.) as f32;
    FLOW_RADIUS + (radius - FLOW_RADIUS) * done
}

type Click = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct NotificationCenter {
    services: Services,
    machine: NotifMachine,
    preview_timer: Option<Task<()>>,
    /// Конец ухода превью в колокольчик (`after-leave`).
    leave_timer: Option<Task<()>>,
    /// Номер приёма превью: новый — `notif-receive` заново.
    receives: usize,
    /// Рамка колокольчика прошлого кадра (px окна): от неё панель и превью.
    bell: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
    focus_bell: bool,
    scroll: Scroll,
    suspended: bool,
    /// Последнее показанное превью: уходит с тем же содержимым.
    peek: NotifRow,
    peek_update: bool,
    /// Esc, пока панель открыта (`keydown` документа с захватом): до окон и действий.
    esc: Option<Subscription>,
    /// Конец ухода закрытой панели: до него пункты ещё строятся.
    panel_until: Option<Instant>,
    _events: Task<()>,
}

impl NotificationCenter {
    pub fn new(services: &Services, cx: &mut Context<NotificationCenter>) -> NotificationCenter {
        let start = Instant::now();
        let mono = Rc::new(move || start.elapsed().as_secs_f64() * 1000.);
        let events =
            crate::bridge::listen(cx, services.subscribe(), |this: &mut Self, event, cx| {
                if event.channel == "notifications-changed" {
                    let effects = this.machine.changed();
                    this.run(effects, cx);
                }
            });
        let mut this = NotificationCenter {
            services: services.clone(),
            machine: NotifMachine::new(WallClock::system(), mono),
            preview_timer: None,
            leave_timer: None,
            receives: 0,
            bell: Rc::default(),
            focus: cx.focus_handle().tab_stop(true),
            focus_bell: false,
            scroll: Scroll::new(),
            suspended: false,
            peek: NotifRow::default(),
            peek_update: false,
            esc: None,
            panel_until: None,
            _events: events,
        };
        let effects = this.machine.mounted();
        this.run(effects, cx);
        this
    }

    /// Побочные действия машины: вызовы, таймер превью, события оболочке, буфер обмена.
    fn run(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Invoke { tag, channel, args } => {
                    let reply = self.services.invoke(channel, args);
                    cx.spawn(async move |this, cx| {
                        let reply = reply.await;
                        this.update(cx, |this, cx| {
                            let effects = match tag {
                                Tag::List(sequence) => this.machine.list_reply(sequence, reply),
                                // `await invoke(…); refresh()`: отказ до refresh не доходит.
                                Tag::Then if reply.is_ok() => this.machine.then_refresh(),
                                Tag::Then | Tag::Ignore => Vec::new(),
                            };
                            this.run(effects, cx);
                        })
                        .ok();
                    })
                    .detach();
                }
                Effect::StartPreviewTimer(ms) => {
                    self.preview_timer = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_secs_f64(ms.max(0.) / 1000.))
                            .await;
                        this.update(cx, |this, cx| {
                            this.preview_timer = None;
                            let effects = this.machine.dismiss_preview();
                            this.run(effects, cx);
                        })
                        .ok();
                    }));
                }
                Effect::StopPreviewTimer => self.preview_timer = None,
                // `uiState.openDropdowns` в 0.4.9 только держит Esc окна под меню; Esc панели
                // здесь дальше не идёт.
                Effect::OpenMenus(_) => {}
                Effect::FocusToggle => self.focus_bell = true,
                Effect::NextTick => cx
                    .spawn(async move |this, cx| {
                        this.update(cx, |this, cx| {
                            let effects = this.machine.tick_after_dismiss();
                            this.run(effects, cx);
                        })
                        .ok();
                    })
                    .detach(),
                Effect::Emit(event) => emit(event, cx),
                Effect::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            }
        }
        // Превью уходит в колокольчик: колокольчик «принимает» его, пока идёт уход.
        if self.machine.returning() && self.leave_timer.is_none() {
            self.receives += 1;
            let leave = registry().transition_length(FLOW, false, AnimGate::still(cx));
            self.leave_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs_f64(leave.max(0.) / 1000.))
                    .await;
                this.update(cx, |this, cx| {
                    this.leave_timer = None;
                    let effects = this.machine.after_preview_leave();
                    this.run(effects, cx);
                })
                .ok();
            }));
        }
        let open = self.machine.is_open();
        if open != self.esc.is_some() {
            self.panel_until = (!open).then(|| {
                let leave = registry().transition_length(FLOW, false, AnimGate::still(cx));
                Instant::now() + Duration::from_secs_f64(leave.max(0.) / 1000.)
            });
            self.esc = open.then(|| {
                let this = cx.entity().downgrade();
                cx.intercept_keystrokes(move |event, _, cx| {
                    if event.keystroke.key != "escape" {
                        return;
                    }
                    cx.stop_propagation();
                    let this = this.clone();
                    cx.defer(move |cx| {
                        this.update(cx, |this, cx| {
                            let effects = this.machine.close(true);
                            this.run(effects, cx);
                        })
                        .ok();
                    });
                })
            });
        }
        cx.notify();
    }

    /// Обработчик щелчка: шаг машины по `f`.
    fn act(cx: &Context<Self>, f: impl Fn(&mut NotifMachine) -> Vec<Effect> + 'static) -> Click {
        let this = cx.entity().downgrade();
        Rc::new(move |_, cx| {
            this.update(cx, |this, cx| {
                let opening = !this.machine.is_open();
                let effects = f(&mut this.machine);
                // Список создаётся заново при каждом открытии — прокрутка с начала.
                if opening && this.machine.is_open() {
                    this.scroll = Scroll::new();
                }
                this.run(effects, cx);
            })
            .ok();
        })
    }
}

/// `$emit` колокольчика — обработчики `LauncherShell`.
fn emit(event: Event, cx: &mut App) {
    let Some(flows) = ShellContext::get(cx).map(|shell| shell.flows.clone()) else {
        return;
    };
    let payload = |target: Target| json!({"target": target.target, "gameId": target.game_id, "version": target.version});
    match event {
        Event::OpenDump(game) => flows.on_open_crash_dump(game.as_deref().unwrap_or("")),
        Event::ExportDiagnostics => flows.open_diagnostics(),
        Event::OpenDiscord => flows.on_open_discord(),
        Event::WhatsNew(target) => flows.on_whats_new(&payload(target)),
        Event::RunUpdate(target) => flows.on_run_update_action(&payload(target)),
    }
}

/// `--notif-tint`: акцент, у предупреждения и ошибки — свои.
fn tint(level: &str, theme: &Theme) -> Hsla {
    match level {
        "warning" => rgb(0xffb37a).into(),
        "error" => rgb(0xff7a7a).into(),
        _ => theme.color("l-accent"),
    }
}

/// `.notif__plate`: значок в плашке.
fn plate(size: f32, radius: Rems, bg: Hsla, color: Hsla, classes: &str, glyph: f32) -> gpui::Div {
    div()
        .flex_none()
        .size(u(size))
        .rounded(radius)
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(classes, glyph, color))
}

/// Обработчик щелчка, который не доходит до пункта (`@click.stop`).
fn stop(on_click: Click) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        cx.stop_propagation();
        on_click(window, cx);
    }
}

/// `.notif__action`: квадрат 32, значок 12; наведение — `--l-accent-soft`.
fn action(
    id: impl Into<ElementId>,
    classes: &'static str,
    label: String,
    theme: &Theme,
    size: f32,
    on_click: Click,
) -> impl IntoElement {
    let id: ElementId = id.into();
    let (dim, text, soft) = (
        theme.color("l-text-dim"),
        theme.color("l-text"),
        theme.color("l-accent-soft"),
    );
    let radius = theme.rem("l-radius-sm");
    hover(child_id(&id, "hover"), Timing::fast(), move |t| {
        div()
            .id(id)
            .flex_none()
            .size(u(size))
            .rounded(radius)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .button_text()
            .bg(soft.opacity(t))
            .text_color(mix(dim, text, t))
            .tip(label)
            .on_click(stop(on_click))
            .child(fa_glyph(classes, 12.).w(u(16.)).flex().justify_center())
    })
}

/// `.notif__crash-btn` (и `__update-btn`): высота 24, рамка тенью внутрь.
fn crash_button(
    id: String,
    classes: &'static str,
    label: String,
    update: bool,
    theme: &Theme,
    zoom: f32,
    on_click: Click,
) -> impl IntoElement {
    let (text, dim, accent) = (
        theme.color("l-text"),
        theme.color("l-text-dim"),
        theme.color("accent"),
    );
    let (soft, edge, strong) = (
        theme.color("accent-soft"),
        theme.color("accent-border"),
        theme.color("l-border-strong"),
    );
    let radius = theme.rem("l-radius-sm");
    hover(
        ElementId::Name(format!("{id}-hover").into()),
        Timing::fast(),
        move |t| {
            let (bg, line, glyph) = if update {
                (soft, mix(edge, accent, t), accent)
            } else {
                (white(0.05 + 0.05 * t), strong, dim)
            };
            div()
                .id(ElementId::Name(id.into()))
                .flex()
                .items_center()
                .gap(u(8.))
                .h(u(24.))
                .px(u(12.))
                .rounded(radius)
                .bg(bg)
                .shadow(vec![ring(zoom, 1., line)])
                .text_color(text)
                .text_px(11.5, 16.)
                .whitespace_nowrap()
                .cursor_pointer()
                .button_text()
                .on_click(stop(on_click))
                .child(
                    fa_glyph(classes, 11.)
                        .w(u(12.))
                        .flex()
                        .justify_center()
                        .text_color(glyph),
                )
                .child(label)
        },
    )
}

/// Слушает окно, пока панель открыта: нажатие мимо колокольчика и панели закрывает
/// (`onDocumentDown`; Esc — [`NotificationCenter::esc`]).
fn watch(bell: Bounds<Pixels>, this: WeakEntity<NotificationCenter>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                let at = event.position;
                if phase.capture() && !bounds.contains(&at) && !bell.contains(&at) {
                    this.update(cx, |this, cx| {
                        let effects = this.machine.close(false);
                        this.run(effects, cx);
                    })
                    .ok();
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

impl NotificationCenter {
    /// Пункт списка (`.notif__item`).
    fn item(
        &self,
        index: usize,
        row: NotifRow,
        tr: &Tr,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let radius_sm = theme.rem("l-radius-sm");
        let tint = tint(row.level, &theme);
        let element_id = ElementId::Name(format!("notif-item-{}", row.id).into());
        let focus = focus_handle(&element_id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let id = row.id.clone();
        let read = Self::act(cx, move |machine| machine.mark_read(&id));
        let (text, dim, faint) = (
            theme.color("l-text"),
            theme.color("l-text-dim"),
            theme.color("l-text-faint"),
        );

        let mut buttons: Vec<AnyElement> = Vec::new();
        let button = |name: &str, classes, label: String, update, on_click: Click| {
            crash_button(
                format!("notif-{name}-{}", row.id),
                classes,
                label,
                update,
                &theme,
                zoom,
                on_click,
            )
            .into_any_element()
        };
        if row.whats_new {
            let id = row.id.clone();
            buttons.push(button(
                "whats-new",
                "fas fa-list-ul",
                tr("#L_N_WHATS_NEW", &[]),
                false,
                Self::act(cx, move |machine| machine.whats_new(&id)),
            ));
        }
        if row.update {
            let id = row.id.clone();
            buttons.push(button(
                "update",
                "fas fa-arrow-circle-down",
                tr("#L_N_UPDATE_NOW", &[]),
                true,
                Self::act(cx, move |machine| machine.run_update(&id)),
            ));
        }
        let version_row = buttons.len();
        if row.crash {
            let id = row.id.clone();
            let act = |f: fn(&NotifMachine, &str) -> Vec<Effect>| {
                let id = id.clone();
                Self::act(cx, move |machine| f(machine, &id))
            };
            if row.crash_dump {
                buttons.push(button(
                    "dump",
                    "fas fa-folder-open",
                    tr("#L_CR_OPEN_FOLDER", &[]),
                    false,
                    act(NotifMachine::open_dump),
                ));
            }
            buttons.push(button(
                "export",
                "fas fa-file-export",
                tr("#L_CR_EXPORT", &[]),
                false,
                act(|_, _| vec![Effect::Emit(Event::ExportDiagnostics)]),
            ));
            buttons.push(button(
                "discord",
                "fab fa-discord",
                "Discord".into(),
                false,
                act(|_, _| vec![Effect::Emit(Event::OpenDiscord)]),
            ));
            buttons.push(button(
                "copy",
                "fas fa-copy",
                tr("#L_CR_COPY", &[]),
                false,
                act(NotifMachine::copy_code),
            ));
        }
        let crash_buttons = buttons.split_off(version_row);
        let actions = |buttons: Vec<AnyElement>| {
            (!buttons.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_wrap()
                    .gap(u(8.))
                    .mt(u(4.))
                    .children(buttons)
            })
        };

        let body = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(u(4.))
            .pt(u(6.))
            // Заголовок 13/500 (переносится по ширине) и «×N» 11/600: в строке 20 их базовые
            // линии сходятся (в 0.4.9 у счётчика line-height 1 на линии заголовка).
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_x(u(4.))
                    .text_color(text)
                    .child(
                        div()
                            .flex_shrink(1.)
                            .min_w_0()
                            .text_px(13., 20.)
                            .font_weight(FontWeight::MEDIUM)
                            .child(row.title),
                    )
                    .when(!row.count.is_empty(), |line| {
                        line.child(
                            div()
                                .flex_none()
                                .text_px(11., 20.)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(faint)
                                .child(row.count),
                        )
                    }),
            )
            .when(!row.detail.is_empty(), |body| {
                body.child(
                    div()
                        .text_px(12., 16.)
                        .text_color(dim)
                        .clamp(3)
                        .child(row.detail),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .line_height(u(16.))
                    .text_color(faint)
                    .child(div().text_size(u(11.)).child(row.time))
                    .child(
                        div()
                            .font_family("Consolas")
                            .text_size(u(10.5))
                            .child(row.code),
                    ),
            )
            .children(actions(buttons))
            .children(actions(crash_buttons));

        let target = row.id.clone();
        let unread = !row.read;
        let accent = theme.color("accent");
        let focus_ring = theme.color("accent-focus");
        let icon = row.icon;
        let el = hover(
            ElementId::Name(format!("notif-item-hover-{}", row.id).into()),
            Timing::fast(),
            move |t| {
                div()
                    .id(element_id)
                    .track_focus(&focus)
                    .relative()
                    .flex_none()
                    .flex()
                    .items_start()
                    .gap(u(12.))
                    .p(u(12.))
                    .rounded(radius_sm)
                    .bg(white(0.05 * t))
                    .cursor_pointer()
                    .button_text()
                    .on_click(move |_, window, cx| read(window, cx))
                    .child(plate(32., radius_sm, tint.opacity(0.16), tint, icon, 14.))
                    .child(body)
                    .when(unread, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .mt(u(12.))
                                .size(u(8.))
                                .rounded_full()
                                .bg(accent),
                        )
                    })
                    .when(ring_on, |el| {
                        el.child(outline(zoom, theme.len("l-radius-sm"), -2., 2., focus_ring))
                    })
            },
        );
        let rise = keyframes(
            ElementId::Name(format!("notif-rise-{target}").into()),
            "notif-item",
            move |frame| apply(div().child(el), frame),
        )
        .index(index.min(8));
        crate::snap::target(format!("notif-item-{target}"), rise)
    }
}

impl Render for NotificationCenter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `watch suspended`: окно поверх прячет превью и возвращает его в очередь.
        let suspended = ShellRoot::overlay_open(cx);
        if suspended != self.suspended {
            self.suspended = suspended;
            let effects = self.machine.set_suspended(suspended);
            self.run(effects, cx);
        }
        if std::mem::take(&mut self.focus_bell) {
            window.focus(&self.focus, cx);
        }
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let shell = ShellContext::get(cx).map(|shell| shell.shell.clone());
        let lang = shell
            .as_ref()
            .map_or(Language::En, |shell| shell.language());
        let tr: Tr = Rc::new(move |key, args| match &shell {
            Some(shell) => shell.tr(key, args),
            None => hlsr_ui::tr(lang, key, args),
        });
        let view = self.machine.view(lang, &tr);
        if view.peek_shown {
            self.peek = view.peek.clone();
            self.peek_update = view.peek_update;
        }
        let (text, dim, faint) = (
            theme.color("l-text"),
            theme.color("l-text-dim"),
            theme.color("l-text-faint"),
        );
        let accent = theme.color("accent");
        let radius_sm = theme.rem("l-radius-sm");

        // Колокольчик: цвет `--l-text-dim`, открыт/наведён/есть превью — `--l-text`, нажат — акцент.
        let lit = view.open || view.announcing;
        let receiving = view.returning.then_some(self.receives);
        let badge = (view.unread > 0).then(|| view.badge.clone());
        let inset = theme.len("l-header-inset");
        let ring_on = focus_visible(&self.focus, window);
        let focus = self.focus.clone();
        let probe = anchor_probe(self.bell.clone());
        let toggle = Self::act(cx, NotifMachine::toggle);
        let title = tr("#L_N_TITLE", &[]);
        let bell_theme = theme.clone();
        let bell = hover("notif-bell-hover", Timing::fast(), move |t| {
            let theme = bell_theme;
            let base = mix(dim, text, if lit { 1. } else { t });
            // Цвет значка — от кнопки (нажатие красит её акцентом); при приёме — свой.
            let glyph = move |scale: f32, color: Option<Hsla>| {
                div()
                    .size(u(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        fa_glyph("fas fa-bell", 24. * scale)
                            .when_some(color, |glyph, color| glyph.text_color(color)),
                    )
            };
            let bell_icon = match receiving {
                // notif-receive: 82 % — ×1.12 и цвет акцента.
                Some(receive) => keyframes(
                    ElementId::named_usize("notif-receive", receive),
                    "notif-receive",
                    move |frame| {
                        glyph(
                            frame.scale as f32,
                            Some(mix(base, accent, frame.mix as f32)),
                        )
                    },
                )
                .into_any_element(),
                None => glyph(1., None).into_any_element(),
            };
            div()
                .id("notif-bell")
                .track_focus(&focus)
                .relative()
                .size(theme.rem("l-header-control"))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.rem("l-radius-sm"))
                .cursor_pointer()
                .button_text()
                .text_color(base)
                .active(|style| style.text_color(accent))
                .tip(title)
                .on_click(move |_, window, cx| toggle(window, cx))
                .child(bell_icon)
                .children(badge.map(|badge| {
                    div()
                        .absolute()
                        .top(u(-4.))
                        .right(u(inset - 4.))
                        .min_w(u(16.))
                        .h(u(16.))
                        .px(u(4.))
                        .rounded(u(8.))
                        .bg(accent)
                        .text_color(theme.color("accent-contrast-text"))
                        .text_px(10., 16.)
                        .font_weight(FontWeight::BOLD)
                        .flex()
                        .justify_center()
                        .child(badge)
                }))
                .child(probe)
                .when(ring_on, |el| {
                    el.child(outline(
                        zoom,
                        theme.len("l-radius-sm"),
                        0.,
                        2.,
                        theme.color("accent-focus"),
                    ))
                })
        });
        let bell = crate::snap::target("notif-bell", bell);

        // Панель и превью — от правого нижнего угла колокольчика, на 8 ниже.
        let at = self.bell.get();
        let at = point(at.right(), at.bottom() + zpx(zoom, 8.));
        let this = cx.entity().downgrade();

        let panel = {
            let open = view.open;
            let bell = self.bell.get();
            let height = (f32::from(window.viewport_size().height) / zoom - 120.).min(460.);
            let has_items = view.has_items;
            let head = {
                let mark_all = Self::act(cx, NotifMachine::mark_all_read);
                let clear = Self::act(cx, NotifMachine::clear_all);
                let theme = theme.clone();
                let title = tr("#L_N_TITLE", &[]);
                let labels = (tr("#L_N_MARK_ALL", &[]), tr("#L_N_CLEAR", &[]));
                keyframes("notif-head", "notif-head", move |frame| {
                    let el = div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(u(12.))
                        .p(u(8.))
                        .rounded(radius_sm)
                        .child(plate(32., radius_sm, white(0.04), dim, "fas fa-bell", 14.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(text)
                                .text_px(15., 20.)
                                .font_weight(FontWeight::BOLD)
                                .child(title),
                        )
                        .when(has_items, |el| {
                            el.child(
                                div()
                                    .flex()
                                    .gap(u(4.))
                                    .child(action(
                                        "notif-mark-all",
                                        "fas fa-check-double",
                                        labels.0,
                                        &theme,
                                        32.,
                                        mark_all,
                                    ))
                                    .child(action(
                                        "notif-clear",
                                        "fas fa-trash-alt",
                                        labels.1,
                                        &theme,
                                        32.,
                                        clear,
                                    )),
                            )
                        });
                    apply(el, frame)
                })
            };
            // Закрытая и ушедшая панель не видна: пункты не строятся.
            let shown = open || self.panel_until.is_some_and(|until| Instant::now() < until);
            let content = if has_items {
                let rows = if shown {
                    self.machine.rows(lang, &tr)
                } else {
                    Vec::new()
                };
                let rows: Vec<AnyElement> = rows
                    .into_iter()
                    .enumerate()
                    .map(|(index, row)| self.item(index, row, &tr, window, cx).into_any_element())
                    .collect();
                ScrollArea::new("notif-list", &self.scroll)
                    .bar(Bar::Thin)
                    .min_h_0()
                    .child(div().flex().flex_col().gap(u(4.)).children(rows))
                    .into_any_element()
            } else {
                let empty = tr("#L_N_EMPTY", &[]);
                keyframes("notif-empty", "notif-empty", move |frame| {
                    apply(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(u(12.))
                            .p(u(24.))
                            .text_color(faint)
                            .text_px(13., 20.)
                            .child(plate(
                                48.,
                                radius_sm,
                                white(0.04),
                                faint,
                                "far fa-bell-slash",
                                20.,
                            ))
                            .child(empty),
                        frame,
                    )
                })
                .into_any_element()
            };
            let this = this.clone();
            let origin = flow_origin(&theme);
            let theme = theme.clone();
            presence_with("notif-panel-flow", open, FLOW, move |frame| {
                let radius = u(flow_radius(frame, theme.len("l-radius")));
                pop_surface_rounded(&theme, zoom, radius)
                    .id("notif-panel")
                    .relative()
                    .w(u(372.))
                    .max_h(u(height))
                    .flex()
                    .flex_col()
                    .gap(u(8.))
                    .p(u(8.))
                    .overflow_hidden()
                    // Уходящая панель мышь не ловит (`pointer-events: none`).
                    .when(open, |el| el.occlude().child(watch(bell, this)))
                    .child(head)
                    .child(content)
            })
            .origin(origin)
        };

        let peek = {
            let shown = view.peek_shown;
            let row = self.peek.clone();
            let tint = tint(row.level, &theme);
            let show = Self::act(cx, NotifMachine::show);
            let dismiss = Self::act(cx, NotifMachine::dismiss_preview);
            let id = row.id.clone();
            let update = Self::act(cx, move |machine| machine.run_update(&id));
            let hold = cx.listener(|this: &mut Self, hovered: &bool, _, cx| {
                let effects = if *hovered {
                    this.machine.pause_preview()
                } else {
                    this.machine.resume_preview(false)
                };
                this.run(effects, cx);
            });
            let theme = theme.clone();
            let update_label = tr("#L_N_UPDATE_NOW", &[]);
            let close_label = tr("#L_CLOSE", &[]);
            let peek_update = self.peek_update;
            let origin = flow_origin(&theme);
            presence_with("notif-peek-flow", shown, FLOW, move |frame| {
                let accent_hover = theme.color("accent-hover");
                let body = div()
                    .id("notif-peek-body")
                    .flex_1()
                    .min_w_0()
                    .pt(u(6.))
                    .cursor_pointer()
                    .on_click({
                        let show = show.clone();
                        move |_, window, cx| show(window, cx)
                    })
                    // .notif__peek-title — <button>.
                    .child(
                        div()
                            .text_px(13., 20.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text)
                            .button_text()
                            .child(row.title),
                    )
                    .when(!row.detail.is_empty(), |body| {
                        body.child(
                            div()
                                .mt(u(4.))
                                .text_px(12., 16.)
                                .text_color(dim)
                                .clamp(2)
                                .child(row.detail),
                        )
                    })
                    .when(peek_update, |body| {
                        body.child(div().mt(u(8.)).child(hover(
                            "notif-peek-update-hover",
                            Timing::fast(),
                            move |t| {
                                div()
                                    .id("notif-peek-update")
                                    .flex()
                                    .items_center()
                                    .gap(u(6.))
                                    .text_px(12., 20.)
                                    .text_color(mix(accent, accent_hover, t))
                                    .cursor_pointer()
                                    .button_text()
                                    .on_click(stop(update))
                                    .child(fa_glyph("fas fa-arrow-circle-down", 12.))
                                    .child(update_label)
                            },
                        )))
                    });
                div()
                    .id("notif-peek")
                    .relative()
                    .w(u(344.))
                    .bg(theme.color("l-modal-bg"))
                    .rounded(u(flow_radius(frame, theme.len("l-radius"))))
                    .shadow(vec![shadow(
                        zoom,
                        0.,
                        18.,
                        48.,
                        0.,
                        gpui::black().opacity(0.55),
                    )])
                    .overflow_hidden()
                    .when(shown, |el| el.occlude().on_hover(hold))
                    // radial-gradient(120% 140% at 0% 0%, тон 12 %, прозрачно 60 %).
                    .child(
                        Radial {
                            center: (0., 0.),
                            radius: (1.2, 1.4),
                            stops: vec![(0., 0.12), (0.6, 0.)],
                        }
                        .paint_rounded(
                            344.,
                            96.,
                            flow_radius(frame, theme.len("l-radius")),
                            tint,
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .child(
                        div()
                            .relative()
                            .flex()
                            .items_start()
                            .gap(u(12.))
                            .p(u(12.))
                            .child(plate(
                                32.,
                                radius_sm,
                                tint.opacity(0.16),
                                tint,
                                row.icon,
                                14.,
                            ))
                            .child(body)
                            .child(div().flex_none().mt(u(4.)).mr(u(-4.)).child(action(
                                "notif-peek-close",
                                "fas fa-times",
                                close_label,
                                &theme,
                                24.,
                                dismiss,
                            ))),
                    )
            })
            .origin(origin)
        };

        div()
            .relative()
            .child(bell)
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopRight)
                        .position(at)
                        .snap_to_window()
                        .child(panel),
                )
                .with_priority(Z_PANEL),
            )
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopRight)
                        .position(at)
                        .snap_to_window()
                        .child(peek),
                )
                .with_priority(Z_PEEK),
            )
    }
}

#[cfg(test)]
mod tests {
    use crate::app::LauncherRoot;
    use crate::snap::{bounds_of, pointer_move, press};
    use gpui::{MouseButton, TestAppContext, VisualTestContext};
    use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
    use hlsr_ui::services::Services;
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn fixtures() -> Arc<FixtureServices> {
        let item = |id: u32, read: bool| {
            json!({"id": id, "code": "NETWORK_ERROR", "level": "error", "read": read,
                   "ts": 1_700_000_000_000_u64, "count": 1, "meta": {},
                   "detail": "net::ERR_PROXY_CONNECTION_FAILED"})
        };
        let scene: Scene = serde_json::from_value(json!({
            "id": "t", "route": "/launcher", "platform": "win32",
            "window": {"width": 1600, "height": 900},
            "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
            "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
            "store": {"settings": {"config": {"language": null}}},
            "invoke": {
                "window-command": {"$else": {"ok": true}},
                "notifications-list": {"items": [item(1, false), item(2, true)], "unread": 1},
                "notifications-mark-read": {"ok": true}
            }
        }))
        .unwrap();
        let variant = Variant {
            theme: "hlsr".into(),
            lang: "ru".into(),
            scale: 1.0,
            zoom: None,
            rail: None,
        };
        Arc::new(FixtureServices::new(&scene, &variant).unwrap())
    }

    fn calls(fx: &FixtureServices, channel: &str) -> Vec<Value> {
        fx.log()
            .into_iter()
            .filter(|entry| entry["kind"] == "invoke" && entry["channel"] == channel)
            .map(|entry| entry["args"].clone())
            .collect()
    }

    fn click(cx: &mut VisualTestContext, target: &str) {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let at = bounds_of(target)
            .unwrap_or_else(|| panic!("нет цели {target}"))
            .center();
        cx.update(|window, cx| {
            pointer_move(window, cx, at);
            press(window, cx, at, MouseButton::Left, 1, true);
            press(window, cx, at, MouseButton::Left, 1, false);
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_bell_lists_reads_and_closes_like_0_4_9(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let fx = fixtures();
        let services: Services = fx.clone();
        let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
        cx.run_until_parked();
        // mounted(): список один раз.
        assert_eq!(calls(&fx, "notifications-list").len(), 1);

        // Открытие спрашивает список снова; щелчок по пункту — прочитан, и список заново.
        click(cx, "notif-bell");
        assert_eq!(calls(&fx, "notifications-list").len(), 2);
        click(cx, "notif-item-1");
        assert_eq!(calls(&fx, "notifications-mark-read"), [json!([1])]);
        assert_eq!(calls(&fx, "notifications-list").len(), 3);
        // Прочитанный пункт ничего не спрашивает.
        click(cx, "notif-item-2");
        assert_eq!(calls(&fx, "notifications-mark-read").len(), 1);

        // Esc закрывает: следующий щелчок по колокольчику снова открывает (и спрашивает список).
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        click(cx, "notif-bell");
        assert_eq!(calls(&fx, "notifications-list").len(), 4);
        // Нажатие мимо панели и колокольчика закрывает.
        click(cx, "topbar-tab-home");
        click(cx, "notif-bell");
        assert_eq!(calls(&fx, "notifications-list").len(), 5);
    }
}
