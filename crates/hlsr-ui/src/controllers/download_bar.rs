//! Нижняя панель игры (`DownloadBar.vue` 0.4.9): статус операции (метка, полоса, байты,
//! скорость, оставшееся время, «готово» ещё секунду), отмена и главная кнопка — по статусу
//! доставки `DeliveryStatus` и его `mainButton`; эпизоды Ghosting (`episodeButton`, отмена паузы
//! эпизода, размер нескачанных эпизодов у сборок, «Удалить эпизод» в меню). Вид —
//! `hlsr-gpui/src/games/download_bar.rs`.
//!
//! Состояния операций и успехов — `hlsr_delivery::progress_states` (`shared/progress-states.js`).

use super::boot::{format_number, format_size};
use super::game_demo::GameDemo;
use super::jsv;
use crate::tr::tr;
use hlsr_core::js;
use hlsr_core::language::Language;
use hlsr_delivery::episode_button::episode_button;
use hlsr_delivery::progress_states as progress;
use serde_json::Value;
use std::time::Duration;

/// Сколько полоса стоит на 100 % после успеха (`FINISH_HOLD_MS`).
pub const FINISH_HOLD: Duration = Duration::from_millis(1000);

/// `DELIVERY_LABELS`: состояние доставки → ключ метки.
const DELIVERY_LABELS: [(&str, &str); 18] = [
    ("not-installed", "#L_D_NOT_INSTALLED"),
    ("preparing", "#L_D_PREPARING"),
    ("downloading", "#L_D_DOWNLOADING"),
    ("paused", "#L_D_PAUSED"),
    ("verifying", "#L_D_VERIFYING"),
    ("extracting", "#L_D_EXTRACTING"),
    ("finalizing", "#L_D_FINALIZING"),
    ("applying-update", "#L_D_APPLYING"),
    ("checking-files", "#L_D_CHECKING"),
    ("files-valid", "#L_D_FILES_VALID"),
    ("files-damaged", "#L_D_FILES_DAMAGED"),
    ("repairing", "#L_D_REPAIRING"),
    ("repair-complete", "#L_D_REPAIR_DONE"),
    ("exporting", "#L_D_EXPORTING"),
    ("importing", "#L_D_IMPORTING"),
    ("ready", "#L_D_READY"),
    ("cancelled", "#L_D_CANCELLED"),
    ("error", "#L_D_ERROR"),
];

/// Тон главной кнопки (`TONES`): от него цвета заливки, наведения, нажатия, текста и свечения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ready,
    Progress,
    Repair,
    Danger,
    Muted,
    Neutral,
}

impl Tone {
    /// Имя тона в разметке (`tone-ready`, …).
    pub fn name(self) -> &'static str {
        match self {
            Tone::Ready => "ready",
            Tone::Progress => "progress",
            Tone::Repair => "repair",
            Tone::Danger => "danger",
            Tone::Muted => "muted",
            Tone::Neutral => "neutral",
        }
    }
}

/// `BUTTON[state]`: значок и ключ надписи; `TONES[state]`: тон.
struct ButtonStyle {
    state: &'static str,
    icon: &'static str,
    key: &'static str,
    tone: Tone,
}

const SPIN: &str = "fas fa-circle-notch fa-spin";

