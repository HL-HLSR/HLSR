//! Вкладка «Скрипты» окна настроек Half-Life (`GameScripts.vue` 0.4.7): скрипты гайда
//! yavats/hl1-scripts по главам с поиском и назначенными клавишами, выбранный скрипт — заголовок,
//! «сохранение» и гайд, видео (постер → встроенный плеер), клавиши действий (захват клавиатуры,
//! мыши и колеса; занятая клавиша — вопрос; «как в гайде»; снять), описание — перевод или
//! оригинал (markdown гайда).
//!
//! [`Scripts`] — данные компонента, [`Guide`] — вычисляемые свойства при данных окна; действия,
//! меняющие бинды, возвращают новый черновик (`$emit("input", next)`). Захват клавиши ведёт сам
//! компонент ([`Scripts::capturing`]); нажатия по физическим кодам приносит перехват winit
//! (`adapters::input`, `keys.rs`): [`Scripts::capture_end`].
//!
//! Плеер — дочерний webview (решение 12 спецификации ПП5): где и как его показать, решает
//! [`video_placement`] по слою и прокрутке.

use super::bind_state::{self, Effective, KeysByCommand};
use crate::keyboard_layout::key_name;
use hlsr_core::js::object::JsObject;
use hlsr_core::js::{self, Platform};
use hlsr_delivery::bind_presets::{self, Catalog, command_key};
use hlsr_delivery::game_binds::{self, Bind, BindRead, Override, Overrides, clean_command};
use hlsr_delivery::hl1_guide::{self, Chapter, Script, Translation};
use serde_json::Value;

/// Переводчик `$t(key, …args)`.
pub type Tr<'a> = &'a dyn Fn(&str, &[&str]) -> String;

/// Игра гайда.
pub const GAME_ID: &str = "hl1-won";

/// Адрес картинок гайда (`MD_OPTIONS.imageBase`).
pub fn image_base() -> String {
    format!(
        "https://raw.githubusercontent.com/yavats/hl1-scripts/{}",
        hl1_guide::source().commit
    )
}

/// Скрипты гайда (`MD_OPTIONS.scriptIds`): ссылки `#id` на другой скрипт.
pub fn script_ids() -> Vec<String> {
    hl1_guide::scripts().iter().map(|s| s.id.clone()).collect()
}

/// Вопрос о занятых клавишах: что назначить и что на них сейчас.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Клавиша → команда (порядок ключей объекта JS).
    pub assignments: JsObject<String>,
    pub items: Vec<ConflictItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictItem {
    pub key: String,
    pub name: String,
    pub label: String,
}

/// Данные компонента.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scripts {
    pub query: String,
    pub current_id: String,
    pub playing: bool,
    pub capturing: Option<String>,
    pub conflict: Option<Conflict>,
    /// Описание на английском, как в гайде, вместо перевода.
    pub show_original: bool,
    /// Прокрутка детали в начало (`$refs.scroll.scrollTop = 0`): растёт при смене скрипта.
    pub scroll_reset: u32,
}

impl Default for Scripts {
    fn default() -> Scripts {
        Scripts {
            query: String::new(),
            current_id: hl1_guide::scripts()[0].id.clone(),
            playing: false,
            capturing: None,
            conflict: None,
            show_original: false,
            scroll_reset: 0,
        }
    }
}

/// Глава списка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterView {
    pub id: String,
    pub title: String,
    pub scripts: Vec<ScriptItem>,
}

/// Скрипт в списке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptItem {
    pub id: String,
    pub title: String,
    pub mark: Option<String>,
    pub keys: Vec<String>,
}

/// Клавиша действия.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChip {
    pub key: String,
    pub name: String,
    /// Назначена целиком (иначе — часть чужого составного бинда, её отсюда не снять).
    pub whole: bool,
    /// Бинд из импортированного конфига.
    pub from_config: bool,
    pub title: Option<String>,
}

/// Строка действия скрипта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRow {
    pub command: String,
    pub guide_key: Option<String>,
    pub guide_key_name: String,
    pub keys: Vec<KeyChip>,
}

