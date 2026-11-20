//! Редактор конфигов во вкладке «Конфиги» настроек игры (`shared/game-config.js`): понятные
//! настройки («Чувствительность», «Громкость») вместо переменных движка.
//!
//! Значение берётся оттуда, где движок задаёт его последним: цепочка cfg выполняется так же,
//! как при запуске игры (config.cfg, потом autoexec.cfg или userconfig.cfg, со всеми exec).
//! Новое значение пишется в ту же строку того же файла: правка остаётся у игрока на своём
//! месте, с комментарием. Если переменная нигде не задана или задана не в файле игрока, она
//! пишется в раздел settings файла игрока (userconfig.cfg / autoexec.cfg, 0.4.9): он выполняется
//! последним и перекрывает config.cfg. config.cfg и WON.cfg лаунчер не правит.
//!
//! Переменные, которые пишет персонализация лаунчера (прицел, HUD, FOV, настройки Ghosting),
//! здесь не показываются: их значения всё равно перекрывает hlsr-personalization.cfg.
//!
//! Значения по умолчанию — из эталонных cfg: config.cfg пакета Half-Life 2005 WON для HL1,
//! стоковые значения движка для Source. Для HL1 здесь только переменные из белого списка
//! категорий speedrun.com/hl1 (brightness по таблице разрешён с 07.11, но в каталог этой вкладки не входит).
//!
//! Смещения в строке ([`Command::start`], [`Command::end`]) — байты UTF-8, а не единицы UTF-16
//! как у JS: границы команд всегда на ASCII (`"`, `;`, `//`), срезы строки совпадают.

use hlsr_core::js::number::{number_to_string, to_fixed};
use hlsr_core::js::{self, math_round};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Файл настроек лаунчера 0.4.7. В 0.4.9 его нет (`CONFIG_FILE` убран): правки идут в файл
/// игрока. Константу читает только редактор конфигов `hlsr-ui` до перехода на 0.4.9 (задача G5).
pub const CONFIG_FILE: &str = "hlsr-config.cfg";

/// Файлы, значения из которых не правятся на месте: WON.cfg пакета HL1 ставит fps_max 100 и
/// clockwindow 0, без которых ран не засчитают («Don't mess with this»). Имена — в нижнем
/// регистре.
pub const GENERATED_FILES: [&str; 1] = ["won.cfg"];

/// Порядок, в котором движок выполняет cfg игрока при запуске (`ENGINE_ROOTS`). В GoldSrc
/// autoexec.cfg идёт раньше config.cfg, поэтому Valve и добавила userconfig.cfg. Движок вне
/// списка — `None`.
pub fn engine_roots(engine: &str) -> Option<&'static [&'static str]> {
    match engine {
        "goldsrc" => Some(&["autoexec.cfg", "config.cfg", "userconfig.cfg"]),
        "source" | "source-old" => Some(&["config.cfg", "autoexec.cfg"]),
        _ => None,
    }
}

/// Файл, в конец которого ставится `exec hlsr-config.cfg` (`EXEC_FILES`).
pub fn exec_file(engine: &str) -> Option<&'static str> {
    match engine {
        "goldsrc" => Some("userconfig.cfg"),
        "source" | "source-old" => Some("autoexec.cfg"),
        _ => None,
    }
}

/// Подпись на двух языках (`L`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label {
    pub ru: &'static str,
    pub en: &'static str,
}

const fn l(ru: &'static str, en: &'static str) -> Label {
    Label { ru, en }
}

/// Раздел окна (`SECTIONS`).
#[derive(Debug)]
pub struct Section {
    pub id: &'static str,
    pub icon: &'static str,
    pub title: Label,
}

pub static SECTIONS: [Section; 4] = [
    Section {
        id: "controls",
        icon: "fa-mouse",
        title: l("Управление", "Controls"),
    },
    Section {
        id: "video",
        icon: "fa-desktop",
        title: l("Изображение", "Video"),
    },
    Section {
        id: "audio",
        icon: "fa-volume-up",
        title: l("Звук", "Audio"),
    },
    Section {
        id: "game",
        icon: "fa-gamepad",
        title: l("Игра", "Game"),
    },
];

/// Вид настройки (`type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// Число с ползунком (`min`, `max`, `step`).
    Range,
    /// Доля 0…1 в cfg, проценты 0…100 в окне.
    Percent,
    /// 0/1.
    Toggle,
    /// Знак `m_pitch`: инверсия мыши без потери величины.
    Invert,
    /// Один из вариантов `options`.
    Select,
}

