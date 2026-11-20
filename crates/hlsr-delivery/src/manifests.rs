//! Реестр манифестов (`manifests.js`): четыре встроенных манифеста игр и их файловые
//! манифесты (`config/games`), соответствие идентификаторов лаунчера и сборок, локальные
//! настройки игр и настройки запуска. Манифесты инструментов и сейвов (`config/tools`,
//! `config/saves`), которые читают `tools.js` и `practice-saves.js`, вшиты здесь же.
//!
//! Файлы вшиваются `include_str!` и разбираются при первом обращении. У 0.4.7 то же делает
//! `require` (JSON из `app.asar`); встроенные манифесты не подписаны, подпись — только
//! у удалённого канала.
//!
//! Отличие от 0.4.7: поиск по идентификатору не видит свойств `Object.prototype`
//! (`isKnownGame("constructor")` в JS истинно, здесь — ложно).

use hlsr_core::json_text::parse_js_json;
use serde_json::{Map, Value, json};
use std::sync::OnceLock;

/// Вшитый JSON: текст и разобранное значение (разбирается один раз).
pub struct Embedded {
    text: &'static str,
    parsed: OnceLock<Value>,
}

impl Embedded {
    const fn new(text: &'static str) -> Embedded {
        Embedded {
            text,
            parsed: OnceLock::new(),
        }
    }

    /// Текст файла как он вшит.
    pub fn text(&self) -> &'static str {
        self.text
    }

    /// Разобранное значение, как у `require("….json")`.
    pub fn value(&self) -> &Value {
        self.parsed
            .get_or_init(|| parse_js_json(self.text).expect("вшитый файл config — JSON"))
    }
}

macro_rules! embed {
    ($name:ident, $path:literal) => {
        static $name: Embedded = Embedded::new(include_str!(concat!("../../../config/", $path)));
    };
}

embed!(GHOSTING_RENEWED, "games/ghosting-renewed.manifest.json");
embed!(
    GHOSTING_RENEWED_100,
    "games/ghosting-renewed-1.0.0.files.json"
);
embed!(
    GHOSTING_RENEWED_101,
    "games/ghosting-renewed-1.0.1.files.json"
);
embed!(
    GHOSTING_RENEWED_102,
    "games/ghosting-renewed-1.0.2.files.json"
);
embed!(
    GHOSTING_RENEWED_103,
    "games/ghosting-renewed-1.0.3.files.json"
);
embed!(
    GHOSTING_RENEWED_104,
    "games/ghosting-renewed-1.0.4.files.json"
);
embed!(
    GHOSTING_RENEWED_105,
    "games/ghosting-renewed-1.0.5.files.json"
);
embed!(
    GHOSTING_RENEWED_106,
    "games/ghosting-renewed-1.0.6.files.json"
);
embed!(
    GHOSTING_RENEWED_107,
    "games/ghosting-renewed-1.0.7.files.json"
);
embed!(
    GHOSTING_RENEWED_108,
    "games/ghosting-renewed-1.0.8.files.json"
);
embed!(
    GHOSTING_RENEWED_109,
    "games/ghosting-renewed-1.0.9.files.json"
);
embed!(
    GHOSTING_RENEWED_1010,
    "games/ghosting-renewed-1.0.10.files.json"
);
embed!(
    GHOSTING_RENEWED_1011,
    "games/ghosting-renewed-1.0.11.files.json"
);
embed!(
    GHOSTING_RENEWED_1012,
    "games/ghosting-renewed-1.0.12.files.json"
);
embed!(
    GHOSTING_RENEWED_1013,
    "games/ghosting-renewed-1.0.13.files.json"
);
embed!(HL2_4260, "games/hl2-4260.manifest.json");
embed!(HL2_4260_FILES, "games/hl2-4260-1.0.0.files.json");
embed!(HL2_2707, "games/hl2-2707.manifest.json");
embed!(HL2_2707_FILES, "games/hl2-2707-1.0.0.files.json");
embed!(HL1_WON, "games/hl1-won.manifest.json");
embed!(HL1_WON_FILES, "games/hl1-won-1.0.0.files.json");

