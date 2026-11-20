//! Черновик окна настроек игры (`GameSettingsModal.vue`: `EMPTY`, `LAUNCH_TOGGLES`,
//! `GOLDSRC_TOGGLES`, `RESOLUTIONS`, `REFRESH_RATES`, `errorText`): то, что игрок правит на
//! вкладке «Запуск» и что уходит в `game-settings-save` обычной копией.

use super::super::jsv;
use hlsr_core::js;
use serde_json::{Map, Value, json};

/// Подпись пояснения под переключателем: свой текст (`hint`) или ключ перевода (`hintKey`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    Raw(&'static str),
    Key(&'static str),
}

/// Переключатель запуска (`LAUNCH_TOGGLES`, `GOLDSRC_TOGGLES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toggle {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: Hint,
    /// Показывается, только пока включён этот переключатель (`requires`).
    pub requires: Option<&'static str>,
}

const fn toggle(key: &'static str, label: &'static str, hint: &'static str) -> Toggle {
    Toggle {
        key,
        label,
        hint: Hint::Raw(hint),
        requires: None,
    }
}

/// Source и Ghosting.
pub const LAUNCH_TOGGLES: [Toggle; 8] = [
    toggle("console", "#L_GS_CONSOLE", "-console"),
    toggle("novid", "#L_GS_NOVID", "-novid"),
    toggle("borderless", "#L_GS_BORDERLESS", "-windowed -noborder"),
    toggle("dx9", "#L_GS_DX9", "-dx9"),
    toggle("nocrashdialog", "#L_GS_NOCRASHDIALOG", "-nocrashdialog"),
    toggle("noasync", "#L_GS_NOASYNC", "-noasync"),
    toggle("noaafonts", "#L_GS_NOAAFONTS", "-noaafonts"),
    toggle("high", "#L_GS_HIGH", "-high"),
];

/// HL1 (WON 2005): только то, что движок читает из командной строки; RInput — не аргумент,
/// лаунчер запускает его рядом с игрой.
pub const GOLDSRC_TOGGLES: [Toggle; 8] = [
    Toggle {
        key: "rinput",
        label: "#L_GS_RINPUT",
        hint: Hint::Key("#L_GS_RINPUT_HINT"),
        requires: None,
    },
    // Патч курсора в меню имеет смысл только вместе с RInput.
    Toggle {
        key: "rinputMenuFix",
        label: "#L_GS_RINPUT_MENUFIX",
        hint: Hint::Key("#L_GS_RINPUT_MENUFIX_HINT"),
        requires: Some("rinput"),
    },
    toggle("windowed", "#L_GS_WINDOWED", "-windowed"),
    toggle("console", "#L_GS_CONSOLE", "-console"),
    toggle("developer", "#L_GS_DEVELOPER", "-dev"),
    toggle("condebug", "#L_GS_CONDEBUG", "-condebug"),
    toggle("nojoy", "#L_GS_NOJOY", "-nojoy"),
    toggle("noaafonts", "#L_GS_NOAAFONTS", "-noaafonts"),
];

pub const RESOLUTIONS: [(u32, u32); 13] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 960),
    (1440, 1080),
    (1280, 720),
    (1366, 768),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
    (1680, 1050),
    (1920, 1200),
];

pub const REFRESH_RATES: [u32; 9] = [60, 75, 100, 120, 144, 165, 180, 240, 360];

/// Идентификатор Ghosting в лаунчере (`GHOSTING_GAME_ID`).
pub const GHOSTING_GAME_ID: &str = "218";
/// Игры на GoldSrc (`GOLDSRC_GAME_IDS`).
pub const GOLDSRC_GAME_IDS: [&str; 1] = ["70"];

/// Пункт списка: id и подпись (`{id, label}` `UiDropdown`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub label: String,
}