/// Адреса видео скрипта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoLinks {
    pub poster: String,
    pub embed: String,
    pub watch: String,
}

/// Что окно знает о биндах.
pub struct Inputs<'a> {
    /// Черновик переназначений (`draft.binds`).
    pub value: &'a Value,
    /// Бинды cfg игры с несохранённым импортом; `None` — игра не установлена.
    pub key_binds: Option<&'a BindRead>,
    /// Язык интерфейса: «en», «ru».
    pub lang: &'a str,
    pub platform: Platform,
    pub t: Tr<'a>,
}

/// Вычисляемые свойства вкладки.
pub struct Guide<'a> {
    state: &'a Scripts,
    inputs: &'a Inputs<'a>,
    catalog: Option<&'static Catalog>,
    overrides: Overrides,
    file_binds: JsObject<Bind>,
    pub effective: JsObject<Effective>,
    keys_by_command: KeysByCommand,
}

impl<'a> Guide<'a> {
    pub fn new(state: &'a Scripts, inputs: &'a Inputs<'a>) -> Guide<'a> {
        let overrides = bind_state::overrides_of(inputs.value);
        let file_binds = inputs
            .key_binds
            .map(|binds| binds.binds.clone())
            .unwrap_or_default();
        let effective = bind_state::effective_binds(&file_binds, &overrides);
        let aliases = inputs
            .key_binds
            .map(|binds| binds.aliases.clone())
            .unwrap_or_default();
        let keys_by_command = bind_state::keys_by_part(&effective, &aliases);
        Guide {
            state,
            inputs,
            catalog: bind_presets::catalog_for(GAME_ID, inputs.platform),
            overrides,
            file_binds,
            effective,
            keys_by_command,
        }
    }

    fn t(&self, key: &str, args: &[&str]) -> String {
        (self.inputs.t)(key, args)
    }

    fn key_name(&self, key: &str) -> String {
        key_name(key, |id| self.t(id, &[]))
    }

    fn keys_of_command(&self, command: &str) -> Vec<String> {
        self.keys_by_command
            .get(&command_key(command))
            .cloned()
            .unwrap_or_default()
    }

    /// Выбранный скрипт.
    pub fn current(&self) -> &'static Script {
        hl1_guide::script_by_id(&self.state.current_id).unwrap_or(&hl1_guide::scripts()[0])
    }

    fn matches(script: &Script, chapter: &Chapter, needle: &str) -> bool {
        let mut parts: Vec<&str> = vec![
            &script.title,
            &script.section,
            script.credit.as_deref().unwrap_or(""),
            &chapter.title,
        ];
        parts.extend(script.actions.iter().map(|action| action.command.as_str()));
        let haystack = parts.join(" ").to_lowercase();
        split_whitespace(needle)
            .iter()
            .all(|word| haystack.contains(word.as_str()))
    }

