//! Окно установщика (`src/installer/installer.js` 0.4.9): что показать по состоянию окна (`render`)
//! и что делают кнопки. Правила занятости, отмена и закрытие во время работы — у
//! [`InstallerWindow`] (`launcher-update/installer-window.js`, `hlsr-update`); здесь — страница:
//! шаги перед формой новой установки («Привет», в Windows ещё SmartScreen/Defender), заголовок и
//! описание фазы (ошибка ОС — по коду, антивирус — свой экран с Discord), чипы «Шаг N из M» и
//! версии, значок фазы, видимость строк, надписи и значки кнопок, прогресс, предупреждение,
//! подтверждение удаления (оно живёт только в окне: «Отмена» возвращает прежнее состояние;
//! «Также удалить игры» включено) и флажки. Окно читает [`InstallerView`]; долгие методы окна идут
//! не в потоке интерфейса ([`Spawn`]).
//!
//! Поля 0.4.9 `platform`, `errorCode`, `errorFile` и `uninstallPlan.libraryBlocked`/`libraryRoots`
//! страница читает из `to_value()` по именам 0.4.9 ([`Extra`]).

use crate::tr;
use hlsr_core::js;
use hlsr_core::language::Language;
use hlsr_update::installer_window::{InstallerState, InstallerWindow, Phase, UninstallPlan};
use serde_json::Value;
use std::sync::{Arc, Mutex, MutexGuard};

/// Чипы под заголовком (`renderMeta`) после чипа шага.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Meta {
    #[default]
    None,
    /// Один чип: «Версия X» или «Установлена X».
    Chip(String),
    /// Обновление: «было → станет».
    Update { from: String, to: String },
}

/// Тон значка фазы (`data-tone`): `ok` в 0.4.9 своего правила не имеет и красится акцентом.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Accent,
    Ok,
    Danger,
    Muted,
    Caution,
}

/// Шаг перед формой новой установки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intro {
    Welcome,
    Defender,
}

/// Значки окна — контуры `ICONS` installer.js 0.4.9 (имена те же).
pub type Icon = &'static str;

/// Кнопка «Отмена»/«Назад» (`#dismiss`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dismiss {
    pub label: String,
    pub icon: Icon,
}

/// Сообщение под прогрессом (`#error`): ошибка (`danger`) или предупреждение.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub danger: bool,
}

/// Плашка про SmartScreen (`#defender`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callout {
    pub lead: String,
    pub title: String,
    pub steps: String,
    pub why: String,
}

/// Что видно в окне — итог `render(state)` 0.4.9.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallerView {
    pub language: Language,
    pub phase: Phase,
    /// Шаг перед формой (`document.body.dataset.phase = step || phase`).
    pub intro: Option<Intro>,
    pub title: String,
    pub description: String,
    /// «Шаг N из M».
    pub step_chip: Option<String>,
    pub meta: Meta,
    pub badge_icon: Icon,
    pub badge_tone: Tone,
    /// Три строки шага «Привет»: подпись и пояснение.
    pub welcome: Option<[(String, String); 3]>,
    pub callout: Option<Callout>,
    /// `#destination` спрятан (шаги, антивирус, после удаления).
    pub destination_hidden: bool,
    pub directory: String,
    pub browse_disabled: bool,
    pub browse_hidden: bool,
    pub desktop_hidden: bool,
    pub desktop_checked: bool,
    pub games_shown: bool,
    pub games_hint: String,
    pub games_checked: bool,
    pub uninstall_shown: bool,
    pub uninstall_disabled: bool,
    /// «Открыть Discord» (ошибка антивируса).
    pub discord: Option<String>,
    pub activity_shown: bool,
    pub step: String,
    /// Проценты (только установка в Windows); `None` — бегущая полоса.
    pub progress: Option<u32>,
    pub progress_text: String,
    pub notice: Option<Notice>,
    pub action_disabled: bool,
    pub action_label: String,
    pub action_icon: Icon,
    /// Значок основной кнопки крутится (`icon spin`).
    pub action_spin: bool,
    /// `body[data-phase]` ошибки, подтверждения и удаления: основная кнопка красная.
    pub danger_action: bool,
    pub dismiss: Option<Dismiss>,
    pub dismiss_disabled: bool,
    /// Постоянные подписи (`location`, `desktop`, `desktopHint`, `games`, `uninstall`, подсказки
    /// кнопок окна и выбора папки).
    pub location: String,
    pub desktop: String,
    pub desktop_hint: String,
    pub games: String,
    pub uninstall: String,
    pub browse: String,
    pub minimize: String,
    pub close: String,
}

