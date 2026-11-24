//! Сплиты LiveSplit для каждой игры (`livesplit-splits.js`). У Ghosting это файлы из архива
//! LiveSplit (по главам и по картам), для остальных игр лаунчер пишет файлы сам в
//! `splits/hlsr/<gameId>/` папки LiveSplit. Файл пишется, только если его ещё нет: LiveSplit
//! хранит в нём личные рекорды и историю попыток.
//!
//! Категории — как на speedrun.com: HL1 WON (Scripted / Scriptless), HL2 Old Engine для 2707 и
//! 4231–4260 (With Scripts / Scriptless) и New Engine для 5288 (Any% / Any% No Voidclip). Сплиты
//! HL2 ставит SourceSplit (настройки в самих сплитах), HL1 — BunnySplit через Bunnymod XT
//! (компонент в раскладке).
//!
//! LiveSplit открывает сплиты и раскладку из аргументов `-s` и `-l`. Пути относительные, от
//! папки LiveSplit: на Linux он запущен под Wine, и относительный путь не нужно переводить в путь
//! Wine. Тексты `.lss` и `.lsl` — с `\r\n` и меткой порядка байтов у `.lss`, как их пишет 0.4.7.
//!
//! BunnySplit (`resources/livesplit-components`) собран из github.com/YaLTeR/LiveSplit.BunnySplit
//! (8024493) Roslyn (`csc.exe` из Visual Studio Build Tools) против сборок .NET Framework 4.8.1,
//! с `LiveSplit.Core.dll` и `UpdateManager.dll` из архива LiveSplit; команда сборки — в
//! `livesplit-splits.js` 0.4.9. Прежняя сборка mcs из wine-mono падала на .NET Framework
//! (`MissingMethodException` в `SetSettings`): такую копию лаунчер заменяет, чужую версию — нет.

use crate::atomic::write_file_atomic;
use crate::errors::RawError;
use crate::file_ops::hex;
use crate::game_binds::Encoding;
use crate::node_fs;
use crate::ports::Logger;
use hlsr_core::js::path::NodePath;
use regress::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

/// `COMPONENTS_DIRECTORY`: папка компонентов в каталоге ресурсов.
pub const COMPONENTS_DIRECTORY: &str = "livesplit-components";
/// `BUNNYSPLIT`.
pub const BUNNYSPLIT: &str = "LiveSplit.BunnySplit.dll";
const BUNNYSPLIT_VERSION: &str = "2.0.2.0";
/// `BUNNYSPLIT_SHA256`: поставляемый BunnySplit.
pub const BUNNYSPLIT_SHA256: &str =
    "8e76fd5b006638b6e4bd76881ffaaff34bc36553a33b7bef92b061340445a3aa";
/// `BUNNYSPLIT_BROKEN_SHA256`: копии, собранные mcs, — их лаунчер заменяет.
pub const BUNNYSPLIT_BROKEN_SHA256: &[&str] =
    &["0c6a8d199f8ef6c5e3bc32239569f017568220b03314a5424a56acd714279794"];
const SETTINGS_FILE: &str = "settings.cfg";
/// Автосплиттер Ghosting (тот же файл лежит в открытом репозитории для LiveSplit.AutoSplitters).
pub const GHOSTING_ASL: &str = "GhostingRenewed.asl";
const GHOSTING_ASL_TEXT: &str = include_str!("../data/livesplit/GhostingRenewed.asl");

const HL1_CHAPTERS: &[&str] = &[
    "Anomalous Materials",
    "Unforeseen Consequences",
    "Office Complex",
    "We've Got Hostiles",
    "Blast Pit",
    "Power Up",
    "On A Rail",
    "Apprehension",
    "Residue Processing",
    "Questionable Ethics",
    "Surface Tension",
    "Forget About Freeman",
    "Lambda Core",
    "Xen",
    "Gonarch's Lair",
    "Interloper",
    "Nihilanth",
];

const HL2_CHAPTERS: &[&str] = &[
    "Point Insertion",
    "A Red Letter Day",
    "Route Kanal",
    "Water Hazard",
    "Black Mesa East",
    "We Don't Go To Ravenholm",
    "Highway 17",
    "Sandtraps",
    "Nova Prospekt",
    "Entanglement",
    "Anticitizen One",
    "Follow Freeman!",
    "Our Benefactors",
    "Dark Energy",
];