impl Item {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Item {
        Item {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// Перевод: ключ и аргументы → текст (`$t`).
pub type Tr<'a> = &'a dyn Fn(&str, &[&str]) -> String;

/// Ключи черновика в том порядке, в котором их перечисляет `Object.keys(EMPTY)`: `EMPTY`
/// собирается `reduce` с начальным объектом, поэтому переключатели запуска идут после него.
pub const KEYS: [&str; 27] = [
    "rinput",
    "rinputMenuFix",
    "windowed",
    "developer",
    "condebug",
    "nojoy",
    "width",
    "height",
    "refreshRate",
    "launchArgs",
    "protonOptions",
    "excludeCpu0",
    "startTimer",
    "vulkan",
    "language",
    "personalization",
    "packs",
    "bxt",
    "binds",
    "console",
    "novid",
    "borderless",
    "dx9",
    "nocrashdialog",
    "noasync",
    "noaafonts",
    "high",
];

/// `EMPTY`.
fn empty_values() -> Map<String, Value> {
    let mut map = Map::new();
    for key in KEYS {
        let value = match key {
            "rinput" => json!(true),
            "width" | "height" | "refreshRate" => json!(0),
            "launchArgs" | "protonOptions" => json!(""),
            "language" => json!("auto"),
            "personalization" | "packs" | "bxt" => Value::Null,
            "binds" => json!({}),
            _ => json!(false),
        };
        map.insert(key.to_owned(), value);
    }
    map
}

/// `defaultPersonalization()`.
pub fn default_personalization() -> Value {
    hlsr_delivery::ghosting_personalization::default_personalization().to_value()
}

/// `normalizePersonalization(value)`: сохранённая персонализация, дополненная значениями по
/// умолчанию (старые сохранения не знали новых полей); цвета — по каналам.
pub fn normalize_personalization(value: Option<&Value>) -> Value {
    let Some(Value::Object(own)) = value else {
        return default_personalization();
    };
    let Value::Object(mut result) = default_personalization() else {
        return Value::Null;
    };
    for (key, entry) in own {
        result.insert(key.clone(), entry.clone());
    }
    for (key, base) in [
        ("hudColor", json!({"r": 255, "g": 220, "b": 0})),
        ("crossColor", json!({"r": 0, "g": 255, "b": 0})),
    ] {
        let mut color = base.as_object().cloned().unwrap_or_default();
        if let Some(Value::Object(own_color)) = own.get(key) {
            for (channel, entry) in own_color {
                color.insert(channel.clone(), entry.clone());
            }
        }
        result.insert(key.to_owned(), Value::Object(color));
    }
    Value::Object(result)
}

/// Черновик настроек игры.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    values: Map<String, Value>,
}

impl Draft {
    /// `withPersonalization({})`.
    pub fn empty() -> Draft {
        Draft::from_settings(None)
    }

    /// `withPersonalization(settings)`: `Object.assign({}, EMPTY, settings || {})` и
    /// персонализация с умолчаниями. Чужие поля (`liveSplitOverlay`…) остаются в черновике,
    /// но в `game-settings-save` не уходят.
    pub fn from_settings(settings: Option<&Value>) -> Draft {
        let mut values = empty_values();
        if let Some(Value::Object(own)) = settings {
            for (key, value) in own {
                values.insert(key.clone(), value.clone());
            }
        }
        let personalization = normalize_personalization(values.get("personalization"));
        values.insert("personalization".into(), personalization);
        Draft { values }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.values.insert(key.to_owned(), value);
    }

    /// `!!draft[key]`.
    pub fn flag(&self, key: &str) -> bool {
        jsv::truthy(self.values.get(key))
    }

    /// Строка поля; другие значения — пустая строка (поле ввода показывает `v-model`).
    pub fn text(&self, key: &str) -> String {
        match self.values.get(key) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => js::to_string(other),
        }
    }

    /// `draft.width` / `draft.height` / `draft.refreshRate` как число (`0` — нет).
    pub fn number(&self, key: &str) -> f64 {
        jsv::or_zero(self.values.get(key))
    }