/// Поля состояния окна 0.4.9 вне [`InstallerState`] (см. описание модуля).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extra {
    /// `process.platform` (`win32` — есть шаг SmartScreen).
    pub platform: Option<String>,
    pub error_code: Option<String>,
    pub error_file: Option<String>,
    /// `uninstallPlan.libraryBlocked`: `other`/`test`/`unverified`.
    pub library_blocked: Option<String>,
    /// `uninstallPlan.libraryRoots`: все папки, которые очистит удаление.
    pub library_roots: Option<Vec<String>>,
}

impl Extra {
    /// Поля по именам 0.4.9 из формы `installer-state`.
    pub fn from_value(value: &Value) -> Extra {
        let text =
            |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        let plan = value.get("uninstallPlan").unwrap_or(&Value::Null);
        let mut extra = Extra {
            platform: text(value, "platform"),
            error_code: text(value, "errorCode"),
            error_file: text(value, "errorFile"),
            ..Extra::default()
        };
        extra.set_plan(plan);
        extra
    }

    fn set_plan(&mut self, plan: &Value) {
        self.library_blocked = plan
            .get("libraryBlocked")
            .and_then(Value::as_str)
            .filter(|blocked| !blocked.is_empty())
            .map(str::to_owned);
        self.library_roots = plan
            .get("libraryRoots")
            .and_then(Value::as_array)
            .map(|roots| {
                roots
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            });
    }
}

/// Чего нет в состоянии окна, но есть в странице: флажки, шаги и кнопки, нажатые до ответа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageInputs {
    /// `#desktop` (по умолчанию отмечен).
    pub desktop: bool,
    /// `#games` (отмечен; каждое подтверждение удаления отмечает его снова).
    pub games: bool,
    /// Шагов перед формой (`intro.length`); `None` — ещё не было первого `render`.
    pub intro: Option<u8>,
    pub intro_step: u8,
    /// `$("action").disabled = true` до конца обработчика.
    pub action_pending: bool,
    pub uninstall_pending: bool,
    pub browse_pending: bool,
}

impl Default for PageInputs {
    fn default() -> Self {
        PageInputs {
            desktop: true,
            games: true,
            intro: None,
            intro_step: 0,
            action_pending: false,
            uninstall_pending: false,
            browse_pending: false,
        }
    }
}

impl PageInputs {
    /// `intro` первого `render`: новая установка — «Привет», в Windows ещё SmartScreen.
    pub fn first_render(&mut self, state: &InstallerState, extra: &Extra) {
        if self.intro.is_none() {
            let steps = match (state.phase, state.update) {
                (Phase::Idle, false) if extra.platform.as_deref() == Some("win32") => 2,
                (Phase::Idle, false) => 1,
                _ => 0,
            };
            self.intro = Some(steps);
        }
    }

    fn intro_len(&self) -> u8 {
        self.intro.unwrap_or(0)
    }

    /// Шаг перед формой сейчас.
    fn intro_now(&self, phase: Phase) -> Option<Intro> {
        let step = if self.intro_step == 0 {
            Intro::Welcome
        } else {
            Intro::Defender
        };
        (phase == Phase::Idle && self.intro_step < self.intro_len()).then_some(step)
    }

    /// `introBack`: «Назад» к прошлому шагу.
    fn intro_back(&self, state: &InstallerState) -> bool {
        state.phase == Phase::Idle
            && !state.cancellable
            && self.intro_step > 0
            && self.intro_len() > 0
    }
}

/// Язык страницы: `strings[state.language] || strings.ru`.
pub fn language_of(state: &InstallerState) -> Language {
    if state.language == "en" {
        Language::En
    } else {
        Language::Ru
    }
}

fn text(lang: Language, key: &str) -> String {
    tr(lang, &format!("installer.{key}"), &[])
}

fn text_with(lang: Language, key: &str, args: &[&str]) -> String {
    tr(lang, &format!("installer.{key}"), args)
}

fn filled(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// `formatBytes(bytes, text)` 0.4.9: «12,4 ГБ» / «12.4 GB», КБ — с округлением вверх.
pub fn format_bytes(bytes: f64, lang: Language) -> String {
    if !bytes.is_finite() || bytes <= 0.0 {
        return String::new();
    }
    let (size, unit) = if bytes < 1_048_576.0 {
        ((bytes / 1024.0).ceil().to_string(), 0)
    } else if bytes < 1_073_741_824.0 {
        (js::to_fixed(bytes / 1_048_576.0, 0), 1)
    } else {
        (js::to_fixed(bytes / 1_073_741_824.0, 1), 2)
    };
    let size = size.replacen('.', &text(lang, "decimal"), 1);
    format!("{size}\u{a0}{}", text(lang, &format!("units.{unit}")))
}

/// `errorKind(code)`: ошибка ОС по коду — нет места, нет доступа (или файлы заняты), остальное.
pub fn error_kind(code: Option<&str>) -> Option<&'static str> {
    let code = code.unwrap_or_default();
    match code {
        "ENOSPC" | "EDQUOT" => Some("noSpace"),
        "EACCES" | "EPERM" | "EBUSY" | "EROFS" => Some("noAccess"),
        "UNKNOWN" => Some("system"),
        _ if code.len() > 1
            && code.starts_with('E')
            && code[1..].bytes().all(|b| b.is_ascii_uppercase()) =>
        {
            Some("system")
        }
        _ => None,
    }
}