    /// `keysOfScript(script)`: клавиши действий скрипта (общая команда другой заготовки — не он).
    fn keys_of_script(&self, script: &Script) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        for action in &script.actions {
            let preset = self
                .catalog
                .and_then(|catalog| catalog.preset_of(&action.command));
            if let Some(preset) = preset
                && preset.guide.as_deref() != Some(script.id.as_str())
                && preset.aliases.is_empty()
            {
                continue;
            }
            for key in self.keys_of_command(&action.command) {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        keys.sort_by(|a, b| js::cmp_utf16(a, b));
        keys
    }

    /// Главы со скриптами под поиском.
    pub fn chapters(&self) -> Vec<ChapterView> {
        let needle = js::trim(&self.state.query).to_lowercase();
        let mut result = Vec::new();
        for chapter in hl1_guide::chapters() {
            let scripts: Vec<ScriptItem> = hl1_guide::scripts()
                .iter()
                .filter(|script| script.chapter == chapter.id)
                .filter(|script| needle.is_empty() || Self::matches(script, chapter, &needle))
                .map(|script| ScriptItem {
                    id: script.id.clone(),
                    title: script.title.clone(),
                    mark: script.mark.clone(),
                    keys: self
                        .keys_of_script(script)
                        .iter()
                        .map(|key| self.key_name(key))
                        .collect(),
                })
                .collect();
            if !scripts.is_empty() {
                result.push(ChapterView {
                    id: chapter.id.clone(),
                    title: chapter.title.clone(),
                    scripts,
                });
            }
        }
        result
    }

    pub fn chapter_title(&self) -> String {
        let current = self.current();
        hl1_guide::chapters()
            .iter()
            .find(|chapter| chapter.id == current.chapter)
            .map_or_else(String::new, |chapter| chapter.title.clone())
    }

    pub fn guide_url(&self) -> String {
        format!("{}#readme", hl1_guide::source().repo)
    }

    /// Постер, плеер и страница YouTube; у скрипта без видео — `None`.
    pub fn video(&self) -> Option<VideoLinks> {
        let video = self.current().video.as_ref()?;
        Some(VideoLinks {
            poster: format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", video.id),
            embed: format!(
                "https://www.youtube-nocookie.com/embed/{}?autoplay=1&rel=0&start={}",
                video.id, video.start
            ),
            watch: if video.start != 0 {
                format!(
                    "https://www.youtube.com/watch?v={}&t={}",
                    video.id, video.start
                )
            } else {
                format!("https://www.youtube.com/watch?v={}", video.id)
            },
        })
    }

    /// Перевод описания на язык интерфейса, если он есть для этой версии гайда.
    pub fn translation(&self) -> Option<Translation> {
        hl1_guide::translation_of(self.current(), self.inputs.lang)
    }

    /// Описание (`description`): перевод или оригинал — заметки и markdown.
    pub fn description(&self) -> (Vec<String>, String) {
        match self.translation() {
            Some(translation) if !self.state.show_original => {
                (translation.notes, translation.usage)
            }
            _ => {
                let current = self.current();
                (current.notes.clone(), current.usage.clone())
            }
        }
    }

    fn label_of(&self, command: &str) -> String {
        match self.catalog.and_then(|catalog| catalog.preset_of(command)) {
            Some(preset) => {
                let local = if self.inputs.lang == "ru" {
                    &preset.label.ru
                } else {
                    &preset.label.en
                };
                if local.is_empty() {
                    preset.label.en.clone()
                } else {
                    local.clone()
                }
            }
            None => command.to_owned(),
        }
    }

    /// `keyChip(key, command)`.
    fn key_chip(&self, key: &str, command: &str) -> KeyChip {
        let bound = self.effective.get(key);
        let bound_command = bound.map_or("", |bound| bound.command.as_str());
        let whole = command_key(bound_command) == command_key(command);
        let source = bound
            .and_then(|bound| bound.source.as_deref())
            .unwrap_or("");
        let imported = self
            .inputs
            .key_binds
            .map(|binds| binds.imported.as_slice())
            .unwrap_or_default();
        let from_config = game_binds::is_import_source(source, imported);
        let name = self.key_name(key);
        let title = if !whole {
            Some(self.t("#L_SCR_KEY_PART", &[&name, bound_command]))
        } else if from_config {
            Some(self.t("#L_SCR_KEY_CONFIG", &[&name]))
        } else {
            None
        };
        KeyChip {
            key: key.to_owned(),
            name,
            whole,
            from_config,
            title: title.filter(|title| !title.is_empty()),
        }
    }

    /// Строки действий скрипта с клавишами.
    pub fn action_rows(&self) -> Vec<ActionRow> {
        self.current()
            .actions
            .iter()
            .map(|action| {
                let guide_key = action
                    .guide_key
                    .as_deref()
                    .filter(|key| !key.is_empty())
                    .and_then(game_binds::normalize_key);
                let mut keys = self.keys_of_command(&action.command);
                keys.sort_by(|a, b| js::cmp_utf16(a, b));
                ActionRow {
                    command: action.command.clone(),
                    guide_key_name: guide_key
                        .as_deref()
                        .map_or_else(String::new, |key| self.key_name(key)),
                    guide_key,
                    keys: keys
                        .iter()
                        .map(|key| self.key_chip(key, &action.command))
                        .collect(),
                }
            })
            .collect()
    }

    /// «Как в гайде»: у действия есть клавиша гайда, и она не назначена.
    pub fn guide_keys_missing(rows: &[ActionRow]) -> bool {
        rows.iter().any(|row| {
            row.guide_key
                .as_ref()
                .is_some_and(|guide| !row.keys.iter().any(|key| &key.key == guide))
        })
    }

    /// Alias скрипта, которые cfg игрока объявляет не так, как гайд.
    pub fn own_aliases(&self) -> Vec<String> {
        let Some(game) = self.inputs.key_binds.map(|binds| &binds.aliases) else {
            return Vec::new();
        };
        let mut names: Vec<String> = Vec::new();
        for (name, value) in &self.current().aliases {
            if hl1_guide::has_player_token(value) {
                continue;
            }
            let Some(own) = game.get(name) else { continue };
            if command_key(own) != command_key(value) && !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    pub fn uses_sensitivity(&self) -> bool {
        self.current()
            .aliases
            .iter()
            .any(|(_, value)| value.contains(hl1_guide::SENSITIVITY_TOKEN))
    }

    pub fn sensitivity(&self) -> String {
        let value = self
            .inputs
            .key_binds
            .and_then(|binds| binds.cvars.get("sensitivity"))
            .map(String::as_str);
        hl1_guide::clean_sensitivity(value)
    }

    /// Назначения → вопрос о занятых клавишах или сразу новый черновик.
    fn request(&self, assignments: JsObject<String>) -> Requested {
        let mut items = Vec::new();
        for (key, command) in assignments.iter() {
            if let Some(bound) = self.effective.get(key)
                && command_key(&bound.command) != command_key(command)
            {
                items.push(ConflictItem {
                    key: key.to_owned(),
                    name: self.key_name(key),
                    label: self.label_of(&bound.command),
                });
            }
        }
        if items.is_empty() {
            Requested::Apply(self.apply(&assignments))
        } else {
            Requested::Ask(Conflict { assignments, items })
        }
    }

    /// `apply(assignments)`: черновик с назначениями.
    fn apply(&self, assignments: &JsObject<String>) -> Value {
        let mut next = self.overrides.clone();
        for (key, command) in assignments.iter() {
            next.insert(
                key,
                Override {
                    command: clean_command(command),
                    base: bind_state::base_of(key, &self.file_binds, &self.overrides),
                },
            );
        }
        game_binds::overrides_to_value(&next)
    }

    /// Снять скрипт с клавиши: вернуть то, что было на ней до лаунчера.
    fn unassigned(&self, key: &str) -> Value {
        let mut next = self.overrides.clone();
        let entry = match next.get(key) {
            Some(entry) => Override {
                command: entry.base.clone(),
                base: entry.base.clone(),
            },
            None => Override {
                command: String::new(),
                base: self
                    .file_binds
                    .get(key)
                    .map_or_else(String::new, |bind| bind.command.clone()),
            },
        };
        next.insert(key, entry);
        game_binds::overrides_to_value(&next)
    }
}

enum Requested {
    Ask(Conflict),
    Apply(Value),
}

/// `needle.split(/\s+/)` для обрезанной строки.
fn split_whitespace(text: &str) -> Vec<String> {
    let mut words = vec![String::new()];
    let mut space = false;
    for c in text.chars() {
        if js::is_whitespace(c) {
            if !space {
                words.push(String::new());
            }
            space = true;
        } else {
            words.last_mut().expect("слово").push(c);
            space = false;
        }
    }
    words
}

/// Итог действия: новый черновик биндов и что сделать с захватом клавиши.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// `$emit("input", next)`.
    pub emitted: Option<Value>,
    /// Начать ожидание клавиши (подписка на нажатия).
    pub capture_started: bool,
    /// Ожидание кончилось или прервано (отписка).
    pub capture_stopped: bool,
    /// Открыть адрес (`hl1-guide-open-link`).
    pub open_link: Option<String>,
}

impl Scripts {
    fn stop_capture(&mut self, outcome: &mut Outcome) {
        self.capturing = None;
        outcome.capture_stopped = true;
    }

    /// Открыть скрипт (список или ссылка `data-script`): плеер, захват и вопрос гаснут, деталь —
    /// в начало (`watch currentId`).
    pub fn open(&mut self, id: &str) -> Outcome {
        let mut outcome = Outcome::default();
        if hl1_guide::script_by_id(id).is_none() || self.current_id == id {
            return outcome;
        }
        self.current_id = id.to_owned();
        self.playing = false;
        self.stop_capture(&mut outcome);
        self.conflict = None;
        self.scroll_reset = self.scroll_reset.wrapping_add(1);
        outcome
    }

    /// Вкладка закрыта (`watch active`, `beforeUnmount`): плеер и захват гаснут.
    pub fn deactivate(&mut self) -> Outcome {
        let mut outcome = Outcome::default();
        self.playing = false;
        self.stop_capture(&mut outcome);
        outcome
    }

    /// Ссылка в описании: на другой скрипт гайда или наружу.
    pub fn link(&mut self, kind: &str, value: &str) -> Outcome {
        match kind {
            "script" => self.open(value),
            "href" => Outcome {
                open_link: Some(value.to_owned()),
                ..Outcome::default()
            },
            _ => Outcome::default(),
        }
    }

    /// «Как в гайде»: клавиши гайда для действий, у которых их нет.
    pub fn assign_guide_keys(&mut self, inputs: &Inputs<'_>) -> Outcome {
        let guide = Guide::new(self, inputs);
        let mut assignments = JsObject::new();
        for row in guide.action_rows() {
            if let Some(key) = &row.guide_key
                && !row.keys.iter().any(|chip| &chip.key == key)
            {
                assignments.insert(key.clone(), row.command.clone());
            }
        }
        let requested = guide.request(assignments);
        self.requested(requested)
    }

    fn requested(&mut self, requested: Requested) -> Outcome {
        match requested {
            Requested::Ask(conflict) => {
                self.conflict = Some(conflict);
                Outcome::default()
            }
            Requested::Apply(next) => Outcome {
                emitted: Some(next),
                ..Outcome::default()
            },
        }
    }

    /// «Заменить» в вопросе о занятых клавишах.
    pub fn confirm_conflict(&mut self, inputs: &Inputs<'_>) -> Outcome {
        let Some(conflict) = self.conflict.take() else {
            return Outcome::default();
        };
        let next = Guide::new(self, inputs).apply(&conflict.assignments);
        Outcome {
            emitted: Some(next),
            ..Outcome::default()
        }
    }

    /// «Отмена» в вопросе.
    pub fn cancel_conflict(&mut self) {
        self.conflict = None;
    }

    /// Крестик на клавише действия.
    pub fn unassign(&mut self, inputs: &Inputs<'_>, key: &str) -> Outcome {
        Outcome {
            emitted: Some(Guide::new(self, inputs).unassigned(key)),
            ..Outcome::default()
        }
    }

    /// Нажатие кнопки «Назначить»: ожидание клавиши. Повторное нажатие той же кнопки — уже кнопка
    /// мыши (её разбирает перехват), ожидание не перезапускается.
    pub fn start_capture(&mut self, command: &str) -> Outcome {
        let mut outcome = Outcome::default();
        if self.capturing.as_deref() == Some(command) {
            return outcome;
        }
        self.stop_capture(&mut outcome);
        self.conflict = None;
        self.capturing = Some(command.to_owned());
        outcome.capture_started = true;
        outcome
    }

    /// Ожидание кончилось: `owner` — кто ждал, `key` — нажатая клавиша (`None` — Esc, нажатие вне
    /// кнопки, незнакомая кнопка мыши).
    pub fn capture_end(&mut self, inputs: &Inputs<'_>, owner: &str, key: Option<&str>) -> Outcome {
        let mut outcome = Outcome::default();
        if self.capturing.as_deref() != Some(owner) {
            return outcome;
        }
        let command = self.capturing.clone();
        self.stop_capture(&mut outcome);
        if let (Some(key), Some(command)) = (key, command) {
            let mut assignments = JsObject::new();
            assignments.insert(key, command);
            let requested = Guide::new(self, inputs).request(assignments);
            let next = self.requested(requested);
            outcome.emitted = next.emitted;
        }
        outcome
    }

    /// Нажатие постера: плеер (есть webview) или страница YouTube в браузере.
    pub fn play(&mut self, inputs: &Inputs<'_>, webview: bool) -> Outcome {
        let guide = Guide::new(self, inputs);
        let Some(video) = guide.video() else {
            return Outcome::default();
        };
        if webview {
            self.playing = true;
            Outcome::default()
        } else {
            Outcome {
                open_link: Some(video.watch),
                ..Outcome::default()
            }
        }
    }
}

/// Прямоугольник в логических пикселях окна.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        Rect {
            x,
            y,
            width: (right - x).max(0.0),
            height: (bottom - y).max(0.0),
        }
    }

    fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