    /// Полезная нагрузка `game-settings-save`: ключи `EMPTY` по порядку; нет персонализации —
    /// умолчание.
    pub fn payload(&self) -> Value {
        let mut payload = Map::new();
        for key in KEYS {
            let value = self.values.get(key).cloned().unwrap_or(Value::Null);
            payload.insert(key.to_owned(), value);
        }
        if !jsv::truthy(payload.get("personalization")) {
            payload.insert("personalization".into(), default_personalization());
        }
        Value::Object(payload)
    }

    /// «Сбросить» (`resetDraft`): только то, что есть в окне; персонализация, паки и BXT
    /// остаются, переназначения клавиш становятся возвратами.
    pub fn reset(&mut self) {
        let binds = self.values.get("binds").cloned().unwrap_or(Value::Null);
        let reset = hlsr_delivery::game_binds::overrides_to_value(
            &hlsr_delivery::game_binds::reset_overrides(
                &hlsr_delivery::game_binds::sanitize_overrides(&binds),
            ),
        );
        let keep = |key: &str| self.values.get(key).cloned().unwrap_or(Value::Null);
        let (personalization, packs, bxt) = (keep("personalization"), keep("packs"), keep("bxt"));
        let mut next = Draft::from_settings(Some(&json!({
            "personalization": personalization,
            "packs": packs,
            "bxt": bxt,
        })));
        next.values.insert("binds".into(), reset);
        *self = next;
    }
}

/// Переключатели вкладки «Запуск» (`visibleLaunchToggles`).
pub fn visible_toggles(goldsrc: bool, draft: &Draft) -> Vec<&'static Toggle> {
    if !goldsrc {
        return LAUNCH_TOGGLES.iter().collect();
    }
    GOLDSRC_TOGGLES
        .iter()
        .filter(|toggle| toggle.requires.is_none_or(|key| draft.flag(key)))
        .collect()
}

/// `resolutionValue`: «ШxВ» или «auto».
pub fn resolution_value(draft: &Draft) -> String {
    let (width, height) = (draft.number("width"), draft.number("height"));
    if width != 0.0 && height != 0.0 && !width.is_nan() && !height.is_nan() {
        format!(
            "{}x{}",
            js::number::number_to_string(width),
            js::number::number_to_string(height)
        )
    } else {
        "auto".to_owned()
    }
}

/// `resolutionItems`: «Как в игре», стандартные разрешения и текущее, если его нет в списке.
pub fn resolution_items(draft: &Draft, tr: Tr<'_>) -> Vec<Item> {
    let mut items = vec![Item::new("auto", tr("#L_GS_RESOLUTION_AUTO", &[]))];
    for (w, h) in RESOLUTIONS {
        items.push(Item::new(format!("{w}x{h}"), format!("{w} × {h}")));
    }
    let current = resolution_value(draft);
    if !items.iter().any(|item| item.id == current) {
        items.push(Item::new(current.clone(), current.replacen('x', " × ", 1)));
    }
    items
}

/// `resolutionLabel`.
pub fn resolution_label(draft: &Draft, tr: Tr<'_>) -> String {
    let value = resolution_value(draft);
    resolution_items(draft, tr)
        .into_iter()
        .find(|item| item.id == value)
        .map_or(value, |item| item.label)
}

/// `setResolution(id)`: `/^(\d+)x(\d+)$/` → ширина и высота, иначе 0 × 0.
pub fn set_resolution(draft: &mut Draft, id: &str) {
    let parsed = id.split_once('x').and_then(|(w, h)| {
        let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
        (digits(w) && digits(h)).then(|| (js::to_number(w), js::to_number(h)))
    });
    let (width, height) = parsed.unwrap_or((0.0, 0.0));
    draft.set("width", jsv::number_value(width));
    draft.set("height", jsv::number_value(height));
}

