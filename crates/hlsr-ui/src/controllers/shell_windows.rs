//! Логика окон оболочки 0.4.7 без Slint: «Недоступно» (`UnavailableModal.vue`: причина → значок,
//! тексты, кнопки), заметки версии игры (`GameNotesModal.vue`), журнал изменений
//! (`ChangelogModal.vue` + `changelog.js`: выпуски не новее версии лаунчера, будущий выпуск из
//! карточки обновления, заметки игр, фокус на версии), окно запуска LiveSplit
//! (`LiveSplitLaunchModal.vue`: игра и сплиты), выбор папки библиотеки (`LibrarySetupModal.vue`).
//! Сверка — скрипты компонентов, выполненные в Node (`tools/reference/ui/shell-windows.js`,
//! `tests/shell_windows.rs`).

use super::shell_flows::{LiveSplitGame, LiveSplitPick, short_name};
use crate::services::{BoxFuture, Reply, Services};
use hlsr_core::js;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::OnceLock;

// ---- «Недоступно» ----------------------------------------------------------------------------

/// Кнопка окна «Недоступно».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnavailableAction {
    pub id: &'static str,
    pub icon: &'static str,
    /// Ключ подписи.
    pub key: &'static str,
}

const RECHECK: UnavailableAction = UnavailableAction {
    id: "recheck",
    icon: "fas fa-sync-alt",
    key: "#L_UA_ACT_RECHECK",
};
const OPEN_STEAM: UnavailableAction = UnavailableAction {
    id: "open-steam",
    icon: "fab fa-steam",
    key: "#L_UA_ACT_OPEN_STEAM",
};
const INSTALL_SDK: UnavailableAction = UnavailableAction {
    id: "install-sdk",
    icon: "fas fa-download",
    key: "#L_UA_ACT_SDK",
};
const LIBRARY: UnavailableAction = UnavailableAction {
    id: "open-library-settings",
    icon: "fas fa-folder-open",
    key: "#L_UA_ACT_LIBRARY",
};
const REPAIR: UnavailableAction = UnavailableAction {
    id: "repair",
    icon: "fas fa-wrench",
    key: "#L_UA_ACT_REPAIR",
};
const DIAGNOSTICS: UnavailableAction = UnavailableAction {
    id: "open-diagnostics",
    icon: "fas fa-stethoscope",
    key: "#L_UA_ACT_DIAG",
};
const DISCORD: UnavailableAction = UnavailableAction {
    id: "open-discord",
    icon: "fab fa-discord",
    key: "#L_UA_ACT_DISCORD",
};
const UPDATE_LAUNCHER: UnavailableAction = UnavailableAction {
    id: "check-launcher-update",
    icon: "fas fa-arrow-up",
    key: "#L_UA_ACT_UPDATE_LAUNCHER",
};

/// Вид окна «Недоступно» по причине.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnavailableView {
    pub icon: &'static str,
    pub title: &'static str,
    pub text: &'static str,
    pub actions: &'static [UnavailableAction],
}

const fn view(
    icon: &'static str,
    title: &'static str,
    text: &'static str,
    actions: &'static [UnavailableAction],
) -> UnavailableView {
    UnavailableView {
        icon,
        title,
        text,
        actions,
    }
}

const UNKNOWN: UnavailableView = view(
    "fas fa-info-circle",
    "#L_UA_UNKNOWN",
    "#L_UA_UNKNOWN_T",
    &[RECHECK, DIAGNOSTICS, DISCORD],
);

