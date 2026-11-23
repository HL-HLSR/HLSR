//! Бинды окна настроек игры: клавиатура (`GameKeyboard.vue`, окно поверх настроек), вкладка
//! «Скрипты» (`GameScripts.vue`) и импорт биндов и cfg целиком из окна клавиатуры
//! (`GameSettingsModal.vue`: `importBinds`, `adoptImport`, `removeImportedCfg`, подключение
//! импорта в `submit`). Черновик биндов общий (`draft.binds`): «Сохранить» сохраняет всё.

use super::*;
use crate::controllers::bind_state;
use crate::controllers::game_keyboard::{self, KeyboardView, RunMode};
use crate::controllers::game_scripts::{self, Outcome, ScriptsView};
use hlsr_core::js::object::JsObject;
use hlsr_delivery::cfg_import::{self, ImportFiles, Picked, Root};
use hlsr_delivery::game_binds::{self, BindRead};
use hlsr_delivery::{bind_presets, goldsrc_import_adopt};
use serde_json::Map;

/// Конфиг HL1, который подключится после «Сохранить» (`pendingCfgImport`).
#[derive(Debug, Clone)]
pub(super) struct PendingImport {
    pub text: String,
    /// `{root, files, missing}` для `game-binds-import-apply`.
    pub payload: Value,
    /// Черновик до первого импорта: отмена импорта до «Сохранить» возвращает его.
    pub undo: Undo,
}

#[derive(Debug, Clone)]
pub(super) struct Undo {
    pub binds: Value,
    pub personalization: Value,
    pub bxt: Value,
}

fn root_value(root: &Root) -> Value {
    let mut value = Map::new();
    value.insert("name".into(), json!(root.name));
    value.insert("text".into(), json!(root.text));
    if root.virtual_root {
        value.insert("virtual".into(), json!(true));
    }
    Value::Object(value)
}

fn files_value(files: &ImportFiles) -> Value {
    let mut value = Map::new();
    for (key, file) in files.iter() {
        let mut entry = Map::new();
        entry.insert("name".into(), json!(file.name));
        if let Some(target) = &file.target {
            entry.insert("target".into(), json!(target));
        }
        entry.insert("text".into(), json!(file.text));
        value.insert(key.to_owned(), Value::Object(entry));
    }
    Value::Object(value)
}

/// Объект JS в порядке ключей (`Object.keys`): индексы по возрастанию, затем вставка.
fn js_ordered(map: &Map<String, Value>) -> JsObject<Value> {
    map.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn object_of(object: &JsObject<Value>) -> Value {
    Value::Object(
        object
            .iter()
            .map(|(key, value)| (key.to_owned(), value.clone()))
            .collect(),
    )
}

/// Какой текст ошибки записи импорта показать (`importErrorText`).
fn import_error_key(result: Option<&Value>) -> &'static str {
    match result.and_then(|r| jsv::str_of(jsv::prop(r, "code"))) {
        Some("GAME_RUNNING") => "#L_D_ERR_GAME_RUNNING",
        Some("BINDS_FILE_TOO_LARGE") => "#L_KB_IMPORT_TOO_LARGE",
        _ => "#L_KB_IMPORT_WRITE_FAILED",
    }
}

impl Session {
    /// Режим рана клавиатуры из prefs (`readRunMode()` при создании компонента).
    pub fn set_keyboard_prefs(&self, prefs: Rc<dyn game_keyboard::KeyboardPrefs>) {
        let mode = RunMode::from_stored(prefs.run_mode().as_deref());
        self.state.borrow_mut().keyboard.run_mode = mode;
        *self.keyboard_prefs.borrow_mut() = prefs;
    }

    /// Бинды для клавиатуры и «Скриптов» (`shownKeyBinds`): с ещё не сохранённым импортом — как
    /// они будут в игре после «Сохранить».
    pub(super) fn shown_key_binds(state: &State) -> Option<BindRead> {
        let read = bind_state::key_binds_from_value(state.key_binds.as_ref());
        match &state.pending_cfg_import {
            Some(pending) => Some(game_binds::with_import(read.as_ref(), &pending.text)),
            None => read,
        }
    }