/// Карты с подсплитами по главам, как в сплитах Ghosting (`splits/maps`).
const HL2_MAPS: &[&str] = &[
    "-d1_trainstation_01",
    "-d1_trainstation_02",
    "-d1_trainstation_03",
    "{Point Insertion} d1_trainstation_04",
    "-d1_trainstation_05",
    "{A Red Letter Day} d1_trainstation_06",
    "-d1_canals_01",
    "-d1_canals_01a",
    "-d1_canals_02",
    "-d1_canals_03",
    "{Route Kanal} d1_canals_05",
    "-d1_canals_06",
    "-d1_canals_07",
    "-d1_canals_08",
    "-d1_canals_09",
    "-d1_canals_10",
    "-d1_canals_11",
    "-d1_canals_12",
    "{Water Hazard} d1_canals_13",
    "-d1_eli_01",
    "{Black Mesa East} d1_eli_02",
    "-d1_town_01",
    "-d1_town_01a",
    "-d1_town_02",
    "-d1_town_02a",
    "-d1_town_04",
    "{We Don't Go To Ravenholm} d1_town_05",
    "-d2_coast_01",
    "-d2_coast_03",
    "-d2_coast_04",
    "-d2_coast_05",
    "{Highway 17} d2_coast_07",
    "-d2_coast_09",
    "-d2_coast_10",
    "-d2_coast_11",
    "-d2_coast_12",
    "{Sandtraps} d2_prison_01",
    "-d2_prison_02",
    "-d2_prison_03",
    "-d2_prison_04",
    "{Nova Prospekt} d2_prison_05",
    "-d2_prison_06",
    "-d2_prison_07",
    "-d2_prison_08",
    "{Entanglement} d3_c17_01",
    "-d3_c17_02",
    "-d3_c17_03",
    "-d3_c17_04",
    "-d3_c17_05",
    "-d3_c17_06a",
    "-d3_c17_06b",
    "-d3_c17_07",
    "{Anticitizen One} d3_c17_08",
    "-d3_c17_09",
    "-d3_c17_10a",
    "-d3_c17_10b",
    "-d3_c17_11",
    "-d3_c17_12",
    "-d3_c17_12b",
    "{Follow Freeman!} d3_c17_13",
    "-d3_citadel_01",
    "-d3_citadel_02",
    "-d3_citadel_03",
    "-d3_citadel_04",
    "{Our Benefactors} d3_citadel_05",
    "{Dark Energy} Dark Energy",
];

type Entries = Vec<(&'static str, &'static str)>;

/// Настройки SourceSplit из сплитов Ghosting: по главам сплит только на переходах между главами
/// (список MTL), по картам — на каждом переходе.
fn source_split_settings(kind: Kind) -> Entries {
    let chapters = kind == Kind::Chapters;
    let pick = |if_chapters: &'static str, if_maps: &'static str| {
        if chapters { if_chapters } else { if_maps }
    };
    vec![
        ("Version", pick("3.3.4.6", "3.3.4.5")),
        ("AutoSplitEnabled", "True"),
        ("SplitInterval", "1"),
        ("UseSplitInterval", "True"),
        ("UseMTL", "True"),
        ("MTLModeSetting", pick("Allow", "Disallow")),
        ("AutoSplitOnLevelTrans", "True"),
        ("AutoSplitOnGenericMap", "False"),
        ("AutoSplitOnSpecial", "True"),
        ("AutoStartEnabled", "True"),
        ("AutoStopEnabled", "True"),
        ("AutoResetEnabled", "True"),
        ("FirstMapAutoReset", "True"),
        ("StartMap", ""),
        ("AddAutoStartType", "NewGame"),
        ("AllowAdditionalAutoStart", "False"),
        ("ShowGameTime", "True"),
        ("ShowAltTime", "False"),
        ("ShowTickCount", "False"),
        ("ShowCurDemo", "False"),
        ("GameTimeDecimalPlaces", "6"),
        ("HoldUntilPause", "True"),
        ("RTAStartOffset", "True"),
        ("ServerInitialTicks", "False"),
        ("SLPenalty", pick("1", "0")),
        ("SplitInstead", "False"),
        ("ResetMapTransitions", "False"),
        ("CountEngineTicks", "True"),
        ("CountPauses", "True"),
        ("CountDisconnects", "False"),
        ("CountInactive", "False"),
        ("CountAutomatic", "True"),
        ("CountDemoInterop", "False"),
        ("PrintDemoInfo", "False"),
        ("DemoParserPath", ""),
        ("SetGameTimeOnLaunch", "True"),
        (
            "MapTransitionList",
            pick(
                "d1_trainstation_06,d1_canals_01|d1_trainstation_04,d1_trainstation_05|d1_canals_05,d1_canals_06|\
                 d1_canals_13,d1_eli_01|d1_eli_02,d1_town_01|d1_town_05,d2_coast_01|d2_coast_07,d2_coast_09|\
                 d2_prison_01,d2_prison_02|d3_c17_01,d3_c17_02|d2_prison_05,d2_prison_06|d3_c17_08,d3_c17_09|\
                 d3_c17_13,d3_citadel_01|d3_citadel_05,d3_breen_01",
                "d1_canals_07,d1_canals_08",
            ),
        ),
    ]
}