embed!(LIVESPLIT_TOOL, "tools/livesplit.manifest.json");
embed!(HL1_PRACTICE_SAVES, "saves/hl1-practice-saves.manifest.json");

/// Манифест LiveSplit (`config/tools/livesplit.manifest.json`, для `tools.js`).
pub fn livesplit_tool_manifest() -> &'static Embedded {
    &LIVESPLIT_TOOL
}

/// Манифест сейвов практики HL1 (`config/saves/hl1-practice-saves.manifest.json`, для `practice-saves.js`).
pub fn hl1_practice_saves_manifest() -> &'static Embedded {
    &HL1_PRACTICE_SAVES
}

/// `MANIFESTS`: порядок ключей — как в объекте JS.
static MANIFESTS: [(&str, &Embedded); 4] = [
    ("ghosting-renewed", &GHOSTING_RENEWED),
    ("hl2-4260", &HL2_4260),
    ("hl2-2707", &HL2_2707),
    ("hl1-won", &HL1_WON),
];

/// `FILES_MANIFESTS`: по играм, версии — в порядке объявления.
static FILES_MANIFESTS: [(&str, &[(&str, &Embedded)]); 4] = [
    (
        "ghosting-renewed",
        &[
            ("1.0.0", &GHOSTING_RENEWED_100),
            ("1.0.1", &GHOSTING_RENEWED_101),
            ("1.0.2", &GHOSTING_RENEWED_102),
            ("1.0.3", &GHOSTING_RENEWED_103),
            ("1.0.4", &GHOSTING_RENEWED_104),
            ("1.0.5", &GHOSTING_RENEWED_105),
            ("1.0.6", &GHOSTING_RENEWED_106),
            ("1.0.7", &GHOSTING_RENEWED_107),
            ("1.0.8", &GHOSTING_RENEWED_108),
            ("1.0.9", &GHOSTING_RENEWED_109),
            ("1.0.10", &GHOSTING_RENEWED_1010),
            ("1.0.11", &GHOSTING_RENEWED_1011),
            ("1.0.12", &GHOSTING_RENEWED_1012),
            ("1.0.13", &GHOSTING_RENEWED_1013),
        ],
    ),
    ("hl2-4260", &[("1.0.0", &HL2_4260_FILES)]),
    ("hl2-2707", &[("1.0.0", &HL2_2707_FILES)]),
    ("hl1-won", &[("1.0.0", &HL1_WON_FILES)]),
];

/// `Object.keys(MANIFESTS)`.
pub fn game_ids() -> Vec<&'static str> {
    MANIFESTS.iter().map(|(game_id, _)| *game_id).collect()
}

/// `MANIFESTS[gameId]`.
pub fn manifest_of(game_id: &str) -> Option<&'static Value> {
    MANIFESTS
        .iter()
        .find(|(id, _)| *id == game_id)
        .map(|(_, manifest)| manifest.value())
}

/// `MANIFESTS` целиком, по порядку.
pub fn manifests() -> Vec<(&'static str, &'static Value)> {
    MANIFESTS
        .iter()
        .map(|(game_id, manifest)| (*game_id, manifest.value()))
        .collect()
}

/// `FILES_MANIFESTS[gameId]`: версия и вшитый файловый манифест.
pub fn files_manifests_of(game_id: &str) -> &'static [(&'static str, &'static Embedded)] {
    FILES_MANIFESTS
        .iter()
        .find(|(id, _)| *id == game_id)
        .map_or(&[], |(_, versions)| *versions)
}

/// `filesManifestOf(gameId, version)`: версия — `String(version)`; нет — `None`.
pub fn files_manifest_of(game_id: &str, version: &str) -> Option<&'static Value> {
    files_manifests_of(game_id)
        .iter()
        .find(|(known, _)| *known == version)
        .map(|(_, manifest)| manifest.value())
}