/// `VIEWS[reason] || VIEWS.UNKNOWN_UNAVAILABLE`.
pub fn unavailable_view(reason: &str) -> UnavailableView {
    match reason {
        "STEAM_NOT_RUNNING" => view(
            "fab fa-steam",
            "#L_UA_STEAM_NOT_RUNNING",
            "#L_UA_STEAM_NOT_RUNNING_T",
            &[OPEN_STEAM, RECHECK, DIAGNOSTICS],
        ),
        "STEAM_INTEGRATION_UNAVAILABLE" => view(
            "fas fa-plug",
            "#L_UA_STEAM_INTEGRATION",
            "#L_UA_STEAM_INTEGRATION_T",
            &[RECHECK, DIAGNOSTICS, DISCORD],
        ),
        "SOURCE_SDK_2013_MISSING" => view(
            "fas fa-cube",
            "#L_UA_SDK_MISSING",
            "#L_UA_SDK_MISSING_T",
            &[INSTALL_SDK, RECHECK],
        ),
        "REQUIRED_LICENSE_MISSING" => view(
            "fas fa-id-card",
            "#L_UA_LICENSE",
            "#L_UA_LICENSE_T",
            &[OPEN_STEAM, RECHECK, DISCORD],
        ),
        "LICENSE_CHECK_UNAVAILABLE" => view(
            "fas fa-question-circle",
            "#L_UA_LICENSE_CHECK",
            "#L_UA_LICENSE_CHECK_T",
            &[RECHECK, DIAGNOSTICS],
        ),
        "GAME_NOT_INSTALLED" => view(
            "fas fa-download",
            "#L_UA_NOT_INSTALLED",
            "#L_UA_NOT_INSTALLED_T",
            &[RECHECK],
        ),
        "DELIVERY_CHANNEL_DISABLED" => view(
            "fas fa-satellite-dish",
            "#L_UA_CHANNEL",
            "#L_UA_CHANNEL_T",
            &[RECHECK, DISCORD],
        ),
        "MANIFEST_UNAVAILABLE" => view(
            "fas fa-wifi",
            "#L_UA_MANIFEST",
            "#L_UA_MANIFEST_T",
            &[RECHECK, DIAGNOSTICS],
        ),
        "MANIFEST_SIGNATURE_INVALID" => view(
            "fas fa-shield-alt",
            "#L_UA_SIGNATURE",
            "#L_UA_SIGNATURE_T",
            &[RECHECK, DISCORD],
        ),
        "UPDATE_REQUIRED" => view(
            "fas fa-sync-alt",
            "#L_UA_UPDATE_REQUIRED",
            "#L_UA_UPDATE_REQUIRED_T",
            &[RECHECK],
        ),
        "GAME_FILES_DAMAGED" => view(
            "fas fa-wrench",
            "#L_UA_DAMAGED",
            "#L_UA_DAMAGED_T",
            &[REPAIR],
        ),
        "LIBRARY_UNAVAILABLE" => view(
            "fas fa-folder-minus",
            "#L_UA_LIBRARY",
            "#L_UA_LIBRARY_T",
            &[LIBRARY, RECHECK],
        ),
        "LIBRARY_READ_ONLY" => view(
            "fas fa-lock",
            "#L_UA_LIBRARY_RO",
            "#L_UA_LIBRARY_RO_T",
            &[LIBRARY, RECHECK],
        ),
        "INSUFFICIENT_DISK_SPACE" => view(
            "fas fa-hdd",
            "#L_UA_SPACE",
            "#L_UA_SPACE_T",
            &[LIBRARY, RECHECK],
        ),
        "GAME_PROCESS_ALREADY_RUNNING" => view(
            "fas fa-gamepad",
            "#L_UA_RUNNING",
            "#L_UA_RUNNING_T",
            &[RECHECK],
        ),
        "BUILD_METADATA_MISSING" => view(
            "fas fa-file-alt",
            "#L_UA_BUILD_META",
            "#L_UA_BUILD_META_MISSING_T",
            &[REPAIR, DIAGNOSTICS],
        ),
        "BUILD_METADATA_INVALID" => view(
            "fas fa-file-alt",
            "#L_UA_BUILD_META",
            "#L_UA_BUILD_META_INVALID_T",
            &[REPAIR, DIAGNOSTICS],
        ),
        "BUILD_METADATA_MISMATCH" => view(
            "fas fa-file-alt",
            "#L_UA_BUILD_META",
            "#L_UA_BUILD_META_MISMATCH_T",
            &[REPAIR, DIAGNOSTICS],
        ),
        "BUILD_METADATA_UNSUPPORTED" => view(
            "fas fa-file-alt",
            "#L_UA_BUILD_META_UNSUPPORTED",
            "#L_UA_BUILD_META_UNSUPPORTED_T",
            &[UPDATE_LAUNCHER, DIAGNOSTICS, DISCORD],
        ),
        "BUILD_REQUIRES_NEWER_LAUNCHER" => view(
            "fas fa-arrow-alt-circle-up",
            "#L_UA_BUILD_NEWER_LAUNCHER",
            "#L_UA_BUILD_NEWER_LAUNCHER_T",
            &[UPDATE_LAUNCHER, DIAGNOSTICS, DISCORD],
        ),
        _ => UNKNOWN,
    }
}