/// BunnySplit: сплит при первом входе в главу HL1 и на смерти Нихиланта, старт и сброс — по
/// таймеру BXT.
fn bunnysplit_settings() -> Entries {
    vec![
        ("Version", BUNNYSPLIT_VERSION),
        ("EnableAutoSplit", "True"),
        ("SplitOnGameEnd", "True"),
        ("SplitOnHL1Chapters", "True"),
        ("SplitOnOP4Chapters", "False"),
        ("SplitOnBSChapters", "False"),
        ("SplitOnGMCChapters", "False"),
        ("SplitOnMaps", "False"),
        ("EnableAutoReset", "True"),
        ("EnableAutoStart", "True"),
    ]
}

/// Вид набора: по главам или по картам (и базовая раскладка Ghosting того же вида).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Chapters,
    Maps,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Chapters => "chapters",
            Kind::Maps => "maps",
        }
    }

    /// `BASE_LAYOUTS[kind]`: раскладка из архива LiveSplit, на её основе пишутся раскладки игр.
    fn base_layout(self) -> &'static str {
        match self {
            Kind::Chapters => "splits/chapters/chapters.lsl",
            Kind::Maps => "splits/maps/maps.lsl",
        }
    }
}

/// Автосплиттер набора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Autosplitter {
    BunnySplit,
    SourceSplit,
}

/// Готовые файлы из архива LiveSplit (у Ghosting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bundled {
    pub splits: &'static str,
    pub layout: &'static str,
}

/// Набор сплитов игры.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub kind: Kind,
    /// Категория speedrun.com; у наборов из архива LiveSplit её нет.
    pub category: Option<&'static str>,
    pub bundled: Option<Bundled>,
    game_name: &'static str,
    category_name: &'static str,
    variables: Vec<(&'static str, &'static str)>,
    segments: &'static [&'static str],
    autosplitter: Option<Autosplitter>,
}

fn hl1(id: &'static str, scripts: &'static str) -> Preset {
    Preset {
        id,
        kind: Kind::Chapters,
        category: Some(scripts),
        bundled: None,
        game_name: "Half-Life",
        category_name: "WON",
        variables: vec![("Scripts", scripts), ("Segments", "RTA")],
        segments: HL1_CHAPTERS,
        autosplitter: Some(Autosplitter::BunnySplit),
    }
}

fn hl2(
    id: &'static str,
    kind: Kind,
    category: &'static str,
    category_name: &'static str,
    variables: Vec<(&'static str, &'static str)>,
) -> Preset {
    Preset {
        id,
        kind,
        category: Some(category),
        bundled: None,
        game_name: "Half-Life 2",
        category_name,
        variables,
        segments: if kind == Kind::Chapters {
            HL2_CHAPTERS
        } else {
            HL2_MAPS
        },
        autosplitter: Some(Autosplitter::SourceSplit),
    }
}

fn old_engine(build: &'static str) -> Vec<Preset> {
    let variables = |scripts| vec![("Scripts", scripts), ("Build", build)];
    let with = "With Scripts";
    let without = "Scriptless";
    let name = "Old Engine";
    vec![
        hl2(
            "scripts-chapters",
            Kind::Chapters,
            with,
            name,
            variables(with),
        ),
        hl2(
            "scriptless-chapters",
            Kind::Chapters,
            without,
            name,
            variables(without),
        ),
        hl2("scripts-maps", Kind::Maps, with, name, variables(with)),
        hl2(
            "scriptless-maps",
            Kind::Maps,
            without,
            name,
            variables(without),
        ),
    ]
}