/// `exclusionFolder(dir)`: родитель каталога установки (рядом собирается новая версия); каталог
/// в корне диска — он сам.
pub fn exclusion_folder(dir: &str) -> String {
    const SEP: [char; 2] = ['\\', '/'];
    // `/[\\/]+[^\\/]*$/`: последняя часть пути вместе с разделителями перед ней.
    let parent = match dir.rfind(SEP) {
        Some(at) => dir[..=at].trim_end_matches(SEP),
        None => dir,
    };
    let drive = parent.len() == 2
        && parent.as_bytes()[0].is_ascii_alphabetic()
        && parent.as_bytes()[1] == b':';
    if parent.is_empty() || drive {
        dir.to_owned()
    } else {
        parent.to_owned()
    }
}

/// Работа идёт: всё, кроме покоя, итогов и подтверждения.
pub fn is_busy(phase: Phase) -> bool {
    !matches!(
        phase,
        Phase::Idle
            | Phase::Cancelled
            | Phase::Error
            | Phase::Done
            | Phase::UninstallConfirm
            | Phase::Uninstalled
    )
}

fn removing(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::UninstallConfirm | Phase::Uninstalling | Phase::Uninstalled
    )
}

/// `renderMeta(text, busy && !uninstalling)` без чипа шага.
fn meta_of(state: &InstallerState, lang: Language, busy: bool) -> Meta {
    let installed = filled(&state.installed_version);
    let version = filled(&state.version);
    match (state.phase, installed, version) {
        (phase, Some(from), Some(to)) if state.update && (phase == Phase::Idle || busy) => {
            Meta::Update {
                from: from.to_owned(),
                to: to.to_owned(),
            }
        }
        (Phase::Done, Some(installed), _) => Meta::Chip(text_with(lang, "installed", &[installed])),
        (phase, _, Some(version)) if !removing(phase) => {
            Meta::Chip(text_with(lang, "version", &[version]))
        }
        _ => Meta::None,
    }
}

/// `badgeFor(busy, step, antivirus)`: значок в квадрате слева от заголовка.
fn badge_of(
    state: &InstallerState,
    busy: bool,
    intro: Option<Intro>,
    antivirus: bool,
) -> (Icon, Tone) {
    if intro == Some(Intro::Defender) {
        return ("shield", Tone::Caution);
    }
    if antivirus {
        return ("shield", Tone::Danger);
    }
    match state.phase {
        Phase::Done => ("done", Tone::Ok),
        Phase::Error => ("error", Tone::Danger),
        Phase::Cancelled => ("cancelled", Tone::Muted),
        Phase::Uninstalled => ("broom", Tone::Muted),
        Phase::UninstallConfirm | Phase::Uninstalling => ("trash", Tone::Danger),
        _ if busy => ("box", Tone::Accent),
        _ if state.update => ("update", Tone::Accent),
        _ => ("download", Tone::Accent),
    }
}