/// `actions` 0.4.9: без ремонта по файлам (`fileRepair` статуса) — без «Починить», а если
/// кнопок не осталось — «Диагностика» и Discord.
pub fn unavailable_actions(reason: &str, file_repair: bool) -> Vec<UnavailableAction> {
    let actions = unavailable_view(reason).actions;
    if file_repair {
        return actions.to_vec();
    }
    let left: Vec<UnavailableAction> = actions
        .iter()
        .copied()
        .filter(|action| action.id != REPAIR.id)
        .collect();
    if left.is_empty() {
        vec![DIAGNOSTICS, DISCORD]
    } else {
        left
    }
}

// ---- Тексты из main ---------------------------------------------------------------------------

/// `item.replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim()`.
pub fn plain_text(item: &str) -> String {
    let mut out = String::with_capacity(item.len());
    let mut chars = item.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            // `<[^>]*>`: тег до первой `>`; без неё `<` остаётся текстом.
            let rest: String = chars.clone().collect();
            if let Some(end) = rest.find('>') {
                for _ in 0..rest[..end].chars().count() + 1 {
                    chars.next();
                }
                out.push(' ');
                continue;
            }
        }
        out.push(c);
    }
    let mut collapsed = String::with_capacity(out.len());
    let mut space = false;
    for c in out.chars() {
        if js::is_whitespace(c) {
            if !space {
                collapsed.push(' ');
            }
            space = true;
        } else {
            collapsed.push(c);
            space = false;
        }
    }
    js::trim(&collapsed).to_owned()
}

/// Пункты, как их чистит окно: строки с текстом, без тегов, по одному пробелу, не длиннее
/// `max_len` (в единицах UTF-16; `None` — без обрезки), не больше `max_items`.
fn clean_items(items: &[Value], max_len: Option<usize>, max_items: Option<usize>) -> Vec<String> {
    let cleaned = items
        .iter()
        .filter_map(Value::as_str)
        .filter(|item| !js::trim(item).is_empty())
        .map(|item| {
            let text = plain_text(item);
            match max_len {
                Some(len) => js::slice_utf16(&text, len),
                None => text,
            }
        })
        .filter(|item| !item.is_empty());
    match max_items {
        Some(count) => cleaned.take(count).collect(),
        None => cleaned.collect(),
    }
}

// ---- Заметки версии игры ----------------------------------------------------------------------

/// Заголовок `#L_GN_TITLE` с «<игра> <версия>».
pub fn game_notes_title(
    tr: impl Fn(&str, &[&str]) -> String,
    game_title: &str,
    version: &str,
) -> String {
    tr("#L_GN_TITLE", &[&format!("{game_title} {version}")])
}

/// `items`: пункты заметок той же версии — не больше 20, по 300 знаков (0.4.9: предел
/// `MAX_ITEM_LENGTH` из `release-notes.js`, там пункт уже обрезан по слову).
pub fn game_notes_items(notes: Option<&Value>, version: &str) -> Vec<String> {
    let Some(notes) = notes.filter(|notes| js::truthy(notes)) else {
        return Vec::new();
    };
    let Some(Value::Array(items)) = notes.get("items") else {
        return Vec::new();
    };
    if notes.get("version").and_then(Value::as_str) != Some(version) {
        return Vec::new();
    }
    clean_items(items, Some(300), Some(20))
}

// ---- Журнал изменений -------------------------------------------------------------------------

/// Группа пунктов выпуска (`title` — подзаголовок или версия сборки игры).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReleaseGroup {
    pub title: Option<String>,
    pub items: Vec<String>,
}

/// Выпуск журнала.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub version: Option<String>,
    pub title: String,
    pub groups: Vec<ReleaseGroup>,
    #[serde(default)]
    pub current: bool,
    #[serde(default)]
    pub upcoming: bool,
}

#[derive(Deserialize)]
struct ChangelogData {
    title: BTreeMap<String, String>,
    releases: Vec<Structured>,
    legacy: BTreeMap<String, Vec<Release>>,
}

#[derive(Deserialize)]
struct Structured {
    version: String,
    ru: Vec<String>,
    en: Vec<String>,
}

fn changelog_data() -> &'static ChangelogData {
    static DATA: OnceLock<ChangelogData> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(include_str!("../../data/changelog.json"))
            .expect("data/changelog.json разбирается")
    })
}