/// Значение по умолчанию в единицах cfg (`default`): число или булево.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldDefault {
    Num(f64),
    Bool(bool),
}

/// Значение в единицах окна: булево (переключатель, инверсия) или число.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowValue {
    Bool(bool),
    Num(f64),
}

/// Вариант выбора (`options`).
#[derive(Debug, Clone, Copy)]
pub struct SelectOption {
    pub value: f64,
    pub label: Label,
}

/// Настройка. `archived` — движок сам сохраняет переменную в config.cfg. `skip` — игры, где
/// переменной нет или её пишет персонализация.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub cvar: &'static str,
    pub section: &'static str,
    pub kind: FieldType,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub default: FieldDefault,
    pub engines: &'static [&'static str],
    pub archived: bool,
    pub skip: &'static [&'static str],
    pub label: Label,
    pub hint: Option<Label>,
    /// Подписи краёв ползунка.
    pub scale: Option<[Label; 2]>,
    pub options: &'static [SelectOption],
    /// Переменная, без которой эта настройка не действует.
    pub requires: Option<&'static str>,
}

const GOLDSRC: &[&str] = &["goldsrc"];
const SOURCE: &[&str] = &["source", "source-old"];
const ALL: &[&str] = &["goldsrc", "source", "source-old"];

const BASE: Field = Field {
    cvar: "",
    section: "",
    kind: FieldType::Toggle,
    min: None,
    max: None,
    step: None,
    default: FieldDefault::Bool(false),
    engines: ALL,
    archived: true,
    skip: &[],
    label: l("", ""),
    hint: None,
    scale: None,
    options: &[],
    requires: None,
};

const fn option(value: f64, label: Label) -> SelectOption {
    SelectOption { value, label }
}

const OFF: SelectOption = option(0.0, l("Выключено", "Off"));

