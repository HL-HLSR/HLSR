//! Раздел «Демо» (`GameDemos.vue` 0.4.9, HL1 и Ghosting): итог, «Обновить», папка (HL1),
//! «Выбрать все / Снять все»; демо строками с галочкой, датой и размером, у HL1 — команда для
//! консоли и удаление с вопросом в строке; упаковка выбранных в zip с процентом, «Показать в
//! папке», ссылки speedrun.com и Google Drive. Логика — [`GameDemos`]; каналы зовёт вид.

use super::footer::icon_label;
use crate::icons::{fa_glyph, icon};
use crate::kit::button::{focus_handle, focus_visible, outline, white};
use crate::kit::{CheckLook, Checked, check};
use crate::layer::TipExt;
use crate::motion::{Timing, hover, mix};
use crate::scale::{u, zoom};
use crate::scroll::Scroll;
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, ElementId, FontWeight, IntoElement,
    ParentElement, Render, Role, SharedString, Styled, Task, Window, div, prelude::*, relative,
    rgb,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::game_demos::{COPIED_MS, Engine, GameDemos, PackNext, format_date_local};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::time::Duration;

/// Раздел «Демо» одной игры: закэшированный вид во всю ячейку раздела ([`crate::cache`]), поэтому
/// прокрутку тела раздела ведёт сам. Читает только своё состояние: каждая его перемена —
/// `cx.notify()` этого вида; сессия настроек игры уведомляет его, как прочие разделы.
pub struct GameDemosView {
    services: Services,
    game_id: String,
    engine: Engine,
    language: Language,
    demos: GameDemos,
    scroll: Scroll,
    _events: Task<()>,
}

impl GameDemosView {
    pub fn new(
        services: &Services,
        game_id: &str,
        engine: Engine,
        language: Language,
        cx: &mut Context<Self>,
    ) -> Self {
        let events = crate::bridge::listen(cx, services.subscribe(), |this, event, cx| {
            if event.channel == "game-demos-pack-progress" && this.demos.packing {
                this.demos.progress(&this.game_id, &event.payload);
                cx.notify();
            }
        });
        GameDemosView {
            services: services.clone(),
            game_id: game_id.to_owned(),
            engine,
            language,
            demos: GameDemos::default(),
            scroll: Scroll::new(),
            _events: events,
        }
    }

    fn t(&self, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(self.language, key, args)
    }