const BUTTONS: [ButtonStyle; 22] = [
    ButtonStyle {
        state: "ready",
        icon: "fas fa-play",
        key: "#L_B_PLAY",
        tone: Tone::Ready,
    },
    ButtonStyle {
        state: "not-installed",
        icon: "fas fa-download",
        key: "#L_B_INSTALL",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "episode-not-installed",
        icon: "fas fa-download",
        key: "#L_BT_EPISODE_DOWNLOAD",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "episode-paused",
        icon: "fas fa-play",
        key: "#L_B_RESUME",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "unavailable",
        icon: "fas fa-ban",
        key: "#L_B_UNAVAILABLE",
        tone: Tone::Muted,
    },
    ButtonStyle {
        state: "checking-updates",
        icon: SPIN,
        key: "#L_B_CHECKING_UPDATES",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "update-available",
        icon: "fas fa-sync-alt",
        key: "#L_B_UPDATE",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "downloading-update",
        icon: "fas fa-pause",
        key: "#L_B_PAUSE",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "update-paused",
        icon: "fas fa-play",
        key: "#L_B_RESUME",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "verifying-update",
        icon: SPIN,
        key: "#L_B_VERIFYING_UPDATE",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "applying-update",
        icon: SPIN,
        key: "#L_B_APPLYING_UPDATE",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "update-error",
        icon: "fas fa-redo",
        key: "#L_B_UPDATE_ERROR",
        tone: Tone::Danger,
    },
    ButtonStyle {
        state: "starting-game",
        icon: SPIN,
        key: "#L_B_STARTING_GAME",
        tone: Tone::Neutral,
    },
    ButtonStyle {
        state: "game-running",
        icon: "fas fa-times",
        key: "#L_B_GAME_RUNNING",
        tone: Tone::Neutral,
    },
    ButtonStyle {
        state: "stopping-game",
        icon: SPIN,
        key: "#L_B_STOPPING_GAME",
        tone: Tone::Neutral,
    },
    ButtonStyle {
        state: "downloading",
        icon: "fas fa-pause",
        key: "#L_B_PAUSE",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "download-paused",
        icon: "fas fa-play",
        key: "#L_B_RESUME",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "installing",
        icon: SPIN,
        key: "#L_B_INSTALLING",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "busy",
        icon: SPIN,
        key: "#L_B_BUSY",
        tone: Tone::Progress,
    },
    ButtonStyle {
        state: "error",
        icon: "fas fa-redo",
        key: "#L_B_RETRY",
        tone: Tone::Danger,
    },
    ButtonStyle {
        state: "repair-required",
        icon: "fas fa-wrench",
        key: "#L_B_REPAIR",
        tone: Tone::Repair,
    },
    ButtonStyle {
        state: "update-launcher-required",
        icon: "fas fa-arrow-up",
        key: "#L_B_UPDATE_LAUNCHER",
        tone: Tone::Muted,
    },
];

/// Действия, которые кнопка передаёт оболочке (`ACTIONS`); `none` и прочее кнопка не жмёт.
pub const ACTIONS: [&str; 9] = [
    "play", "stop", "install", "update", "pause", "resume", "retry", "repair", "explain",
];

/// Состояния доставки, в которых показывается кнопка отмены.
const CANCELLABLE: [&str; 7] = [
    "preparing",
    "downloading",
    "paused",
    "verifying",
    "extracting",
    "exporting",
    "importing",
];

/// Главная кнопка: что нарисовать и что она делает.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonView {
    /// `mainButton.state`.
    pub state: String,
    /// `mainButton.action`.
    pub action: String,
    /// `enabled !== false`.
    pub enabled: bool,
    pub icon: &'static str,
    pub label: String,
    pub tone: Tone,
}

/// Событие эпизодной кнопки (`install-episode`): «Скачать эпизод» и «Продолжить» эпизода — свой
/// канал, а не установка Ghosting.
pub const INSTALL_EPISODE: &str = "install-episode";

impl ButtonView {
    /// Действие по нажатию (`onAction`): пустое, если кнопка выключена или действия нет в
    /// `ACTIONS`; у кнопки эпизода — [`INSTALL_EPISODE`].
    pub fn pressed(&self) -> Option<&str> {
        if !self.enabled || !ACTIONS.contains(&self.action.as_str()) {
            return None;
        }
        if matches!(
            self.state.as_str(),
            "episode-not-installed" | "episode-paused"
        ) {
            return Some(INSTALL_EPISODE);
        }
        Some(self.action.as_str())
    }
}