pub static FIELDS: &[Field] = &[
    Field {
        cvar: "sensitivity",
        section: "controls",
        kind: FieldType::Range,
        min: Some(0.1),
        max: Some(10.0),
        step: Some(0.01),
        default: FieldDefault::Num(3.0),
        label: l("Чувствительность", "Sensitivity"),
        hint: Some(l(
            "Скорость поворота камеры мышью.",
            "How fast the mouse turns the camera.",
        )),
        ..BASE
    },
    Field {
        cvar: "zoom_sensitivity_ratio",
        section: "controls",
        kind: FieldType::Range,
        min: Some(0.1),
        max: Some(4.0),
        step: Some(0.05),
        default: FieldDefault::Num(1.0),
        engines: SOURCE,
        label: l("Чувствительность в приближении", "Zoomed sensitivity"),
        hint: Some(l(
            "Множитель чувствительности при приближении костюма и прицела.",
            "Sensitivity multiplier while zoomed in.",
        )),
        ..BASE
    },
    Field {
        cvar: "m_pitch",
        section: "controls",
        kind: FieldType::Invert,
        default: FieldDefault::Bool(false),
        label: l("Инверсия мыши", "Invert mouse"),
        hint: Some(l(
            "Мышь от себя — взгляд вниз.",
            "Pushing the mouse forward looks down.",
        )),
        ..BASE
    },
    Field {
        cvar: "m_filter",
        section: "controls",
        kind: FieldType::Toggle,
        default: FieldDefault::Bool(false),
        label: l("Сглаживание мыши", "Mouse smoothing"),
        hint: Some(l(
            "Усредняет движение за два кадра. Плавнее, но с задержкой.",
            "Averages movement over two frames: smoother, but delayed.",
        )),
        ..BASE
    },
    Field {
        cvar: "m_customaccel",
        section: "controls",
        kind: FieldType::Toggle,
        default: FieldDefault::Bool(false),
        engines: SOURCE,
        label: l("Ускорение мыши", "Mouse acceleration"),
        hint: Some(l(
            "Чем быстрее движение, тем дальше поворот.",
            "Faster movement turns further.",
        )),
        ..BASE
    },
    Field {
        cvar: "fps_max",
        section: "video",
        kind: FieldType::Range,
        min: Some(20.0),
        max: Some(100.0),
        step: Some(1.0),
        default: FieldDefault::Num(100.0),
        engines: GOLDSRC,
        label: l("Ограничение FPS", "FPS limit"),
        hint: Some(l(
            "От FPS в Half-Life зависит физика. По правилам WON выше 100 нельзя.",
            "In Half-Life the physics depend on FPS. WON rules allow at most 100.",
        )),
        ..BASE
    },
    Field {
        cvar: "fps_max",
        section: "video",
        kind: FieldType::Range,
        min: Some(30.0),
        max: Some(999.0),
        step: Some(1.0),
        default: FieldDefault::Num(300.0),
        engines: SOURCE,
        label: l("Ограничение FPS", "FPS limit"),
        ..BASE
    },
    Field {
        cvar: "gamma",
        section: "video",
        kind: FieldType::Range,
        min: Some(1.8),
        max: Some(3.0),
        step: Some(0.05),
        default: FieldDefault::Num(2.5),
        engines: GOLDSRC,
        label: l("Гамма", "Gamma"),
        hint: Some(l(
            "Меняется после перезапуска игры.",
            "Takes effect after restarting the game.",
        )),
        ..BASE
    },
    Field {
        cvar: "mat_monitorgamma",
        section: "video",
        kind: FieldType::Range,
        min: Some(1.6),
        max: Some(2.6),
        step: Some(0.05),
        default: FieldDefault::Num(2.2),
        engines: SOURCE,
        label: l("Яркость", "Brightness"),
        scale: Some([l("Светлее", "Brighter"), l("Темнее", "Darker")]),
        ..BASE
    },
    Field {
        cvar: "cl_showfps",
        section: "video",
        kind: FieldType::Select,
        default: FieldDefault::Num(0.0),
        engines: SOURCE,
        archived: false,
        skip: &["ghosting-renewed"],
        label: l("Счётчик FPS", "FPS counter"),
        options: &[
            OFF,
            option(1.0, l("FPS", "FPS")),
            option(2.0, l("FPS и время кадра", "FPS and frame time")),
        ],
        ..BASE
    },
    Field {
        cvar: "net_graph",
        section: "video",
        kind: FieldType::Select,
        default: FieldDefault::Num(0.0),
        engines: GOLDSRC,
        label: l("Сетевой график", "Net graph"),
        hint: Some(l(
            "Показывает FPS, пинг и потери пакетов в углу экрана.",
            "Shows FPS, ping and packet loss in a corner of the screen.",
        )),
        options: &[
            OFF,
            option(1.0, l("Кратко", "Brief")),
            option(2.0, l("Подробно", "Detailed")),
            option(3.0, l("Только цифры", "Numbers only")),
        ],
        ..BASE
    },
    Field {
        cvar: "volume",
        section: "audio",
        kind: FieldType::Percent,
        default: FieldDefault::Num(1.0),
        engines: SOURCE,
        label: l("Общая громкость", "Master volume"),
        ..BASE
    },
    Field {
        cvar: "volume",
        section: "audio",
        kind: FieldType::Percent,
        default: FieldDefault::Num(0.4),
        engines: GOLDSRC,
        label: l("Общая громкость", "Master volume"),
        ..BASE
    },
    Field {
        cvar: "snd_musicvolume",
        section: "audio",
        kind: FieldType::Percent,
        default: FieldDefault::Num(1.0),
        engines: &["source"],
        label: l("Громкость музыки", "Music volume"),
        ..BASE
    },
    Field {
        cvar: "MP3Volume",
        section: "audio",
        kind: FieldType::Percent,
        default: FieldDefault::Num(0.8),
        engines: GOLDSRC,
        label: l("Громкость музыки", "Music volume"),
        ..BASE
    },
    Field {
        cvar: "suitvolume",
        section: "audio",
        kind: FieldType::Percent,
        default: FieldDefault::Num(0.25),
        engines: GOLDSRC,
        label: l("Голос костюма HEV", "HEV suit voice"),
        ..BASE
    },
    Field {
        cvar: "closecaption",
        section: "audio",
        kind: FieldType::Toggle,
        default: FieldDefault::Bool(false),
        engines: SOURCE,
        label: l("Субтитры", "Captions"),
        ..BASE
    },
    Field {
        cvar: "cc_subtitles",
        section: "audio",
        kind: FieldType::Toggle,
        default: FieldDefault::Bool(false),
        engines: SOURCE,
        requires: Some("closecaption"),
        label: l("Только речь", "Dialogue only"),
        hint: Some(l(
            "Без подписей к звукам.",
            "No captions for sound effects.",
        )),
        ..BASE
    },
    Field {
        cvar: "hud_fastswitch",
        section: "controls",
        kind: FieldType::Toggle,
        default: FieldDefault::Bool(false),
        label: l("Быстрая смена оружия", "Fast weapon switch"),
        hint: Some(l(
            "Оружие берётся сразу, без меню выбора.",
            "Weapons are switched at once, without the selection menu.",
        )),
        ..BASE
    },
    Field {
        cvar: "skill",
        section: "game",
        kind: FieldType::Select,
        default: FieldDefault::Num(1.0),
        engines: SOURCE,
        label: l("Сложность", "Difficulty"),
        options: &[
            option(1.0, l("Лёгкая", "Easy")),
            option(2.0, l("Средняя", "Normal")),
            option(3.0, l("Сложная", "Hard")),
        ],
        ..BASE
    },
    Field {
        cvar: "crosshair",
        section: "game",
        kind: FieldType::Toggle,
        default: FieldDefault::Bool(true),
        engines: SOURCE,
        label: l("Прицел", "Crosshair"),
        ..BASE
    },
    Field {
        cvar: "hud_quickinfo",
        section: "game",
        kind: FieldType::Toggle,
        default: FieldDefault::Bool(true),
        engines: SOURCE,
        skip: &["ghosting-renewed"],
        label: l("Индикаторы у прицела", "Crosshair indicators"),
        hint: Some(l(
            "Дуги здоровья и патронов вокруг прицела.",
            "Health and ammo arcs around the crosshair.",
        )),
        ..BASE
    },
];