/// Где сейчас окно плеера и что над ним (решение 12).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct VideoScene {
    /// Видео идёт (`playing`), вкладка «Скрипты» видна.
    pub playing: bool,
    /// Рамка видео (16:9) в окне.
    pub frame: Rect,
    /// Видимая часть прокрутки детали скрипта.
    pub viewport: Rect,
    /// Над окном настроек что-то открыто: меню, подсказка, тост, окно выше (клавиатура,
    /// подтверждение), заставка смены темы.
    pub covered: bool,
    /// Идёт переход: окно проявляется или уходит, свайп страниц, прокрутка с анимацией.
    pub moving: bool,
}

/// Как показать webview плеера.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VideoPlacement {
    /// Плеера нет (не играет).
    Closed,
    /// Webview есть, но скрыт (что-то поверх, переход, рамка вне видимой части).
    Hidden,
    /// Показан: `frame` — место видео (содержимое не масштабируется), `clip` — видимая часть
    /// (окно webview обрезано прямоугольником, как iframe краем прокрутки).
    Shown { frame: Rect, clip: Rect },
}

/// Правила webview плеера: нативное окно не обрезается краем прокрутки и не участвует в
/// анимациях, поэтому оно скрыто, пока поверх что-то открыто или идёт переход, а при прокрутке
/// двигается вместе с рамкой и обрезается видимой частью; рамка целиком вне неё — скрыто.
pub fn video_placement(scene: &VideoScene) -> VideoPlacement {
    if !scene.playing {
        return VideoPlacement::Closed;
    }
    if scene.covered || scene.moving || scene.frame.is_empty() {
        return VideoPlacement::Hidden;
    }
    let clip = scene.frame.intersect(&scene.viewport);
    if clip.is_empty() {
        return VideoPlacement::Hidden;
    }
    VideoPlacement::Shown {
        frame: scene.frame,
        clip,
    }
}