/// `refreshItems`: «Как в игре» (0), частоты и текущая, если её нет в списке.
pub fn refresh_items(draft: &Draft, tr: Tr<'_>) -> Vec<Item> {
    let hz = |value: f64| tr("#L_GS_HZ", &[&js::number::number_to_string(value)]);
    let mut items = vec![Item::new("0", tr("#L_GS_RESOLUTION_AUTO", &[]))];
    for rate in REFRESH_RATES {
        items.push(Item::new(rate.to_string(), hz(f64::from(rate))));
    }
    let current = draft.number("refreshRate");
    let id = js::number::number_to_string(current);
    if !items.iter().any(|item| item.id == id) {
        items.push(Item::new(id, hz(current)));
    }
    items
}

/// `refreshLabel`: пустая, если текущей частоты нет в списке.
pub fn refresh_label(draft: &Draft, tr: Tr<'_>) -> String {
    let id = js::number::number_to_string(draft.number("refreshRate"));
    refresh_items(draft, tr)
        .into_iter()
        .find(|item| item.id == id)
        .map(|item| item.label)
        .unwrap_or_default()
}

/// Название языка игры (`LANGUAGE_NAMES`); незнакомый язык в списке не показывается.
pub fn language_name(id: &str) -> Option<&'static str> {
    match id {
        "english" => Some("English"),
        "russian" => Some("Русский"),
        _ => None,
    }
}

/// `languageItems`.
pub fn language_items(languages: &[String], tr: Tr<'_>) -> Vec<Item> {
    let mut items = vec![Item::new("auto", tr("#L_GS_LANG_AUTO", &[]))];
    for id in languages {
        if let Some(name) = language_name(id) {
            items.push(Item::new(id.clone(), name));
        }
    }
    items
}

/// `languageLabel`.
pub fn language_label(draft: &Draft, languages: &[String], tr: Tr<'_>) -> String {
    let current = draft.text("language");
    language_items(languages, tr)
        .into_iter()
        .find(|item| item.id == current)
        .map_or_else(|| tr("#L_GS_LANG_AUTO", &[]), |item| item.label)
}

/// Ошибка сохранения параметров запуска (`error` ответа `game-settings-save`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveError {
    pub code: Option<String>,
    pub param: Option<String>,
    pub field: Option<String>,
}

impl SaveError {
    /// `result.error`: `{code, param?, field?}`; не объект — пустая.
    pub fn from_value(value: Option<&Value>) -> SaveError {
        let text = |name: &str| {
            value
                .and_then(|value| jsv::prop(value, name))
                .filter(|value| jsv::truthy(Some(value)))
                .map(js::to_string)
        };
        SaveError {
            code: text("code"),
            param: text("param"),
            field: text("field"),
        }
    }
}