/// `key(name)`: имя команды без учёта регистра.
fn key(name: &str) -> String {
    name.to_lowercase()
}

/// Раздел окна с настройками сборки.
#[derive(Debug)]
pub struct CatalogSection {
    pub section: &'static Section,
    pub fields: Vec<&'static Field>,
}

/// Настройки сборки: по движку и без тех, что пишет персонализация (`catalogFor`).
#[derive(Debug)]
pub struct Catalog {
    pub engine: String,
    pub sections: Vec<CatalogSection>,
    pub fields: Vec<&'static Field>,
    by_cvar: HashMap<String, &'static Field>,
}

impl Catalog {
    /// `catalog.byCvar.get(cvar)`: имя — в нижнем регистре.
    pub fn by_cvar(&self, lowercase_cvar: &str) -> Option<&'static Field> {
        self.by_cvar.get(lowercase_cvar).copied()
    }
}

/// `catalogFor(gameId, engine)`: `None` — движок не поддерживается.
pub fn catalog_for(game_id: &str, engine: &str) -> Option<Catalog> {
    engine_roots(engine)?;
    let fields: Vec<&'static Field> = FIELDS
        .iter()
        .filter(|field| field.engines.contains(&engine) && !field.skip.contains(&game_id))
        .collect();
    let sections = SECTIONS
        .iter()
        .map(|section| CatalogSection {
            section,
            fields: fields
                .iter()
                .copied()
                .filter(|field| field.section == section.id)
                .collect(),
        })
        .filter(|section| !section.fields.is_empty())
        .collect();
    let by_cvar = fields
        .iter()
        .map(|field| (key(field.cvar), *field))
        .collect();
    Some(Catalog {
        engine: engine.to_owned(),
        sections,
        fields,
        by_cvar,
    })
}

/// `round(value, step)`: число до знаков шага (не больше 6).
fn round(value: f64, step: f64) -> f64 {
    let step = if step == 0.0 { 1.0 } else { step };
    let digits = (-step.log10()).ceil().clamp(0.0, 6.0) as usize;
    js::to_number(&to_fixed(value, digits))
}