fn new_engine(build: &'static str) -> Vec<Preset> {
    let variables = |voidclip| vec![("With/Without Voidclip", voidclip), ("Build", build)];
    let any = "Any%";
    let no_voidclip = "Any% No Voidclip";
    let name = "New Engine";
    vec![
        hl2("any-chapters", Kind::Chapters, any, name, variables(any)),
        hl2(
            "novoidclip-chapters",
            Kind::Chapters,
            no_voidclip,
            name,
            variables(no_voidclip),
        ),
        hl2("any-maps", Kind::Maps, any, name, variables(any)),
        hl2(
            "novoidclip-maps",
            Kind::Maps,
            no_voidclip,
            name,
            variables(no_voidclip),
        ),
    ]
}

fn bundled(id: &'static str, kind: Kind, splits: &'static str, layout: &'static str) -> Preset {
    Preset {
        id,
        kind,
        category: None,
        bundled: Some(Bundled { splits, layout }),
        game_name: "",
        category_name: "",
        variables: Vec::new(),
        segments: &[],
        autosplitter: None,
    }
}

/// `PRESETS`: наборы по играм, в порядке 0.4.7.
static PRESETS: LazyLock<Vec<(&'static str, Vec<Preset>)>> = LazyLock::new(|| {
    vec![
        // Готовые файлы из архива LiveSplit (в них же рекорды прежних версий).
        (
            "ghosting-renewed",
            vec![
                bundled(
                    "chapters",
                    Kind::Chapters,
                    "splits/chapters/chapters.lss",
                    "splits/chapters/chapters.lsl",
                ),
                bundled(
                    "maps",
                    Kind::Maps,
                    "splits/maps/maps.lss",
                    "splits/maps/maps.lsl",
                ),
            ],
        ),
        (
            "hl1-won",
            vec![hl1("scripted", "Scripted"), hl1("scriptless", "Scriptless")],
        ),
        ("hl2-2707", old_engine("2707")),
        ("hl2-4260", old_engine("4231-4260")),
        ("hl2-5288", new_engine("5288")),
    ]
});

/// `presetsOf(gameId)`: наборы игры; чужой идентификатор — пусто.
pub fn presets_of(game_id: &str) -> &'static [Preset] {
    PRESETS
        .iter()
        .find(|(id, _)| *id == game_id)
        .map_or(&[], |(_, presets)| presets.as_slice())
}

/// `presetOf(gameId, presetId)`: набор по id, без него (или с чужим id) — первый набор игры.
pub fn preset_of(game_id: &str, preset_id: Option<&str>) -> Option<&'static Preset> {
    let presets = presets_of(game_id);
    presets
        .iter()
        .find(|preset| Some(preset.id) == preset_id)
        .or_else(|| presets.first())
}

/// `describe(gameId)`: для окна выбора — id, вид (главы или карты) и категория speedrun.com.
pub fn describe(game_id: &str) -> Vec<Value> {
    presets_of(game_id)
        .iter()
        .map(|preset| json!({"id": preset.id, "kind": preset.kind.as_str(), "category": preset.category}))
        .collect()
}

/// Пути сплитов и раскладки внутри папки LiveSplit (через `/`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    pub splits: String,
    pub layout: String,
}

/// `filesOf(gameId, preset)`.
pub fn files_of(game_id: &str, preset: &Preset) -> Files {
    if let Some(bundled) = &preset.bundled {
        return Files {
            splits: bundled.splits.to_owned(),
            layout: bundled.layout.to_owned(),
        };
    }
    let base = format!("splits/hlsr/{game_id}/{}", preset.id);
    Files {
        splits: format!("{base}.lss"),
        layout: format!("{base}.lsl"),
    }
}

/// Пути для аргументов LiveSplit — через `\`: так их пишет и сам LiveSplit.
fn windows_path(relative: &str) -> String {
    relative.replace('/', "\\")
}

