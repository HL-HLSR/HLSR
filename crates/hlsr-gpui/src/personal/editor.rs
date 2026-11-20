//! Раздел `personal` страницы «Персонализация» — редактор внешнего вида одной игры
//! (`GamePersonalization.vue` 0.4.9): черновик по игре (`game-settings-get`), подразделы схемы
//! ([`super::extras`]: поля, цвет и прицел — [`super::hud`], код настроек — [`super::code`]),
//! их список уходит на страницу (`nav`), подвал: «Сбросить внешний вид», заметка, «Перечитать» /
//! «Отмена» при конфликте файла, «Отменить изменения», «Сохранить» со спиннером. Логика —
//! `PersonalizationPanel`, `ExtrasEditor`, `HudEditor`, `LookCode` (`hlsr-ui`).

use crate::configs::footer::{BtnLook, settings_btn};
use crate::configs::nav::{NavItem, PersonalNav};
use crate::configs::sections::SectionHost;
use crate::icons::{fa_glyph, icon};
use crate::kit::{ColorField, TextInput};
use crate::motion::{FADE, choose};
use crate::scale::u;
use crate::scroll::Scroll;
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Pixels, Render,
    Styled, Subscription, Task, Window, div, rgb,
};
use hlsr_ui::controllers::hud_editor::HudEditor;
use hlsr_ui::controllers::look_code::{Busy, LookCode};
use hlsr_ui::controllers::personalization_panel::{
    GOLDSRC_GAME_IDS, Note, PersonalizationPanel, SAVED_MS,
};
use hlsr_ui::controllers::settings_extras::{ExtrasEditor, PERSONALIZATION, Schema};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

/// Рамка прошлого кадра.
pub(super) type Probe = Rc<Cell<Bounds<Pixels>>>;

/// Редактор после загрузки черновика (`<GhostingSettingsExtras v-if="drafts[gameId]">`).
pub(super) struct Look {
    pub extras: ExtrasEditor,
    pub hud: HudEditor,
    pub code: LookCode,
    /// `modPacks`: моды игры с диска (HUD-мод для превью).
    pub mod_packs: Option<Value>,
}

/// Поля ввода и рамки (создаются в кадре: им нужно окно).
#[derive(Default)]
pub(super) struct Widgets {
    pub numbers: HashMap<&'static str, Entity<TextInput>>,
    /// Поля чисел, выключенные сейчас (`<fieldset disabled>`).
    pub numbers_off: std::collections::HashSet<&'static str>,
    pub texts: HashMap<&'static str, Entity<TextInput>>,
    pub colors: HashMap<&'static str, Entity<ColorField>>,
    pub hud_hex: Option<Entity<TextInput>>,
    pub channels: Option<[Entity<TextInput>; 3]>,
    /// Свой цвет палитры HUD, прицела и цвет точки.
    pub pickers: Option<[Entity<ColorField>; 3]>,
    pub code_input: Option<Entity<TextInput>>,
    /// Вся область редактора, колонка полей, превью.
    pub probes: [Probe; 3],
    /// Ширина области, по которой посчитан прошлый кадр (CSS px).
    pub width: f32,
    pub fields_scroll: Scroll,
    pub pers_scroll: Scroll,
    pub code_scroll: Scroll,
    /// Рамка круга HUD (точка круга от её угла).
    pub wheel_box: Probe,
    /// Превью (`GhostingSettingsPreview`, G9): создаётся при первом разделе с кадром.
    pub preview: Option<Entity<super::preview::GhostingPreview>>,
    /// Оверлей LiveSplit и моды (G10): живут, пока открыт их подраздел (`v-if`).
    pub livesplit: Option<(Entity<super::livesplit::LiveSplitOverlay>, Subscription)>,
    pub packs: Option<(Entity<super::packs::CustomPacksView>, Subscription)>,
    /// `liveSplitSnapshot`: последний снимок окна LiveSplit для превью.
    pub overlay_snapshot: Option<Rc<hlsr_ui::controllers::preview::Snapshot>>,
}

/// Редактор внешнего вида одной игры.
pub struct GamePersonalization {
    pub(super) host: SectionHost,
    pub(super) schema: Schema,
    pub(super) panel: PersonalizationPanel,
    pub(super) look: Option<Look>,
    pub(super) widgets: Widgets,
    /// `fieldsWidth` (PaneSplitter `pz-fields:<игра>`).
    pub(super) fields_w: Option<f64>,
    saved_timer: Option<Task<()>>,
    pub(super) subs: Vec<Subscription>,
}