/// `/^[+-]?(\d+\.?\d*|\.\d+)$/` для текста без пробелов по краям.
fn is_plain_decimal(text: &str) -> bool {
    let rest = text.strip_prefix(['+', '-']).unwrap_or(text).as_bytes();
    let digits = |from: usize| {
        rest[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let whole = digits(0);
    if whole > 0 {
        let mut end = whole;
        if rest.get(end) == Some(&b'.') {
            end += 1;
        }
        end += digits(end);
        end == rest.len()
    } else {
        rest.first() == Some(&b'.') && digits(1) > 0 && 1 + digits(1) == rest.len()
    }
}

/// `numberOf(raw)` для текста: десятичная запись без порядка, иначе `NaN`.
fn number_of_text(raw: &str) -> f64 {
    let text = js::trim(raw);
    if is_plain_decimal(text) {
        js::to_number(text)
    } else {
        f64::NAN
    }
}

/// `numberOf(value)` для значения из окна: число (конечное) как есть, остальное — по тексту
/// `String(value)`.
fn number_of_value(value: &Value) -> f64 {
    match value {
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite()).unwrap_or(f64::NAN),
        other => number_of_text(&js::to_string(other)),
    }
}

/// Значение из cfg → значение в окне. `None` — в cfg что-то не то (или значения нет), и в окне
/// показывается значение по умолчанию. Значение cfg — строка (`fromCfg`).
pub fn from_cfg(field: &Field, raw: Option<&str>) -> Option<WindowValue> {
    let n = number_of_text(raw?);
    if n.is_nan() {
        return None;
    }
    match field.kind {
        FieldType::Toggle => Some(WindowValue::Bool(n != 0.0)),
        FieldType::Invert => Some(WindowValue::Bool(n < 0.0)),
        FieldType::Percent => Some(WindowValue::Num(math_round(n * 100.0).clamp(0.0, 100.0))),
        FieldType::Range => Some(WindowValue::Num(n)),
        FieldType::Select => field
            .options
            .iter()
            .find(|item| item.value == n)
            .map(|item| WindowValue::Num(item.value)),
    }
}

/// Значение по умолчанию в единицах окна (`defaultValue`).
pub fn default_value(field: &Field) -> WindowValue {
    match (field.kind, field.default) {
        (FieldType::Percent, FieldDefault::Num(x)) => WindowValue::Num(math_round(x * 100.0)),
        (_, FieldDefault::Num(x)) => WindowValue::Num(x),
        (_, FieldDefault::Bool(b)) => WindowValue::Bool(b),
    }
}

/// `formatNumber(value)`: `String(Number(Number(value).toFixed(6)))`.
fn format_number(value: f64) -> String {
    number_to_string(js::to_number(&to_fixed(value, 6)))
}

/// Значение из окна → строка для cfg; `None` — значение не годится (`toCfg`). `previous` — что
/// стоит в cfg сейчас: у инверсии сохраняется величина `m_pitch`.
pub fn to_cfg(field: &Field, value: &Value, previous: Option<&str>) -> Option<String> {
    match field.kind {
        FieldType::Toggle => value
            .as_bool()
            .map(|on| if on { "1" } else { "0" }.to_owned()),
        FieldType::Invert => {
            let invert = value.as_bool()?;
            let magnitude = previous.map_or(f64::NAN, number_of_text).abs();
            let magnitude = if magnitude.is_nan() || magnitude == 0.0 {
                0.022
            } else {
                magnitude
            };
            Some(format_number(if invert { -magnitude } else { magnitude }))
        }
        FieldType::Percent => {
            let n = number_of_value(value);
            if n.is_nan() || !(0.0..=100.0).contains(&n) {
                return None;
            }
            Some(format_number(math_round(n) / 100.0))
        }
        FieldType::Range => {
            let n = number_of_value(value);
            let (min, max) = (field.min?, field.max?);
            if n.is_nan() || n < min || n > max {
                return None;
            }
            Some(format_number(round(n, field.step.unwrap_or(1.0))))
        }
        FieldType::Select => {
            let n = number_of_value(value);
            field
                .options
                .iter()
                .any(|item| item.value == n)
                .then(|| format_number(n))
        }
    }
}

/// Слово строки cfg: значение без кавычек и было ли оно в кавычках.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub value: String,
    pub quoted: bool,
}

/// Команда строки cfg с её местом в строке: `[start, end)` — байты строки, `args` — слова.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub start: usize,
    pub end: usize,
    pub args: Vec<Token>,
}

/// `tokens(text)`: `/"([^"]*)"?|(\S+)/g`.
fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let ch = text[at..].chars().next().expect("есть символ");
        if js::is_whitespace(ch) {
            at += ch.len_utf8();
        } else if ch == '"' {
            let start = at + 1;
            let end = text[start..].find('"').map_or(text.len(), |p| start + p);
            out.push(Token {
                value: text[start..end].to_owned(),
                quoted: true,
            });
            at = if end < text.len() { end + 1 } else { end };
        } else {
            let length: usize = text[at..]
                .chars()
                .take_while(|c| !js::is_whitespace(*c))
                .map(char::len_utf8)
                .sum();
            out.push(Token {
                value: text[at..at + length].replace('"', ""),
                quoted: false,
            });
            at += length;
        }
    }
    out
}