/// `escapeXml(value)`.
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn settings_xml(entries: &[(&str, &str)], indent: &str) -> String {
    entries
        .iter()
        .map(|(name, value)| {
            if value.is_empty() {
                format!("{indent}<{name}>\r\n{indent}</{name}>")
            } else {
                format!("{indent}<{name}>{}</{name}>", escape_xml(value))
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// `splitsXml(preset)`: файл `.lss` набора (с меткой порядка байтов в начале).
pub fn splits_xml(preset: &Preset) -> String {
    let variables = preset
        .variables
        .iter()
        .map(|(name, value)| {
            format!(
                "      <Variable name=\"{}\">{}</Variable>",
                escape_xml(name),
                escape_xml(value)
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    let segments = preset
        .segments
        .iter()
        .map(|name| {
            [
                "    <Segment>".to_owned(),
                format!("      <Name>{}</Name>", escape_xml(name)),
                "      <Icon />".to_owned(),
                "      <SplitTimes>".to_owned(),
                "        <SplitTime name=\"Personal Best\" />".to_owned(),
                "      </SplitTimes>".to_owned(),
                "      <BestSegmentTime />".to_owned(),
                "      <SegmentHistory />".to_owned(),
                "    </Segment>".to_owned(),
            ]
            .join("\r\n")
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    let autosplitter = if preset.autosplitter == Some(Autosplitter::SourceSplit) {
        format!(
            "  <AutoSplitterSettings>\r\n{}\r\n    <Misc />\r\n  </AutoSplitterSettings>",
            settings_xml(&source_split_settings(preset.kind), "    ")
        )
    } else {
        "  <AutoSplitterSettings />".to_owned()
    };
    let variables = if variables.is_empty() {
        "    <Variables />".to_owned()
    } else {
        format!("    <Variables>\r\n{variables}\r\n    </Variables>")
    };
    [
        "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_owned(),
        "<Run version=\"1.7.0\">".to_owned(),
        "  <GameIcon />".to_owned(),
        format!("  <GameName>{}</GameName>", escape_xml(preset.game_name)),
        format!(
            "  <CategoryName>{}</CategoryName>",
            escape_xml(preset.category_name)
        ),
        "  <LayoutPath>".to_owned(),
        "  </LayoutPath>".to_owned(),
        "  <Metadata>".to_owned(),
        "    <Run id=\"\" />".to_owned(),
        "    <Platform usesEmulator=\"False\">PC</Platform>".to_owned(),
        "    <Region>".to_owned(),
        "    </Region>".to_owned(),
        variables,
        "    <CustomVariables />".to_owned(),
        "  </Metadata>".to_owned(),
        "  <Offset>00:00:00</Offset>".to_owned(),
        "  <AttemptCount>0</AttemptCount>".to_owned(),
        "  <AttemptHistory />".to_owned(),
        "  <Segments>".to_owned(),
        segments,
        "  </Segments>".to_owned(),
        autosplitter,
        "</Run>".to_owned(),
        String::new(),
    ]
    .join("\r\n")
}

/// `layoutXml(base, preset)`: раскладка игры — раскладка Ghosting того же вида; для HL1 в неё
/// добавляется BunnySplit. В базовой раскладке нет `</Components>` — `None`.
pub fn layout_xml(base: &str, preset: &Preset) -> Option<String> {
    if preset.autosplitter != Some(Autosplitter::BunnySplit) {
        return Some(base.to_owned());
    }
    let component = [
        "    <Component>".to_owned(),
        format!("      <Path>{BUNNYSPLIT}</Path>"),
        "      <Settings>".to_owned(),
        settings_xml(&bunnysplit_settings(), "        "),
        "      </Settings>".to_owned(),
        "    </Component>".to_owned(),
        "  </Components>".to_owned(),
    ]
    .join("\r\n");
    const CLOSING: &str = "</Components>";
    let end = base.rfind(CLOSING)?;
    let line_start = base[..end].rfind('\n').map_or(0, |at| at + 1);
    Some(format!(
        "{}{component}{}",
        &base[..line_start],
        &base[end + CLOSING.len()..]
    ))
}

/// Регулярное выражение 0.4.7 (`regress`: `\s` и `\d` как в JS).
fn js_regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("выражение 0.4.7 разбирается")
}

static RECENT_SPLITS_EMPTY: LazyLock<Regex> = LazyLock::new(|| js_regex(r"<RecentSplits\s*/>"));
static ACTIVE_AUTOSPLITTERS_EMPTY: LazyLock<Regex> =
    LazyLock::new(|| js_regex(r"<ActiveAutoSplitters\s*/>"));
static SERVER_STARTUP: LazyLock<Regex> =
    LazyLock::new(|| js_regex(r"<ServerStartup>(\d+)</ServerStartup>"));
static SERVER_STATE: LazyLock<Regex> =
    LazyLock::new(|| js_regex(r"<ServerState>(\d+)</ServerState>"));
static SERVER_PORT: LazyLock<Regex> = LazyLock::new(|| js_regex(r"<ServerPort>\d*</ServerPort>"));
static SETTINGS_END: LazyLock<Regex> = LazyLock::new(|| js_regex(r"</Settings>\s*$"));

/// `text.replace(regex, replacement)` без особых знаков в замене: первое вхождение.
fn replace_first(text: &str, regex: &Regex, replacement: &str) -> String {
    match regex.find(text) {
        Some(found) => format!(
            "{}{replacement}{}",
            &text[..found.start()],
            &text[found.end()..]
        ),
        None => text.to_owned(),
    }
}

/// `join` по правилам `path` этой ОС.
fn join(parts: &[&str]) -> PathBuf {
    PathBuf::from(NodePath::native().join(parts))
}

fn text_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Итог [`LiveSplitSplits::prepare`]: аргументы LiveSplit и набор.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub args: Vec<String>,
    pub preset: Option<&'static Preset>,
}

/// `LiveSplitSplits`: запись сплитов, раскладки и компонентов в папку LiveSplit.
pub struct LiveSplitSplits {
    resources_path: PathBuf,
    logger: Arc<dyn Logger>,
    broken_sha256: Vec<String>,
}

impl LiveSplitSplits {
    /// `new LiveSplitSplits({ resourcesPath, logger })`.
    pub fn new(resources_path: impl Into<PathBuf>, logger: Arc<dyn Logger>) -> LiveSplitSplits {
        LiveSplitSplits {
            resources_path: resources_path.into(),
            logger,
            broken_sha256: BUNNYSPLIT_BROKEN_SHA256
                .iter()
                .map(|sha| (*sha).to_owned())
                .collect(),
        }
    }

    /// `options.brokenSha256` (самотесты).
    pub fn with_broken_sha256(mut self, broken: Vec<String>) -> LiveSplitSplits {
        self.broken_sha256 = broken;
        self
    }

    /// `writeIfMissing(file, content)`: файл есть — не трогается.
    fn write_if_missing(&self, file: &Path, content: &str) -> Result<bool, RawError> {
        if file.exists() {
            return Ok(false);
        }
        if let Some(dir) = file.parent() {
            node_fs::mkdir_recursive(dir)?;
        }
        write_file_atomic(file, content.as_bytes())?;
        Ok(true)
    }

    /// Автосплиттер Ghosting в `Components` (время берётся из игры, см. сам файл): его можно
    /// выбрать в «Scriptable Auto Splitter» с любым именем игры, в том числе «Half-Life 2:
    /// Category Extensions». Файл наш — обновляется, если отличается.
    pub fn install_ghosting_asl(&self, root: &Path) -> Result<bool, RawError> {
        let target = join(&[&text_of(root), "Components", GHOSTING_ASL]);
        if node_fs::read_bytes(&target).is_ok_and(|bytes| bytes == GHOSTING_ASL_TEXT.as_bytes()) {
            return Ok(false);
        }
        if let Some(dir) = target.parent() {
            node_fs::mkdir_recursive(dir)?;
        }
        write_file_atomic(&target, GHOSTING_ASL_TEXT.as_bytes())?;
        self.logger
            .info(&format!("[tools] livesplit: {GHOSTING_ASL} установлен"));
        Ok(true)
    }

    /// Таймер раскладки Ghosting — с миллисекундами (просьба владельца 24.11: итог как 49:01.790).
    /// Меняется только формат LiveSplit по умолчанию (`1.23`), свой выбор игрока остаётся.
    pub fn timer_milliseconds(&self, layout: &Path) -> Result<bool, RawError> {
        const HUNDREDTHS: &str = "<TimerFormat>1.23</TimerFormat>";
        let Ok(text) = node_fs::read_text(layout, Encoding::Utf8) else {
            return Ok(false);
        };
        if !text.contains(HUNDREDTHS) {
            return Ok(false);
        }
        let next = text.replace(HUNDREDTHS, "<TimerFormat>1.234</TimerFormat>");
        write_file_atomic(layout, next.as_bytes())?;
        Ok(true)
    }

    /// `installBunnySplit(root)`: BunnySplit в `Components`, если его там нет или это известная
    /// сломанная сборка: версию, которую пользователь положил сам, лаунчер не трогает.
    pub fn install_bunny_split(&self, root: &Path) -> Result<bool, RawError> {
        let source = join(&[
            &text_of(&self.resources_path),
            COMPONENTS_DIRECTORY,
            BUNNYSPLIT,
        ]);
        let target = join(&[&text_of(root), "Components", BUNNYSPLIT]);
        if !source.exists() {
            return Err(RawError::new(format!(
                "{BUNNYSPLIT} не найден в ресурсах лаунчера"
            )));
        }
        let exists = target.exists();
        if exists {
            let digest = hex(&Sha256::digest(node_fs::read_bytes(&target)?));
            if !self.broken_sha256.contains(&digest) {
                return Ok(false);
            }
        }
        let temporary = PathBuf::from(format!("{}.{}.tmp", text_of(&target), std::process::id()));
        let copied = node_fs::copy_file(&source, &temporary).and_then(|()| {
            std::fs::rename(&temporary, &target).map_err(|error| {
                RawError::fs_with_dest(&error, "rename", &temporary, Some(&target))
            })
        });
        // Временный файл не критичен; после переноса его уже нет.
        let _ = std::fs::remove_file(&temporary);
        copied?;
        let digest = hex(&Sha256::digest(node_fs::read_bytes(&target)?));
        self.logger.info(&format!(
            "[tools] livesplit: {} {BUNNYSPLIT} ({})",
            if exists {
                "заменён сломанный BunnySplit, установлен"
            } else {
                "установлен"
            },
            &digest[..12]
        ));
        Ok(true)
    }

    /// `seedSettings(root, preset, splitsPath)`: сплиты новые для LiveSplit, в `settings.cfg` они
    /// попадают с игровым временем, иначе LiveSplit показал бы реальное. Автосплиттер Half-Life 2
    /// (SourceSplit) включается, если его выключили. Нет файла — ничего не делается.
    fn seed_settings(
        &self,
        root: &Path,
        preset: &Preset,
        splits_path: &str,
    ) -> Result<bool, RawError> {
        let file = join(&[&text_of(root), SETTINGS_FILE]);
        let Ok(text) = node_fs::read_text(&file, Encoding::Utf8) else {
            return Ok(false);
        };
        let mut next = text.clone();
        let splits_xml_path = escape_xml(splits_path);
        let entry = format!(
            "<SplitsFile gameName=\"{}\" categoryName=\"{}\" lastTimingMethod=\"GameTime\" lastHotkeyProfile=\"Default\">{splits_xml_path}</SplitsFile>",
            escape_xml(preset.game_name),
            escape_xml(preset.category_name),
        );
        if !next.contains(&format!(">{splits_xml_path}</SplitsFile>")) {
            if RECENT_SPLITS_EMPTY.find(&next).is_some() {
                next = replace_first(
                    &next,
                    &RECENT_SPLITS_EMPTY,
                    &format!("<RecentSplits>\r\n    {entry}\r\n  </RecentSplits>"),
                );
            } else if next.contains("<RecentSplits>") {
                next = next.replacen(
                    "<RecentSplits>",
                    &format!("<RecentSplits>\r\n    {entry}"),
                    1,
                );
            }
        }
        if preset.autosplitter == Some(Autosplitter::SourceSplit)
            && !next.contains(&format!(
                "<AutoSplitter>{}</AutoSplitter>",
                preset.game_name
            ))
        {
            let item = format!(
                "<AutoSplitter>{}</AutoSplitter>",
                escape_xml(preset.game_name)
            );
            if ACTIVE_AUTOSPLITTERS_EMPTY.find(&next).is_some() {
                next = replace_first(
                    &next,
                    &ACTIVE_AUTOSPLITTERS_EMPTY,
                    &format!("<ActiveAutoSplitters>\r\n    {item}\r\n  </ActiveAutoSplitters>"),
                );
            } else if next.contains("<ActiveAutoSplitters>") {
                next = next.replacen(
                    "<ActiveAutoSplitters>",
                    &format!("<ActiveAutoSplitters>\r\n    {item}"),
                    1,
                );
            }
        }
        if next == text {
            return Ok(false);
        }
        write_file_atomic(&file, next.as_bytes())?;
        Ok(true)
    }

    /// `enableServer(root, port)`: «время из LiveSplit» в Ghosting (`gh_timer_mirror`) спрашивает
    /// время у LiveSplit Server по TCP на 127.0.0.1. Сервер встроен в LiveSplit, но по умолчанию
    /// выключен, а под Proton меню Control найти непросто. Перед запуском LiveSplit включается
    /// старт TCP-сервера и порт из настроек игры. Открытый LiveSplit перезапишет файл при выходе,
    /// поэтому только до старта. `ServerStartup`: 0 — выкл, 1 — TCP, 2 — WebSocket, 3 — как в
    /// прошлый раз.
    pub fn enable_server(&self, root: &Path, port: i64) -> Result<bool, RawError> {
        let file = join(&[&text_of(root), SETTINGS_FILE]);
        let Ok(text) = node_fs::read_text(&file, Encoding::Utf8) else {
            return Ok(false);
        };
        let mut next = text.clone();
        let startup = SERVER_STARTUP.find(&next).map(|found| {
            (
                next[found.range()].to_owned(),
                found
                    .group(1)
                    .map(|range| next[range].to_owned())
                    .unwrap_or_default(),
            )
        });
        let state = SERVER_STATE
            .find(&next)
            .and_then(|found| found.group(1).map(|range| next[range].to_owned()));
        let starts_tcp = startup.as_ref().is_some_and(|(_, value)| {
            value == "1" || (value == "3" && state.as_deref() == Some("1"))
        });
        if !starts_tcp {
            next = match &startup {
                Some((matched, _)) => {
                    next.replacen(matched.as_str(), "<ServerStartup>1</ServerStartup>", 1)
                }
                None => replace_first(
                    &next,
                    &SETTINGS_END,
                    "  <ServerStartup>1</ServerStartup>\r\n</Settings>\r\n",
                ),
            };
        }
        let port_tag = format!("<ServerPort>{port}</ServerPort>");
        next = if SERVER_PORT.find(&next).is_some() {
            replace_first(&next, &SERVER_PORT, &port_tag)
        } else {
            replace_first(
                &next,
                &SETTINGS_END,
                &format!("  {port_tag}\r\n</Settings>\r\n"),
            )
        };
        if next == text {
            return Ok(false);
        }
        write_file_atomic(&file, next.as_bytes())?;
        self.logger.info(&format!(
            "[tools] livesplit: TCP-сервер включён на порту {port} (время из LiveSplit в Ghosting)"
        ));
        Ok(true)
    }

    /// `prepare(root, gameId, presetId)`: готовит сплиты и раскладку игры и возвращает аргументы
    /// LiveSplit. Без сплитов для игры — пустой список: LiveSplit откроет последние.
    pub fn prepare(
        &self,
        root: &Path,
        game_id: &str,
        preset_id: Option<&str>,
    ) -> Result<Prepared, RawError> {
        let Some(preset) = preset_of(game_id, preset_id) else {
            return Ok(Prepared {
                args: Vec::new(),
                preset: None,
            });
        };
        let files = files_of(game_id, preset);
        let root_text = text_of(root);
        let splits_file = join(&[&root_text, &files.splits]);
        let layout_file = join(&[&root_text, &files.layout]);

        if preset.bundled.is_some() {
            // Не критично для запуска: папку могут держать антивирус или синхронизация.
            if let Err(error) = self
                .install_ghosting_asl(root)
                .and_then(|_| self.timer_milliseconds(&layout_file))
            {
                self.logger.warn(&format!(
                    "[tools] livesplit: автосплиттер Ghosting не поставлен: {}",
                    error.message
                ));
            }
        }
        if preset.bundled.is_none() {
            if self.write_if_missing(&splits_file, &splits_xml(preset))? {
                self.logger.info(&format!(
                    "[tools] livesplit: созданы сплиты {}",
                    files.splits
                ));
            }
            if !layout_file.exists() {
                let base_path = preset.kind.base_layout();
                let base = match node_fs::read_text(&join(&[&root_text, base_path]), Encoding::Utf8)
                {
                    Ok(text) => Some(text),
                    Err(_) => {
                        self.logger
                            .warn(&format!("[tools] livesplit: нет раскладки {base_path}"));
                        None
                    }
                };
                let layout = base
                    .filter(|base| !base.is_empty())
                    .and_then(|base| layout_xml(&base, preset))
                    .filter(|layout| !layout.is_empty());
                if let Some(layout) = layout {
                    self.write_if_missing(&layout_file, &layout)?;
                }
            }
            if preset.autosplitter == Some(Autosplitter::BunnySplit) {
                self.install_bunny_split(root)?;
            }
            self.seed_settings(root, preset, &windows_path(&files.splits))?;
        }

        if !splits_file.exists() {
            self.logger.warn(&format!(
                "[tools] livesplit: сплиты {} не найдены",
                files.splits
            ));
            return Ok(Prepared {
                args: Vec::new(),
                preset: Some(preset),
            });
        }
        let mut args = vec!["-s".to_owned(), windows_path(&files.splits)];
        if layout_file.exists() {
            args.push("-l".to_owned());
            args.push(windows_path(&files.layout));
        }
        Ok(Prepared {
            args,
            preset: Some(preset),
        })
    }
}
