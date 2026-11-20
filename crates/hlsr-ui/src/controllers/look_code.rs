//! Код внешнего вида (`GamePersonalizationCode.vue` 0.4.7, задача 30): выгрузить и
//! скопировать (`game-personalization-code-export` + буфер обмена), вставить и
//! импортировать (`game-personalization-code-import` → `import` родителю). Без окна:
//! состояние; каналы и буфер обмена зовёт адаптер и возвращает ответы сюда.
//!
//! 0.4.9: моды в коде — [`ModsApply`] (`applyMods`: `custom-packs-get`, затем
//! `custom-packs-set` по одному, заметки «переключены / нет файлов / не вышло» под формой).

use hlsr_delivery::personalization_code::MAX_CODE_LENGTH;
use serde_json::Value;

/// Ключи ошибок импорта по коду ответа; прочее — «код повреждён».
const ERROR_KEYS: [(&str, &str); 3] = [
    ("PERSONALIZATION_CODE_WRONG_GAME", "#L_PZ_CODE_WRONG_GAME"),
    (
        "PERSONALIZATION_CODE_VERSION_UNSUPPORTED",
        "#L_PZ_CODE_VERSION",
    ),
    ("PERSONALIZATION_CODE_TOO_LARGE", "#L_PZ_CODE_TOO_LARGE"),
];

/// Переводчик строк (`$t`).
pub type Translate<'a> = &'a dyn Fn(&str, &[&str]) -> String;

/// Заметка под формой импорта о модах из кода.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModNote {
    pub text: String,
    pub error: bool,
}

/// Принятый код: вид и моды (если есть — сначала [`ModsApply`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub look: Value,
    pub mods: Option<Value>,
}

/// Что занято: выгрузка или импорт.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    Export,
    Import,
}

#[derive(Debug, Clone)]
pub struct LookCode {
    pub game_id: String,
    look: Value,
    pub invalid: bool,
    pub disabled: bool,
    pub export_code: String,
    pub code_input: String,
    pub busy: Option<Busy>,
    pub copied: bool,
    /// Ключи строк ошибок; пусто — нет.
    pub export_error: &'static str,
    pub import_error: &'static str,
    pub mod_notes: Vec<ModNote>,
    export_revision: u64,
}

impl LookCode {
    pub fn new(game_id: &str, look: Value) -> LookCode {
        LookCode {
            game_id: game_id.to_owned(),
            look,
            invalid: false,
            disabled: false,
            export_code: String::new(),
            code_input: String::new(),
            busy: None,
            copied: false,
            export_error: "",
            import_error: "",
            mod_notes: Vec::new(),
            export_revision: 0,
        }
    }

    pub const MAX_LENGTH: usize = MAX_CODE_LENGTH;