/// `notNewerThan(a, b)`: версия `a` не новее `b` (части до «-», числа `parseInt`, недостающие — 0).
pub fn not_newer_than(a: &str, b: &str) -> bool {
    let parse = |value: &str| -> Vec<i64> {
        let value = if value.is_empty() { "0" } else { value };
        value
            .split('-')
            .next()
            .unwrap_or_default()
            .split('.')
            .map(parse_int)
            .collect()
    };
    let (left, right) = (parse(a), parse(b));
    for i in 0..left.len().max(right.len()) {
        let l = left.get(i).copied().unwrap_or(0);
        let r = right.get(i).copied().unwrap_or(0);
        if l != r {
            return l < r;
        }
    }
    true
}

/// `parseInt(part, 10) || 0`: ведущие цифры после пробелов и знака.
fn parse_int(part: &str) -> i64 {
    let text = js::trim(part);
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let number: String = digits.chars().take_while(char::is_ascii_digit).collect();
    number.parse::<i64>().map(|n| sign * n).unwrap_or(0)
}

/// Выпуски `RELEASES` не новее версии лаунчера; первый — текущий.
pub fn structured_changelog(language: &str, version: &str) -> Vec<Release> {
    let data = changelog_data();
    let lang = if language == "en" { "en" } else { "ru" };
    let title = data.title.get(lang).map(String::as_str).unwrap_or_default();
    data.releases
        .iter()
        .filter(|release| not_newer_than(&release.version, version))
        .enumerate()
        .map(|(index, release)| Release {
            version: Some(release.version.clone()),
            title: js::trim(&format!("{title} {}", release.version)).to_owned(),
            groups: vec![ReleaseGroup {
                title: None,
                items: if lang == "en" {
                    release.en.clone()
                } else {
                    release.ru.clone()
                },
            }],
            current: index == 0,
            upcoming: false,
        })
        .collect()
}

/// `buildChangelog(legacyHtml, language, version)`: выпуски не новее версии лаунчера (первый —
/// текущий) и старый журнал (`#UI_CHANGELOG_CONTENT`, разобран выгрузкой `export-changelog.js`).
pub fn build_changelog(language: &str, version: &str) -> Vec<Release> {
    let lang = if language == "en" { "en" } else { "ru" };
    let mut out = structured_changelog(language, version);
    out.extend(
        changelog_data()
            .legacy
            .get(lang)
            .cloned()
            .unwrap_or_default(),
    );
    out
}

/// `VERSION_RE`: `^[0-9A-Za-z.\-+]{1,32}$`.
pub fn is_version(text: &str) -> bool {
    (1..=32).contains(&text.len())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

/// `safeExtraRelease()`: будущий выпуск из карточки обновления (версия и до 20 пунктов).
pub fn safe_extra_release(raw: Option<&Value>) -> Option<Release> {
    let raw = raw.filter(|raw| raw.is_object())?;
    let version = raw.get("version").and_then(Value::as_str)?;
    if !is_version(version) {
        return None;
    }
    let Some(Value::Array(items)) = raw.get("items") else {
        return None;
    };
    let items = clean_items(items, Some(200), Some(20));
    if items.is_empty() {
        return None;
    }
    Some(Release {
        version: Some(version.to_owned()),
        title: format!("HLSR {version}"),
        groups: vec![ReleaseGroup { title: None, items }],
        current: false,
        upcoming: true,
    })
}

/// `allReleases`: будущий выпуск впереди, если его версии ещё нет в журнале.
pub fn all_releases(releases: Vec<Release>, extra: Option<Release>) -> Vec<Release> {
    match extra {
        Some(extra)
            if !releases
                .iter()
                .any(|release| release.version == extra.version) =>
        {
            std::iter::once(extra).chain(releases).collect()
        }
        _ => releases,
    }
}

/// `isFocused(release)`: выпуск, к которому открыли окно.
pub fn is_focused(initial_version: Option<&str>, release: &Release) -> bool {
    initial_version
        .is_some_and(|version| !version.is_empty() && release.version.as_deref() == Some(version))
}

/// Заметки сборок одной игры (`gameNotes` журнала).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameNotes {
    pub game_id: String,
    pub launcher_id: Option<String>,
    /// Версии с пунктами по порядку.
    pub versions: Vec<(String, Vec<String>)>,
}