/// `button` и `buttonView`: нет статуса или `mainButton` — «Играть»; с выбранным эпизодом
/// `mainButton` проходит через `episodeButton`.
pub fn button_view(
    lang: Language,
    delivery: Option<&Value>,
    episode_id: Option<&str>,
) -> ButtonView {
    let main = jsv::field(delivery, "mainButton")
        .filter(|main| js::truthy(main))
        .map(|main| episode_button(main, delivery, episode_id));
    let (state, action, enabled) = match &main {
        None => ("ready".to_owned(), "play".to_owned(), true),
        Some(main) => (
            jsv::str_of(jsv::prop(main, "state"))
                .unwrap_or_default()
                .to_owned(),
            jsv::str_of(jsv::prop(main, "action"))
                .unwrap_or_default()
                .to_owned(),
            jsv::prop(main, "enabled") != Some(&Value::Bool(false)),
        ),
    };
    let style = BUTTONS
        .iter()
        .find(|style| style.state == state)
        .unwrap_or(&BUTTONS[0]);
    ButtonView {
        label: tr(lang, style.key, &[]),
        icon: style.icon,
        tone: style.tone,
        state,
        action,
        enabled,
    }
}

/// Что хранит панель между обновлениями статуса (`watched`, `finishHold`…): после успеха
/// операции полоса стоит на 100 % секунду.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FinishHold {
    active: bool,
    state: Option<String>,
    operation: Option<String>,
    total: f64,
    watched: Watched,
    seen: Seen,
}

/// `watched`: игра, состояние и операция прошлого статуса.
#[derive(Debug, Clone, Default, PartialEq)]
struct Watched {
    game_id: Option<String>,
    state: Option<String>,
    operation: Option<String>,
}

/// Значения, за которыми следят наблюдатели Vue (`deliveryGameId`, `deliveryState`).
#[derive(Debug, Clone, Default, PartialEq)]
struct Seen {
    game_id: Option<String>,
    state: Option<String>,
}

/// Что делать с таймером «готово» после обновления статуса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldTimer {
    /// Таймер не трогать.
    Keep,
    /// Запустить на [`FINISH_HOLD`] (прежний сбросить): удержание началось.
    Start,
    /// Сбросить: удержание снято досрочно (`clearFinishTimer`).
    Clear,
}

fn text_of(value: Option<&Value>) -> Option<String> {
    jsv::str_of(value).map(str::to_owned)
}

fn delivery_ids(delivery: Option<&Value>) -> (Option<String>, Option<String>, Option<String>) {
    (
        text_of(jsv::field(delivery, "gameId")),
        text_of(jsv::field(delivery, "state")),
        text_of(jsv::field(delivery, "operation")),
    )
}

impl FinishHold {
    /// `data()` панели при создании: смотрит за игрой и состоянием уже пришедшего статуса.
    pub fn new(delivery: Option<&Value>) -> FinishHold {
        let (game_id, state, _) = delivery_ids(delivery);
        FinishHold {
            watched: Watched {
                game_id: game_id.clone(),
                state: state.clone(),
                operation: None,
            },
            seen: Seen { game_id, state },
            ..FinishHold::default()
        }
    }

    /// Идёт ли удержание «готово» (`finishHold`).
    pub fn active(&self) -> bool {
        self.active
    }

    /// Таймер сработал: `finishHold = false` и забыть итог.
    pub fn expire(&mut self) {
        self.clear();
    }

    /// `clearFinishTimer` + сброс итога (панель уходит: `beforeUnmount`).
    pub fn reset(&mut self) {
        self.clear();
    }

    fn clear(&mut self) {
        self.active = false;
        self.state = None;
        self.operation = None;
        self.total = 0.0;
    }

    /// Пришёл новый статус (или его не стало). Повторяет наблюдатели `deliveryGameId` и
    /// `deliveryState` в этом порядке, как Vue после каждого обновления.
    pub fn observe(&mut self, delivery: Option<&Value>) -> HoldTimer {
        let (game_id, state, operation) = delivery_ids(delivery);
        let mut timer = HoldTimer::Keep;

        if game_id != self.seen.game_id {
            self.seen.game_id = game_id.clone();
            self.clear();
            timer = HoldTimer::Clear;
            self.watched = Watched {
                game_id: game_id.clone(),
                state: state.clone(),
                operation: operation.clone(),
            };
        }

        if state != self.seen.state {
            self.seen.state = state.clone();
            let previous = std::mem::replace(
                &mut self.watched,
                Watched {
                    game_id: game_id.clone(),
                    state: state.clone(),
                    operation: operation.clone(),
                },
            );
            let held = progress::should_hold_finish(
                previous.game_id.as_deref().zip(previous.state.as_deref()),
                game_id.as_deref().zip(state.as_deref()),
            );
            if held {
                self.clear();
                self.active = true;
                self.state = state;
                // К концу операции main уже снял её с карточки — берём из прошлого состояния.
                self.operation = operation.or(previous.operation);
                self.total = progress_total_bytes(delivery, self);
                timer = HoldTimer::Start;
            } else if !state.as_deref().is_some_and(progress::is_success) {
                self.clear();
                timer = HoldTimer::Clear;
            }
        }
        timer
    }
}

