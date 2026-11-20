//! Страница «Персонализация» (`GameConfigsPanel.vue` 0.4.9, `keep-alive`): заголовок, «?»
//! (обучение), чипы установленных игр (HL2 — «в разработке», LiveSplit — неактивный), строка
//! итога действий (экспорт, отмена импорта, «Показать» правила), пусто — «Сначала установи
//! игру»; ниже настройки выбранной игры ([`GameSettings`], `:key="gameId"`), в их подвале —
//! импорт, экспорт и папка. Несохранённое при смене игры или раздела — вопрос
//! «Остаться / Не сохранять / Сохранить» поверх страницы.

use super::footer::{BtnLook, icon_label, settings_btn};
use super::sections::{GameSettings, SettingsEvent};
use crate::icons::fa_glyph;
use crate::kit::button::{focus_handle, focus_visible, outline};
use crate::layer::TipExt;
use crate::motion::{Timing, hover, mix};
use crate::paint::{ring, shadow};
use crate::scale::{u, viewport, zoom};
use crate::shell::root::ShellContext;
use crate::text::{RUBIK, TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext, Context, ElementId, Entity, FocusHandle, FontWeight, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, Role, Styled, Subscription, Window, div,
    prelude::*, relative, white,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::config_chips::{ConfigGame, config_chips, config_games};
use hlsr_ui::services::Services;
use serde_json::{Value, json};

/// Итог последнего действия страницы: `{ error, text, details, undo, rules }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub error: bool,
    pub text: String,
    pub details: Option<String>,
    pub undo: bool,
    pub rules: bool,
}

impl Notice {
    fn error(text: String) -> Notice {
        Notice {
            error: true,
            text,
            details: None,
            undo: false,
            rules: false,
        }
    }
}

/// Что сделать, когда о несохранённом спросили и можно идти дальше.
pub type Proceed =
    Box<dyn FnOnce(&mut GameConfigsPanel, &mut Window, &mut Context<GameConfigsPanel>)>;

/// Открытый вопрос о несохранённом.
struct Ask {
    part: Option<&'static str>,
    then: Proceed,
}

pub struct GameConfigsPanel {
    services: Services,
    /// Выбранная игра (`gameId`); пусто — игр нет.
    game_id: String,
    /// Раздел, на котором откроется окно игры (`openSection`).
    open_section: String,
    /// Игра «Персонализации» у навигации на прошлом кадре: щелчок «Настройки» в меню игры
    /// меняет её — страница переходит туда (`openToken`).
    seen_nav_game: String,
    /// `openToken` оболочки на прошлом кадре (опрос и обучение: игра и раздел `configs_section`).
    seen_token: u32,
    /// Опрос «Свой конфиг»: импорт ждёт окно этой игры (`startImport`); был ли вопрос о
    /// несохранённом.
    import_wait: Option<(String, bool)>,
    busy: bool,
    notice: Option<Notice>,
    unsaved: Option<Ask>,
    /// «Сохранить» в вопросе: идёт сохранение, потом — дальше (если всё записалось).
    after_save: Option<Ask>,
    /// Фокус вопроса: Esc внутри — «Остаться».
    dialog_focus: FocusHandle,
    settings: Option<(String, Entity<GameSettings>, Subscription)>,
}

fn language_of(cx: &App) -> Language {
    ShellContext::get(cx).map_or(Language::En, |context| context.shell.language())
}

impl GameConfigsPanel {
    pub fn new(services: &Services, cx: &mut Context<GameConfigsPanel>) -> Self {
        let games = Self::games(cx);
        let wanted = ShellContext::get(cx)
            .map(|context| context.nav.configs_game())
            .unwrap_or_default();
        let first = games
            .iter()
            .find(|game| game.id == wanted)
            .or(games.first())
            .map(|game| game.id.clone())
            .unwrap_or_default();
        // Вход снаружи (опрос, обучение): раздел `initialSection`; токен уже учтён.
        let (open_section, seen_token) = ShellContext::get(cx).map_or_else(Default::default, |c| {
            c.flows
                .with_state(|s| (s.configs_section.clone(), s.configs_open_token))
        });
        GameConfigsPanel {
            services: services.clone(),
            game_id: first,
            open_section,
            seen_nav_game: wanted,
            seen_token,
            import_wait: None,
            busy: false,
            notice: None,
            unsaved: None,
            after_save: None,
            dialog_focus: cx.focus_handle(),
            settings: None,
        }
    }