/// `filesManifestVersions(gameId)`: `Object.keys(…).sort()`.
pub fn files_manifest_versions(game_id: &str) -> Vec<&'static str> {
    let mut versions: Vec<&'static str> = files_manifests_of(game_id)
        .iter()
        .map(|(version, _)| *version)
        .collect();
    versions.sort_by(|a, b| hlsr_core::js::cmp_utf16(a, b));
    versions
}

/// `BY_LAUNCHER_ID`: ключи — числа, поэтому порядок `Object.keys` по возрастанию.
pub const BY_LAUNCHER_ID: [(&str, &str); 3] = [
    ("70", "hl1-won"),
    ("218", "ghosting-renewed"),
    ("220", "hl2-4260"),
];

/// Сборка игры внутри идентификатора лаунчера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Build {
    pub build: &'static str,
    pub game_id: &'static str,
    /// `steam-depot` — сборка ставится из депо Steam.
    pub delivery: Option<&'static str>,
}

/// `BUILDS_BY_LAUNCHER_ID`.
pub const BUILDS_BY_LAUNCHER_ID: [(&str, &[Build]); 1] = [(
    "220",
    &[
        Build {
            build: "4260",
            game_id: "hl2-4260",
            delivery: None,
        },
        Build {
            build: "2707",
            game_id: "hl2-2707",
            delivery: None,
        },
        Build {
            build: "5288",
            game_id: "hl2-5288",
            delivery: Some("steam-depot"),
        },
    ],
)];

fn builds_for(launcher_id: &str) -> &'static [Build] {
    BUILDS_BY_LAUNCHER_ID
        .iter()
        .find(|(id, _)| *id == launcher_id)
        .map_or(&[], |(_, builds)| *builds)
}

/// `buildGameIdOf(launcherId, build)`.
pub fn build_game_id_of(launcher_id: &str, build: &str) -> Option<&'static str> {
    builds_for(launcher_id)
        .iter()
        .find(|item| item.build == build)
        .map(|item| item.game_id)
}

/// `buildsOf(launcherId)`.
pub fn builds_of(launcher_id: &str) -> Vec<&'static str> {
    builds_for(launcher_id)
        .iter()
        .map(|item| item.build)
        .collect()
}

/// `DEPOT_BUILDS`: сборки, которые ставятся из депо Steam.
pub fn depot_builds() -> Vec<&'static str> {
    BUILDS_BY_LAUNCHER_ID
        .iter()
        .flat_map(|(_, builds)| builds.iter())
        .filter(|item| item.delivery == Some("steam-depot"))
        .map(|item| item.game_id)
        .collect()
}

/// `isDepotBuild(gameId)`.
pub fn is_depot_build(game_id: &str) -> bool {
    depot_builds().contains(&game_id)
}

/// `FILE_REPAIR_GAMES`: игры, у которых работают «Проверить файлы» и «Восстановить игру» по
/// файлам (меню, окно «Почему недоступно» и сам сервис доставки).
pub const FILE_REPAIR_GAMES: [&str; 1] = ["ghosting-renewed"];

/// `isFileRepairGame(gameId)`.
pub fn is_file_repair_game(game_id: &str) -> bool {
    FILE_REPAIR_GAMES.contains(&game_id)
}

/// `isKnownGame(gameId)`: пустое значение (`undefined`, `null`, `""`) — пустая строка.
pub fn is_known_game(game_id: &str) -> bool {
    MANIFESTS.iter().any(|(id, _)| *id == game_id) || is_depot_build(game_id)
}

/// `LOCAL_CONFIG[gameId]`: где у игры скриншоты, записи, cfg и какие exe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalConfig {
    pub screenshots_path: &'static str,
    pub runs_path: Option<&'static str>,
    pub runs_extension: Option<&'static str>,
    pub runs_language_dirs: Option<bool>,
    /// Папки демо (без вложенных) — только у Ghosting.
    pub demo_dirs: &'static [&'static str],
    pub demo_extension: Option<&'static str>,
    pub config_path: &'static str,
    pub executables: &'static [&'static str],
    pub settings_section: &'static str,
    pub shortcut_name: &'static str,
}