/// `errorText(error)`.
pub fn error_text(error: &SaveError, tr: Tr<'_>) -> String {
    let param = error.param.clone().unwrap_or_default();
    match error.code.as_deref() {
        Some("TOO_LONG") => tr("#L_GS_ERR_TOO_LONG", &[]),
        Some("CONTROL_CHARS") => tr("#L_GS_ERR_CONTROL_CHARS", &[]),
        Some("UNBALANCED_QUOTES") => tr("#L_GS_ERR_QUOTES", &[]),
        Some("FORBIDDEN_PARAM") => tr("#L_GS_ERR_FORBIDDEN", &[&param]),
        Some("PROTECTED_ENV") => tr("#L_GS_ERR_PROTECTED_ENV", &[&param]),
        Some("WRAPPER_INVALID") => tr("#L_GS_ERR_WRAPPER", &[&param]),
        _ => tr("#L_GS_ERR_INVALID", &[]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(key: &str, args: &[&str]) -> String {
        let mut text = key.to_owned();
        for arg in args {
            text = format!("{text}({arg})");
        }
        text
    }

    #[test]
    fn empty_draft_follows_empty_and_payload_keeps_its_key_order() {
        let draft = Draft::empty();
        assert!(draft.flag("rinput"));
        assert!(!draft.flag("console"));
        assert_eq!(draft.text("language"), "auto");
        assert_eq!(draft.get("binds"), Some(&json!({})));
        // personalization по умолчанию — не null.
        assert_eq!(
            draft.get("personalization").unwrap()["crossColor"],
            json!({"r": 0, "g": 255, "b": 0})
        );
        let payload = draft.payload();
        let keys: Vec<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        // serde_json без preserve_order сортирует ключи — состав тот же, что у EMPTY.
        assert_eq!(keys.len(), KEYS.len());
        for key in KEYS {
            assert!(payload.get(key).is_some(), "{key}");
        }
    }

    #[test]
    fn settings_from_main_override_defaults_and_keep_foreign_fields_out_of_the_payload() {
        let draft = Draft::from_settings(Some(&json!({
            "console": true,
            "launchArgs": "+fps_max 300",
            "liveSplitOverlay": {"enabled": true},
            "personalization": {"hudColor": {"r": 1}, "extra": {"gh_hud_bg": true}},
        })));
        assert!(draft.flag("console"));
        assert_eq!(draft.text("launchArgs"), "+fps_max 300");
        let personalization = draft.get("personalization").unwrap();
        // Недостающие каналы цвета дополняются (старые сохранения).
        assert_eq!(
            personalization["hudColor"],
            json!({"r": 1, "g": 220, "b": 0})
        );
        assert_eq!(personalization["extra"], json!({"gh_hud_bg": true}));
        assert_eq!(personalization["crossPreset"], "standard");
        assert!(draft.get("liveSplitOverlay").is_some());
        assert!(draft.payload().get("liveSplitOverlay").is_none());
    }

    #[test]
    fn goldsrc_menu_fix_needs_rinput() {
        let mut draft = Draft::empty();
        let keys = |draft: &Draft| {
            visible_toggles(true, draft)
                .iter()
                .map(|toggle| toggle.key)
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&draft)[..2], ["rinput", "rinputMenuFix"]);
        draft.set("rinput", json!(false));
        assert_eq!(keys(&draft)[0], "rinput");
        assert!(!keys(&draft).contains(&"rinputMenuFix"));
        assert_eq!(keys(&draft).len(), 7);
        assert_eq!(visible_toggles(false, &draft).len(), 8);
        assert_eq!(visible_toggles(false, &draft)[0].key, "console");
    }

    #[test]
    fn resolution_round_trip_and_unknown_current() {
        let mut draft = Draft::empty();
        assert_eq!(resolution_value(&draft), "auto");
        assert_eq!(resolution_label(&draft, &tr), "#L_GS_RESOLUTION_AUTO");
        set_resolution(&mut draft, "1280x720");
        assert_eq!(
            (draft.number("width"), draft.number("height")),
            (1280.0, 720.0)
        );
        assert_eq!(resolution_label(&draft, &tr), "1280 × 720");
        // Текущее разрешение вне списка добавляется в конец.
        set_resolution(&mut draft, "1000x700");
        let items = resolution_items(&draft, &tr);
        assert_eq!(items.len(), 1 + RESOLUTIONS.len() + 1);
        assert_eq!(items.last().unwrap(), &Item::new("1000x700", "1000 × 700"));
        assert_eq!(resolution_label(&draft, &tr), "1000 × 700");
        // Не «ШxВ» — сброс в 0 × 0.
        set_resolution(&mut draft, "auto");
        assert_eq!((draft.number("width"), draft.number("height")), (0.0, 0.0));
        set_resolution(&mut draft, "12x");
        assert_eq!(resolution_value(&draft), "auto");
    }

    #[test]
    fn refresh_rate_items_and_label() {
        let mut draft = Draft::empty();
        assert_eq!(refresh_label(&draft, &tr), "#L_GS_RESOLUTION_AUTO");
        draft.set("refreshRate", json!(144));
        assert_eq!(refresh_label(&draft, &tr), "#L_GS_HZ(144)");
        draft.set("refreshRate", json!(90));
        let items = refresh_items(&draft, &tr);
        assert_eq!(items.len(), 1 + REFRESH_RATES.len() + 1);
        assert_eq!(items.last().unwrap(), &Item::new("90", "#L_GS_HZ(90)"));
        assert_eq!(refresh_label(&draft, &tr), "#L_GS_HZ(90)");
    }

    #[test]
    fn game_language_lists_only_known_languages() {
        let languages = vec![
            "english".to_owned(),
            "french".to_owned(),
            "russian".to_owned(),
        ];
        let items = language_items(&languages, &tr);
        assert_eq!(
            items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["auto", "english", "russian"]
        );
        let mut draft = Draft::empty();
        assert_eq!(language_label(&draft, &languages, &tr), "#L_GS_LANG_AUTO");
        draft.set("language", json!("russian"));
        assert_eq!(language_label(&draft, &languages, &tr), "Русский");
        // Языка, которого нет в списке, — «как в Steam».
        draft.set("language", json!("french"));
        assert_eq!(language_label(&draft, &languages, &tr), "#L_GS_LANG_AUTO");
        // Нет второго пункта — строки «Язык игры» нет (languageAvailable).
        assert_eq!(language_items(&[], &tr).len(), 1);
    }

    #[test]
    fn save_errors_map_to_texts() {
        let case = |value: Value| error_text(&SaveError::from_value(Some(&value)), &tr);
        assert_eq!(case(json!({"code": "TOO_LONG"})), "#L_GS_ERR_TOO_LONG");
        assert_eq!(
            case(json!({"code": "CONTROL_CHARS"})),
            "#L_GS_ERR_CONTROL_CHARS"
        );
        assert_eq!(
            case(json!({"code": "UNBALANCED_QUOTES"})),
            "#L_GS_ERR_QUOTES"
        );
        assert_eq!(
            case(json!({"code": "FORBIDDEN_PARAM", "param": "-steam"})),
            "#L_GS_ERR_FORBIDDEN(-steam)"
        );
        assert_eq!(
            case(json!({"code": "PROTECTED_ENV", "param": "WINEPREFIX"})),
            "#L_GS_ERR_PROTECTED_ENV(WINEPREFIX)"
        );
        assert_eq!(
            case(json!({"code": "WRAPPER_INVALID", "param": "-x"})),
            "#L_GS_ERR_WRAPPER(-x)"
        );
        assert_eq!(case(json!({"code": "OTHER"})), "#L_GS_ERR_INVALID");
        // error: undefined / null — общий текст (`errorText(null)`).
        assert_eq!(
            error_text(&SaveError::from_value(None), &tr),
            "#L_GS_ERR_INVALID"
        );
        // FORBIDDEN_PARAM без param — `error.param || ""`.
        assert_eq!(
            case(json!({"code": "FORBIDDEN_PARAM"})),
            "#L_GS_ERR_FORBIDDEN()"
        );
    }

    #[test]
    fn reset_keeps_personalization_and_turns_overrides_into_returns() {
        let mut draft = Draft::from_settings(Some(&json!({
            "console": true,
            "launchArgs": "-x",
            "width": 800,
            "height": 600,
            "personalization": {"hudEnabled": true},
            "packs": {"hud": "ag"},
            "bxt": {"fov": 100},
            "binds": {"F5": {"command": "save", "base": "quicksave"}},
        })));
        draft.reset();
        assert!(!draft.flag("console"));
        assert_eq!(draft.text("launchArgs"), "");
        assert_eq!(draft.number("width"), 0.0);
        assert_eq!(draft.get("personalization").unwrap()["hudEnabled"], true);
        assert_eq!(draft.get("packs"), Some(&json!({"hud": "ag"})));
        assert_eq!(draft.get("bxt"), Some(&json!({"fov": 100})));
        // Переназначение стало возвратом на то, что было в игре.
        assert_eq!(
            draft.get("binds").unwrap()["F5"],
            json!({"command": "quicksave", "base": "quicksave"})
        );
    }
}