/// `loadGameNotes()`: ответ `game-release-notes-all` → заметки игр (пустые отброшены).
pub fn game_notes_list(reply: &Value) -> Vec<GameNotes> {
    let Value::Array(list) = reply else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|game| {
            let game_id = game.get("gameId").and_then(Value::as_str)?;
            let Some(Value::Array(versions)) = game.get("versions") else {
                return None;
            };
            let versions: Vec<(String, Vec<String>)> = versions
                .iter()
                .filter(|entry| js::truthy(entry))
                .filter_map(|entry| {
                    let version = js::to_template_string(entry.get("version"));
                    if !is_version(&version) {
                        return None;
                    }
                    let Some(Value::Array(items)) = entry.get("items") else {
                        return None;
                    };
                    let items = clean_items(items, None, None);
                    (!items.is_empty()).then_some((version, items))
                })
                .collect();
            (!versions.is_empty()).then(|| GameNotes {
                game_id: game_id.to_owned(),
                launcher_id: game
                    .get("launcherId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                versions,
            })
        })
        .collect()
}

/// `gameTitle(game)`: короткое имя игры оболочки, иначе id доставки.
pub fn game_title(game: &GameNotes) -> String {
    game.launcher_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .and_then(short_name)
        .map(str::to_owned)
        .unwrap_or_else(|| game.game_id.clone())
}

// ---- Окно запуска LiveSplit -------------------------------------------------------------------

/// `kindLabel(kind)`.
pub fn kind_label(kind: Option<&str>) -> &'static str {
    if kind == Some("maps") {
        "#L_TOOL_SPLITS_MAPS"
    } else {
        "#L_TOOL_SPLITS_CHAPTERS"
    }
}

/// Выбор в окне LiveSplit: игра и набор сплитов.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveSplitDialog {
    pub games: Vec<LiveSplitGame>,
    pub chosen: Map<String, Value>,
    pub game_id: Option<String>,
    pub preset_id: Option<String>,
}

fn preset_id(preset: &Value) -> Option<String> {
    preset
        .get("id")
        .filter(|id| !id.is_null())
        .map(js::to_string)
}

impl LiveSplitDialog {
    /// `data()` + `created()`: игра по умолчанию, если она есть, иначе первая; сплиты — прежние.
    pub fn new(
        games: Vec<LiveSplitGame>,
        default_game: Option<&str>,
        chosen: Map<String, Value>,
    ) -> LiveSplitDialog {
        let game_id = default_game
            .filter(|id| games.iter().any(|game| game.id == *id))
            .map(str::to_owned)
            .or_else(|| games.first().map(|game| game.id.clone()));
        let mut dialog = LiveSplitDialog {
            games,
            chosen,
            game_id,
            preset_id: None,
        };
        dialog.preset_id = dialog.remembered_preset();
        dialog
    }

    /// Выбранная игра.
    pub fn game(&self) -> Option<&LiveSplitGame> {
        self.games
            .iter()
            .find(|game| Some(&game.id) == self.game_id.as_ref())
    }

    /// Наборы выбранной игры.
    pub fn presets(&self) -> &[Value] {
        self.game()
            .map(|game| game.presets.as_slice())
            .unwrap_or(&[])
    }

    /// `rememberedPreset()`: выбранный раньше набор игры, если он есть, иначе первый.
    pub fn remembered_preset(&self) -> Option<String> {
        let saved = self.game().and_then(|game| {
            let key = game.game_id.clone().unwrap_or_else(|| "null".into());
            self.chosen.get(&key).cloned()
        });
        let presets = self.presets();
        if let Some(saved) = saved
            && presets
                .iter()
                .any(|preset| preset.get("id") == Some(&saved))
        {
            return Some(js::to_string(&saved));
        }
        presets.first().and_then(preset_id)
    }

    /// `pickGame(id)`.
    pub fn pick_game(&mut self, id: &str) {
        if self.game_id.as_deref() == Some(id) {
            return;
        }
        self.game_id = Some(id.to_owned());
        self.preset_id = self.remembered_preset();
    }

    /// `accept()`: ответ окна (без игры — нет ответа).
    pub fn accept(&self) -> Option<LiveSplitPick> {
        let game = self.game()?;
        Some(LiveSplitPick {
            game: game.id.clone(),
            game_id: game.game_id.clone(),
            preset: self.preset_id.clone(),
        })
    }