impl LocalConfig {
    /// Объект как у 0.4.7 (те же ключи и порядок).
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("screenshotsPath".to_owned(), json!(self.screenshots_path));
        object.insert("runsPath".to_owned(), json!(self.runs_path));
        if let Some(extension) = self.runs_extension {
            object.insert("runsExtension".to_owned(), json!(extension));
        }
        if let Some(dirs) = self.runs_language_dirs {
            object.insert("runsLanguageDirs".to_owned(), json!(dirs));
        }
        if !self.demo_dirs.is_empty() {
            object.insert("demoDirs".to_owned(), json!(self.demo_dirs));
        }
        if let Some(extension) = self.demo_extension {
            object.insert("demoExtension".to_owned(), json!(extension));
        }
        object.insert("configPath".to_owned(), json!(self.config_path));
        object.insert("executables".to_owned(), json!(self.executables));
        object.insert("settingsSection".to_owned(), json!(self.settings_section));
        object.insert("shortcutName".to_owned(), json!(self.shortcut_name));
        Value::Object(object)
    }
}

const LOCAL_BASE: LocalConfig = LocalConfig {
    screenshots_path: "hl2/screenshots",
    runs_path: None,
    runs_extension: None,
    runs_language_dirs: None,
    demo_dirs: &[],
    demo_extension: None,
    config_path: "hl2/cfg",
    executables: &["hl2.exe"],
    settings_section: "games",
    shortcut_name: "",
};

/// `LOCAL_CONFIG`: порядок ключей — как в объекте JS.
pub static LOCAL_CONFIG: [(&str, LocalConfig); 5] = [
    (
        "ghosting-renewed",
        LocalConfig {
            screenshots_path: "ghostingmod/screenshots",
            runs_path: Some("ghostingmod/runs"),
            // Демо лежат в ghostingmod и ghostingmod/demos, без вложенных папок.
            demo_dirs: &["ghostingmod", "ghostingmod/demos"],
            demo_extension: Some(".dem"),
            config_path: "ghostingmod/cfg",
            shortcut_name: "HLSR - Ghosting Mod Renewed",
            ..LOCAL_BASE
        },
    ),
    (
        "hl2-4260",
        LocalConfig {
            shortcut_name: "HLSR - Half-Life 2 (4260)",
            ..LOCAL_BASE
        },
    ),
    (
        "hl2-2707",
        LocalConfig {
            shortcut_name: "HLSR - Half-Life 2 (2707)",
            ..LOCAL_BASE
        },
    ),
    (
        "hl2-5288",
        LocalConfig {
            shortcut_name: "HLSR - Half-Life 2 (5288)",
            ..LOCAL_BASE
        },
    ),
    (
        "hl1-won",
        LocalConfig {
            screenshots_path: "Half-Life/valve_WON",
            // Демо (record/stop) движок пишет в папку мода.
            runs_path: Some("Half-Life/valve_WON"),
            runs_extension: Some(".dem"),
            runs_language_dirs: Some(true),
            config_path: "Half-Life/valve_WON",
            executables: &["hl.exe"],
            shortcut_name: "HLSR - Half-Life (WON)",
            ..LOCAL_BASE
        },
    ),
];

/// `localConfigOf(gameId)`.
pub fn local_config_of(game_id: &str) -> Option<&'static LocalConfig> {
    LOCAL_CONFIG
        .iter()
        .find(|(id, _)| *id == game_id)
        .map(|(_, config)| config)
}

/// Зависимость, которую лаунчер обновляет с GitHub перед каждым запуском.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub id: &'static str,
    pub title: &'static str,
    pub repo: &'static str,
    /// Регулярное выражение имени ресурса релиза (синтаксис JS).
    pub asset: &'static str,
    pub extract: Option<&'static str>,
    pub target: &'static str,
    pub version_file: &'static str,
    /// Закреплённый релиз: тег и SHA-256 файла (без него файл не ставится).
    pub pin: Option<Pin>,
}