    /// `configGames`: установленные игры в порядке библиотеки.
    fn games(cx: &App) -> Vec<ConfigGame> {
        ShellContext::get(cx).map_or_else(Vec::new, |context| {
            config_games(&context.flows.state().delivery)
        })
    }

    fn t(&self, cx: &App, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(language_of(cx), key, args)
    }

    fn settings(&self) -> Option<&Entity<GameSettings>> {
        self.settings.as_ref().map(|(_, entity, _)| entity)
    }

    /// Несохранённое на странице (`unsavedParts`): черновик окна и раздел «Персонализация»;
    /// `part` — только эта часть (смена раздела слева).
    fn dirty(&self, part: Option<&str>, cx: &App) -> (bool, bool) {
        let Some(settings) = self.settings() else {
            return (false, false);
        };
        let settings = settings.read(cx);
        let session = settings.session();
        let draft = part != Some("personal") && session.draft_loaded() && session.has_unsaved();
        let personal = part != Some("settings") && settings.personal().read(cx).dirty();
        (draft, personal)
    }

    /// `settleUnsaved(part)`: правок нет — сразу дальше, иначе вопрос.
    pub fn settle_unsaved(
        &mut self,
        part: Option<&'static str>,
        then: Proceed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.unsaved.is_some() || self.after_save.is_some() {
            return;
        }
        let (draft, personal) = self.dirty(part, cx);
        if !draft && !personal {
            // После текущего обновления: `then` трогает настройки, которые могут быть заняты
            // (вопрос задают из их обработчика).
            cx.defer_in(window, move |this, window, cx| then(this, window, cx));
            return;
        }
        self.unsaved = Some(Ask { part, then });
        window.focus(&self.dialog_focus, cx);
        cx.notify();
    }