/// Команды одной строки cfg с их местом в строке. Разбор как у движка: команды делит «;» вне
/// кавычек, «//» вне кавычек — комментарий до конца строки, кавычки не экранируются и
/// закрываются концом строки (`lineCommands`). Команды без слов не возвращаются.
pub fn line_commands(line: &str) -> Vec<Command> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut end = line.len();
    let bytes = line.as_bytes();
    for (i, ch) in line.char_indices() {
        if quoted {
            if ch == '"' {
                quoted = false;
            }
            continue;
        }
        if ch == '"' {
            quoted = true;
        } else if ch == '/' && bytes.get(i + 1) == Some(&b'/') {
            end = i;
            break;
        } else if ch == ';' {
            spans.push((start, i));
            start = i + 1;
        }
    }
    spans.push((start, end));
    spans
        .into_iter()
        .map(|(start, end)| Command {
            start,
            end,
            args: tokens(&line[start..end]),
        })
        .filter(|command| !command.args.is_empty())
        .collect()
}

/// `String(text || "").split("\n")`.
fn split_lines(text: &str) -> std::str::Split<'_, char> {
    text.split('\n')
}

/// Где переменная задана последней (`found[verb]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub value: String,
    /// Имя файла так, как оно написано в exec.
    pub file: String,
    pub line: usize,
    /// Номер команды в строке (среди команд со словами).
    pub index: usize,
    /// Файл лаунчера или WON.cfg: на месте не правится.
    pub generated: bool,
}

/// Итог выполнения цепочки cfg.
#[derive(Debug, Default)]
pub struct Executed {
    /// Имя переменной в нижнем регистре → последнее место, где она задана.
    pub found: HashMap<String, Hit>,
    /// Прочитанные файлы в порядке выполнения.
    pub files: Vec<String>,
}

/// `/\.[a-z0-9]+$/i`: у имени есть расширение.
fn has_extension(file: &str) -> bool {
    let bytes = file.as_bytes();
    let tail = bytes
        .iter()
        .rev()
        .take_while(|b| b.is_ascii_alphanumeric())
        .count();
    tail > 0 && tail < bytes.len() && bytes[bytes.len() - tail - 1] == b'.'
}

/// `/^[A-Za-z0-9._-]+\.cfg$/`: имя cfg без папок (расширение — только строчными).
fn is_plain_cfg_name(file: &str) -> bool {
    file.len() > 4
        && file.ends_with(".cfg")
        && file[..file.len() - 4]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

struct Run<'a> {
    read_file: &'a mut dyn FnMut(&str) -> Option<String>,
    wanted: HashSet<String>,
    visited: HashSet<String>,
    out: Executed,
}

impl Run<'_> {
    fn run(&mut self, name: &str, depth: usize) {
        let mut file = name.to_owned();
        if !has_extension(&file) {
            file.push_str(".cfg");
        }
        let lower = file.to_lowercase();
        if depth > 8 || self.visited.contains(&lower) {
            return;
        }
        if !is_plain_cfg_name(&file) {
            return;
        }
        let Some(text) = (self.read_file)(&file) else {
            return;
        };
        self.visited.insert(lower.clone());
        self.out.files.push(file.clone());
        let generated = GENERATED_FILES.contains(&lower.as_str());
        for (line, raw_line) in split_lines(&text).enumerate() {
            let raw_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            for (index, command) in line_commands(raw_line).iter().enumerate() {
                let verb = key(&command.args[0].value);
                if verb == "exec" && command.args.len() >= 2 {
                    self.run(&command.args[1].value, depth + 1);
                } else if self.wanted.contains(&verb) && command.args.len() >= 2 {
                    self.out.found.insert(
                        verb,
                        Hit {
                            value: command.args[1].value.clone(),
                            file: file.clone(),
                            line,
                            index,
                            generated,
                        },
                    );
                }
            }
        }
    }
}