/// `pin` зависимости: релиз по тегу и SHA-256 его файла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub tag: &'static str,
    pub sha256: &'static str,
}

impl Dependency {
    fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("id".to_owned(), json!(self.id));
        object.insert("title".to_owned(), json!(self.title));
        object.insert("repo".to_owned(), json!(self.repo));
        object.insert("asset".to_owned(), json!(self.asset));
        if let Some(extract) = self.extract {
            object.insert("extract".to_owned(), json!(extract));
        }
        object.insert("target".to_owned(), json!(self.target));
        object.insert("versionFile".to_owned(), json!(self.version_file));
        if let Some(pin) = &self.pin {
            object.insert(
                "pin".to_owned(),
                json!({"tag": pin.tag, "sha256": pin.sha256}),
            );
        }
        Value::Object(object)
    }
}

/// Готовые HUD и прицелы кладутся в спрайты мода поверх файлов valve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packs {
    pub sprites_dir: &'static str,
    pub base_sprites_dir: &'static str,
}

/// `LAUNCH_CONFIG[gameId]`: что нужно для запуска. Пустой список означает «поля нет».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchConfig {
    pub steam_app_id: u32,
    pub requires_owned_app_ids: &'static [u32],
    /// `goldsrc` у HL1; у остальных — движок Source по умолчанию.
    pub engine: Option<&'static str>,
    pub executable: &'static str,
    pub game_directory: &'static str,
    pub steam_app_id_file: Option<&'static str>,
    pub injector: Option<&'static str>,
    /// Путь к `RInput.exe` (`rinput.executable`).
    pub rinput_executable: Option<&'static str>,
    pub environment: &'static [(&'static str, &'static str)],
    pub packs: Option<Packs>,
    pub dependencies: &'static [Dependency],
    pub arguments: &'static [&'static str],
    pub source_sdk_required: bool,
    pub owns_steam_app_id_file: bool,
    pub steam_app_id_file_text: Option<&'static str>,
    pub presence_bridge: Option<bool>,
    pub steam_bridge: Option<bool>,
    /// Windows: переключатель Vulkan (DXVK из сборки игры, `vulkan_backend`).
    pub vulkan: Option<bool>,
}

impl LaunchConfig {
    /// Объект как у 0.4.7 (те же ключи и порядок).
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("steamAppId".to_owned(), json!(self.steam_app_id));
        if !self.requires_owned_app_ids.is_empty() {
            object.insert(
                "requiresOwnedAppIds".to_owned(),
                json!(self.requires_owned_app_ids),
            );
        }
        if let Some(engine) = self.engine {
            object.insert("engine".to_owned(), json!(engine));
        }
        object.insert("executable".to_owned(), json!(self.executable));
        object.insert("gameDirectory".to_owned(), json!(self.game_directory));
        if let Some(file) = self.steam_app_id_file {
            object.insert("steamAppIdFile".to_owned(), json!(file));
        }
        if let Some(injector) = self.injector {
            object.insert("injector".to_owned(), json!(injector));
        }
        if let Some(executable) = self.rinput_executable {
            object.insert("rinput".to_owned(), json!({"executable": executable}));
        }
        if !self.environment.is_empty() {
            let environment: Map<String, Value> = self
                .environment
                .iter()
                .map(|(key, value)| ((*key).to_owned(), json!(value)))
                .collect();
            object.insert("environment".to_owned(), Value::Object(environment));
        }
        if let Some(packs) = &self.packs {
            object.insert(
                "packs".to_owned(),
                json!({"spritesDir": packs.sprites_dir, "baseSpritesDir": packs.base_sprites_dir}),
            );
        }
        if !self.dependencies.is_empty() {
            object.insert(
                "dependencies".to_owned(),
                Value::Array(self.dependencies.iter().map(Dependency::to_json).collect()),
            );
        }
        object.insert("arguments".to_owned(), json!(self.arguments));
        object.insert(
            "sourceSdkRequired".to_owned(),
            json!(self.source_sdk_required),
        );
        object.insert(
            "ownsSteamAppIdFile".to_owned(),
            json!(self.owns_steam_app_id_file),
        );
        if let Some(text) = self.steam_app_id_file_text {
            object.insert("steamAppIdFileText".to_owned(), json!(text));
        }
        if let Some(bridge) = self.presence_bridge {
            object.insert("presenceBridge".to_owned(), json!(bridge));
        }
        if let Some(bridge) = self.steam_bridge {
            object.insert("steamBridge".to_owned(), json!(bridge));
        }
        if let Some(vulkan) = self.vulkan {
            object.insert("vulkan".to_owned(), json!(vulkan));
        }
        Value::Object(object)
    }
}