impl GamePersonalization {
    pub fn new(host: &SectionHost, cx: &mut Context<GamePersonalization>) -> Self {
        let goldsrc = GOLDSRC_GAME_IDS.contains(&host.game_id.as_str());
        let mut panel = PersonalizationPanel::new("hlsr");
        let load = panel.set_active_tab(&host.game_id);
        let fields_w = crate::shell::root::ShellContext::get(cx)
            .and_then(|context| context.root.upgrade())
            .and_then(|root| {
                let prefs = root.read(cx).prefs();
                let prefs = prefs.borrow();
                hlsr_ui::controllers::pane_size::read_size(
                    prefs.get("pane"),
                    &format!("pz-fields:{}", host.game_id),
                )
            });
        let mut this = GamePersonalization {
            host: host.clone(),
            schema: if goldsrc {
                Schema::Goldsrc
            } else {
                Schema::Ghosting
            },
            panel,
            look: None,
            widgets: Widgets::default(),
            fields_w,
            saved_timer: None,
            subs: Vec::new(),
        };
        if let Some(id) = load {
            this.load(id, None, cx);
        }
        this
    }

    pub(super) fn t(&self, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(self.host.language, key, args)
    }

    /// `ipcRenderer.invoke(…)` с ответом в этот вид.
    pub(super) fn call(
        &self,
        channel: &str,
        args: Vec<Value>,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<Value, String>, &mut Context<Self>) + 'static,
    ) {
        let reply = self.host.services.invoke(channel, args);
        cx.spawn(async move |this, cx| {
            let result = reply.await.map_err(|error| format!("{error:?}"));
            this.update(cx, |this, cx| {
                done(this, result, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Без ответа (`invoke(…).catch(() => {})`).
    pub(super) fn fire(&self, channel: &str, args: Vec<Value>, cx: &mut Context<Self>) {
        let reply = self.host.services.invoke(channel, args);
        cx.background_spawn(async move {
            let _ = reply.await;
        })
        .detach();
    }

    /// `load(id, accept)`: `token` — перечитывание по фокусу (ответ принимается, только если
    /// он последний и черновик не трогали).
    fn load(&mut self, id: String, token: Option<u64>, cx: &mut Context<Self>) {
        let args = vec![json!(id)];
        self.call("game-settings-get", args, cx, move |this, reply, cx| {
            match token {
                Some(token) => {
                    if !this.panel.focus_loaded(&id, token, reply) {
                        return;
                    }
                }
                None => this.panel.loaded(&id, reply),
            }
            this.mount(cx);
            this.sync();
        });
    }

    /// Черновик есть: редактор (`GhostingSettingsExtras` со слотом кода) — один раз.
    fn mount(&mut self, cx: &mut Context<Self>) {
        if self.look.is_some() {
            return;
        }
        let (Some(value), Some(draft)) = (self.panel.editor_value(), self.panel.draft()) else {
            return;
        };
        let game = self.host.game_id.clone();
        let look = draft.look.clone();
        self.look = Some(Look {
            extras: ExtrasEditor::new(self.schema, &game, value.clone(), self.panel.overlay()),
            hud: HudEditor::new(value, self.schema == Schema::Goldsrc),
            code: LookCode::new(&game, look),
            mod_packs: None,
        });
        // Включён ли HUD-мод, превью нужно знать и без открытой вкладки модов.
        if self.schema == Schema::Ghosting {
            self.call(
                "custom-packs-get",
                vec![json!(game)],
                cx,
                |this, reply, _| {
                    let Ok(result) = reply else { return };
                    if let Some(look) = this.look.as_mut()
                        && result.get("status").and_then(Value::as_str) == Some("ok")
                        && look.mod_packs.is_none()
                    {
                        look.mod_packs = result.get("packs").cloned();
                    }
                },
            );
        }
    }

    /// Черновик изменился: окна редактора получают новое значение (`:value="editorValue"`),
    /// проверка значений — странице (`@validity`), код — новый вид (`watch look`).
    pub(super) fn sync(&mut self) {
        let Some(value) = self.panel.editor_value() else {
            return;
        };
        let overlay = self.panel.overlay();
        let draft_look = self.panel.draft().map(|d| d.look.clone());
        let Some(look) = self.look.as_mut() else {
            return;
        };
        look.extras.set_value(value.clone());
        look.extras.set_overlay(overlay);
        look.hud.set_value(value);
        if let Some(draft_look) = draft_look {
            look.code.set_look(draft_look);
        }
        let valid = look.extras.valid();
        self.panel.set_validity(valid);
        let look = self.look.as_mut().expect("есть");
        look.code.invalid = self.panel.is_invalid();
        look.code.disabled = self.panel.saving;
    }

    /// `@input` редактора: новое значение вида.
    pub(super) fn input(&mut self, value: Value, cx: &mut Context<Self>) {
        self.panel.on_editor_input(value);
        self.sync();
        cx.notify();
    }

    /// `@overlay` редактора оверлея LiveSplit (G10).
    pub fn overlay_input(&mut self, value: Value, cx: &mut Context<Self>) {
        self.panel.on_overlay_input(value);
        self.sync();
        cx.notify();
    }

    /// Подразделы для списка слева (`$emit("nav")`): выбранный и пункты с числом правок.
    pub fn nav(&self) -> PersonalNav {
        let Some(look) = &self.look else {
            return PersonalNav::default();
        };
        PersonalNav {
            active: look.extras.active_nav_id().to_owned(),
            items: look
                .extras
                .nav_items()
                .into_iter()
                .map(|item| NavItem {
                    id: item.id.to_owned(),
                    icon: item.icon,
                    label: self.t(item.title, &[]),
                    changes: item.changes,
                })
                .collect(),
        }
    }

    /// Есть несохранённое (`$emit("dirty")`).
    pub fn dirty(&self) -> bool {
        self.panel.is_dirty(&self.host.game_id)
    }

    /// Щелчок по подразделу в списке слева (`personalNav.active = id`): поля — к началу.
    pub fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(look) = self.look.as_mut() {
            look.extras.select_section(id);
        }
        self.widgets.fields_scroll = Scroll::new();
        cx.notify();
    }

    /// Идёт импорт кода: моды уже переключаются, вид ещё не в черновике.
    fn importing(&self) -> bool {
        self.look
            .as_ref()
            .is_some_and(|look| look.code.busy == Some(Busy::Import))
    }

    /// «Сохранить» (и из вопроса о несохранённом); не во время импорта кода — его вид
    /// сохранение отбросило бы.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.importing() {
            return;
        }
        let Some((id, payload)) = self.panel.begin_save() else {
            return;
        };
        self.sync();
        let mut args = vec![json!(id), payload.clone(), json!("personalization")];
        if let Some(options) = self.panel.save_options(&id) {
            args.push(options);
        }
        cx.notify();
        self.call("game-settings-save", args, cx, move |this, reply, cx| {
            let ok = this.panel.saved(&id, payload, reply);
            this.sync();
            if ok {
                this.saved_timer = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(SAVED_MS))
                        .await;
                    this.update(cx, |this, cx| {
                        this.panel.saved_expired();
                        cx.notify();
                    })
                    .ok();
                }));
            }
        });
    }

    /// «Отменить изменения» (и «Не сохранять» из вопроса о несохранённом).
    pub fn revert_game(&mut self, cx: &mut Context<Self>) {
        self.panel.revert_game();
        self.sync();
        cx.notify();
    }

    /// «Сбросить внешний вид».
    fn reset_game(&mut self, cx: &mut Context<Self>) {
        self.panel.reset_game();
        self.sync();
        cx.notify();
    }

    /// «Перечитать»: черновик заменяется файлом игрока.
    fn reload_after_change(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.panel.reload_after_change() {
            self.load(id, None, cx);
        }
        cx.notify();
    }

    /// Раздел снова открыт: файл игрока могли сохранить другие разделы.
    pub fn on_window_focus(&mut self, cx: &mut Context<Self>) {
        if let Some((id, token)) = self.panel.begin_focus_load() {
            self.load(id, Some(token), cx);
        }
    }

    /// `importCodeLook({gameId, look, packs})`: вид из кода — в черновик, раздел «Цвет и прицел».
    pub(super) fn import_code_look(
        &mut self,
        look: &Value,
        packs: Option<Value>,
        cx: &mut Context<Self>,
    ) {
        let game = self.host.game_id.clone();
        if !self.panel.import_code_look(&game, look) {
            return;
        }
        let draft_look = self.panel.draft().map(|d| d.look.clone());
        if let Some(state) = self.look.as_mut() {
            if packs.is_some() {
                state.mod_packs = packs;
            }
            // Объект заменён: deep watch кода срабатывает и на те же значения.
            if let Some(draft_look) = draft_look {
                state.code.replace_look(draft_look);
            }
        }
        self.sync();
        self.select(PERSONALIZATION, cx);
    }

    fn footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let dirty = self.dirty();
        let saving = self.panel.saving;
        // `<transition name="l-fade" mode="out-in">`: у обоих итогов импорта кода один ключ
        // `imported` — их текст меняется на месте, без перехода.
        let keyed = |note: Note| match note {
            Note::CodeUnchanged => Note::CodeImported,
            other => other,
        };
        let current = self.panel.note();
        let (language, dim) = (self.host.language, theme.color("l-text-dim"));
        let note = choose("pz-note", current.map(keyed), FADE, move |key: &Note| {
            // Входящее — нынешнее примечание, уходящее — своё.
            let note = current.filter(|now| keyed(*now) == *key).unwrap_or(*key);
            let color = match note.tone() {
                1 => rgb(0xff8a98).into(),
                2 => rgb(0x7fe0a0).into(),
                _ => dim,
            };
            let text = hlsr_ui::tr(language, note.key(), &[]);
            div()
                .text_px(13., 13. * 1.2)
                .text_color(color)
                .flex()
                .items_center()
                .gap(u(4.))
                .when(note == Note::Saved, |el| {
                    el.child(fa_glyph("fas fa-check", 13.))
                })
                .child(text)
        })
        .out_in();
        let conflict = self.panel.conflict_shown();
        let on = |f: fn(&mut Self, &mut Context<Self>)| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _, cx| f(this, cx))
        };
        div()
            .relative()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(u(12.))
            .pt(u(16.))
            .mt(u(16.))
            // `box-shadow: inset 0 1px 0 var(--l-modal-border)`.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(u(1.))
                    .bg(theme.color("l-modal-border")),
            )
            .child(
                settings_btn("pz-reset-game", BtnLook::Ghost)
                    .fade()
                    .on_click(on(Self::reset_game))
                    .child(fa_glyph("fas fa-undo", 13.5))
                    .child(self.t("#L_PZ_RESET_GAME", &[])),
            )
            .child(div().flex_1())
            .child(note)
            .when(conflict, |el| {
                el.child(
                    settings_btn("pz-reload", BtnLook::Plain)
                        .fade()
                        .disabled(saving)
                        .on_click(on(Self::reload_after_change))
                        .child(self.t("#L_GS_CFG_CHANGED_RELOAD", &[])),
                )
                .child(
                    settings_btn("pz-conflict-cancel", BtnLook::Ghost)
                        .fade()
                        .on_click(on(|this, cx| {
                            this.panel.dismiss_conflict();
                            cx.notify();
                        }))
                        .child(self.t("#L_CANCEL", &[])),
                )
            })
            .child(
                settings_btn("pz-revert", BtnLook::Plain)
                    .fade()
                    .disabled(!dirty || saving)
                    .on_click(on(Self::revert_game))
                    .child(self.t("#L_PZ_REVERT", &[])),
            )
            .child(crate::snap::target(
                "pz-save",
                settings_btn("pz-save", BtnLook::Primary)
                    .fade()
                    .disabled(!self.panel.can_save() || self.importing())
                    .on_click(on(Self::save))
                    .when(saving, |el| {
                        el.child(icon(
                            "fas fa-spinner fa-spin",
                            13.5,
                            theme.color("accent-contrast-text"),
                        ))
                    })
                    .child(self.t("#L_SAVE", &[])),
            ))
            .into_any_element()
    }
}

impl Render for GamePersonalization {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let theme = cx.global::<Theme>().clone();
        let root = div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .font_family(RUBIK)
            .text_color(theme.color("l-text"));
        if self.look.is_none() {
            return root.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(u(10.))
                    .text_color(theme.color("l-text-dim"))
                    .text_px(14., 14. * 1.2)
                    .child(icon(
                        "fas fa-spinner fa-spin",
                        14.,
                        theme.color("l-text-dim"),
                    ))
                    .child(self.t("#L_GS_LOADING", &[])),
            );
        }
        let work = self.render_extras(window, cx);
        let footer = self.footer(cx);
        root.child(div().flex().flex_col().flex_1().min_h_0().child(work))
            .child(footer)
    }
}