    /// Half-Life 1 импортирует конфиг целиком, остальные игры — только бинды.
    pub(super) fn whole_cfg_import(state: &State) -> bool {
        state
            .binds_game_id
            .as_deref()
            .and_then(bind_presets::engine_for)
            == Some("goldsrc")
    }

    fn imported_cfg_on_disk(state: &State) -> bool {
        let Some(key_binds) = state.key_binds.as_ref() else {
            return false;
        };
        if jsv::prop(key_binds, "imported")
            .and_then(Value::as_array)
            .is_some_and(|list| !list.is_empty())
        {
            return true;
        }
        // Импорт прошлых версий лаунчера (hlsr-import.cfg).
        jsv::prop(key_binds, "files")
            .and_then(Value::as_array)
            .is_some_and(|files| {
                files.iter().any(|name| {
                    js_str(Some(name)).to_lowercase() == game_binds::IMPORT_FILE.to_lowercase()
                })
            })
    }

    /// Кнопка «Убрать импорт» видна.
    pub(super) fn imported_cfg_active(state: &State) -> bool {
        Self::whole_cfg_import(state)
            && (state.pending_cfg_import.is_some() || Self::imported_cfg_on_disk(state))
    }

    fn draft_binds(state: &State) -> Value {
        state
            .draft
            .get("binds")
            .cloned()
            .unwrap_or_else(|| json!({}))
    }