/// Основа HL2: все сборки Half-Life 2 запускаются одинаково.
const HL2_LAUNCH: LaunchConfig = LaunchConfig {
    steam_app_id: 220,
    requires_owned_app_ids: &[220],
    engine: None,
    executable: "hl2.exe",
    game_directory: "hl2",
    steam_app_id_file: None,
    injector: None,
    rinput_executable: None,
    environment: &[],
    packs: None,
    dependencies: &[],
    arguments: &["-steam", "-game", "hl2"],
    source_sdk_required: false,
    owns_steam_app_id_file: true,
    steam_app_id_file_text: None,
    presence_bridge: None,
    steam_bridge: None,
    vulkan: None,
};

/// `LAUNCH_CONFIG`: порядок ключей — как в объекте JS.
pub static LAUNCH_CONFIG: [(&str, LaunchConfig); 5] = [
    (
        "ghosting-renewed",
        LaunchConfig {
            steam_app_id: 243730,
            requires_owned_app_ids: &[],
            arguments: &["-steam", "-game", "ghostingmod"],
            game_directory: "ghostingmod",
            // Движок сам переписывает steam_appid.txt значением из gameinfo.txt мода,
            // с которым запущен: hl2.exe без -game (двойной клик) оставляет "220".
            // Лаунчер возвращает файл байт в байт как в сборке (движок дописывает \0),
            // чтобы проверка целостности его не браковала.
            steam_app_id_file_text: Some("243730\n\0"),
            // client.dll шлёт карту, FPS и таймер забега для Discord (game-presence).
            presence_bridge: Some(true),
            // Linux: umu отключает lsteamclient, и Steam не видел игру. Через мост
            // к клиенту Steam она видна под AppID 243730, как на Windows.
            steam_bridge: Some(true),
            // Windows: переключатель Vulkan (DXVK из сборки игры, vulkan_backend).
            vulkan: Some(true),
            ..HL2_LAUNCH
        },
    ),
    ("hl2-4260", HL2_LAUNCH),
    ("hl2-2707", HL2_LAUNCH),
    ("hl2-5288", HL2_LAUNCH),
    // GoldSrc: вместо gameinfo.txt мод описывает liblist.gam, а steam_appid.txt
    // лежит рядом с hl.exe. Игра стартует только через инжектор Bunnymod XT.
    // Старому steam.dll нужен настоящий мост к Steam, поэтому на Linux игра
    // идёт через Proton в режиме Steam, а не через umu.
    (
        "hl1-won",
        LaunchConfig {
            steam_app_id: 70,
            requires_owned_app_ids: &[70],
            engine: Some("goldsrc"),
            executable: "Half-Life/hl.exe",
            game_directory: "Half-Life/valve_WON",
            steam_app_id_file: Some("Half-Life/steam_appid.txt"),
            injector: Some("Bunnymod XT/Injector.exe"),
            rinput_executable: Some("RInput/RInput.exe"),
            // На Wayland-драйвере Wine OpenGL идёт через EGL и упирается в частоту
            // композитора (в окне 60 FPS); через XWayland (GLX) лимита нет.
            environment: &[
                ("BXT_DISABLE_VSYNC", "1"),
                ("PROTON_ENABLE_WAYLAND", "0"),
                ("__GL_SYNC_TO_VBLANK", "0"),
            ],
            packs: Some(Packs {
                sprites_dir: "Half-Life/valve_WON/sprites",
                base_sprites_dir: "Half-Life/valve/sprites",
            }),
            // Обновляются перед каждым запуском с GitHub, как update.bat сборки.
            dependencies: &[
                Dependency {
                    id: "bxt",
                    title: "Bunnymod XT",
                    repo: "YaLTeR/BunnymodXT",
                    asset: r"^(\d+-)?BunnymodXT-Windows\.7z$",
                    extract: Some("BunnymodXT.dll"),
                    target: "Bunnymod XT/BunnymodXT.dll",
                    version_file: "Bunnymod XT/version",
                    pin: None,
                },
                Dependency {
                    id: "injector",
                    title: "Bunnymod XT Injector",
                    repo: "YaLTeR/BunnymodXT-Injector",
                    asset: r"^Injector\.exe$",
                    extract: None,
                    target: "Bunnymod XT/Injector.exe",
                    version_file: "Bunnymod XT/.hlsr-injector-version",
                    // У этого релиза GitHub не отдаёт SHA-256, а Injector запускается при каждом
                    // старте игры. v3.2: тот же Injector, что в сборке игры; Microsoft Defender
                    // метит v3.3 как HackTool:Win32/Injector!MTB, v3.2 не трогает.
                    pin: Some(Pin {
                        tag: "v3.2",
                        sha256: "c4d576f87dbb0ce46ad36e7015e67b1666f9aff1e834d5f0f7ef12e102b5dad9",
                    }),
                },
            ],
            arguments: &[
                "-steam",
                "-gl",
                "-32bpp",
                "-game",
                "valve_WON",
                "-noforcemparms",
            ],
            steam_bridge: Some(true),
            ..HL2_LAUNCH
        },
    ),
];