    /// `gameLabel`.
    pub fn game_label(&self) -> &'static str {
        if self.game_id == "70" {
            "Half-Life"
        } else {
            "HL2: Ghosting"
        }
    }

    /// Примечание о файлах — только у Ghosting.
    pub fn files_note(&self) -> bool {
        self.game_id == "218"
    }

    pub fn look(&self) -> &Value {
        &self.look
    }

    /// `watch look (deep)`: правка значений — прежний код устарел; те же значения — ничего.
    pub fn set_look(&mut self, look: Value) {
        if self.look != look {
            self.replace_look(look);
        }
    }

    /// `draft.look = …` (импорт кода): deep watch Vue срабатывает и на замену объекта теми же
    /// значениями — прежний код устарел всегда.
    pub fn replace_look(&mut self, look: Value) {
        self.look = look;
        self.export_revision += 1;
        self.export_code.clear();
        self.copied = false;
        self.export_error = "";
    }

    pub fn set_code_input(&mut self, text: &str) {
        self.code_input = text.to_owned();
        self.import_error = "";
        self.mod_notes.clear();
    }

    pub fn can_copy(&self) -> bool {
        !self.disabled && self.busy.is_none() && !self.invalid
    }

    pub fn can_import(&self) -> bool {
        !self.disabled && self.busy.is_none() && !hlsr_core::js::trim(&self.code_input).is_empty()
    }

    /// `copyCode()` начинается: `Some(ревизия)` — звать канал выгрузки с `look`.
    pub fn begin_copy(&mut self) -> Option<u64> {
        if !self.can_copy() {
            return None;
        }
        self.busy = Some(Busy::Export);
        self.export_error = "";
        self.copied = false;
        Some(self.export_revision)
    }

    /// Ответ выгрузки: `Some(код)` — копировать в буфер; `None` — закончено (ошибка или
    /// устаревшая ревизия).
    pub fn export_reply(&mut self, revision: u64, reply: Result<Value, String>) -> Option<String> {
        if self.export_revision != revision {
            return None;
        }
        match reply {
            Ok(result) if result.get("status").and_then(Value::as_str) == Some("ok") => {
                let code = result
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                self.export_code = code.clone();
                Some(code)
            }
            Ok(_) => {
                self.export_error = "#L_PZ_CODE_EXPORT_FAILED";
                self.busy = None;
                None
            }
            Err(_) => {
                self.export_error = if self.export_code.is_empty() {
                    "#L_PZ_CODE_EXPORT_FAILED"
                } else {
                    "#L_PZ_CODE_COPY_FAILED"
                };
                self.busy = None;
                None
            }
        }
    }

    /// Итог копирования в буфер.
    pub fn copy_done(&mut self, revision: u64, ok: bool) {
        if self.export_revision == revision {
            if ok {
                self.copied = true;
            } else {
                self.export_error = "#L_PZ_CODE_COPY_FAILED";
            }
        }
        self.busy = None;
    }

    /// `importCode()` начинается: `Some(код)` — звать канал импорта.
    pub fn begin_import(&mut self) -> Option<String> {
        if !self.can_import() {
            return None;
        }
        self.busy = Some(Busy::Import);
        self.import_error = "";
        self.mod_notes.clear();
        Some(self.code_input.clone())
    }

    /// Ответ импорта: `Some` — родителю (`$emit('import')`); с модами форма занята, пока
    /// [`ModsApply`] не кончит ([`Self::mods_done`]).
    pub fn import_reply(
        &mut self,
        game_id: &str,
        reply: Result<Value, String>,
    ) -> Option<Imported> {
        self.busy = None;
        if self.game_id != game_id {
            return None;
        }
        match reply {
            Ok(result) if result.get("status").and_then(Value::as_str) == Some("ok") => {
                let mods = result
                    .get("mods")
                    .filter(|m| hlsr_core::js::truthy(m))
                    .cloned();
                if mods.is_some() {
                    self.busy = Some(Busy::Import);
                }
                Some(Imported {
                    look: result.get("look").cloned().unwrap_or(Value::Null),
                    mods,
                })
            }
            Ok(result) => {
                let code = result
                    .get("error")
                    .and_then(|e| e.get("code"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                self.import_error = ERROR_KEYS
                    .iter()
                    .find(|(known, _)| *known == code)
                    .map_or("#L_PZ_CODE_INVALID", |(_, key)| key);
                None
            }
            Err(_) => {
                self.import_error = "#L_PZ_CODE_IMPORT_FAILED";
                None
            }
        }
    }

    /// Моды из кода переключены (или нет): заметки под формой, форма свободна.
    pub fn mods_done(&mut self, notes: Vec<ModNote>) {
        self.mod_notes = notes;
        self.busy = None;
    }

    /// Значок кнопки копирования (`fas fa-spinner fa-spin` / `fas fa-check` / `far fa-copy`).
    pub fn copy_icon(&self) -> &'static str {
        if self.busy == Some(Busy::Export) {
            "fas fa-spinner fa-spin"
        } else if self.copied {
            "fas fa-check"
        } else {
            "far fa-copy"
        }
    }

    pub fn import_icon(&self) -> &'static str {
        if self.busy == Some(Busy::Import) {
            "fas fa-spinner fa-spin"
        } else {
            "fas fa-file-import"
        }
    }
}

/// `applyMods(mods)`: моды из кода переключаются сразу, как на вкладке модов. Чего не вышло,
/// то названо под формой; вид из кода импортируется в любом случае.
#[derive(Debug, Clone)]
pub struct ModsApply {
    mods: Value,
    /// Список на момент начала (`for … of packs`: замена `packs` обход не меняет).
    initial: Vec<Value>,
    packs: Option<Value>,
    next: usize,
    switched: Vec<(String, bool)>,
    missing: Vec<String>,
    failed: String,
}