    /// `ipcRenderer.invoke(…)` с ответом в этот вид.
    fn call(
        &self,
        channel: &str,
        args: Vec<Value>,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<Value, ()>, &mut Context<Self>) + 'static,
    ) {
        let reply = self.services.invoke(channel, args);
        cx.spawn(async move |this, cx| {
            let result = reply.await.map_err(|_| ());
            this.update(cx, |this, cx| {
                done(this, result, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Без ответа (`ipcRenderer.invoke` без `await`).
    fn fire(&self, channel: &str, args: Vec<Value>, cx: &mut Context<Self>) {
        let reply = self.services.invoke(channel, args);
        cx.background_spawn(async move {
            let _ = reply.await;
        })
        .detach();
    }

    /// `load()`: раздел открыт (`watch active`) или «Обновить».
    pub fn load(&mut self, cx: &mut Context<Self>) {
        if !self.demos.begin_load() {
            return;
        }
        cx.notify();
        let id = json!(self.game_id);
        self.call("game-demos-list", vec![id], cx, |this, result, _| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0., |since| since.as_millis() as f64);
            let language = this.language;
            let t = |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args);
            this.demos
                .list_done(result.as_ref().map_err(|_| ()), now, &t);
        });
    }

    fn pack(&mut self, cx: &mut Context<Self>) {
        let Some(items) = self.demos.begin_pack() else {
            return;
        };
        let id = json!(self.game_id);
        self.call(
            "game-demos-pack",
            vec![id, items],
            cx,
            |this, result, cx| {
                let language = this.language;
                let t = |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args);
                match this.demos.pack_done(result.as_ref().map_err(|_| ()), &t) {
                    PackNext::Reveal(name) => {
                        this.fire("game-demos-archive-reveal", vec![json!(name)], cx)
                    }
                    PackNext::Reload => this.load(cx),
                    PackNext::Nothing => {}
                }
            },
        );
    }

    fn remove(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(name) = self.demos.begin_delete(key) else {
            return;
        };
        let key = key.to_owned();
        let args = vec![json!(self.game_id), json!(name)];
        self.call("game-runs-delete", args, cx, move |this, result, _| {
            let language = this.language;
            let t = |k: &str, args: &[&str]| hlsr_ui::tr(language, k, args);
            this.demos
                .delete_done(&key, result.as_ref().map_err(|_| ()), &t);
        });
    }

    fn copy(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(command) = self.demos.copy(key) else {
            return;
        };
        // Журнал 0.4.9 ведёт вызов канала буфера обмена preload; запись — окно.
        if let Some(args) = hlsr_ui::clipboard::bridge_args(&command) {
            self.fire(hlsr_ui::clipboard::BRIDGE_CHANNEL, args, cx);
        }
        cx.write_to_clipboard(ClipboardItem::new_string(command));
        let key = key.to_owned();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(COPIED_MS))
                .await;
            this.update(cx, |this, cx| {
                this.demos.copy_expired(&key);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// `.gd__btn`: 36 px, поля 6/14, рамка `--l-border-strong`, 13 px / 600; `primary` — подложка
/// `--l-accent-soft`, текст акцентом.
fn gd_btn(
    id: impl Into<ElementId>,
    primary: bool,
    disabled: bool,
    children: Vec<AnyElement>,
    window: &mut Window,
    cx: &mut App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let id = id.into();
    let zoom = zoom(window);
    let focus = focus_handle(&id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let theme = cx.global::<Theme>();
    let (radius, accent) = (theme.len("l-radius-sm"), theme.color("l-accent"));
    let (bg, text) = if primary {
        (theme.color("l-accent-soft"), accent)
    } else {
        (white(0.05), theme.color("l-text"))
    };
    let border = theme.color("l-border-strong");
    let inner = id.clone();
    hover(id, Timing::fast(), move |t| {
        let t = if disabled { 0. } else { t };
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(u(8.))
            .min_h(u(36.))
            .px(u(14.))
            .py(u(6.))
            .border_1()
            .border_color(border)
            .rounded(u(radius))
            .bg(if primary { bg } else { mix(bg, white(0.1), t) })
            .text_color(text)
            .font_family(RUBIK)
            .text_size(u(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .whitespace_nowrap()
            .children(children)
            .when(disabled, |el| el.opacity(0.5))
            .when(!disabled, |el| {
                el.track_focus(&focus).cursor_pointer().on_click(on_click)
            })
            .when(ring_on, |el| {
                el.child(outline(zoom, radius, 2., 2., accent))
            })
    })
    .into_any_element()
}

/// `.gd__icon`: 32 × 32, наведение .08.
fn gd_icon(
    id: impl Into<ElementId>,
    classes: &'static str,
    danger: bool,
    disabled: bool,
    label: String,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let id = id.into();
    let (dim, text, radius) = (
        theme.color("l-text-dim"),
        theme.color("l-text"),
        theme.len("l-radius-sm"),
    );
    let inner = id.clone();
    hover(id, Timing::fast(), move |t| {
        let t = if disabled { 0. } else { t };
        let color = if danger {
            rgb(0xff8796).into()
        } else {
            mix(dim, text, t)
        };
        let glyph = if classes.contains("fa-spin") {
            icon(classes, 13., color)
        } else {
            fa_glyph(classes, 13.).text_color(color).into_any_element()
        };
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .aria_label(label.clone())
            .tip(label)
            .size(u(32.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(u(radius))
            .bg(white(0.08 * t))
            .child(glyph)
            .when(disabled, |el| el.opacity(0.5))
            .when(!disabled, |el| el.cursor_pointer().on_click(on_click))
    })
    .into_any_element()
}

impl Render for GameDemosView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let theme = cx.global::<Theme>().clone();
        let t = |key: &str| self.t(key, &[]);
        let language = self.language;
        let tr = move |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args);
        let demos = &self.demos;
        let packing = demos.packing;
        let weak = cx.weak_entity();
        let act = |f: fn(&mut GameDemosView, &mut Context<GameDemosView>)| {
            let weak = weak.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| {
                    f(this, cx);
                    cx.notify();
                })
                .ok();
            }
        };
        let keyed = |f: fn(&mut GameDemosView, &str, &mut Context<GameDemosView>), key: String| {
            let weak = weak.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| {
                    f(this, &key, cx);
                    cx.notify();
                })
                .ok();
            }
        };

        let mut head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(u(8.))
            .child(
                div()
                    .text_size(u(12.5))
                    .text_color(theme.color("l-text-dim"))
                    .child(demos.summary(&tr)),
            )
            .child(div().flex_1())
            .child(gd_btn(
                "gd-refresh",
                false,
                demos.loading || packing,
                icon_label("fas fa-sync-alt", t("#L_GS_REFRESH"), true),
                window,
                cx,
                act(|this, cx| this.load(cx)),
            ));
        if self.engine.can_delete() {
            let (services, game) = (self.services.clone(), self.game_id.clone());
            head = head.child(gd_btn(
                "gd-folder",
                false,
                false,
                icon_label("fas fa-folder-open", t("#L_GS_OPEN_DEMOS"), true),
                window,
                cx,
                move |_, _, cx| {
                    let reply = services.invoke("game-runs-open-folder", vec![json!(game)]);
                    cx.background_spawn(async move {
                        let _ = reply.await;
                    })
                    .detach();
                },
            ));
        }
        head = head
            .child(gd_btn(
                "gd-all",
                false,
                demos.demos.is_empty() || packing,
                vec![t("#L_DEMOS_SELECT_ALL").into_any_element()],
                window,
                cx,
                act(|this, _| this.demos.set_all(true)),
            ))
            .child(gd_btn(
                "gd-none",
                false,
                demos.chosen_count() == 0 || packing,
                vec![t("#L_DEMOS_CLEAR_ALL").into_any_element()],
                window,
                cx,
                act(|this, _| this.demos.set_all(false)),
            ));

        let empty = |text: String| super::records::empty_box(&theme, text);
        let body = if demos.loading && demos.demos.is_empty() {
            empty(t("#L_GS_LOADING"))
        } else if let Some(error) = &demos.list_error {
            empty(error.clone())
        } else if demos.demos.is_empty() {
            empty(t("#L_DEMOS_EMPTY"))
        } else {
            let mut list = div().flex().flex_col().gap(u(6.));
            for (index, demo) in demos.demos.iter().enumerate() {
                let key = demo.key();
                let meta = |text: String| {
                    div()
                        .flex_none()
                        .text_size(u(12.))
                        .tnum()
                        .text_color(theme.color("l-text-faint"))
                        .child(text)
                };
                let pick = div()
                    .id(SharedString::from(format!("gd-pick-{index}")))
                    .flex()
                    .items_center()
                    .gap(u(10.))
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .when(!packing, |el| {
                        el.on_click(keyed(|this, key, _| this.demos.toggle(key), key.clone()))
                    })
                    .child(
                        check(
                            SharedString::from(format!("gd-check-{index}")),
                            if demos.is_chosen(&key) {
                                Checked::On
                            } else {
                                Checked::Off
                            },
                            CheckLook::Native,
                        )
                        .disabled(packing)
                        .on_change({
                            let weak = weak.clone();
                            let key = key.clone();
                            move |_, _, cx| {
                                weak.update(cx, |this, cx| {
                                    this.demos.toggle(&key);
                                    cx.notify();
                                })
                                .ok();
                            }
                        }),
                    )
                    .child(fa_glyph("fas fa-film", 13.).text_color(theme.color("l-text-dim")))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(demo.name.clone()),
                    );
                let mut row = div()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .min_h(u(44.))
                    .pl(u(12.))
                    .pr(u(8.))
                    .py(u(4.))
                    .rounded(u(theme.len("l-radius-sm")))
                    .bg(white(0.03))
                    .text_size(u(13.))
                    .text_color(theme.color("l-text"))
                    .child(pick)
                    .child(meta(format_date_local(demo.mtime_ms)))
                    .child(meta(hlsr_ui::controllers::boot::format_size(
                        demo.size, &tr,
                    )));
                if self.engine.can_copy() {
                    let label = tr("#L_GS_DEMO_COPY", &[&demo.command()]);
                    row = row.child(gd_icon(
                        SharedString::from(format!("gd-copy-{index}")),
                        if demos.copied.as_deref() == Some(key.as_str()) {
                            "fas fa-check"
                        } else {
                            "fas fa-terminal"
                        },
                        false,
                        false,
                        label,
                        &theme,
                        keyed(|this, key, cx| this.copy(key, cx), key.clone()),
                    ));
                }
                if self.engine.can_delete() {
                    let busy = demos.deleting.is_some();
                    let actions = if demos.pending_delete.as_deref() == Some(key.as_str()) {
                        div()
                            .flex()
                            .items_center()
                            .gap(u(4.))
                            .child(
                                div()
                                    .text_size(u(12.))
                                    .text_color(theme.color("l-text-dim"))
                                    .child(t("#L_GS_GHOST_DELETE_CONFIRM")),
                            )
                            .child(gd_icon(
                                SharedString::from(format!("gd-yes-{index}")),
                                if demos.deleting.as_deref() == Some(key.as_str()) {
                                    "fas fa-spinner fa-spin"
                                } else {
                                    "fas fa-check"
                                },
                                true,
                                busy,
                                t("#L_GS_GHOST_DELETE_YES"),
                                &theme,
                                keyed(|this, key, cx| this.remove(key, cx), key.clone()),
                            ))
                            .child(gd_icon(
                                SharedString::from(format!("gd-no-{index}")),
                                "fas fa-times",
                                false,
                                busy,
                                t("#L_CANCEL"),
                                &theme,
                                act(|this, _| this.demos.cancel_delete()),
                            ))
                    } else {
                        let mut actions = div().flex().items_center().gap(u(4.));
                        if demos.is_unique(demo) {
                            actions = actions.child(gd_icon(
                                SharedString::from(format!("gd-delete-{index}")),
                                "fas fa-trash-alt",
                                false,
                                busy,
                                format!("{}: {}", t("#L_GS_DEMO_DELETE"), demo.name),
                                &theme,
                                keyed(|this, key, _| this.demos.ask_delete(key), key.clone()),
                            ));
                        }
                        actions
                    };
                    row = row.child(actions.flex_none());
                }
                list = list.child(row);
            }
            list.into_any_element()
        };

        let error = |text: &str| {
            div()
                .text_size(u(12.))
                .text_color(rgb(0xff8796))
                .child(text.to_owned())
        };
        let pack_icon = if packing {
            icon("fas fa-spinner fa-spin", 13., theme.color("l-accent"))
        } else {
            fa_glyph("fas fa-file-archive", 13.).into_any_element()
        };
        let mut pack = div()
            .flex()
            .flex_col()
            .items_start()
            .gap(u(10.))
            .child(gd_btn(
                "gd-pack",
                true,
                demos.chosen_count() == 0 || packing,
                vec![pack_icon, demos.pack_label(&tr).into_any_element()],
                window,
                cx,
                act(|this, cx| this.pack(cx)),
            ))
            .when_some(demos.pack_error.as_deref(), |el, text| {
                el.child(error(text))
            });
        if let Some(label) = demos.archive_label(&tr) {
            let link = |key: &'static str| {
                let services = self.services.clone();
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    let reply = services.invoke("open-external-link", vec![json!(key)]);
                    cx.background_spawn(async move {
                        let _ = reply.await;
                    })
                    .detach();
                }
            };
            let reveal = {
                let (services, name) = (
                    self.services.clone(),
                    demos
                        .archive
                        .as_ref()
                        .map(|a| a.name.clone())
                        .unwrap_or_default(),
                );
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    let reply = services.invoke("game-demos-archive-reveal", vec![json!(name)]);
                    cx.background_spawn(async move {
                        let _ = reply.await;
                    })
                    .detach();
                }
            };
            pack = pack.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(u(8.))
                    .text_size(u(12.5))
                    .text_color(theme.color("l-text-dim"))
                    .child(div().min_w_0().child(label))
                    .child(gd_btn(
                        "gd-reveal",
                        false,
                        false,
                        icon_label("fas fa-folder-open", t("#L_DEMOS_REVEAL"), true),
                        window,
                        cx,
                        reveal,
                    ))
                    .child(gd_btn(
                        "gd-speedrun",
                        false,
                        false,
                        icon_label("fas fa-external-link-alt", "speedrun.com".into(), true),
                        window,
                        cx,
                        link(self.engine.speedrun_key()),
                    ))
                    .child(gd_btn(
                        "gd-drive",
                        false,
                        false,
                        icon_label("fab fa-google-drive", "Google Drive".into(), true),
                        window,
                        cx,
                        link("google-drive"),
                    )),
            );
        }

        let content = div()
            .flex()
            .flex_col()
            .gap(u(12.))
            .min_w_0()
            .font_family(RUBIK)
            .line_height(relative(1.15))
            .child(head)
            .child(body)
            .when_some(demos.delete_error.as_deref(), |el, text| {
                el.child(error(text))
            })
            .child(pack);
        super::sections::body("demos", &self.scroll, content.into_any_element())
    }
}

/// Движок раздела по игре лаунчера: HL1 — GoldSrc, остальное — Ghosting.
pub fn engine_of(game_id: &str) -> Engine {
    if hlsr_ui::controllers::game_settings::draft::GOLDSRC_GAME_IDS.contains(&game_id) {
        Engine::GoldSrc
    } else {
        Engine::Ghosting
    }
}