/// Webview плеера (WebView2 / WebKitGTK; `hlsr-app/src/webview.rs`). Только адреса встраивания
/// YouTube; остальное — в системный браузер; моста в лаунчер нет.
pub trait VideoHost {
    /// Встроенный плеер доступен (есть WebView2 / WebKitGTK).
    fn available(&self) -> bool;
    /// Открыть адрес встраивания (новый webview; прежний закрывается).
    fn open(&self, url: &str);
    /// Положение и видимость.
    fn place(&self, placement: VideoPlacement);
    /// Закрыть (смена скрипта, вкладки, окна).
    fn close(&self);
}

/// Нет встроенного плеера: постер и «Открыть на YouTube».
pub struct NoVideo;

impl VideoHost for NoVideo {
    fn available(&self) -> bool {
        false
    }
    fn open(&self, _url: &str) {}
    fn place(&self, _placement: VideoPlacement) {}
    fn close(&self) {}
}

/// Строка действия для показа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    pub row: ActionRow,
    /// Кнопка ждёт клавишу (`.is-listening`).
    pub listening: bool,
    /// Подпись кнопки: «Нажмите клавишу», «Добавить» или «Назначить».
    pub capture_label: String,
}

/// Всё, что показывает вкладка.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptsView {
    pub query: String,
    pub chapters: Vec<ChapterView>,
    pub current_id: String,
    pub title: String,
    /// «Глава · раздел · автор».
    pub meta: String,
    pub save: Option<String>,
    pub guide_url: String,
    pub video: Option<VideoLinks>,
    pub playing: bool,
    pub installed: bool,
    pub guide_keys_missing: bool,
    pub rows: Vec<RowView>,
    /// Текст вопроса о занятых клавишах; пусто — вопроса нет.
    pub conflict: String,
    pub hints: Vec<String>,
    /// Есть перевод описания (кнопка «перевод/оригинал»).
    pub translation: bool,
    pub show_original: bool,
    /// Подсказка кнопки перевода.
    pub translate_label: String,
    pub notes: Vec<String>,
    pub usage: String,
    pub source_note: String,
    pub scroll_reset: u32,
}