/// Выполняет cfg по порядку (`roots`), как движок, и запоминает, где каждая переменная из
/// `cvars` задана последней. `read_file(имя)` — текст файла или `None` (`execute`).
pub fn execute(
    read_file: &mut dyn FnMut(&str) -> Option<String>,
    roots: &[&str],
    cvars: &[&str],
) -> Executed {
    let mut run = Run {
        read_file,
        wanted: cvars.iter().map(|cvar| key(cvar)).collect(),
        visited: HashSet::new(),
        out: Executed::default(),
    };
    for root in roots {
        run.run(root, 0);
    }
    run.out
}

/// Итог [`import_values`].
#[derive(Debug, Default)]
pub struct Imported {
    /// Последнее значение каждой переменной каталога в единицах окна, в порядке появления.
    pub values: Vec<(&'static str, WindowValue)>,
    /// Переменные, чьё значение не понято.
    pub skipped: Vec<&'static str>,
}

/// Импорт чужого cfg во вкладку: последнее значение каждой переменной каталога в единицах окна.
/// `text` — команды в порядке выполнения (цепочка exec уже раскрыта, см. `cfg_import`). Число
/// вне диапазона окна прижимается к краю, непонятное значение пропускается (`importValues`).
pub fn import_values(catalog: &Catalog, text: &str) -> Imported {
    let mut raw: Vec<(String, String)> = Vec::new();
    for line in split_lines(text) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        for command in line_commands(line) {
            let verb = key(&command.args[0].value);
            if catalog.by_cvar.contains_key(&verb) && command.args.len() >= 2 {
                let value = command.args[1].value.clone();
                match raw.iter_mut().find(|(name, _)| *name == verb) {
                    Some(entry) => entry.1 = value,
                    None => raw.push((verb, value)),
                }
            }
        }
    }
    let mut out = Imported::default();
    for (cvar, value) in raw {
        let field = catalog.by_cvar[&cvar];
        let Some(mut next) = from_cfg(field, Some(&value)) else {
            out.skipped.push(field.cvar);
            continue;
        };
        if let (FieldType::Range, WindowValue::Num(n), Some(min), Some(max)) =
            (field.kind, next, field.min, field.max)
        {
            let n = round(n, field.step.unwrap_or(1.0));
            next = WindowValue::Num(max.min(n).max(min));
        }
        out.values.push((field.cvar, next));
    }
    out
}

/// Новое значение на месте старого: та же строка, та же команда, отступы, комментарий остаются
/// (написание имени — как у слова, без кавычек). `None` — такой строки или команды нет
/// (`replaceAt`).
pub fn replace_at(text: &str, line: usize, index: usize, value: &str) -> Option<String> {
    let mut lines: Vec<&str> = split_lines(text).collect();
    let raw = *lines.get(line)?;
    let (body, cr) = match raw.strip_suffix('\r') {
        Some(body) => (body, "\r"),
        None => (raw, ""),
    };
    let command = line_commands(body).into_iter().nth(index)?;
    if command.args.len() < 2 {
        return None;
    }
    let segment = &body[command.start..command.end];
    let lead_end = segment
        .char_indices()
        .find(|(_, c)| !js::is_whitespace(*c))
        .map_or(segment.len(), |(at, _)| at);
    let trail_start = segment
        .char_indices()
        .rev()
        .take_while(|(_, c)| js::is_whitespace(*c))
        .last()
        .map_or(segment.len(), |(at, _)| at);
    let (lead, trail) = (&segment[..lead_end], &segment[trail_start..]);
    let quoted = command.args[1].quoted || value.chars().any(js::is_whitespace);
    let value = if quoted {
        format!("\"{value}\"")
    } else {
        value.to_owned()
    };
    let next = format!("{lead}{} {value}{trail}", command.args[0].value);
    let replaced = format!(
        "{}{next}{}{cr}",
        &body[..command.start],
        &body[command.end..]
    );
    lines[line] = &replaced;
    Some(lines.join("\n"))
}

/// Строка в конец файла с тем же переводом строки, что в файле (`appendLine`). `default_eol`
/// пустой или не задан — `\n`.
pub fn append_line(text: &str, line: &str, default_eol: Option<&str>) -> String {
    let eol = if text.contains("\r\n") {
        "\r\n"
    } else if text.contains('\n') {
        "\n"
    } else {
        default_eol.filter(|eol| !eol.is_empty()).unwrap_or("\n")
    };
    let suffix = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        eol
    };
    format!("{text}{suffix}{line}{eol}")
}