/// `progressTotalBytes`.
fn progress_total_bytes(delivery: Option<&Value>, hold: &FinishHold) -> f64 {
    if hold.active && hold.total != 0.0 {
        return hold.total;
    }
    let chain = jsv::field(delivery, "chain").filter(|chain| js::truthy(chain));
    if let Some(total) = chain.and_then(|chain| jsv::prop(chain, "totalBytes"))
        && js::truthy(total)
    {
        return jsv::or_zero(Some(total));
    }
    jsv::or_zero(jsv::field(delivery, "totalBytes"))
}

/// Всё, что нужно панели, чтобы себя нарисовать.
pub struct Input<'a> {
    pub lang: Language,
    pub demo: &'a GameDemo,
    /// `buildValue`: выбранная сборка (у Ghosting — и эпизод).
    pub build: &'a str,
    /// `DeliveryStatus` выбранной игры; `None` — ещё не пришёл.
    pub delivery: Option<&'a Value>,
    pub hold: &'a FinishHold,
}

/// Что показывает панель.
#[derive(Debug, Clone, PartialEq)]
pub struct BarView {
    /// Блок статуса есть (`showStatus`).
    pub show_status: bool,
    /// Полоса, % (`Math.round`, без ограничения 0…100).
    pub percent: f64,
    pub status_text: String,
    /// Заливка полупрозрачная: операция закончилась (`done`).
    pub fill_done: bool,
    pub done_text: String,
    pub total_text: String,
    pub right_meta: String,
    pub can_cancel: bool,
    /// `pausedEpisode`: эпизод на паузе — отмена убирает архив только его (`cancel(episode)`).
    pub paused_episode: Option<&'static str>,
    /// `deliveryGameId`: игра, меню которой спрашивает `game-menu-state`.
    pub game_id: Option<String>,
    /// `buildBadges`: подпись справа у сборки — общий размер нескачанных эпизодов.
    pub build_badges: Vec<(&'static str, String)>,
    /// `menuEpisode`: выбран эпизод — `Some(installed)`, меню удаляет эпизод, а не игру.
    pub menu_episode: Option<bool>,
    pub button: ButtonView,
}

/// Панель по статусу доставки.
pub fn view(input: &Input<'_>) -> BarView {
    let episode = input.demo.episode_of(input.build);
    let button = button_view(input.lang, input.delivery, episode);
    let paused_episode = episode.filter(|_| button.state == "episode-paused");
    BarView {
        show_status: show_status(input),
        percent: percent(input),
        status_text: status_text(input),
        fill_done: !is_downloading(input),
        done_text: done_text(input),
        total_text: total_text(input),
        right_meta: right_meta(input),
        can_cancel: paused_episode.is_some() || can_cancel(input.delivery),
        paused_episode,
        game_id: text_of(jsv::field(input.delivery, "gameId")),
        build_badges: build_badges(input),
        menu_episode: episode.map(|id| episode_flag(input.delivery, id, "installed")),
        button,
    }
}

/// `(delivery.episodes || {})[id][flag]` как истина JS.
fn episode_flag(delivery: Option<&Value>, id: &str, flag: &str) -> bool {
    let episode = jsv::field(delivery, "episodes").and_then(|episodes| jsv::prop(episodes, id));
    jsv::truthy(episode.and_then(|episode| jsv::prop(episode, flag)))
}

/// `buildBadges`: «Скачать» качает все недостающие эпизоды сразу, поэтому у каждого — их общий
/// размер; без размера хоть одного из них (манифеста нет) — ничего.
fn build_badges(input: &Input<'_>) -> Vec<(&'static str, String)> {
    let episodes = jsv::field(input.delivery, "episodes");
    let entry = |id: &str| episodes.and_then(|episodes| jsv::prop(episodes, id));
    let mut missing: Vec<&str> = Vec::new();
    for (_, id) in input.demo.episodes {
        let state = entry(id).filter(|state| js::truthy(state));
        if state.is_some_and(|state| !jsv::truthy(jsv::prop(state, "installed")))
            && !missing.contains(id)
        {
            missing.push(id);
        }
    }
    let sizes: Vec<f64> = missing
        .iter()
        .map(|id| jsv::num(entry(id).and_then(|state| jsv::prop(state, "size"))).unwrap_or(0.0))
        .collect();
    if missing.is_empty() || sizes.iter().any(|size| size.is_nan() || *size <= 0.0) {
        return Vec::new();
    }
    let t = |key: &str, args: &[&str]| tr(input.lang, key, args);
    let size = format_size(sizes.iter().sum(), &t);
    input
        .demo
        .episodes
        .iter()
        .filter(|(_, id)| missing.contains(id))
        .map(|(build, _)| (*build, size.clone()))
        .collect()
}

/// `delivery && delivery.state`.
fn state_of(delivery: Option<&Value>) -> Option<&str> {
    jsv::str_of(jsv::field(delivery, "state"))
}

/// `game.demo.download || {done: 0, total: 0}`.
fn demo_download(input: &Input<'_>) -> (f64, f64) {
    input
        .demo
        .download
        .map_or((0.0, 0.0), |download| (download.done, download.total))
}

fn can_cancel(delivery: Option<&Value>) -> bool {
    delivery.is_some_and(js::truthy)
        && !jsv::is_str(jsv::field(delivery, "source"), "steam-depot")
        && state_of(delivery).is_some_and(|state| CANCELLABLE.contains(&state))
}

fn show_status(input: &Input<'_>) -> bool {
    if !input.delivery.is_some_and(js::truthy) {
        return demo_download(input).1 > 0.0;
    }
    match state_of(input.delivery) {
        Some(state) if progress::is_progress(state) => true,
        Some("files-damaged") => true,
        _ => input.hold.active,
    }
}

fn is_downloading(input: &Input<'_>) -> bool {
    if input.delivery.is_some_and(js::truthy) {
        return state_of(input.delivery).is_some_and(progress::is_busy);
    }
    let (done, total) = demo_download(input);
    done < total
}

fn percent(input: &Input<'_>) -> f64 {
    if input.hold.active {
        return 100.0;
    }
    if input.delivery.is_some_and(js::truthy) {
        return jsv::round(jsv::or_zero(jsv::field(input.delivery, "percent")));
    }
    let (done, total) = demo_download(input);
    if total > 0.0 {
        jsv::round(done / total * 100.0)
    } else {
        0.0
    }
}

/// `errorText(code)`: текст кода ошибки доставки, нет такого — общий.
fn error_text(lang: Language, code: Option<&Value>) -> String {
    let key = format!("#L_D_ERR_{}", js::to_template_string(code));
    let text = tr(lang, &key, &[]);
    if text == key {
        tr(lang, "#L_D_ERROR", &[])
    } else {
        text
    }
}

fn is_updating(delivery: Option<&Value>) -> bool {
    jsv::is_str(jsv::field(delivery, "operation"), "update")
}

fn chain(delivery: Option<&Value>) -> Option<&Value> {
    jsv::field(delivery, "chain").filter(|chain| js::truthy(chain))
}

fn update_status_text(lang: Language, delivery: Option<&Value>) -> String {
    let chain = chain(delivery);
    let state = state_of(delivery);
    if let Some(chain) = chain
        && jsv::num(jsv::prop(chain, "steps")).is_some_and(|steps| steps > 1.0)
        && state == Some("downloading")
    {
        return tr(
            lang,
            "#L_U_G_CHAIN",
            &[
                &js::to_template_string(jsv::prop(chain, "step")),
                &js::to_template_string(jsv::prop(chain, "steps")),
            ],
        );
    }
    match state {
        Some("downloading" | "paused") => {
            let to = chain.map_or_else(String::new, |chain| {
                js::to_template_string(jsv::prop(chain, "toVersion"))
            });
            tr(lang, "#L_U_G_DOWNLOADING", &[&to])
        }
        Some("preparing" | "verifying") => tr(lang, "#L_U_G_VERIFYING", &[]),
        _ => tr(lang, "#L_U_G_APPLYING", &[]),
    }
}

fn steam_download(delivery: Option<&Value>) -> bool {
    delivery.is_some_and(js::truthy)
        && jsv::is_str(jsv::field(delivery, "source"), "steam-depot")
        && matches!(state_of(delivery), Some("preparing" | "downloading"))
}

fn counts_files(delivery: Option<&Value>) -> bool {
    delivery.is_some_and(js::truthy)
        && matches!(
            state_of(delivery),
            Some("checking-files" | "repairing" | "files-damaged")
        )
}

fn status_text(input: &Input<'_>) -> String {
    let lang = input.lang;
    let delivery = input.delivery.filter(|delivery| js::truthy(delivery));
    let Some(delivery) = delivery else {
        let (done, total) = demo_download(input);
        return tr(
            lang,
            if done < total {
                "#L_DOWNLOADING_FILES"
            } else {
                "#L_BUILD_INSTALLED"
            },
            &[],
        );
    };
    let delivery = Some(delivery);
    let state = state_of(delivery);

    if input.hold.active {
        let key = match (input.hold.state.as_deref(), input.hold.operation.as_deref()) {
            (Some("files-valid"), _) => "#L_D_FILES_VALID",
            (Some("repair-complete"), _) => "#L_D_REPAIR_DONE",
            (_, Some("update")) => "#L_U_G_DONE",
            (_, Some("export")) => "#L_D_EXPORT_DONE",
            (_, Some("import")) => "#L_D_IMPORT_DONE",
            _ => "#L_D_COMPLETED",
        };
        return tr(lang, key, &[]);
    }
    if steam_download(delivery) {
        return tr(lang, "#L_D_DOWNLOADING_STEAM", &[]);
    }
    if is_updating(delivery) {
        return update_status_text(lang, delivery);
    }
    if jsv::is_str(jsv::field(delivery, "operation"), "repair-download")
        && state == Some("preparing")
    {
        return tr(lang, "#L_DOWNLOADING_FILES", &[]);
    }
    if state == Some("files-damaged")
        && let Some(integrity) = jsv::field(delivery, "integrity").filter(|v| js::truthy(v))
    {
        return tr(
            lang,
            "#L_D_FILES_DAMAGED_N",
            &[&js::to_template_string(jsv::prop(integrity, "damaged"))],
        );
    }
    if state == Some("not-installed") && !jsv::truthy(jsv::field(delivery, "installable")) {
        return tr(lang, "#L_D_UNAVAILABLE", &[]);
    }
    if state == Some("error")
        && let Some(error) = jsv::field(delivery, "error").filter(|v| js::truthy(v))
    {
        return error_text(lang, jsv::prop(error, "code"));
    }
    let key = DELIVERY_LABELS
        .iter()
        .find(|(known, _)| Some(*known) == state)
        .map_or("#L_D_NOT_INSTALLED", |(_, key)| *key);
    tr(lang, key, &[])
}

/// `formatNumber(value, digits, $t)`: десятичный знак языка.
fn number(lang: Language, value: f64, digits: usize) -> String {
    format_number(value, digits, &|key: &str, args: &[&str]| {
        tr(lang, key, args)
    })
}

/// `sizeText(mbytes)`: «513 MB» или «1,23 ГБ» / «1.23 GB».
fn size_text(lang: Language, mbytes: f64) -> String {
    if mbytes >= 1000.0 {
        return format!(
            "{} {}",
            number(lang, mbytes / 1000.0, 2),
            tr(lang, "#L_GB", &[])
        );
    }
    format!("{} {}", jsv::number_text(mbytes), tr(lang, "#L_MB", &[]))
}

/// `bytesText(bytes)`: всего меньше мегабайта (докачка пары cfg) — в КБ, а не «0 МБ / 0 МБ».
fn bytes_text(lang: Language, bytes: f64, total: f64) -> String {
    if total > 0.0 && total < 1024.0 * 1024.0 {
        return format!(
            "{} {}",
            jsv::number_text((bytes / 1024.0).ceil()),
            tr(lang, "#L_KB", &[])
        );
    }
    size_text(lang, jsv::round(bytes / (1024.0 * 1024.0)))
}

fn files_unit(input: &Input<'_>) -> String {
    tr(input.lang, "#L_D_FILES_UNIT", &[])
}

fn done_text(input: &Input<'_>) -> String {
    let lang = input.lang;
    let delivery = input.delivery.filter(|delivery| js::truthy(delivery));
    let Some(delivery) = delivery else {
        return size_text(lang, demo_download(input).0);
    };
    let delivery = Some(delivery);
    if steam_download(delivery) {
        return jsv::number_text(jsv::or_zero(jsv::field(delivery, "filesDone")));
    }
    if counts_files(delivery) {
        return jsv::number_text(jsv::or_zero(jsv::field(delivery, "checked")));
    }
    let total = progress_total_bytes(delivery, input.hold);
    if input.hold.active {
        return bytes_text(lang, total, total);
    }
    if let Some(chain) = chain(delivery) {
        let done = jsv::or_zero(jsv::prop(chain, "doneBytes"))
            + jsv::or_zero(jsv::field(delivery, "bytesDownloaded"));
        return bytes_text(lang, done, total);
    }
    bytes_text(
        lang,
        jsv::or_zero(jsv::field(delivery, "bytesDownloaded")),
        total,
    )
}

fn total_text(input: &Input<'_>) -> String {
    let lang = input.lang;
    let delivery = input.delivery.filter(|delivery| js::truthy(delivery));
    let Some(delivery) = delivery else {
        return size_text(lang, demo_download(input).1);
    };
    let delivery = Some(delivery);
    if steam_download(delivery) {
        return format!(
            "{} {}",
            jsv::number_text(jsv::or_zero(jsv::field(delivery, "filesTotal"))),
            files_unit(input)
        );
    }
    if counts_files(delivery) {
        return format!(
            "{} {}",
            jsv::number_text(jsv::or_zero(jsv::field(delivery, "totalChecked"))),
            files_unit(input)
        );
    }
    let total = progress_total_bytes(delivery, input.hold);
    bytes_text(lang, total, total)
}

fn speed_text(lang: Language, delivery: Option<&Value>) -> String {
    let mb = jsv::or_zero(jsv::field(delivery, "speedBps")) / (1024.0 * 1024.0);
    tr(lang, "#L_D_SPEED", &[&number(lang, mb, 1)])
}

fn eta_text(lang: Language, delivery: Option<&Value>) -> String {
    let seconds = jsv::field(delivery, "etaSeconds");
    if !jsv::truthy(seconds) {
        return String::new();
    }
    let seconds = jsv::or_zero(seconds);
    if seconds < 60.0 {
        let unit = tr(lang, "#L_D_SEC", &[]);
        return tr(
            lang,
            "#L_D_ETA",
            &[&format!("{} {unit}", jsv::number_text(seconds))],
        );
    }
    let unit = tr(lang, "#L_D_MIN", &[]);
    let minutes = jsv::round(seconds / 60.0);
    tr(
        lang,
        "#L_D_ETA",
        &[&format!("{} {unit}", jsv::number_text(minutes))],
    )
}

fn right_meta(input: &Input<'_>) -> String {
    if input.hold.active {
        return "100%".to_owned();
    }
    let percent = jsv::number_text(percent(input));
    let delivery = input.delivery.filter(|delivery| js::truthy(delivery));
    if state_of(delivery) == Some("downloading") && jsv::truthy(jsv::field(delivery, "speedBps")) {
        return format!(
            "{percent}% · {} · {}",
            speed_text(input.lang, delivery),
            eta_text(input.lang, delivery)
        );
    }
    format!("{percent}%")
}

#[cfg(test)]
mod tests;