    /// Подпись набора: категория или вид (`kindLabel`), под категорией — вид мелко.
    pub fn preset_label(
        preset: &Value,
        tr: impl Fn(&str, &[&str]) -> String,
    ) -> (String, Option<String>) {
        let kind = tr(kind_label(preset.get("kind").and_then(Value::as_str)), &[]);
        match preset.get("category").filter(|value| js::truthy(value)) {
            Some(category) => (js::to_string(category), Some(kind)),
            None => (kind, None),
        }
    }
}

// ---- Выбор папки библиотеки -------------------------------------------------------------------

/// `freeSpaceGb`: свободное место в ГБ с одним знаком и запятой; нет — `None`.
pub fn free_space_gb(free_space_mb: f64) -> Option<String> {
    if !free_space_mb.is_finite() || free_space_mb <= 0.0 {
        return None;
    }
    Some(js::to_fixed(free_space_mb / 1024.0, 1).replace('.', ","))
}

/// Ключ ошибки пути: нет доступа или путь не подходит.
pub fn path_error_key(error: Option<&Value>) -> &'static str {
    if error.and_then(Value::as_str) == Some("no-access") {
        "#L_S_PATH_NO_ACCESS"
    } else {
        "#L_S_PATH_INVALID"
    }
}

/// Состояние окна выбора папки (`data()`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LibrarySetup {
    pub path: String,
    pub free_space_mb: f64,
    pub has_cyrillic: bool,
    /// Ключ ошибки (`#L_S_PATH_*`).
    pub error: Option<&'static str>,
}

impl LibrarySetup {
    pub fn new(initial_path: &str) -> LibrarySetup {
        LibrarySetup {
            path: initial_path.to_owned(),
            ..LibrarySetup::default()
        }
    }

    /// `freeSpaceGb`.
    pub fn free_space_gb(&self) -> Option<String> {
        free_space_gb(self.free_space_mb)
    }

    /// Ответ `validate-install-path` (`validate`): верно ли.
    pub fn apply_check(&mut self, check: &Value) -> bool {
        if !check.get("ok").is_some_and(js::truthy) {
            self.error = Some(path_error_key(check.get("error")));
            return false;
        }
        self.error = None;
        self.path = check.get("path").map(js::to_string).unwrap_or_default();
        let mb = js::value::to_number(check.get("freeSpaceMb"));
        self.free_space_mb = if mb.is_nan() { 0.0 } else { mb };
        self.has_cyrillic = check.get("hasCyrillic").is_some_and(js::truthy);
        true
    }

    /// `validate(target)`: проверить путь у main.
    pub async fn validate(services: &Services, state: &std::cell::RefCell<Self>, target: &str) {
        let reply = Self::start_validation(services, target);
        Self::accept_validation(state, reply.await);
    }

    /// Вызов `validate-install-path` (уходит сразу, ответ — по `await`).
    pub fn start_validation(services: &Services, target: &str) -> BoxFuture<'static, Reply> {
        services.invoke(
            "validate-install-path",
            vec![Value::String(target.to_owned())],
        )
    }

    /// Ответ `validate-install-path`: `check.ok` у пустого ответа — исключение, состояние не
    /// меняется.
    pub fn accept_validation(state: &std::cell::RefCell<Self>, reply: Reply) {
        if let Ok(check) = reply
            && js::truthy(&check)
        {
            state.borrow_mut().apply_check(&check);
        }
    }

    /// `choose()`: системный выбор папки, затем проверка.
    pub async fn choose(services: &Services, state: &std::cell::RefCell<Self>) {
        let current = state.borrow().path.clone();
        let reply = services.invoke("select-install-folder", vec![Value::String(current)]);
        let Ok(selected) = reply.await else {
            return;
        };
        if !js::truthy(&selected) {
            return;
        }
        Self::validate(services, state, &js::to_string(&selected)).await;
    }

    /// `confirm()`: записать путь; удачно — подтверждённый путь. 0.4.9: `busy` (идёт загрузка
    /// или запущена игра) — «папка занята».
    pub async fn confirm(services: &Services, state: &std::cell::RefCell<Self>) -> Option<String> {
        let path = state.borrow().path.clone();
        let reply = services.invoke("set-library-path", vec![Value::String(path)]);
        let result = reply.await.ok().filter(js::truthy)?;
        if !result.get("ok").is_some_and(js::truthy) {
            let error = result.get("error");
            state.borrow_mut().error = Some(if error.and_then(Value::as_str) == Some("busy") {
                "#L_S_PATH_BUSY"
            } else {
                path_error_key(error)
            });
            return None;
        }
        Some(js::to_template_string(result.get("path")))
    }
}