/// `gamesHint(size, where)`: `[«Вместе с…», size, where].filter(Boolean).join(" · ")`.
fn games_hint(lang: Language, plan: &UninstallPlan, extra: &Extra) -> String {
    let size = format_bytes(plan.library_bytes as f64, lang);
    let roots = extra
        .library_roots
        .clone()
        .unwrap_or_else(|| vec![plan.library_root.clone().unwrap_or_default()]);
    let place = roots.join(", ");
    // Пустые части шаблона «… · %0% · %1%» уходят вместе с разделителем.
    let full = text_with(lang, "gamesHint", &["\u{1}", "\u{2}"]);
    full.split(" · ")
        .filter_map(|part| match part {
            "\u{1}" => (!size.is_empty()).then(|| size.clone()),
            "\u{2}" => (!place.is_empty()).then(|| place.clone()),
            other => Some(other.to_owned()),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Шаг под прогрессом: `text[update && installing ? "updating" : phase] || ""`.
fn step_of(lang: Language, state: &InstallerState) -> String {
    match state.phase {
        Phase::Installing if state.update => text(lang, "updating"),
        Phase::Verifying | Phase::Installing | Phase::Cancelling | Phase::Uninstalling => {
            text(lang, state.phase.as_str())
        }
        _ => String::new(),
    }
}

/// Заголовок и описание.
fn heading(
    state: &InstallerState,
    extra: &Extra,
    lang: Language,
    intro: Option<Intro>,
    antivirus: bool,
    busy: bool,
) -> (String, String) {
    let pair = |title: &str, description: &str| (text(lang, title), text(lang, description));
    let phase = state.phase;
    match intro {
        Some(Intro::Welcome) => return pair("title", "welcomeDescription"),
        Some(Intro::Defender) => return pair("defenderTitle", "defenderDescription"),
        None => {}
    }
    if antivirus {
        return pair("antivirusTitle", "antivirusDescription");
    }
    match phase {
        Phase::UninstallConfirm => pair("uninstallTitle", "uninstallDescription"),
        Phase::Uninstalling => pair("uninstallTitle", "uninstallingDescription"),
        _ if busy && state.update => pair("updateTitle", "updatingDescription"),
        _ if busy => pair("title", "installingDescription"),
        Phase::Error => {
            let title = if state.update {
                "updateErrorTitle"
            } else {
                "errorTitle"
            };
            let description = error_kind(extra.error_code.as_deref())
                .map_or("errorDescription".to_owned(), |kind| {
                    format!("{kind}Description")
                });
            pair(title, &description)
        }
        Phase::Uninstalled => pair("uninstalledTitle", "uninstalledDescription"),
        Phase::Cancelled => pair("cancelledTitle", "cancelledDescription"),
        Phase::Idle if state.update => pair("updateTitle", "updateDescription"),
        Phase::Done => pair("doneTitle", "doneDescription"),
        _ => pair("title", "description"),
    }
}

/// `render(state)` 0.4.9.
pub fn view(state: &InstallerState, extra: &Extra, inputs: &PageInputs) -> InstallerView {
    let lang = language_of(state);
    let phase = state.phase;
    let busy = is_busy(phase);
    let removing = removing(phase);
    let intro = inputs.intro_now(phase);
    let error = filled(&state.error);
    let antivirus = phase == Phase::Error && extra.error_code.as_deref() == Some("ANTIVIRUS");
    let (title, description) = heading(state, extra, lang, intro, antivirus, busy);
    let step_chip = (phase == Phase::Idle && inputs.intro_len() > 0).then(|| {
        text_with(
            lang,
            "step",
            &[
                &(inputs.intro_step + 1).to_string(),
                &(inputs.intro_len() + 1).to_string(),
            ],
        )
    });
    let (badge_icon, badge_tone) = badge_of(state, busy, intro, antivirus);
    let plan = state.uninstall_plan.as_ref();
    let games = plan.filter(|plan| {
        phase == Phase::UninstallConfirm && plan.library_exists && plan.library_bytes > 0
    });
    let (action_label, action_icon) = if intro.is_some() {
        (text(lang, "next"), "arrowRight")
    } else if busy {
        let label = if phase == Phase::Uninstalling {
            "uninstalling"
        } else if state.update {
            "updateBusy"
        } else {
            "busy"
        };
        (text(lang, label), "spinner")
    } else {
        match phase {
            Phase::Done => (text(lang, "launch"), "open"),
            Phase::Error => (text(lang, "retry"), "retry"),
            Phase::UninstallConfirm => (text(lang, "uninstall"), "trash"),
            Phase::Uninstalled => (text(lang, "close"), "cancel"),
            _ if state.update => (text(lang, "update"), "update"),
            _ => (text(lang, "install"), "download"),
        }
    };
    let back = || Dismiss {
        label: text(lang, "back"),
        icon: "back",
    };
    let dismiss = if phase == Phase::Uninstalled {
        (state.can_go_back != Some(false)).then(back)
    } else if state.cancellable || matches!(phase, Phase::Cancelling | Phase::UninstallConfirm) {
        Some(Dismiss {
            label: text(lang, "cancel"),
            icon: "cancel",
        })
    } else if inputs.intro_back(state) {
        Some(back())
    } else {
        None
    };
    let progress = state.progress.filter(|_| busy);
    let blocked = plan
        .filter(|_| phase == Phase::UninstallConfirm)
        .and(extra.library_blocked.as_deref())
        .map(|kind| {
            let key = format!("gamesBlocked.{kind}");
            let found = text(lang, &key);
            if found == format!("installer.{key}") {
                text(lang, "gamesBlocked.other")
            } else {
                found
            }
        });
    let warning = filled(&state.warning).map(str::to_owned).or(blocked);
    let notice = (error.is_some() || warning.is_some()).then(|| Notice {
        text: if antivirus {
            text_with(
                lang,
                "antivirusHelp",
                &[
                    extra.error_file.as_deref().unwrap_or_default(),
                    &exclusion_folder(&state.directory),
                ],
            )
        } else if let (Some(_), Some(_)) = (error, error_kind(extra.error_code.as_deref())) {
            text_with(
                lang,
                "errorCode",
                &[extra.error_code.as_deref().unwrap_or_default()],
            )
        } else {
            error.map(str::to_owned).or(warning).unwrap_or_default()
        },
        danger: error.is_some() && !antivirus,
    });
    InstallerView {
        language: lang,
        phase,
        intro,
        title,
        description,
        step_chip,
        meta: meta_of(state, lang, busy && !removing),
        badge_icon,
        badge_tone,
        welcome: (intro == Some(Intro::Welcome)).then(|| {
            ["Builds", "Play", "Update"].map(|key| {
                (
                    text(lang, &format!("welcome{key}")),
                    text(lang, &format!("welcome{key}Hint")),
                )
            })
        }),
        callout: (intro == Some(Intro::Defender)).then(|| Callout {
            lead: text(lang, "calloutLead"),
            title: text(lang, "calloutTitle"),
            steps: text(lang, "calloutSteps"),
            why: text(lang, "calloutWhy"),
        }),
        destination_hidden: phase == Phase::Uninstalled || intro.is_some() || antivirus,
        directory: state.directory.clone(),
        browse_disabled: busy || !state.can_choose_directory || removing || inputs.browse_pending,
        browse_hidden: !state.can_choose_directory,
        desktop_hidden: busy || phase == Phase::Done || removing || intro.is_some() || antivirus,
        desktop_checked: inputs.desktop,
        games_shown: games.is_some(),
        games_hint: games
            .map(|plan| games_hint(lang, plan, extra))
            .unwrap_or_default(),
        games_checked: inputs.games,
        uninstall_shown: matches!(phase, Phase::Done | Phase::Error) && state.can_uninstall,
        uninstall_disabled: busy || inputs.uninstall_pending,
        discord: antivirus.then(|| text(lang, "discord")),
        activity_shown: busy || phase == Phase::Uninstalling,
        step: step_of(lang, state),
        progress,
        progress_text: progress
            .map(|value| format!("{value}%"))
            .unwrap_or_default(),
        notice,
        action_disabled: busy || inputs.action_pending,
        action_label,
        action_icon,
        action_spin: busy && intro.is_none(),
        danger_action: intro.is_none()
            && matches!(
                phase,
                Phase::Error | Phase::UninstallConfirm | Phase::Uninstalling
            ),
        dismiss,
        dismiss_disabled: phase == Phase::Cancelling,
        location: text(lang, "location"),
        desktop: text(lang, "desktop"),
        desktop_hint: text(lang, "desktopHint"),
        games: text(lang, "games"),
        uninstall: text(lang, "uninstall"),
        browse: text(lang, "browse"),
        minimize: text(lang, "minimize"),
        close: text(lang, "close"),
    }
}

/// Состояние окна из формы `installer-state` 0.4.9 (сцены снимков). Незнакомая фаза — `idle`, как
/// пустой `data-phase`.
pub fn state_from_value(value: &Value) -> InstallerState {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let flag = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
    let phase = match text("phase").as_deref() {
        Some("verifying") => Phase::Verifying,
        Some("installing") => Phase::Installing,
        Some("done") => Phase::Done,
        Some("error") => Phase::Error,
        Some("cancelling") => Phase::Cancelling,
        Some("cancelled") => Phase::Cancelled,
        Some("uninstall-confirm") => Phase::UninstallConfirm,
        Some("uninstalling") => Phase::Uninstalling,
        Some("uninstalled") => Phase::Uninstalled,
        _ => Phase::Idle,
    };
    let plan = value
        .get("uninstallPlan")
        .filter(|plan| plan.is_object())
        .map(|plan| {
            let text = |key: &str| plan.get(key).and_then(Value::as_str).map(str::to_owned);
            let bytes = |key: &str| plan.get(key).and_then(Value::as_u64).unwrap_or(0);
            UninstallPlan {
                managed: plan
                    .get("managed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                install_root: text("installRoot").unwrap_or_default(),
                install_bytes: bytes("installBytes"),
                library_root: text("libraryRoot"),
                library_exists: plan
                    .get("libraryExists")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                library_bytes: bytes("libraryBytes"),
                desktop_entry: text("desktopEntry"),
                library_roots: plan
                    .get("libraryRoots")
                    .and_then(Value::as_array)
                    .map(|roots| {
                        roots
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    }),
                library_blocked: text("libraryBlocked"),
            }
        });
    InstallerState {
        phase,
        directory: text("directory").unwrap_or_default(),
        language: text("language").unwrap_or_default(),
        can_uninstall: flag("canUninstall"),
        can_choose_directory: flag("canChooseDirectory"),
        cancellable: flag("cancellable"),
        version: text("version"),
        installed_version: text("installedVersion"),
        update: flag("update"),
        error: text("error"),
        warning: text("warning"),
        progress: value
            .get("progress")
            .and_then(Value::as_f64)
            .map(|p| p.max(0.0) as u32),
        uninstall_plan: plan,
        can_go_back: value.get("canGoBack").and_then(Value::as_bool),
        platform: text("platform").unwrap_or_default(),
        error_code: text("errorCode"),
        error_file: text("errorFile"),
    }
}

/// Запуск долгого метода окна вне потока интерфейса.
pub type Spawn = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;
/// Диалог выбора каталога: начальный каталог → выбранный.
pub type PickDirectory = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
/// Получатель того, что показать (из любого потока).
pub type ViewSink = Arc<dyn Fn(InstallerView) + Send + Sync>;
/// Действие страницы без ответа (`installer-open-discord`).
pub type Command = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Page {
    /// Последнее показанное состояние (`state` страницы).
    shown: InstallerState,
    extra: Extra,
    /// Состояние до подтверждения удаления: «Отмена» возвращает к нему.
    before_uninstall: Option<(InstallerState, Extra)>,
    inputs: PageInputs,
}

impl Page {
    fn view(&self) -> InstallerView {
        view(&self.shown, &self.extra, &self.inputs)
    }
}

/// Страница окна установщика: кнопки → методы [`InstallerWindow`], состояние → [`InstallerView`].
pub struct InstallerController {
    window: Arc<InstallerWindow>,
    page: Mutex<Page>,
    sink: ViewSink,
    spawn: Spawn,
    pick: PickDirectory,
    open_discord: Command,
}

impl InstallerController {
    /// Страница над окном: слушатель `installer-state` ставится здесь, первое состояние
    /// (`installer.state().then(render)`) показывается сразу.
    pub fn new(
        window: Arc<InstallerWindow>,
        sink: ViewSink,
        spawn: Spawn,
        pick: PickDirectory,
        open_discord: Command,
    ) -> Arc<InstallerController> {
        let controller = Arc::new(InstallerController {
            window: window.clone(),
            page: Mutex::new(Page::default()),
            sink,
            spawn,
            pick,
            open_discord,
        });
        let weak = Arc::downgrade(&controller);
        window.set_listener(move |state| {
            if let Some(controller) = weak.upgrade() {
                controller.render(state.clone());
            }
        });
        controller.render(window.state());
        controller
    }

    fn page(&self) -> MutexGuard<'_, Page> {
        self.page
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `render(next)` по состоянию окна.
    fn render(&self, next: InstallerState) {
        let extra = Extra::from_value(&next.to_value());
        self.show(next, extra);
    }

    fn show(&self, next: InstallerState, extra: Extra) {
        let view = {
            let mut page = self.page();
            page.inputs.first_render(&next, &extra);
            page.shown = next;
            page.extra = extra;
            page.view()
        };
        (self.sink)(view);
    }

    /// `render(state)` — то же состояние с новыми флажками и нажатиями.
    fn refresh(&self, change: impl FnOnce(&mut Page)) {
        let view = {
            let mut page = self.page();
            change(&mut page);
            page.view()
        };
        (self.sink)(view);
    }

    /// Что сейчас показано.
    pub fn view(&self) -> InstallerView {
        self.page().view()
    }

    pub fn shown(&self) -> InstallerState {
        self.page().shown.clone()
    }

    pub fn window(&self) -> &Arc<InstallerWindow> {
        &self.window
    }

    /// Флажок «Ярлык на рабочем столе».
    pub fn set_desktop(&self, checked: bool) {
        self.refresh(|page| page.inputs.desktop = checked);
    }

    /// Флажок «Также удалить игры».
    pub fn set_games(&self, checked: bool) {
        self.refresh(|page| page.inputs.games = checked);
    }

    /// Основная кнопка (`#action`): следующий шаг, открыть, удалить, закрыть или установить.
    pub fn action(self: &Arc<Self>) {
        let (phase, inputs, games_shown) = {
            let mut page = self.page();
            if page.inputs.intro_now(page.shown.phase).is_some() {
                page.inputs.intro_step += 1;
                let view = page.view();
                drop(page);
                (self.sink)(view);
                return;
            }
            if is_busy(page.shown.phase) || page.inputs.action_pending {
                return;
            }
            (page.shown.phase, page.inputs, page.view().games_shown)
        };
        if phase == Phase::Uninstalled {
            self.close();
            return;
        }
        self.refresh(|page| page.inputs.action_pending = true);
        let this = self.clone();
        (self.spawn)(Box::new(move || {
            match phase {
                Phase::Done => {
                    this.window.launch();
                }
                Phase::UninstallConfirm => {
                    this.window.uninstall(games_shown && inputs.games);
                }
                _ => {
                    this.window.install(inputs.desktop);
                }
            }
            this.refresh(|page| page.inputs.action_pending = false);
        }));
    }

    /// «Удалить HLSR» (`#uninstall`): план удаления — и подтверждение в самой странице.
    pub fn uninstall(self: &Arc<Self>) {
        {
            let page = self.page();
            if is_busy(page.shown.phase) || page.inputs.uninstall_pending {
                return;
            }
        }
        self.refresh(|page| page.inputs.uninstall_pending = true);
        let this = self.clone();
        (self.spawn)(Box::new(move || {
            let plan = this.window.uninstall_plan();
            let view = {
                let mut page = this.page();
                page.inputs.uninstall_pending = false;
                if let Some(plan) = plan {
                    // Решение владельца: по умолчанию удаление полное, папка HLSR уходит целиком.
                    page.inputs.games = true;
                    page.before_uninstall = Some((page.shown.clone(), page.extra.clone()));
                    page.extra.set_plan(&plan.to_value());
                    let mut next = page.shown.clone();
                    next.phase = Phase::UninstallConfirm;
                    next.error = None;
                    next.warning = None;
                    next.uninstall_plan = Some(plan);
                    page.shown = next;
                }
                page.view()
            };
            (this.sink)(view);
        }));
    }

    /// «Отмена»/«Назад» (`#dismiss`).
    pub fn dismiss(self: &Arc<Self>) {
        let (phase, intro_back, before) = {
            let page = self.page();
            (
                page.shown.phase,
                page.inputs.intro_back(&page.shown),
                page.before_uninstall.clone(),
            )
        };
        match (phase, before) {
            _ if intro_back => self.refresh(|page| page.inputs.intro_step -= 1),
            (Phase::Cancelling, _) => {}
            (Phase::Uninstalled, _) => {
                let this = self.clone();
                (self.spawn)(Box::new(move || {
                    this.window.back();
                }));
            }
            (Phase::UninstallConfirm, Some((before, extra))) => self.show(before, extra),
            _ => {
                self.window.cancel();
            }
        }
    }

    /// «Открыть Discord» (`installer-open-discord`, только в фазе `error`): браузер открывается не
    /// в потоке интерфейса.
    pub fn discord(&self) {
        let window = self.window.clone();
        let open = self.open_discord.clone();
        (self.spawn)(Box::new(move || {
            window.open_discord(|| open());
        }));
    }

    /// «Выбрать папку» (`#browse`).
    pub fn browse(self: &Arc<Self>) {
        if self.page().view().browse_disabled {
            return;
        }
        self.refresh(|page| page.inputs.browse_pending = true);
        let this = self.clone();
        (self.spawn)(Box::new(move || {
            let pick = this.pick.clone();
            let result = this.window.select_directory(|start| pick(start));
            this.refresh(|page| {
                page.inputs.browse_pending = false;
                if let Err(message) = result {
                    page.shown.error = Some(message);
                    page.extra.error_code = None;
                    page.extra.error_file = None;
                }
            });
        }));
    }

    /// Кнопка «Закрыть», «Закрыть» после удаления и закрытие окна системой: во время работы окно
    /// остаётся до её конца (отменяемая — отменяется); закрыть сейчас — `true`.
    pub fn close(&self) -> bool {
        self.window.request_close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shown(value: Value, steps: u8) -> InstallerView {
        let state = state_from_value(&value);
        let extra = Extra::from_value(&value);
        let mut inputs = PageInputs::default();
        inputs.first_render(&state, &extra);
        inputs.intro_step = steps;
        view(&state, &extra, &inputs)
    }

    #[test]
    fn new_install_walks_welcome_and_defender_steps_on_windows() {
        let idle = json!({ "phase": "idle", "platform": "win32", "version": "0.5.0",
            "directory": "C:\\HLSR\\Launcher", "canChooseDirectory": true });
        let welcome = shown(idle.clone(), 0);
        assert_eq!(welcome.intro, Some(Intro::Welcome));
        assert_eq!(welcome.step_chip.as_deref(), Some("Шаг 1 из 3"));
        assert_eq!(welcome.meta, Meta::Chip("Версия 0.5.0".into()));
        assert_eq!(
            (welcome.action_label.as_str(), welcome.action_icon),
            ("Далее", "arrowRight")
        );
        assert!(welcome.destination_hidden && welcome.desktop_hidden && welcome.dismiss.is_none());
        let defender = shown(idle.clone(), 1);
        assert_eq!(defender.intro, Some(Intro::Defender));
        assert_eq!(
            (defender.badge_icon, defender.badge_tone),
            ("shield", Tone::Caution)
        );
        assert_eq!(defender.dismiss.unwrap().label, "Назад");
        let form = shown(idle.clone(), 2);
        assert_eq!(form.intro, None);
        assert_eq!(form.step_chip.as_deref(), Some("Шаг 3 из 3"));
        assert_eq!(form.action_label, "Установить");
        assert!(!form.destination_hidden && !form.desktop_hidden);
        // Linux: только «Привет»; обновление открывается сразу формой.
        let linux = shown(json!({ "phase": "idle", "platform": "linux" }), 0);
        assert_eq!(linux.step_chip.as_deref(), Some("Шаг 1 из 2"));
        let update = shown(
            json!({ "phase": "idle", "platform": "win32", "update": true,
            "installedVersion": "0.4.9", "version": "0.5.0" }),
            0,
        );
        assert_eq!((update.intro, update.step_chip.clone()), (None, None));
        assert_eq!(
            update.meta,
            Meta::Update {
                from: "0.4.9".into(),
                to: "0.5.0".into()
            }
        );
    }

    #[test]
    fn os_errors_show_kind_and_code_antivirus_its_own_screen() {
        let base = json!({ "phase": "error", "language": "en", "directory": "C:\\HLSR\\Launcher",
            "error": "ENOSPC: no space left on device", "errorCode": "ENOSPC" });
        let space = shown(base.clone(), 0);
        assert_eq!(
            space.description,
            "The disk is out of space. Free some up and click Retry."
        );
        let notice = space.notice.unwrap();
        assert_eq!(
            (notice.text.as_str(), notice.danger),
            ("Error code for support: ENOSPC", true)
        );
        let mut other = base.clone();
        other["errorCode"] = json!("EXDEV");
        assert!(
            shown(other, 0)
                .description
                .starts_with("Couldn't write the files")
        );
        let mut plain = base.clone();
        plain["errorCode"] = Value::Null;
        assert_eq!(
            shown(plain, 0).notice.unwrap().text,
            "ENOSPC: no space left on device"
        );
        let mut av = base;
        av["errorCode"] = json!("ANTIVIRUS");
        av["errorFile"] = json!("C:\\HLSR\\Launcher\\HLSR.exe");
        let av = shown(av, 0);
        assert_eq!(av.title, "Antivirus stopped the installation");
        assert_eq!((av.badge_icon, av.badge_tone), ("shield", Tone::Danger));
        assert_eq!(av.discord.as_deref(), Some("Open Discord"));
        assert!(av.destination_hidden && av.desktop_hidden);
        let notice = av.notice.unwrap();
        assert!(!notice.danger);
        assert!(
            notice
                .text
                .starts_with("File: C:\\HLSR\\Launcher\\HLSR.exe\n1. Allow")
        );
        assert!(notice.text.contains("add the C:\\HLSR folder"));
    }

    #[test]
    fn uninstall_lists_roots_with_size_and_explains_a_blocked_library() {
        let plan = json!({ "phase": "uninstall-confirm", "uninstallPlan": { "managed": true,
            "installRoot": "C:\\HLSR\\Launcher", "libraryRoot": "C:\\HLSR", "libraryExists": true,
            "libraryBytes": 16_320_875_725u64, "libraryRoots": ["C:\\HLSR", "D:\\Games\\HLSR"] } });
        let view = shown(plan.clone(), 0);
        assert!(view.games_shown && view.games_checked);
        assert_eq!(
            view.games_hint,
            "Вместе с сейвами, демками, настройками и входом · 15,2\u{a0}ГБ · C:\\HLSR, D:\\Games\\HLSR"
        );
        let mut blocked = plan;
        blocked["uninstallPlan"]["libraryBlocked"] = json!("test");
        let notice = shown(blocked, 0).notice.unwrap();
        assert_eq!(
            (notice.text.as_str(), notice.danger),
            ("Тестовая сборка не удаляет игры и настройки.", false)
        );
    }

    #[test]
    fn busy_update_and_helpers() {
        let busy = shown(
            json!({ "phase": "installing", "update": true, "installedVersion": "0.4.9",
            "version": "0.5.0", "cancellable": true, "progress": 42 }),
            0,
        );
        assert_eq!(
            (busy.title.as_str(), busy.action_label.as_str()),
            ("Обновление HLSR", "Обновление…")
        );
        assert_eq!(busy.step, "Обновляем файлы лаунчера");
        assert!(busy.action_spin && busy.activity_shown);
        assert_eq!(busy.progress_text, "42%");
        assert_eq!(format_bytes(1500.0, Language::Ru), "2\u{a0}КБ");
        assert_eq!(format_bytes(734_003_200.0, Language::En), "700\u{a0}MB");
        assert_eq!(exclusion_folder("C:\\HLSR\\Launcher"), "C:\\HLSR");
        assert_eq!(exclusion_folder("C:\\HLSR"), "C:\\HLSR");
        assert_eq!(
            exclusion_folder("/home/u/.local/opt/hlsr"),
            "/home/u/.local/opt"
        );
        assert_eq!(error_kind(Some("EBUSY")), Some("noAccess"));
        assert_eq!(error_kind(Some("ANTIVIRUS")), None);
        assert_eq!(error_kind(Some("UNKNOWN")), Some("system"));
    }
}