impl Scripts {
    /// Состояние для показа.
    pub fn view(&self, inputs: &Inputs<'_>) -> ScriptsView {
        let guide = Guide::new(self, inputs);
        let t = |key: &str, args: &[&str]| (inputs.t)(key, args);
        let current = guide.current();
        let mut meta = guide.chapter_title();
        if current.section != current.title {
            meta.push_str(" · ");
            meta.push_str(&current.section);
        }
        if let Some(credit) = current.credit.as_deref().filter(|c| !c.is_empty()) {
            meta.push_str(" · ");
            meta.push_str(&t("#L_SCR_BY", &[credit]));
        }
        let rows = guide.action_rows();
        let guide_keys_missing = Guide::guide_keys_missing(&rows);
        let scripted = rows.iter().any(|row| !row.keys.is_empty());
        let mut hints = Vec::new();
        if current.needs_auto_alias {
            hints.push(t("#L_SCR_AUTO_ALIAS", &[]));
        }
        if guide.uses_sensitivity() {
            hints.push(t("#L_SCR_SENSITIVITY", &[&guide.sensitivity()]));
        }
        if scripted {
            hints.push(t("#L_SCR_SCRIPTED_ONLY", &[]));
        }
        let own = guide.own_aliases();
        if !own.is_empty() {
            hints.push(t("#L_SCR_OWN_ALIAS", &[&own.join(", ")]));
        }
        let conflict = match &self.conflict {
            Some(conflict) if conflict.items.len() == 1 => {
                let item = &conflict.items[0];
                t("#L_SCR_BUSY_ONE", &[&item.name, &item.label])
            }
            Some(conflict) => {
                let list: Vec<String> = conflict
                    .items
                    .iter()
                    .map(|item| format!("{} ({})", item.name, item.label))
                    .collect();
                t("#L_SCR_BUSY_MANY", &[&list.join(", ")])
            }
            None => String::new(),
        };
        let translation = guide.translation().is_some();
        let (notes, usage) = guide.description();
        ScriptsView {
            query: self.query.clone(),
            chapters: guide.chapters(),
            current_id: current.id.clone(),
            title: current.title.clone(),
            meta,
            save: current.save.clone().filter(|s| !s.is_empty()),
            guide_url: guide.guide_url(),
            video: guide.video(),
            playing: self.playing,
            installed: inputs.key_binds.is_some(),
            guide_keys_missing,
            rows: rows
                .into_iter()
                .map(|row| {
                    let listening = self.capturing.as_deref() == Some(row.command.as_str());
                    let capture_label = if listening {
                        t("#L_SCR_PRESS_KEY", &[])
                    } else if row.keys.is_empty() {
                        t("#L_SCR_ASSIGN", &[])
                    } else {
                        t("#L_SCR_ADD_KEY", &[])
                    };
                    RowView {
                        row,
                        listening,
                        capture_label,
                    }
                })
                .collect(),
            conflict,
            hints,
            translation,
            show_original: self.show_original,
            translate_label: t(
                if self.show_original {
                    "#L_SCR_SHOW_TRANSLATION"
                } else {
                    "#L_SCR_SHOW_ORIGINAL"
                },
                &[],
            ),
            notes,
            usage,
            source_note: t(
                if translation && !self.show_original {
                    "#L_SCR_SOURCE_NOTE_TRANSLATED"
                } else {
                    "#L_SCR_SOURCE_NOTE"
                },
                &[],
            ),
            scroll_reset: self.scroll_reset,
        }
    }
}