    fn lang(&self) -> &'static str {
        self.env.language.code()
    }

    /// Переназначения последнего сохранения (`savedBinds`); нет `binds` — `empty`.
    fn saved_binds<'s>(state: &'s State, empty: &'s Value) -> Option<&'s Value> {
        let draft = state.draft_saved.as_ref()?;
        Some(draft.get("binds").unwrap_or(empty))
    }

    /// Клавиатура для показа (окно клавиатуры собрано заранее).
    pub(super) fn keyboard_view(&self, state: &State) -> Option<KeyboardView> {
        let game_id = state.binds_game_id.as_deref()?;
        let binds = Self::draft_binds(state);
        // `savedBinds` — переназначения последнего сохранения, `keyBinds` — cfg с диска («Пианино»).
        let empty = json!({});
        let saved = Self::saved_binds(state, &empty);
        // cfg с диска разбирается один раз: без несохранённого импорта он же `shownKeyBinds`.
        let saved_read = bind_state::key_binds_from_value(state.key_binds.as_ref());
        let imported = state
            .pending_cfg_import
            .as_ref()
            .map(|pending| game_binds::with_import(saved_read.as_ref(), &pending.text));
        let shown = imported.as_ref().or(saved_read.as_ref());
        let t = |key: &str, args: &[&str]| self.t(key, args);
        let inputs = game_keyboard::Inputs {
            game_id,
            value: &binds,
            key_binds: shown,
            lang: self.lang(),
            platform: self.platform(),
            t: &t,
            full: self.keyboard_prefs.borrow().keyboard_full(),
            saved_value: saved,
            saved_key_binds: saved_read.as_ref(),
        };
        Some(state.keyboard.view(&inputs))
    }

    /// Клавиатура для вида раздела (без остального [`View`]); `None` — у игры её нет.
    pub fn keyboard(&self) -> Option<KeyboardView> {
        self.keyboard_view(&self.state.borrow())
    }

    /// «Скрипты» для вида раздела.
    pub fn scripts(&self) -> ScriptsView {
        self.scripts_view(&self.state.borrow())
    }

    /// Вкладка «Скрипты» для показа.
    pub(super) fn scripts_view(&self, state: &State) -> ScriptsView {
        let binds = Self::draft_binds(state);
        let shown = Self::shown_key_binds(state);
        let t = |key: &str, args: &[&str]| self.t(key, args);
        let inputs = game_scripts::Inputs {
            value: &binds,
            key_binds: shown.as_ref(),
            lang: self.lang(),
            platform: self.platform(),
            t: &t,
        };
        state.scripts.view(&inputs)
    }

    // ---- клавиатура -------------------------------------------------------------------------

    fn keyboard_change(
        self: &Rc<Self>,
        change: impl FnOnce(&mut game_keyboard::Keyboard, &game_keyboard::Inputs<'_>) -> Option<Value>,
    ) {
        {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            let Some(game_id) = state.binds_game_id.clone() else {
                return;
            };
            let binds = Self::draft_binds(state);
            let shown = Self::shown_key_binds(state);
            let t = |key: &str, args: &[&str]| self.t(key, args);
            let inputs = game_keyboard::Inputs {
                game_id: &game_id,
                value: &binds,
                key_binds: shown.as_ref(),
                lang: self.lang(),
                platform: self.platform(),
                t: &t,
                full: self.keyboard_prefs.borrow().keyboard_full(),
                saved_value: None,
                saved_key_binds: None,
            };
            if let Some(next) = change(&mut state.keyboard, &inputs) {
                state.draft.set("binds", next);
            }
        }
        self.dirty();
        self.pump();
    }

    /// Нажатие клавиши схемы или клавиши вне схемы.
    pub fn kb_select(self: &Rc<Self>, key: &str) {
        self.keyboard_change(|kb, _| {
            kb.select(Some(key));
            None
        });
    }

    /// Указатель над клавишей (пусто — ушёл со схемы).
    pub fn kb_hover(self: &Rc<Self>, key: Option<&str>) {
        let key = key.filter(|key| !key.is_empty()).map(str::to_owned);
        if self.state.borrow().keyboard.hovered == key {
            return;
        }
        self.keyboard_change(|kb, _| {
            kb.hovered = key;
            None
        });
    }

    /// Заготовка или «снять» (пустая команда) для выбранной клавиши.
    pub fn kb_assign(self: &Rc<Self>, command: &str) {
        self.keyboard_change(|kb, inputs| kb.assign(inputs, command));
    }

    pub fn kb_revert(self: &Rc<Self>) {
        self.keyboard_change(|kb, inputs| kb.revert(inputs));
    }

    pub fn kb_set_custom(self: &Rc<Self>, text: &str) {
        self.keyboard_change(|kb, _| {
            kb.set_custom(text);
            None
        });
    }

    /// «Применить» своей команды (кнопка или Enter в поле, пока кнопка доступна).
    pub fn kb_assign_custom(self: &Rc<Self>) {
        if !self.state.borrow().keyboard.custom_enabled() {
            return;
        }
        self.keyboard_change(|kb, inputs| kb.assign_custom(inputs));
    }

    pub fn kb_set_query(self: &Rc<Self>, text: &str) {
        self.keyboard_change(|kb, _| {
            kb.query = text.to_owned();
            None
        });
    }

    /// Категория (пусто — «Все»).
    pub fn kb_toggle_category(self: &Rc<Self>, id: &str) {
        self.keyboard_change(|kb, _| {
            kb.toggle_category(Some(id).filter(|id| !id.is_empty()));
            None
        });
    }

    /// Scriptless / Scripted; режим запоминается.
    pub fn kb_set_run_mode(self: &Rc<Self>, mode: &str) {
        let mode = RunMode::from_stored(Some(mode));
        let prefs = self.keyboard_prefs.borrow().clone();
        prefs.set_run_mode(mode.id());
        self.keyboard_change(|kb, _| {
            kb.run_mode = mode;
            None
        });
    }

    /// «Вся клавиатура» / «Основной блок» (`setKeyboardFull`): выбор запоминается.
    pub fn kb_set_full(self: &Rc<Self>, full: bool) {
        let prefs = self.keyboard_prefs.borrow().clone();
        prefs.set_keyboard_full(full);
        self.dirty();
        self.pump();
    }

    /// `watch everyKeyBound` (`immediate`) клавиатуры, пока она есть у игры: стало «занята вся
    /// схема» — «Пианино» (`unlockPiano`: `runner-secret-unlock`, ошибка не видна).
    pub(super) fn watch_piano(&self) {
        let (every, changed) = {
            let state = self.state.borrow();
            let every = state.binds_game_id.as_deref().map(|game_id| {
                let empty = json!({});
                let saved_read = bind_state::key_binds_from_value(state.key_binds.as_ref());
                game_keyboard::every_key_bound(
                    game_id,
                    self.platform(),
                    Self::saved_binds(&state, &empty),
                    saved_read.as_ref(),
                )
            });
            (every, state.piano != every)
        };
        if !changed {
            return;
        }
        let unlock = {
            let mut state = self.state.borrow_mut_nav();
            let Some(every) = every else {
                state.piano = None;
                return;
            };
            state.piano.replace(every) != Some(every) && every
        };
        if unlock {
            self.fire("runner-secret-unlock", vec![json!("piano")]);
        }
    }

    // ---- импорт биндов и cfg ----------------------------------------------------------------

    /// Кнопка «Импорт» окна клавиатуры (`importBinds`).
    pub fn import_binds(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            if state.importing_binds || state.binds_game_id.is_none() {
                return;
            }
            state.importing_binds = true;
        }
        let session = self.clone();
        self.tasks.spawn(async move {
            let note = match session.run_import_binds().await {
                Ok(note) => note,
                Err(_) => Some(Note::error(session.t("#L_KB_IMPORT_FAILED", &[]))),
            };
            {
                let mut state = session.state.borrow_mut();
                if let Some(note) = note {
                    state.binds_import = Some(note);
                }
                state.importing_binds = false;
            }
            session.dirty();
        });
        self.dirty();
        self.pump();
    }

    async fn run_import_binds(self: &Rc<Self>) -> Result<Option<Note>, String> {
        let game_id = self
            .state
            .borrow()
            .binds_game_id
            .clone()
            .unwrap_or_default();
        let t = |key: &str, args: &[&str]| self.t(key, args);
        let (files, dir) = match import::pick_first(&self.services, &game_id).await? {
            import::FirstPick::Canceled => return Ok(None),
            import::FirstPick::Failed { too_large } => {
                let key = if too_large {
                    "#L_KB_IMPORT_TOO_LARGE"
                } else {
                    "#L_KB_IMPORT_FAILED"
                };
                return Ok(Some(Note::error(t(key, &[]))));
            }
            import::FirstPick::Files { files, dir } => (files, dir),
        };
        let engine = bind_presets::engine_for(&game_id).unwrap_or("goldsrc");
        let picked: Vec<Picked> = files
            .iter()
            .map(|file| Picked {
                name: file.name.clone(),
                text: file.text.clone(),
            })
            .collect();
        let chosen =
            import::with_companions(&self.services, &game_id, engine, picked, dir.as_deref())
                .await?;
        let cfg_import::RootOfPicked {
            root,
            files: picked,
        } = cfg_import::root_of_picked(&chosen);
        let name = root.name.clone();
        let whole = Self::whole_cfg_import(&self.state.borrow());
        let chain =
            import::pick_chain(&self.services, &game_id, &root, dir, engine, !whole, picked)
                .await?;
        let built = cfg_import::build_import(engine, &root, &chain.files, &chain.missing);
        if whole && built.bytes > game_binds::MAX_IMPORT_BYTES {
            return Ok(Some(Note::error(t("#L_KB_IMPORT_TOO_LARGE", &[]))));
        }
        let report = import::import_report(&built.report, &t);
        if whole {
            let preview = game_binds::with_import(None, &built.flat_text);
            let (bind_count, alias_count) = (preview.binds.len(), preview.aliases.len());
            if bind_count == 0 && alias_count == 0 {
                return Ok(Some(Note::error(t("#L_KB_IMPORT_EMPTY", &[&name]))));
            }
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            // Отмена импорта до «Сохранить» возвращает то, что импорт поменял в черновике;
            // повторный импорт — состояние до первого.
            let undo = match &state.pending_cfg_import {
                Some(pending) => pending.undo.clone(),
                None => Undo {
                    binds: Self::draft_binds(state),
                    personalization: state
                        .draft
                        .get("personalization")
                        .cloned()
                        .unwrap_or(Value::Null),
                    bxt: state.draft.get("bxt").cloned().unwrap_or(Value::Null),
                },
            };
            let (extra_short, extra_details) = self.adopt_import(state, &built.flat_text);
            state.pending_cfg_import = Some(PendingImport {
                text: built.flat_text.clone(),
                payload: json!({
                    "root": root_value(&root),
                    "files": files_value(&chain.files),
                    "missing": chain.missing,
                }),
                undo,
            });
            let mut parts = vec![t(
                "#L_KB_IMPORT_CFG_DONE",
                &[&name, &bind_count.to_string(), &alias_count.to_string()],
            )];
            parts.extend(extra_short);
            parts.extend(report.short.iter().cloned());
            let details: Vec<String> = [report.details.clone(), Some(extra_details)]
                .into_iter()
                .flatten()
                .filter(|text| !text.is_empty())
                .collect();
            return Ok(Some(Note {
                error: false,
                text: parts.join(" "),
                details: (!details.is_empty()).then(|| details.join("\n")),
            }));
        }
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let read = bind_state::key_binds_from_value(state.key_binds.as_ref()).unwrap_or_default();
        let overrides = bind_state::overrides_of(&Self::draft_binds(state));
        let catalog = bind_presets::catalog_for(&game_id, self.platform());
        let known = |command: &str| catalog.is_some_and(|c| c.preset_of(command).is_some());
        let imported = game_binds::import_overrides(
            &built.flat_text,
            &game_binds::ImportOptions {
                file_binds: Some(&read.binds),
                overrides: Some(&overrides),
                known_command: Some(&known),
                aliases: Some(&read.aliases),
            },
        );
        if imported.found == 0 {
            return Ok(Some(Note::error(t("#L_KB_IMPORT_EMPTY", &[&name]))));
        }
        state
            .draft
            .set("binds", game_binds::overrides_to_value(&imported.overrides));
        let skipped = imported.skipped_alias + imported.skipped_invalid + imported.skipped_key;
        let mut parts = vec![t(
            "#L_KB_IMPORT_DONE",
            &[&name, &(imported.changed + imported.unbound).to_string()],
        )];
        if skipped > 0 {
            parts.push(t("#L_KB_IMPORT_SKIPPED", &[&skipped.to_string()]));
        }
        parts.extend(report.short.iter().cloned());
        Ok(Some(Note {
            error: false,
            text: parts.join(" "),
            details: report.details.clone(),
        }))
    }

    /// `adoptImport(flatText)`: прицел, HUD и поле зрения из конфига — в персонализацию
    /// лаунчера; переназначения на клавишах, которые конфиг назначает сам, снимаются. Краткие
    /// строки отчёта и подробности.
    fn adopt_import(&self, state: &mut State, flat_text: &str) -> (Vec<String>, String) {
        let t = |key: &str, args: &[&str]| self.t(key, args);
        let mut short = Vec::new();
        let mut details = Vec::new();
        let personalization = match state.draft.get("personalization") {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        let bxt = match state.draft.get("bxt") {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        let adopted = goldsrc_import_adopt::adopt_imported(flat_text, &personalization, &bxt);
        if !adopted.adopted.is_empty() {
            state.draft.set(
                "personalization",
                draft::normalize_personalization(Some(&Value::Object(
                    adopted.personalization.clone(),
                ))),
            );
            state.draft.set("bxt", Value::Object(adopted.bxt.clone()));
            short.push(t(
                "#L_KB_IMPORT_ADOPTED",
                &[&adopted.adopted.len().to_string()],
            ));
            details.push(t(
                "#L_KB_IMPORT_ADOPTED_LIST",
                &[&adopted.adopted.join(", ")],
            ));
        }
        if !adopted.skipped.is_empty() {
            short.push(t(
                "#L_KB_IMPORT_NOT_ADOPTED",
                &[&adopted.skipped.len().to_string()],
            ));
            details.push(t(
                "#L_KB_IMPORT_NOT_ADOPTED_LIST",
                &[&adopted.skipped.join(", ")],
            ));
        }
        let touched = game_binds::import_touched_keys(flat_text);
        if let Value::Object(map) = Self::draft_binds(state) {
            let mut binds = js_ordered(&map);
            let dropped: Vec<String> = binds
                .keys()
                .filter(|key| touched.iter().any(|t| t == key))
                .map(str::to_owned)
                .collect();
            if !dropped.is_empty() {
                for key in &dropped {
                    binds.remove(key);
                }
                state.draft.set("binds", object_of(&binds));
                short.push(t("#L_KB_IMPORT_OVERRIDES_DROPPED", &[&dropped.join(", ")]));
            }
        }
        (short, details.join("\n"))
    }

    /// «Убрать импорт» (`removeImportedCfg`): несохранённый импорт отменяется в черновике,
    /// подключённый — снимается главным процессом.
    pub fn remove_imported_cfg(self: &Rc<Self>) {
        let proceed = {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            if state.importing_binds || state.binds_game_id.is_none() {
                return;
            }
            let mut proceed = true;
            if let Some(pending) = state.pending_cfg_import.take() {
                state.draft.set("binds", pending.undo.binds);
                state
                    .draft
                    .set("personalization", pending.undo.personalization);
                state.draft.set("bxt", pending.undo.bxt);
                state.binds_import = None;
                proceed = Self::imported_cfg_on_disk(state);
            }
            if proceed {
                state.importing_binds = true;
            }
            proceed
        };
        if !proceed {
            self.dirty();
            self.pump();
            return;
        }
        let session = self.clone();
        self.tasks.spawn(async move {
            let game_id = session
                .state
                .borrow()
                .binds_game_id
                .clone()
                .unwrap_or_default();
            let reply = session
                .call("game-binds-import-remove", vec![json!(game_id)])
                .await;
            {
                let t = |key: &str, args: &[&str]| session.t(key, args);
                let mut state = session.state.borrow_mut();
                match reply {
                    Ok(result) if jsv::is_str(jsv::prop(&result, "status"), "ok") => {
                        state.key_binds = jsv::prop(&result, "keyBinds")
                            .filter(|binds| js::truthy(binds))
                            .cloned();
                        state.binds_import = Some(Note {
                            error: false,
                            text: t("#L_KB_IMPORT_REMOVED", &[]),
                            details: None,
                        });
                    }
                    Ok(result) => {
                        state.binds_import =
                            Some(Note::error(t(import_error_key(Some(&result)), &[])));
                    }
                    Err(_) => {
                        state.binds_import = Some(Note::error(t("#L_KB_IMPORT_FAILED", &[])));
                    }
                }
                state.importing_binds = false;
            }
            session.dirty();
        });
        self.dirty();
        self.pump();
    }

    /// `submit`: подключить несохранённый импорт HL1 до сохранения настроек. `false` — не
    /// удалось, сохранение не идёт.
    pub(super) async fn apply_pending_import(self: &Rc<Self>) -> bool {
        let (game_id, payload) = {
            let state = self.state.borrow();
            match &state.pending_cfg_import {
                Some(pending) => (
                    state.binds_game_id.clone().unwrap_or_default(),
                    pending.payload.clone(),
                ),
                None => return true,
            }
        };
        let reply = self
            .call("game-binds-import-apply", vec![json!(game_id), payload])
            .await;
        let t = |key: &str, args: &[&str]| self.t(key, args);
        let mut state = self.state.borrow_mut();
        match reply {
            Ok(imported) if jsv::is_str(jsv::prop(&imported, "status"), "ok") => {
                state.pending_cfg_import = None;
                if let Some(binds) = jsv::prop(&imported, "keyBinds").filter(|b| js::truthy(b)) {
                    state.key_binds = Some(binds.clone());
                }
                true
            }
            Ok(imported) => {
                let text = t(import_error_key(Some(&imported)), &[]);
                state.args_error = Some(text.clone());
                state.binds_import = Some(Note::error(text));
                false
            }
            Err(_) => {
                // catch: любой сбой IPC — общий текст.
                state.args_error = Some(draft::error_text(&SaveError::default(), &t));
                false
            }
        }
    }

    // ---- «Скрипты» --------------------------------------------------------------------------

    fn scripts_change(
        self: &Rc<Self>,
        change: impl FnOnce(&mut game_scripts::Scripts, &game_scripts::Inputs<'_>) -> Outcome,
    ) -> Outcome {
        let outcome = {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            let binds = Self::draft_binds(state);
            let shown = Self::shown_key_binds(state);
            let t = |key: &str, args: &[&str]| self.t(key, args);
            let inputs = game_scripts::Inputs {
                value: &binds,
                key_binds: shown.as_ref(),
                lang: self.lang(),
                platform: self.platform(),
                t: &t,
            };
            let playing = state.scripts.playing;
            let outcome = change(&mut state.scripts, &inputs);
            if let Some(next) = &outcome.emitted {
                state.draft.set("binds", next.clone());
            }
            (outcome, playing && !state.scripts.playing)
        };
        let (outcome, video_closed) = outcome;
        if video_closed {
            self.emit(Event::Video(None));
        }
        if outcome.capture_stopped {
            self.emit(Event::StopCapture);
        }
        if let Some(url) = &outcome.open_link {
            self.fire("hl1-guide-open-link", vec![json!(url)]);
        }
        self.dirty();
        self.pump();
        outcome
    }

    pub fn scr_set_query(self: &Rc<Self>, text: &str) {
        self.scripts_change(|scripts, _| {
            scripts.query = text.to_owned();
            Outcome::default()
        });
    }

    /// Скрипт в списке.
    pub fn scr_open(self: &Rc<Self>, id: &str) {
        self.scripts_change(|scripts, _| scripts.open(id));
    }

    /// Ссылка в описании: `script` — другой скрипт гайда, `href` — наружу.
    pub fn scr_link(self: &Rc<Self>, kind: &str, value: &str) {
        self.scripts_change(|scripts, _| scripts.link(kind, value));
    }

    /// «Сохранение», «Гайд», «Открыть на YouTube» (`openLink`).
    pub fn scr_open_link(&self, url: &str) {
        self.fire("hl1-guide-open-link", vec![json!(url)]);
    }

    /// Постер: плеер (`webview` — есть встроенный) или страница YouTube в браузере.
    pub fn scr_play(self: &Rc<Self>, webview: bool) {
        let outcome = self.scripts_change(|scripts, inputs| scripts.play(inputs, webview));
        let _ = outcome;
        let embed = {
            let state = self.state.borrow();
            state
                .scripts
                .playing
                .then(|| self.scripts_view(&state).video.map(|video| video.embed))
                .flatten()
        };
        if let Some(url) = embed {
            self.emit(Event::Video(Some(url)));
        }
    }

    pub fn scr_assign_guide_keys(self: &Rc<Self>) {
        self.scripts_change(|scripts, inputs| scripts.assign_guide_keys(inputs));
    }

    pub fn scr_confirm_conflict(self: &Rc<Self>) {
        self.scripts_change(|scripts, inputs| scripts.confirm_conflict(inputs));
    }

    pub fn scr_cancel_conflict(self: &Rc<Self>) {
        self.scripts_change(|scripts, _| {
            scripts.cancel_conflict();
            Outcome::default()
        });
    }

    pub fn scr_unassign(self: &Rc<Self>, key: &str) {
        self.scripts_change(|scripts, inputs| scripts.unassign(inputs, key));
    }

    /// «Назначить»/«Добавить»: `true` — началось ожидание клавиши (окно подписывает перехват).
    pub fn scr_start_capture(self: &Rc<Self>, command: &str) -> bool {
        self.scripts_change(|scripts, _| scripts.start_capture(command))
            .capture_started
    }

    /// Ожидание клавиши кончилось (перехват winit): `key` — клавиша или `None`.
    pub fn scr_capture_end(self: &Rc<Self>, owner: &str, key: Option<&str>) {
        self.scripts_change(|scripts, inputs| scripts.capture_end(inputs, owner, key));
    }

    /// Перевод или оригинал описания.
    pub fn scr_toggle_original(self: &Rc<Self>) {
        self.scripts_change(|scripts, _| {
            scripts.show_original = !scripts.show_original;
            Outcome::default()
        });
    }

    /// Вкладка «Скрипты» перестала быть видна (`watch active`).
    pub(super) fn scripts_left(self: &Rc<Self>) {
        let (playing, capturing) = {
            let state = self.state.borrow();
            (state.scripts.playing, state.scripts.capturing.is_some())
        };
        if playing || capturing {
            let mut state = self.state.borrow_mut();
            let _ = state.scripts.deactivate();
        }
        if playing {
            self.emit(Event::Video(None));
        }
        self.emit(Event::StopCapture);
    }

    /// Ждёт ли вкладка клавишу (тесты).
    pub fn scripts_capturing(&self) -> Option<String> {
        self.state.borrow().scripts.capturing.clone()
    }

    /// Режим рана клавиатуры (тесты).
    pub fn keyboard_run_mode(&self) -> RunMode {
        self.state.borrow().keyboard.run_mode
    }
}