    /// Ответ на вопрос: «stay», «discard» или «save».
    fn answer(&mut self, choice: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ask) = self.unsaved.take() else {
            return;
        };
        cx.notify();
        if choice == "stay" {
            return;
        }
        let (draft, personal) = self.dirty(ask.part, cx);
        let Some(settings) = self.settings().cloned() else {
            (ask.then)(self, window, cx);
            return;
        };
        if choice == "save" {
            settings.update(cx, |settings, cx| {
                if personal {
                    settings.personal().update(cx, |p, cx| p.save(cx));
                }
                if draft {
                    // Несохранённый текст редактора cfg тоже сохраняется.
                    settings
                        .configs()
                        .update(cx, |configs, cx| configs.save_files(window, cx));
                    settings.session().submit();
                }
            });
            // Дальше — когда сохранение кончится и правок не останется (`watch` настроек).
            self.after_save = Some(ask);
            self.check_after_save(window, cx);
            return;
        }
        settings.update(cx, |settings, cx| {
            if personal {
                settings.personal().update(cx, |p, cx| p.revert_game(cx));
            }
            if draft {
                settings.session().discard_changes();
            }
        });
        (ask.then)(self, window, cx);
    }

    /// После «Сохранить»: идти дальше, только если всё записалось и файл не изменён снаружи.
    fn check_after_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(settings) = self.settings().cloned() else {
            return;
        };
        if settings.read(cx).session().view().saving {
            return;
        }
        let Some(ask) = self.after_save.take() else {
            return;
        };
        let (draft, personal) = self.dirty(ask.part, cx);
        let changed = settings.read(cx).session().file_changed();
        if !draft && !personal && !changed {
            (ask.then)(self, window, cx);
        }
        cx.notify();
    }

    /// `requestGame(id, section)`: пусто — тот же раздел, что открыт сейчас.
    fn request_game(
        &mut self,
        id: String,
        section: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if id == self.game_id
            || self.busy
            || self.unsaved.is_some()
            || !Self::games(cx).iter().any(|game| game.id == id)
        {
            return;
        }
        let keep = if section.is_empty() {
            self.settings()
                .map(|s| s.read(cx).session().active_section().to_owned())
                .unwrap_or_default()
        } else {
            section
        };
        self.settle_unsaved(
            None,
            Box::new(move |this, _, cx| {
                this.notice = None;
                this.open_section = keep;
                this.game_id = id;
                cx.notify();
            }),
            window,
            cx,
        );
    }

    /// `currentPageSection()`: раздел окна игры (`None` — окна ещё нет).
    pub fn current_page_section(&self, cx: &App) -> Option<&'static str> {
        self.settings()
            .map(|settings| settings.read(cx).session().active_section())
    }

    /// `selectPageSection(id)` (обучение): при несохранённых правках раздел не меняется и вопрос
    /// не открывается — он встал бы под щитом обучения. `None` — окна игры ещё нет.
    pub fn select_page_section(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let settings = self.settings()?.clone();
        let active = settings.read(cx).session().active_section();
        let (draft, personal) = self.dirty(None, cx);
        if id != active && (self.unsaved.is_some() || draft || personal) {
            return Some(false);
        }
        settings.update(cx, |settings, cx| settings.select_section(id, window, cx));
        Some(true)
    }

    /// `watch openToken` и `startImport` (опрос, обучение): страница переходит на игру и раздел
    /// оболочки; «Свой конфиг» — импорт, когда окно игры прочитало настройки.
    fn follow_shell(&mut self, games: &[ConfigGame], window: &mut Window, cx: &mut Context<Self>) {
        let Some(context) = ShellContext::get(cx).cloned() else {
            return;
        };
        let wanted = context.nav.configs_game();
        let (token, section, import) = context.flows.with_state(|s| {
            let import = std::mem::take(&mut s.configs_import_requested);
            (s.configs_open_token, s.configs_section.clone(), import)
        });
        if token != self.seen_token {
            self.seen_token = token;
            self.seen_nav_game = wanted.clone();
            if !wanted.is_empty() {
                let same =
                    wanted == self.game_id && !section.is_empty() && self.settings().is_some();
                let id = wanted.clone();
                cx.defer_in(window, move |this, window, cx| {
                    let known = hlsr_ui::controllers::game_settings::sections::ALL
                        .iter()
                        .find(|known| known.id == section);
                    match (same, known, this.settings().cloned()) {
                        (true, Some(known), Some(settings)) => {
                            settings.update(cx, |s, cx| s.select_section(known.id, window, cx))
                        }
                        _ => {
                            // Занятая страница отклоняет переход молча: импорт ждал бы его
                            // вечно — как 0.4.9, сразу «не загрузилось».
                            let waits = this.import_wait.as_ref().is_some_and(|(w, _)| *w == id);
                            if this.busy && waits && id != this.game_id {
                                this.import_wait = None;
                                this.notice =
                                    Some(Notice::error(this.t(cx, "#L_CFG_IMPORT_NOT_READY", &[])));
                                cx.notify();
                            }
                            this.request_game(id, section, window, cx)
                        }
                    }
                });
            }
        }
        if import {
            let id = if wanted.is_empty() {
                self.game_id.clone()
            } else {
                wanted
            };
            self.import_wait = Some((id, false));
        }
        let Some((id, asked)) = self.import_wait.clone() else {
            return;
        };
        // Игра ещё не та: ждём переход; на вопрос о несохранённом ответили «Остаться» или игры
        // нет — «не загрузилось» (переход, отклонённый занятой страницей, снят выше).
        if id != self.game_id {
            let open = self.unsaved.is_some() || self.after_save.is_some();
            if open {
                self.import_wait = Some((id, true));
            } else if asked || !games.iter().any(|game| game.id == id) {
                self.import_wait = None;
                self.notice = Some(Notice::error(self.t(cx, "#L_CFG_IMPORT_NOT_READY", &[])));
            }
            return;
        }
        let Some(settings) = self.settings() else {
            return;
        };
        use hlsr_ui::controllers::game_settings::session::LoadState;
        match settings.read(cx).session().load_state() {
            LoadState::Loading => {}
            LoadState::Ready => {
                self.import_wait = None;
                cx.defer_in(window, |this, window, cx| this.import_config(window, cx));
            }
            LoadState::Failed => {
                self.import_wait = None;
                self.notice = Some(Notice::error(self.t(cx, "#L_CFG_IMPORT_NOT_READY", &[])));
            }
        }
    }

    /// `ipcRenderer.invoke` с ответом сюда.
    fn call(
        &self,
        channel: &str,
        args: Vec<Value>,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Option<Value>, &mut Context<Self>) + 'static,
    ) {
        let reply = self.services.invoke(channel, args);
        cx.spawn(async move |this, cx| {
            let result = reply.await.ok();
            this.update(cx, |this, cx| {
                done(this, result, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Игра файла игрока: `bindsGameId` окна или сама игра.
    fn binds_game(&self, cx: &App) -> String {
        self.settings()
            .and_then(|s| s.read(cx).session().binds_game_id())
            .unwrap_or_else(|| self.game_id.clone())
    }

    fn export_config(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.settings().is_none() {
            return;
        }
        self.busy = true;
        let id = self.binds_game(cx);
        self.call(
            "player-config-export",
            vec![json!(id)],
            cx,
            |this, result, cx| {
                this.busy = false;
                let status = result
                    .as_ref()
                    .and_then(|r| r.get("status"))
                    .and_then(Value::as_str);
                this.notice = match (status, &result) {
                    (None, None) | (Some("canceled"), _) => return,
                    (Some("ok"), Some(result)) => {
                        let path = result
                            .get("path")
                            .map(hlsr_core::js::to_string)
                            .unwrap_or_default();
                        Some(Notice {
                            error: false,
                            text: this.t(cx, "#L_CFG_EXPORT_DONE", &[&path]),
                            details: None,
                            undo: false,
                            rules: false,
                        })
                    }
                    _ => Some(Notice::error(this.t(cx, "#L_CFG_EXPORT_FAILED", &[]))),
                };
            },
        );
    }

    fn open_folder(&mut self, cx: &mut Context<Self>) {
        let build = self
            .settings()
            .and_then(|s| s.read(cx).session().config_build())
            .map_or(Value::Null, Value::String);
        let reply = self
            .services
            .invoke("game-config-open-folder", vec![json!(self.game_id), build]);
        cx.background_spawn(async move {
            let _ = reply.await;
        })
        .detach();
    }

    /// «Вернуть прежний конфиг» (`undoImport`).
    fn undo_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.settings().is_none() {
            return;
        }
        self.settle_unsaved(
            None,
            Box::new(|this, _, cx| {
                this.busy = true;
                let id = this.binds_game(cx);
                this.call(
                    "player-config-import-undo",
                    vec![json!(id)],
                    cx,
                    |this, result, cx| {
                        this.busy = false;
                        let ok = result
                            .as_ref()
                            .and_then(|r| r.get("status"))
                            .and_then(Value::as_str)
                            == Some("ok");
                        if !ok {
                            let code = result
                                .as_ref()
                                .and_then(|r| r.get("code"))
                                .and_then(Value::as_str);
                            let key = match code {
                                Some("GAME_RUNNING") => "#L_D_ERR_GAME_RUNNING",
                                Some("BINDS_FILE_TOO_LARGE") => "#L_KB_IMPORT_TOO_LARGE",
                                _ => "#L_KB_IMPORT_WRITE_FAILED",
                            };
                            this.notice = Some(Notice::error(this.t(cx, key, &[])));
                            return;
                        }
                        if let Some(settings) = this.settings() {
                            settings.read(cx).session().reload_after_change();
                        }
                        this.notice = Some(Notice {
                            error: false,
                            text: this.t(cx, "#L_KB_IMPORT_REMOVED", &[]),
                            details: None,
                            undo: false,
                            rules: false,
                        });
                    },
                );
            }),
            window,
            cx,
        );
    }

    /// «Показать» в итоге импорта: раздел «Конфиг», вкладка «Проверка» (`showCheck`).
    fn show_rules(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(settings) = self.settings().cloned() else {
            return;
        };
        let from = settings.read(cx).session().active_section();
        let part = hlsr_ui::controllers::game_settings::sections::leave_part(from, "configs");
        self.settle_unsaved(
            part,
            Box::new(move |_, _, cx| {
                settings.update(cx, |settings, cx| {
                    settings.session().show_check();
                    cx.notify();
                });
            }),
            window,
            cx,
        );
    }

    /// Слот `footer-start` подвала настроек: импорт, экспорт, папка. В узком окне (≤ 1400,
    /// с выдвинутой колонкой ≤ 1560) — только значки.
    pub fn footer_start(page: &Entity<Self>, window: &mut Window, cx: &mut App) -> AnyElement {
        let this = page.read(cx);
        let busy = this.busy;
        let rail_open = ShellContext::get(cx).is_some_and(|context| context.nav.rail_open());
        let width = viewport(window).0;
        let labels = !(width <= 1400. || (rail_open && width <= 1560.));
        let t = |key: &str| this.t(cx, key, &[]);
        let (import, export, folder) = (
            t("#L_CFG_IMPORT"),
            t("#L_CFG_EXPORT"),
            t("#L_CFG_OPEN_FOLDER"),
        );
        let weak = page.downgrade();
        let act = |f: fn(&mut GameConfigsPanel, &mut Window, &mut Context<GameConfigsPanel>)| {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| f(this, window, cx)).ok();
            }
        };
        let button = |id: &'static str, icon: &'static str, text: String, disabled: bool| {
            settings_btn(id, BtnLook::Plain)
                .icon_only(!labels)
                .disabled(disabled)
                .label(text.clone())
                .children(icon_label(icon, text, labels))
        };
        div()
            .flex()
            .when(labels, |el| el.flex_wrap())
            .gap(u(8.))
            .child(super::anchors::anchor(
                "configs-import",
                crate::snap::target(
                    "cfgp-import",
                    button("cfgp-import", "fas fa-file-import", import, busy)
                        .on_click(act(|this, window, cx| this.import_config(window, cx))),
                ),
            ))
            .child(super::anchors::anchor(
                "configs-export",
                button("cfgp-export", "fas fa-file-export", export, busy)
                    .on_click(act(|this, _, cx| this.export_config(cx))),
            ))
            .child(
                button("cfgp-folder", "fas fa-folder-open", folder, false)
                    .on_click(act(|this, _, cx| this.open_folder(cx))),
            )
            .into_any_element()
    }

    /// Импорт конфига (`importConfig`): тем же выбором файлов, что и в окне (подсказка HL1,
    /// цепочка exec, файлы рядом), но записывается сразу; окно потом перечитывает файл игрока.
    fn import_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.settings().is_none() {
            return;
        }
        self.settle_unsaved(
            None,
            Box::new(|this, _, cx| this.run_import(cx)),
            window,
            cx,
        );
    }

    fn run_import(&mut self, cx: &mut Context<Self>) {
        use hlsr_ui::controllers::game_settings::import::{self, PagePick};
        let Some(settings) = self.settings().cloned() else {
            return;
        };
        self.busy = true;
        self.notice = None;
        cx.notify();
        let id = self.binds_game(cx);
        let language = language_of(cx);
        let services = self.services.clone();
        let shell = ShellContext::get(cx).map(|context| context.shell.clone());
        let session = settings.read(cx).session().clone();
        cx.spawn(async move |this, cx| {
            let tr = move |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args);
            let engine = import::engine_of(&id);
            let notice = async {
                if let (Some(ask), Some(shell)) = (import::intro(engine, &tr), shell)
                    && !shell.ask_confirm(ask).await.accepted()
                {
                    return Ok(None);
                }
                let payload = match import::pick_player_config(&services, &id, &tr).await? {
                    PagePick::Canceled => return Ok(None),
                    PagePick::Failed(text) => return Ok(Some(import::PageNotice::error(text))),
                    PagePick::Ready(payload) => payload,
                };
                let result = services
                    .invoke("player-config-import-apply", vec![json!(id), payload])
                    .await?;
                if result.get("status").and_then(Value::as_str) != Some("ok") {
                    return Ok(Some(import::PageNotice::error(import::import_error_text(
                        Some(&result),
                        &tr,
                    ))));
                }
                session.reload_after_change();
                // Итог правил speedrun.com (только HL1): `{ errors, warnings }` или ничего.
                let check = services
                    .invoke(
                        "player-config-check",
                        vec![json!(id), json!(session.rule_category())],
                    )
                    .await
                    .ok()
                    .filter(|check| check["status"] == "ok");
                let rules = check.map(|check| {
                    let n = |v: &Value| v.as_f64().unwrap_or(0.).max(0.) as u64;
                    (n(&check["stats"]["errors"]), n(&check["stats"]["warnings"]))
                });
                Ok::<_, String>(Some(import::import_notice(&result, rules, &tr)))
            }
            .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match notice {
                    Ok(Some(notice)) => {
                        this.notice = Some(Notice {
                            error: notice.error,
                            text: notice.text,
                            details: notice.details,
                            undo: notice.undo,
                            rules: notice.rules,
                        })
                    }
                    Ok(None) => {}
                    Err(_) => {
                        this.notice = Some(Notice::error(this.t(cx, "#L_KB_IMPORT_FAILED", &[])))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Настройки выбранной игры: другая игра — новый вид (`:key="gameId"`), прежняя сессия
    /// отпускается.
    fn ensure_settings(
        &mut self,
        games: &[ConfigGame],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .settings
            .as_ref()
            .is_some_and(|(id, _, _)| *id == self.game_id)
        {
            return;
        }
        if let Some((_, old, _)) = self.settings.take() {
            old.read(cx).session().detach();
        }
        let Some(game) = games.iter().find(|game| game.id == self.game_id).cloned() else {
            return;
        };
        let (services, section, page) = (
            self.services.clone(),
            std::mem::take(&mut self.open_section),
            cx.weak_entity(),
        );
        let language = language_of(cx);
        let settings =
            cx.new(|cx| GameSettings::new(&services, &game, &section, language, page, window, cx));
        let sub = cx.subscribe_in(
            &settings,
            window,
            |this, _, event: &SettingsEvent, window, cx| {
                match event {
                    // Любое сохранение снимает итог импорта (R18); тост — у оболочки.
                    SettingsEvent::Saved(_) => {
                        this.notice = None;
                        if let Some(context) = ShellContext::get(cx) {
                            context.flows.on_game_settings_saved();
                        }
                    }
                }
                this.check_after_save(window, cx);
            },
        );
        let observe = cx.observe_in(&settings, window, |this, _, window, cx| {
            this.check_after_save(window, cx);
        });
        observe.detach();
        self.settings = Some((self.game_id.clone(), settings, sub));
    }

    fn chips(
        &self,
        games: &[ConfigGame],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let soon_text = self.t(cx, "#L_CFG_HL2_SOON", &[]);
        let soon_title = self.t(cx, "#L_BUILD_SOON", &[]);
        let mut group = div()
            .flex()
            .gap(u(4.))
            .p(u(4.))
            .rounded(u(theme.len("l-radius")))
            .bg(theme.color("l-chip-bg"))
            .shadow(vec![ring(zoom, 1., theme.color("l-border"))]);
        for (index, chip) in config_chips(games).into_iter().enumerate() {
            let active = !chip.disabled && chip.id == self.game_id;
            let id = ElementId::Name(format!("cfgp-game-{index}").into());
            let focus = focus_handle(&id, window, cx);
            let ring_on = focus_visible(&focus, window);
            let (dim, text, faint) = (
                theme.color("l-text-dim"),
                theme.color("l-text"),
                theme.color("l-text-faint"),
            );
            let (hover_bg, soft, border, accent) = (
                theme.color("l-chip-hover"),
                theme.color("accent-soft"),
                theme.color("accent-border"),
                theme.color("l-accent"),
            );
            let radius = theme.len("l-radius-sm");
            let disabled = !chip.disabled && self.busy;
            let weak = cx.weak_entity();
            let (name, soon, inert, target) =
                (chip.name.clone(), chip.soon, chip.disabled, chip.id.clone());
            let (soon_text, soon_title) = (soon_text.clone(), soon_title.clone());
            let inner = id.clone();
            let el = hover(id, Timing::fast(), move |t| {
                let live = !inert && !active && !disabled;
                let t = if live { t } else { 0. };
                let color = if inert {
                    faint
                } else if active {
                    text
                } else {
                    mix(dim, text, t)
                };
                let bg = if active { soft } else { hover_bg.opacity(t) };
                div()
                    .id(inner)
                    .role(Role::Button)
                    .button_text()
                    .relative()
                    .flex()
                    .items_center()
                    .h(u(36.))
                    .px(u(16.))
                    .rounded(u(radius))
                    .bg(bg)
                    .when(active, |el| el.shadow(vec![ring(zoom, 1., border)]))
                    .text_color(color)
                    .child(spaced(name, 13., 36., FontWeight::SEMIBOLD, 0.04, color))
                    .when(soon, |el| {
                        el.child(
                            div()
                                .ml(u(8.))
                                .text_px(11., 36.)
                                .font_weight(FontWeight::MEDIUM)
                                .child(soon_text),
                        )
                    })
                    .when(inert, |el| el.tip(soon_title))
                    .when(disabled, |el| el.opacity(0.5))
                    .when(!inert && !disabled, |el| {
                        el.track_focus(&focus)
                            .cursor_pointer()
                            .on_click(move |_, window, cx| {
                                let target = target.clone();
                                weak.update(cx, |this, cx| {
                                    this.request_game(target, String::new(), window, cx)
                                })
                                .ok();
                            })
                    })
                    .when(ring_on, |el| {
                        el.child(outline(zoom, radius, 2., 2., accent))
                    })
            });
            let meta = crate::live::Meta::new("tab", format!("cfgp-game-{index}"))
                .label(Some(chip.name.clone().into()))
                .disabled(chip.disabled || disabled)
                .checked(active);
            group = group.child(crate::snap::target(
                format!("cfgp-game-{}", chip.id),
                crate::live::mark(meta, el),
            ));
        }
        group.into_any_element()
    }

    fn head(
        &self,
        games: &[ConfigGame],
        low: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let title = self.t(cx, "#L_TAB_CONFIGS", &[]).to_uppercase();
        let (size, line) = if low { (24., 32.) } else { (32., 40.) };
        let mut head = div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_x(u(32.))
            .gap_y(u(16.))
            .mb(u(if low { 12. } else { 16. }))
            .child(spaced(title, size, line, FontWeight::BOLD, 0.08, white()));
        if !games.is_empty() {
            let help = self.t(cx, "#L_CFG_HELP", &[]);
            let id = ElementId::Name("cfgp-help".into());
            let focus = focus_handle(&id, window, cx);
            let ring_on = focus_visible(&focus, window);
            let (dim, text, bg, hover_bg, border, accent) = (
                theme.color("l-text-dim"),
                theme.color("l-text"),
                theme.color("l-chip-bg"),
                theme.color("l-chip-hover"),
                theme.color("l-border"),
                theme.color("l-accent"),
            );
            let inner = id.clone();
            let game = self.game_id.clone();
            let button = hover(id, Timing::fast(), move |t| {
                div()
                    .id(inner)
                    .role(Role::Button)
                    .button_text()
                    .aria_label(help.clone())
                    .tip(help)
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(u(32.))
                    .ml(u(-16.))
                    .rounded_full()
                    .bg(mix(bg, hover_bg, t))
                    .shadow(vec![ring(zoom, 1., border)])
                    .text_color(mix(dim, text, t))
                    .child(fa_glyph("fas fa-question", 13.))
                    .track_focus(&focus)
                    .cursor_pointer()
                    .on_click(move |_, window, cx| super::tour::help(&game, window, cx))
                    .when(ring_on, |el| el.child(outline(zoom, 16., 2., 2., accent)))
            });
            let help = crate::snap::target("cfgp-help", button);
            let chips = super::anchors::anchor("configs-games", self.chips(games, window, cx));
            head = head.child(help).child(chips);
        }
        head.into_any_element()
    }

    fn notice(&self, notice: &Notice, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let weak = cx.weak_entity();
        let color = theme.color(if notice.error {
            "l-level-error"
        } else {
            "l-text-dim"
        });
        let rules = self.t(cx, "#L_HC_SHOW", &[]);
        let undo = self.t(cx, "#L_CFG_IMPORT_UNDO", &[]);
        div()
            .id("cfgp-notice")
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_x(u(12.))
            .gap_y(u(4.))
            .mb(u(12.))
            .px(u(11.))
            .py(u(7.))
            .rounded(u(theme.len("l-radius-sm")))
            .bg(theme.color("l-note-bg"))
            .border_1()
            .border_color(theme.color("l-border"))
            .text_size(u(13.))
            .line_height(relative(1.4))
            .text_color(color)
            .when_some(notice.details.clone(), |el, details| el.tip(details))
            .child(div().flex_1().min_w_0().child(notice.text.clone()))
            .when(notice.rules, |el| {
                let weak = weak.clone();
                el.child(
                    settings_btn("cfgp-rules", BtnLook::Plain)
                        .compact()
                        .on_click(move |_, window, cx| {
                            weak.update(cx, |this, cx| this.show_rules(window, cx)).ok();
                        })
                        .child(rules),
                )
            })
            .when(notice.undo, |el| {
                el.child(
                    settings_btn("cfgp-undo", BtnLook::Plain)
                        .compact()
                        .disabled(self.busy)
                        .on_click(move |_, window, cx| {
                            weak.update(cx, |this, cx| this.undo_import(window, cx))
                                .ok();
                        })
                        .child(undo),
                )
            })
            .into_any_element()
    }

    fn empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(u(12.))
            .text_size(u(15.))
            .text_color(theme.color("l-text-dim"))
            .child(self.t(cx, "#L_CFG_NO_GAMES", &[]))
            .child(
                settings_btn("cfgp-open-games", BtnLook::Primary)
                    .on_click(|_, _, cx| {
                        if let Some(context) = ShellContext::get(cx) {
                            context.nav.select_section("games");
                        }
                    })
                    .child(self.t(cx, "#L_TAB_GAMES", &[])),
            )
            .into_any_element()
    }

    /// Вопрос о несохранённом: подложка над страницей, окно по центру.
    fn dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let weak = cx.weak_entity();
        let answer = |choice: &'static str| {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| this.answer(choice, window, cx))
                    .ok();
            }
        };
        let on_scrim = {
            let weak = weak.clone();
            move |_: &gpui::MouseDownEvent, window: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| this.answer("stay", window, cx))
                    .ok();
            }
        };
        let on_key = {
            let weak = weak.clone();
            move |event: &KeyDownEvent, window: &mut Window, cx: &mut App| {
                if event.keystroke.key == "escape" {
                    weak.update(cx, |this, cx| this.answer("stay", window, cx))
                        .ok();
                }
            }
        };
        div()
            .id("cfgp-scrim")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.color("l-modal-scrim"))
            .occlude()
            .on_mouse_down(MouseButton::Left, on_scrim)
            .child(
                div()
                    .id("cfgp-dialog")
                    .track_focus(&self.dialog_focus)
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_key_down(on_key)
                    .flex()
                    .flex_col()
                    .gap(u(20.))
                    .max_w(u(440.))
                    .px(u(28.))
                    .py(u(24.))
                    .rounded(u(theme.len("l-radius")))
                    .bg(theme.color("l-modal-bg"))
                    .shadow(vec![
                        ring(zoom, 1., theme.color("l-modal-border")),
                        shadow(zoom, 0., 24., 72., 0., gpui::hsla(0., 0., 0., 0.6)),
                    ])
                    .child(
                        div()
                            .text_size(u(18.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.color("l-text"))
                            .child(self.t(cx, "#L_CFG_UNSAVED_TITLE", &[])),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .justify_end()
                            .gap(u(8.))
                            .child(
                                settings_btn("cfgp-stay", BtnLook::Plain)
                                    .on_click(answer("stay"))
                                    .child(self.t(cx, "#L_CFG_UNSAVED_STAY", &[])),
                            )
                            .child(
                                settings_btn("cfgp-discard", BtnLook::Plain)
                                    .on_click(answer("discard"))
                                    .child(self.t(cx, "#L_CFG_UNSAVED_DISCARD", &[])),
                            )
                            .child(
                                settings_btn("cfgp-save", BtnLook::Primary)
                                    .on_click(answer("save"))
                                    .child(self.t(cx, "#L_CFG_UNSAVED_SAVE", &[])),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for GameConfigsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let theme = cx.global::<Theme>().clone();
        let games = Self::games(cx);
        // Выбранную игру удалили — первая из оставшихся (`watch games`).
        if !games.iter().any(|game| game.id == self.game_id) {
            self.game_id = games
                .first()
                .map(|game| game.id.clone())
                .unwrap_or_default();
        }
        self.follow_shell(&games, window, cx);
        // Инспектору — игра страницы: у навигации только та, на которой страницу открыли.
        crate::live::note("configsGame", self.game_id.clone());
        // «Настройки» в меню игры открывают страницу на ней (`openToken`).
        let wanted = ShellContext::get(cx)
            .map(|context| context.nav.configs_game())
            .unwrap_or_default();
        if wanted != self.seen_nav_game {
            self.seen_nav_game = wanted.clone();
            if !wanted.is_empty() && wanted != self.game_id {
                let id = wanted.clone();
                cx.defer_in(window, move |this, window, cx| {
                    this.request_game(id, String::new(), window, cx)
                });
            }
        }
        self.ensure_settings(&games, window, cx);
        let low = viewport(window).1 <= 700.;
        let head = self.head(&games, low, window, cx);
        let notice = self.notice.clone().map(|notice| self.notice(&notice, cx));
        let body = if games.is_empty() {
            self.empty(cx)
        } else {
            match self.settings() {
                Some(settings) => div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(settings.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            }
        };
        let dialog = self.unsaved.is_some().then(|| self.dialog(window, cx));
        div()
            .id("cfgp")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .font_family(RUBIK)
            .pt(u(theme.len("l-topbar-h") + 24.))
            .pr(u(theme.len("l-edge-pad")))
            .pb(u(theme.len("l-edge-pad")))
            .pl(u(theme.len("l-hero-pad")))
            .child(head)
            .children(notice)
            .child(body)
            .children(dialog)
    }
}