impl ModsApply {
    /// Ответ `custom-packs-get`.
    pub fn start(mods: Value, current: Result<Value, String>) -> ModsApply {
        let ok = current
            .as_ref()
            .ok()
            .filter(|r| r.get("status").and_then(Value::as_str) == Some("ok"));
        let packs = ok.and_then(|r| r.get("packs")).cloned();
        let failed = if packs.as_ref().is_none_or(|p| !hlsr_core::js::truthy(p)) {
            "CUSTOM_PACK_FAILED"
        } else if ok.and_then(|r| r.get("installed")) == Some(&Value::Bool(false)) {
            "NOT_INSTALLED"
        } else {
            ""
        };
        let initial = if failed.is_empty() {
            packs
                .as_ref()
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        ModsApply {
            mods,
            initial,
            packs,
            next: 0,
            switched: Vec::new(),
            missing: Vec::new(),
            failed: failed.to_owned(),
        }
    }

    /// Следующий вызов `custom-packs-set`: `(id мода, включить)`; `None` — всё.
    pub fn next_set(&mut self) -> Option<(String, bool)> {
        while let Some(pack) = self.initial.get(self.next) {
            self.next += 1;
            let id = pack.get("id").and_then(Value::as_str).unwrap_or("");
            let Some(want) = self.mods.get(id).and_then(Value::as_bool) else {
                continue;
            };
            if pack.get("enabled") == Some(&Value::Bool(want)) {
                continue;
            }
            if want && !pack.get("available").is_some_and(hlsr_core::js::truthy) {
                self.missing.push(id.to_owned());
                continue;
            }
            return Some((id.to_owned(), want));
        }
        None
    }

    /// Ответ `custom-packs-set`: отказ — дальше не переключать.
    pub fn set_reply(&mut self, id: &str, want: bool, reply: Result<Value, String>) {
        let result = reply.ok().filter(hlsr_core::js::truthy);
        if let Some(packs) = result
            .as_ref()
            .and_then(|r| r.get("packs"))
            .filter(|p| hlsr_core::js::truthy(p))
        {
            self.packs = Some(packs.clone());
        }
        if result
            .as_ref()
            .and_then(|r| r.get("status"))
            .and_then(Value::as_str)
            != Some("ok")
        {
            self.failed = result
                .as_ref()
                .and_then(|r| r.get("code"))
                .and_then(Value::as_str)
                .filter(|code| !code.is_empty())
                .unwrap_or("CUSTOM_PACK_FAILED")
                .to_owned();
            self.next = self.initial.len();
            return;
        }
        self.switched.push((id.to_owned(), want));
    }

    /// Список модов с диска (последний ответ) — превью узнаёт о HUD-моде сразу.
    pub fn packs(&self) -> Option<Value> {
        self.packs.clone()
    }

    /// Заметки под формой.
    pub fn notes(&self, t: Translate) -> Vec<ModNote> {
        let title = |id: &str| {
            t(
                &format!("#L_CPK_TITLE_{}", id.replace('-', "_").to_uppercase()),
                &[],
            )
        };
        let mut notes = Vec::new();
        if !self.switched.is_empty() {
            let list: Vec<String> = self
                .switched
                .iter()
                .map(|(id, on)| {
                    let key = if *on {
                        "#L_PZ_CODE_MOD_ON"
                    } else {
                        "#L_PZ_CODE_MOD_OFF"
                    };
                    t(key, &[&title(id)])
                })
                .collect();
            notes.push(ModNote {
                text: t("#L_PZ_CODE_MODS_DONE", &[&list.join(", ")]),
                error: false,
            });
        }
        if !self.missing.is_empty() {
            let list: Vec<String> = self.missing.iter().map(|id| title(id)).collect();
            notes.push(ModNote {
                text: t("#L_PZ_CODE_MODS_MISSING", &[&list.join(", ")]),
                error: true,
            });
        }
        if !self.failed.is_empty() {
            let key = super::custom_packs::error_key(&self.failed);
            notes.push(ModNote {
                text: t("#L_PZ_CODE_MODS_FAILED", &[&t(key, &[])]),
                error: true,
            });
        }
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn export_then_copy_and_a_stale_revision_is_dropped() {
        let mut code = LookCode::new("218", json!({"personalization": {}}));
        let revision = code.begin_copy().unwrap();
        assert_eq!(code.busy, Some(Busy::Export));
        assert_eq!(code.begin_copy(), None);
        // Черновик изменился, пока ждали ответ: ответ не принимается.
        code.set_look(json!({"personalization": {"hudEnabled": true}}));
        assert_eq!(
            code.export_reply(revision, Ok(json!({"status": "ok", "code": "HLSR-P1..."}))),
            None
        );
        assert_eq!(code.export_code, "");
        code.busy = None;
        let revision = code.begin_copy().unwrap();
        assert_eq!(
            code.export_reply(revision, Ok(json!({"status": "ok", "code": "HLSR-P1.x"}))),
            Some("HLSR-P1.x".into())
        );
        code.copy_done(revision, true);
        assert!(code.copied && code.busy.is_none());
        assert_eq!(code.copy_icon(), "fas fa-check");
        let revision = code.begin_copy().unwrap();
        assert_eq!(
            code.export_reply(revision, Ok(json!({"status": "error"}))),
            None
        );
        assert_eq!(code.export_error, "#L_PZ_CODE_EXPORT_FAILED");
    }

    #[test]
    fn import_errors_map_to_their_keys() {
        let mut code = LookCode::new("70", json!({}));
        assert_eq!(code.begin_import(), None);
        code.set_code_input("  x ");
        assert_eq!(code.begin_import(), Some("  x ".into()));
        assert_eq!(
            code.import_reply(
                "70",
                Ok(
                    json!({"status": "error", "error": {"code": "PERSONALIZATION_CODE_WRONG_GAME"}})
                )
            ),
            None
        );
        assert_eq!(code.import_error, "#L_PZ_CODE_WRONG_GAME");
        code.begin_import();
        assert_eq!(
            code.import_reply("70", Ok(json!({"status": "error"}))),
            None
        );
        assert_eq!(code.import_error, "#L_PZ_CODE_INVALID");
        code.begin_import();
        assert_eq!(code.import_reply("70", Err("x".into())), None);
        assert_eq!(code.import_error, "#L_PZ_CODE_IMPORT_FAILED");
        code.begin_import();
        assert_eq!(
            code.import_reply("70", Ok(json!({"status": "ok", "look": {"a": 1}}))),
            Some(Imported {
                look: json!({"a": 1}),
                mods: None
            })
        );
        assert!(code.busy.is_none());
        assert_eq!(code.game_label(), "Half-Life");
        assert!(!code.files_note());
    }

    #[test]
    fn mods_from_the_code_switch_one_by_one_and_name_what_failed() {
        let t = |key: &str, args: &[&str]| format!("{key}({})", args.join("|"));
        let mut code = LookCode::new("218", json!({}));
        code.set_code_input("HLSR-P1.218.x.y");
        code.begin_import();
        let reply = json!({"status": "ok", "look": {}, "mods": {"condensed-hud": true, "old-hud": false, "missing": true, "same": true}});
        let imported = code.import_reply("218", Ok(reply)).unwrap();
        assert_eq!(code.busy, Some(Busy::Import));
        let packs = json!({"status": "ok", "installed": true, "packs": [
            {"id": "condensed-hud", "enabled": false, "available": true},
            {"id": "old-hud", "enabled": true, "available": true},
            {"id": "missing", "enabled": false, "available": false},
            {"id": "same", "enabled": true, "available": true},
        ]});
        let mut apply = ModsApply::start(imported.mods.unwrap(), Ok(packs));
        assert_eq!(apply.next_set(), Some(("condensed-hud".into(), true)));
        apply.set_reply(
            "condensed-hud",
            true,
            Ok(json!({"status": "ok", "packs": [{"id": "condensed-hud", "enabled": true}]})),
        );
        assert_eq!(apply.next_set(), Some(("old-hud".into(), false)));
        apply.set_reply(
            "old-hud",
            false,
            Ok(json!({"status": "error", "code": "GAME_RUNNING"})),
        );
        assert_eq!(apply.next_set(), None);
        assert_eq!(apply.packs().unwrap()[0]["enabled"], true);
        code.mods_done(apply.notes(&t));
        assert!(code.busy.is_none());
        assert_eq!(
            code.mod_notes,
            vec![
                ModNote {
                    text: "#L_PZ_CODE_MODS_DONE(#L_PZ_CODE_MOD_ON(#L_CPK_TITLE_CONDENSED_HUD()))"
                        .into(),
                    error: false
                },
                ModNote {
                    text: "#L_PZ_CODE_MODS_FAILED(#L_CPK_ERR_RUNNING())".into(),
                    error: true
                },
            ]
        );
        // Нет списка модов или игра не установлена — ничего не переключается.
        let mut none = ModsApply::start(json!({"a": true}), Err("x".into()));
        assert_eq!(none.next_set(), None);
        assert_eq!(
            none.notes(&t)[0].text,
            "#L_PZ_CODE_MODS_FAILED(#L_CPK_ERR_FAILED())"
        );
        let mut absent = ModsApply::start(
            json!({"a": true}),
            Ok(json!({"status": "ok", "installed": false, "packs": [{"id": "a"}]})),
        );
        assert_eq!(absent.next_set(), None);
        assert_eq!(
            absent.notes(&t)[0].text,
            "#L_PZ_CODE_MODS_FAILED(#L_CPK_NOT_INSTALLED())"
        );
        // Правка поля стирает заметки.
        code.set_code_input("x");
        assert!(code.mod_notes.is_empty());
    }
}