/// `launchConfigOf(gameId)`.
pub fn launch_config_of(game_id: &str) -> Option<&'static LaunchConfig> {
    LAUNCH_CONFIG
        .iter()
        .find(|(id, _)| *id == game_id)
        .map(|(_, config)| config)
}

/// `OWNED_APP_TITLES`.
pub const OWNED_APP_TITLES: [(u32, &str); 2] = [(70, "Half-Life"), (220, "Half-Life 2")];

/// Игра Steam, которая должна быть на аккаунте.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedApp {
    pub app_id: u32,
    pub title: String,
}

/// `requiredOwnedApps()`: без повторов, в порядке `LAUNCH_CONFIG`.
pub fn required_owned_apps() -> Vec<OwnedApp> {
    let mut seen: Vec<OwnedApp> = Vec::new();
    for (_, config) in &LAUNCH_CONFIG {
        for &app_id in config.requires_owned_app_ids {
            if seen.iter().any(|app| app.app_id == app_id) {
                continue;
            }
            let title = OWNED_APP_TITLES
                .iter()
                .find(|(id, _)| *id == app_id)
                .map_or_else(
                    || format!("AppID {app_id}"),
                    |(_, title)| (*title).to_owned(),
                );
            seen.push(OwnedApp { app_id, title });
        }
    }
    seen
}

/// `forLauncherId(launcherId)`: манифест игры по идентификатору в лаунчере (`70`, `218`, `220`).
pub fn for_launcher_id(launcher_id: &str) -> Option<&'static Value> {
    BY_LAUNCHER_ID
        .iter()
        .find(|(id, _)| *id == launcher_id)
        .and_then(|(_, game_id)| manifest_of(game_id))
}

/// `launcherIdOf(gameId)`: прямое соответствие, затем через сборки.
pub fn launcher_id_of(game_id: &str) -> Option<&'static str> {
    BY_LAUNCHER_ID
        .iter()
        .find(|(_, id)| *id == game_id)
        .map(|(launcher_id, _)| *launcher_id)
        .or_else(|| {
            BUILDS_BY_LAUNCHER_ID
                .iter()
                .find(|(_, builds)| builds.iter().any(|item| item.game_id == game_id))
                .map(|(launcher_id, _)| *launcher_id)
        })
}
