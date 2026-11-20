//! Заготовки биндов для вкладки «Клавиатура» (`shared/bind-presets.js`): у каждой
//! игры свои категории.
//!
//! Заготовка — команда для бинда, подпись и, если это скрипт, alias, без которых
//! команда не работает. Alias пишутся в hlsr-binds.cfg, только пока команда
//! назначена на клавишу.
//!
//! `scripted` — команда автоматизирует нажатия (alias, wait, несколько команд,
//! TAS-функции BXT). В HL1 такие бинды допустимы только в категориях Scripted,
//! поэтому клавиатура HL1 прячет их в режиме Scriptless. `practice` — для
//! тренировки: в ране запрещено. `listed = false` — заготовку не предлагают в
//! списке клавиатуры (её назначают в другой вкладке), но клавиатура узнаёт её на
//! клавише.
//!
//! Источники: HL1 — gfx/shell/kb_act.lst сборки, гайд
//! <https://github.com/yavats/hl1-scripts> (базовые скрипты) и команды Bunnymod XT
//! из BunnymodXT.dll; Ghosting — стандартный autoexec.cfg лаунчера; HL2 — гайд
//! New Engine на speedrun.com/hl2 (раздел Key bindings).
//!
//! В 0.4.7 каталог зависит от ОС (`process.platform`): под Linux (Wine) «save :»
//! сейв не удаляет, там работает «save /». Здесь ОС передаётся явно.

use crate::game_binds::{AliasMap, Overrides, alias_map, parse_cfg, reassigned_aliases};
use crate::hl1_guide;
use hlsr_core::js::object::JsObject;
use hlsr_core::js::{self, Platform};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Подпись или пояснение на двух языках.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loc {
    pub ru: String,
    pub en: String,
}

/// Категория заготовок; `tone` задаёт цвет на клавиатуре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    pub id: String,
    pub tone: String,
    pub title: Loc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub category: String,
    pub command: String,
    pub label: Loc,
    pub scripted: bool,
    pub practice: bool,
    pub listed: bool,
    /// Пары «имя, значение» alias, без которых команда не работает.
    pub aliases: Vec<(String, String)>,
    pub note: Option<Loc>,
    /// Скрипт гайда HL1, с которым связана заготовка.
    pub guide: Option<String>,
    /// Только у заготовок скриптов гайда: скрипт ждёт `auto_alias` в game.cfg.
    pub auto_alias: Option<bool>,
}

/// Заготовки одной игры.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub game_id: String,
    pub engine: &'static str,
    pub categories: Vec<Category>,
    pub presets: Vec<Preset>,
    /// Ключ ([`command_key`]) → номер первой заготовки с таким ключом.
    pub by_command: JsObject<usize>,
    pub run_modes: bool,
}

/// Игры с каталогами, в порядке `CATALOGS` 0.4.7.
pub const GAME_IDS: [&str; 5] = [
    "hl1-won",
    "ghosting-renewed",
    "hl2-4260",
    "hl2-5288",
    "hl2-2707",
];

const GAME_ENGINES: [(&str, &str); 5] = [
    ("hl1-won", "goldsrc"),
    ("ghosting-renewed", "source"),
    ("hl2-4260", "source"),
    ("hl2-5288", "source"),
    ("hl2-2707", "source-old"),
];

/// Старт рана из стокового userconfig.cfg пакета Half-Life 2005 WON: так же его
/// даёт гайд «FAQ & Setup» на speedrun.com/hl1.
pub const HL1_RUN_ALIAS: &str =
    "skill 1;map c1a0;bxt_hud_timer 1;bxt_timer_reset;bxt_timer_start;bxt_autorecord run";

/// Удаление сейва в версиях до Steampipe (5135 и старше): имя сейва ровно такой
/// длины. В Steampipe (5288 и новее) — [`steampipe_save_deletion`].
pub fn pre_steampipe_save_deletion() -> String {
    format!("save {};reload", "a".repeat(224))
}

fn save_deletion_name(platform: Platform) -> &'static str {
    if platform == Platform::Linux {
        "/"
    } else {
        ":"
    }
}

/// Удаление сейва в Ghosting.
pub fn ghosting_save_deletion(platform: Platform) -> String {
    format!("save {}; reload", save_deletion_name(platform))
}

/// Удаление сейва в Steampipe-сборках HL2.
pub fn steampipe_save_deletion(platform: Platform) -> String {
    format!("save {};reload", save_deletion_name(platform))
}

pub fn engine_for(game_id: &str) -> Option<&'static str> {
    GAME_ENGINES
        .iter()
        .find(|(id, _)| *id == game_id)
        .map(|(_, engine)| *engine)
}

// ---------------------------------------------------------- построение каталогов

fn l(ru: &str, en: &str) -> Loc {
    Loc {
        ru: ru.to_owned(),
        en: en.to_owned(),
    }
}

fn cat(id: &str, tone: &str, ru: &str, en: &str) -> Category {
    Category {
        id: id.to_owned(),
        tone: tone.to_owned(),
        title: l(ru, en),
    }
}

fn p(category: &str, command: &str, ru: &str, en: &str) -> Preset {
    Preset {
        category: category.to_owned(),
        command: command.to_owned(),
        label: l(ru, en),
        scripted: false,
        practice: false,
        listed: true,
        aliases: Vec::new(),
        note: None,
        guide: None,
        auto_alias: None,
    }
}

impl Preset {
    fn scripted(mut self) -> Self {
        self.scripted = true;
        self
    }

    fn practice(mut self) -> Self {
        self.practice = true;
        self
    }

    fn unlisted(mut self) -> Self {
        self.listed = false;
        self
    }

    fn note(mut self, ru: &str, en: &str) -> Self {
        self.note = Some(l(ru, en));
        self
    }

    fn aliases(mut self, pairs: &[(&str, &str)]) -> Self {
        self.aliases = pairs
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect();
        self
    }
}

// ---------------------------------------------------------------- Half-Life 1

fn hl1_categories() -> Vec<Category> {
    vec![
        cat("movement", "blue", "Движение", "Movement"),
        cat("weapons", "orange", "Оружие", "Weapons"),
        cat("saves", "green", "Сохранения", "Saves"),
        cat("fps", "yellow", "FPS", "FPS"),
        cat("scripts", "purple", "Скрипты", "Scripts"),
        cat("bxt", "pink", "Bunnymod XT", "Bunnymod XT"),
        cat("practice", "red", "Тренировка", "Practice"),
        cat("misc", "grey", "Разное", "Misc"),
        cat("guide", "purple", "Скрипты гайда", "Guide scripts"),
    ]
}

const HL1_WEAPONS: [(&str, &str, &str); 14] = [
    ("weapon_crowbar", "Монтировка", "Crowbar"),
    ("weapon_9mmhandgun", "Пистолет", "Glock"),
    ("weapon_357", "Револьвер .357", ".357 Magnum"),
    ("weapon_9mmAR", "Автомат MP5", "MP5"),
    ("weapon_shotgun", "Дробовик", "Shotgun"),
    ("weapon_crossbow", "Арбалет", "Crossbow"),
    ("weapon_rpg", "Ракетница", "RPG"),
    ("weapon_gauss", "Гаусс", "Gauss"),
    ("weapon_egon", "Глюонная пушка", "Gluon gun"),
    ("weapon_hornetgun", "Улейная пушка", "Hivehand"),
    ("weapon_handgrenade", "Граната", "Grenade"),
    ("weapon_satchel", "Сумка со взрывчаткой", "Satchel"),
    ("weapon_tripmine", "Мина-растяжка", "Tripmine"),
    ("weapon_snark", "Снарки", "Snarks"),
];

fn hl1_presets() -> Vec<Preset> {
    let mut list = vec![
        p("movement", "+forward", "Вперёд", "Forward"),
        p("movement", "+back", "Назад", "Back"),
        p("movement", "+moveleft", "Влево", "Strafe left"),
        p("movement", "+moveright", "Вправо", "Strafe right"),
        p("movement", "+jump", "Прыжок", "Jump"),
        p("movement", "+duck", "Присесть", "Duck"),
        p("movement", "+speed", "Шаг", "Walk"),
        p("movement", "+use", "Использовать", "Use"),
        p("movement", "+moveup", "Всплыть", "Swim up"),
        p("movement", "+movedown", "Погрузиться", "Swim down"),
        p("movement", "+left", "Поворот влево", "Turn left"),
        p("movement", "+right", "Поворот вправо", "Turn right"),
        p("movement", "+lookup", "Взгляд вверх", "Look up"),
        p("movement", "+lookdown", "Взгляд вниз", "Look down"),
        p("movement", "centerview", "Взгляд прямо", "Center view"),
        p("movement", "+strafe", "Стрейф", "Strafe"),
        p("movement", "+mlook", "Обзор мышью", "Mouse look"),
        p("movement", "+klook", "Обзор клавишами", "Keyboard look"),
        p("weapons", "+attack", "Огонь", "Fire"),
        p(
            "weapons",
            "+attack2",
            "Альтернативный огонь",
            "Secondary fire",
        ),
        p("weapons", "+reload", "Перезарядка", "Reload"),
        p("weapons", "slot1", "Слот 1", "Slot 1"),
        p("weapons", "slot2", "Слот 2", "Slot 2"),
        p("weapons", "slot3", "Слот 3", "Slot 3"),
        p("weapons", "slot4", "Слот 4", "Slot 4"),
        p("weapons", "slot5", "Слот 5", "Slot 5"),
        p("weapons", "invprev", "Предыдущее оружие", "Previous weapon"),
        p("weapons", "invnext", "Следующее оружие", "Next weapon"),
        p("weapons", "lastinv", "Последнее оружие", "Last weapon"),
    ];
    list.extend(
        HL1_WEAPONS
            .iter()
            .map(|(command, ru, en)| p("weapons", command, ru, en)),
    );
    list.extend([
        p("weapons", "impulse 100", "Фонарик", "Flashlight"),
        p("weapons", "impulse 201", "Спрей", "Spray"),
        p("saves", "save quick", "Быстрое сохранение", "Quick save"),
        p("saves", "load quick", "Быстрая загрузка", "Quick load"),
        p("saves", "save x; load x", "Сейвлоад", "Save & load").scripted().note(
            "Сохранение и загрузка одним нажатием, квиксейв не трогает.",
            "Saves and loads with one key press and keeps your quick save.",
        ),
        p("fps", "fps_max 100", "100 FPS", "100 FPS")
            .note("Максимум по правилам HL1.", "The maximum allowed by HL1 rules."),
        p("fps", "fps_max 20", "20 FPS", "20 FPS"),
        p("fps", "+fps_20_hold", "20 FPS, пока держишь", "20 FPS while held")
            .scripted()
            .note(
                "Держите клавишу — 20 FPS, отпустите — снова 100.",
                "Hold for 20 FPS, release to go back to 100.",
            )
            .aliases(&[
                ("+fps_20_hold", "fps_max 20; alias -fps_20_hold fps_20_reset"),
                ("fps_20_reset", "fps_max 100; alias -fps_20_hold"),
            ]),
        p("fps", "fps_20_toggle", "20 / 100 FPS переключатель", "20 / 100 FPS toggle")
            .scripted()
            .note("Нажатие — 20 FPS, ещё нажатие — 100.", "Press for 20 FPS, press again for 100.")
            .aliases(&[
                ("fps_20_toggle", "fps_20_toggle1"),
                ("fps_20_toggle1", "fps_max 20; alias fps_20_toggle fps_20_toggle2"),
                ("fps_20_toggle2", "fps_max 100; alias fps_20_toggle fps_20_toggle1"),
            ]),
        p("fps", "4fpsbind", "Кадр на 4 FPS", "One 4 FPS frame")
            .scripted()
            .note("Один кадр на 4 FPS, потом снова 100.", "One frame at 4 FPS, then back to 100.")
            .aliases(&[("4fpsbind", "fps_max 4; w; fps_max 100")]),
        p("scripts", "+usespam", "Спам «Использовать»", "Use spam")
            .scripted()
            .note(
                "Пока клавиша зажата, жмёт +use через кадр.",
                "Presses +use every other frame while held.",
            )
            .aliases(&[
                ("+usespam", "alias _zspecial @use;@use1"),
                ("-usespam", "alias _zspecial;-use"),
                ("@use1", "+use;alias @use @use2;bxt_append _zspecial"),
                ("@use2", "w;alias @use @use3;bxt_append _zspecial"),
                ("@use3", "-use;alias @use @use4;bxt_append _zspecial"),
                ("@use4", "w;alias @use @use1;bxt_append _zspecial"),
            ]),
        p("scripts", "+duckroll", "Даккролл", "Duckroll")
            .scripted()
            .note(
                "Спам приседа для водных бустов. Для движения лучше +bxt_tas_ducktap.",
                "Duck spam for water boosts. For movement +bxt_tas_ducktap is better.",
            )
            .aliases(&[
                ("+duckroll", "alias _zspecial @duck;@dr1"),
                ("-duckroll", "alias _zspecial;-duck"),
                ("@dr1", "+duck;alias @duck @dr2;bxt_append _zspecial"),
                ("@dr2", "w;alias @duck @dr3;bxt_append _zspecial"),
                ("@dr3", "-duck;alias @duck @dr4;bxt_append _zspecial"),
                ("@dr4", "w;alias @duck @dr1;bxt_append _zspecial"),
            ]),
        p("scripts", "+tau", "Гаусс-буст 180°", "180° gauss boost")
            .scripted()
            .note(
                "Держите — заряд, отпустите — выстрел себе под ноги.",
                "Hold to charge, release to boost.",
            )
            .aliases(&[
                ("_taubo", ""),
                (
                    "_taubofunc",
                    "cl_pitchup 180;cl_pitchdown -180;-attack2;wait;cl_pitchup 89;cl_pitchdown 89;force_centerview",
                ),
                ("+tau", "+attack2;alias _taubo _taubofunc"),
                ("-tau", "_taubo;alias _taubo"),
            ]),
        p("scripts", "slowgauss", "Медленный гаусс", "Slow gauss")
            .scripted()
            .note(
                "Заряд на ПКМ, сразу ЛКМ, через ~3 с — эта клавиша. Нужно 1–2 патрона.",
                "Charge with MOUSE2, switch to MOUSE1, after ~3 s press this key. Needs 1–2 ammo.",
            )
            .aliases(&[(
                "slowgauss",
                "cl_pitchup 180;cl_pitchdown -180;-attack;w 50;cl_pitchup 89;cl_pitchdown 89;force_centerview",
            )]),
        p("scripts", "tauswitch", "Переключатель ПКМ: гаусс-буст", "MOUSE2 gauss boost switch")
            .scripted()
            .note(
                "Меняет бинд ПКМ между +tau и +attack2.",
                "Switches MOUSE2 between +tau and +attack2.",
            )
            .aliases(&[
                ("_taubo", ""),
                (
                    "_taubofunc",
                    "cl_pitchup 180;cl_pitchdown -180;-attack2;wait;cl_pitchup 89;cl_pitchdown 89;force_centerview",
                ),
                ("+tau", "+attack2;alias _taubo _taubofunc"),
                ("-tau", "_taubo;alias _taubo"),
                ("tauswitch", "tauswitch1"),
                ("tauswitch1", "bind mouse2 +tau; say gaussboost_ON; alias tauswitch tauswitch2"),
                ("tauswitch2", "bind mouse2 +attack2; say gaussboost_OFF; alias tauswitch tauswitch1"),
            ]),
    ]);
    for (name, ups, wait) in [
        ("obbo2000", "2000", 11),
        ("obbo1600", "1600", 10),
        ("obbo1300", "1300", 9),
        ("obbo1000", "1000", 8),
        ("obbo800", "800", 7),
    ] {
        list.push(
            p(
                "scripts",
                name,
                &format!("Буст от объекта {ups} UPS"),
                &format!("Object boost {ups} UPS"),
            )
            .scripted()
            .aliases(&[(name, &format!("+use;w {wait};-use;+jump;w;-jump"))]),
        );
    }
    list.extend([
        p("scripts", "obboshoot", "Буст от объекта вперёд", "Forward object boost")
            .scripted()
            .note("~1500–1700 UPS вперёд.", "~1500–1700 UPS forward.")
            .aliases(&[("obboshoot", "+use;w 10;-use;+jump;w;-jump;+attack;w;-attack")]),
        p("scripts", "app_obbo", "Обжбуст с дактапом", "Obbo ducktap")
            .scripted()
            .note(
                "Apprehension: спрыгнуть, зажать A, нажать скрипт.",
                "Apprehension: drop down, hold A, press the script.",
            )
            .aliases(&[("app_obbo", "+use; w 9; -use; w; +duck; w 8; -duck")]),
        p("bxt", "+bxt_tas_ducktap", "Дактап (BXT)", "Ducktap (BXT)").scripted(),
        p("bxt", "+bxt_tas_jumpbug", "Джампбаг (BXT)", "Jumpbug (BXT)").scripted(),
        p("bxt", "+bxt_tas_autojump", "TAS-автопрыжок (BXT)", "TAS autojump (BXT)")
            .scripted()
            .note(
                "Держите клавишу — BXT прыгает в первый кадр на земле.",
                "Hold the key and BXT jumps on the first frame on the ground.",
            ),
        // Обычный автопрыжок 0.4.3: в списке его больше нет, но на клавише узнаётся.
        p("bxt", "+autojump", "Автопрыжок (BXT)", "Autojump (BXT)")
            .scripted()
            .unlisted()
            .note(
                "Держите клавишу — BXT прыгает сам при каждом приземлении.",
                "Hold the key and BXT jumps on every landing.",
            )
            .aliases(&[
                ("+autojump", "bxt_autojump 1; +jump"),
                ("-autojump", "-jump; bxt_autojump 0"),
            ]),
        p("bxt", "bxt_timer_reset", "Сбросить таймер", "Reset timer"),
        p("bxt", "bxt_timer_start", "Запустить таймер", "Start timer"),
        p("bxt", "bxt_timer_stop", "Остановить таймер", "Stop timer"),
        p("bxt", "run", "Начать ран", "Start run")
            .note(
                "Сложность, c1a0, таймер BXT и запись демок, как в стандартном userconfig.cfg.",
                "Difficulty, c1a0, BXT timer and demo recording, as in the standard userconfig.cfg.",
            )
            .aliases(&[("run", HL1_RUN_ALIAS)]),
        p("bxt", "stop", "Остановить запись демки", "Stop demo recording"),
        p("practice", "sv_cheats 1", "Читы включить", "Cheats on").practice(),
        p("practice", "sv_cheats 0", "Читы выключить", "Cheats off"),
        p("practice", "noclip", "Полёт сквозь стены", "Noclip").practice(),
        p("practice", "god", "Бессмертие", "God mode").practice(),
        p("practice", "notarget", "Невидимость для врагов", "Notarget").practice(),
        p("practice", "impulse 101", "Всё оружие", "All weapons").practice(),
        p("misc", "toggleconsole", "Консоль", "Console"),
        p("misc", "cancelselect", "Отмена / меню", "Cancel / menu"),
        p("misc", "pause", "Пауза", "Pause"),
        p("misc", "snapshot", "Скриншот", "Screenshot"),
        p("misc", "+showscores", "Таблица очков", "Scoreboard"),
        p("misc", "messagemode", "Чат", "Chat"),
        p("misc", "messagemode2", "Командный чат", "Team chat"),
        p("misc", "+voicerecord", "Голосовой чат", "Voice chat"),
        p("misc", "sizeup", "Больше экран", "Screen size up"),
        p("misc", "sizedown", "Меньше экран", "Screen size down"),
        p("misc", "quit prompt", "Выход из игры", "Quit"),
    ]);
    let guide = hl1_guide_presets(&mut list);
    list.extend(guide);
    list
}

/// Действия скриптов гайда hl1-scripts (вкладка «Скрипты»). Команды, которые уже
/// есть выше (базовые скрипты гайда), остаются прежними заготовками: их alias
/// совпадают с гайдом. Подписи у них — английские названия из гайда, как во
/// вкладке «Скрипты». Общие команды BXT (+bxt_tas_jumpbug) не трогаются.
fn hl1_guide_presets(existing: &mut [Preset]) -> Vec<Preset> {
    // Map JS из пар «ключ, заготовка»: при повторе ключа остаётся последняя.
    let by_key: HashMap<String, usize> = existing
        .iter()
        .enumerate()
        .map(|(index, preset)| (command_key(&preset.command), index))
        .collect();
    let mut known: HashSet<String> = by_key.keys().cloned().collect();
    let mut result = Vec::new();
    for script in hl1_guide::scripts() {
        for action in &script.actions {
            let key = command_key(&action.command);
            let label = if script.actions.len() > 1 {
                format!("{}: {}", script.title, action.command)
            } else {
                script.title.clone()
            };
            if let Some(&index) = by_key.get(&key) {
                let preset = &mut existing[index];
                if preset.category != "bxt" && !preset.aliases.is_empty() {
                    preset.label = l(&label, &label);
                    if preset.guide.as_deref().is_none_or(str::is_empty) {
                        preset.guide = Some(script.id.clone());
                    }
                }
            }
            if !known.insert(key) {
                continue;
            }
            let mut preset = p("guide", &action.command, &label, &label)
                .scripted()
                .unlisted()
                .note(
                    "Скрипт из гайда hl1-scripts. Видео и описание — во вкладке «Скрипты».",
                    "A script from the hl1-scripts guide. See the Scripts tab for the video and description.",
                );
            preset.guide = Some(script.id.clone());
            preset.auto_alias = Some(script.needs_auto_alias);
            preset.aliases = script.aliases.clone();
            result.push(preset);
        }
    }
    result
}

// ------------------------------------------------------- Source: общие части

fn source_movement() -> Vec<Preset> {
    vec![
        p("movement", "+forward", "Вперёд", "Forward"),
        p("movement", "+back", "Назад", "Back"),
        p("movement", "+moveleft", "Влево", "Strafe left"),
        p("movement", "+moveright", "Вправо", "Strafe right"),
        p("movement", "+jump", "Прыжок", "Jump").note(
            "Прыжок на колесо мыши нужен для AHop.",
            "Jump on the mouse wheel is needed for AHop.",
        ),
        p("movement", "+duck", "Присесть", "Duck"),
        p(
            "movement",
            "toggle_duck",
            "Присесть (переключатель)",
            "Toggle duck",
        )
        .note(
            "Меняет присед во время AHop без потери скорости.",
            "Changes crouch state while AHopping without losing speed.",
        ),
        p("movement", "+speed", "Бег", "Sprint"),
        p("movement", "+walk", "Шаг", "Walk"),
        p("movement", "+use", "Использовать", "Use"),
        p("movement", "+zoom", "Зум", "Zoom"),
        p("movement", "+strafe", "Стрейф", "Strafe"),
    ]
}

fn source_combat() -> Vec<Preset> {
    let mut list = vec![
        p("weapons", "+attack", "Огонь", "Fire"),
        p(
            "weapons",
            "+attack2",
            "Альтернативный огонь",
            "Secondary fire",
        ),
        p("weapons", "+reload", "Перезарядка", "Reload"),
    ];
    for n in 1..=6 {
        list.push(p(
            "weapons",
            &format!("slot{n}"),
            &format!("Слот {n}"),
            &format!("Slot {n}"),
        ));
    }
    list.extend([
        p("weapons", "invprev", "Предыдущее оружие", "Previous weapon"),
        p("weapons", "invnext", "Следующее оружие", "Next weapon"),
        p("weapons", "lastinv", "Последнее оружие", "Last weapon"),
        p(
            "weapons",
            "phys_swap",
            "Грави-пушка ↔ оружие",
            "Gravity gun swap",
        ),
        p("weapons", "impulse 100", "Фонарик", "Flashlight"),
    ]);
    list
}

/// После квикклипа оружие листать нельзя: нужен бинд на каждое.
const HL2_WEAPONS: [(&str, &str, &str); 11] = [
    ("weapon_crowbar", "Монтировка", "Crowbar"),
    ("weapon_physcannon", "Грави-пушка", "Gravity gun"),
    ("weapon_pistol", "Пистолет", "Pistol"),
    ("weapon_357", "Револьвер .357", ".357 Magnum"),
    ("weapon_smg1", "Автомат SMG", "SMG"),
    ("weapon_ar2", "Импульсная винтовка", "Pulse rifle"),
    ("weapon_shotgun", "Дробовик", "Shotgun"),
    ("weapon_crossbow", "Арбалет", "Crossbow"),
    ("weapon_frag", "Граната", "Grenade"),
    ("weapon_rpg", "Ракетница", "RPG"),
    ("weapon_bugbait", "Феропод", "Bugbait"),
];

fn use_weapon((weapon, ru, en): &(&str, &str, &str)) -> Preset {
    p("weapons", &format!("use {weapon}"), ru, en).note(
        "Работает и после квикклипа.",
        "Works after a quickclip too.",
    )
}

fn source_misc() -> Vec<Preset> {
    vec![
        p("misc", "toggleconsole", "Консоль", "Console"),
        p("misc", "cancelselect", "Отмена / меню", "Cancel / menu"),
        p("misc", "pause", "Пауза", "Pause"),
        p("misc", "jpeg", "Скриншот", "Screenshot"),
        p("misc", "quit", "Выход из игры", "Quit"),
    ]
}

// ------------------------------------------------------------------- Ghosting

fn ghosting_categories() -> Vec<Category> {
    vec![
        cat("movement", "blue", "Движение", "Movement"),
        cat("weapons", "orange", "Оружие", "Weapons"),
        cat("saves", "green", "Сохранения", "Saves"),
        cat("rebinds", "yellow", "Рибинды колеса", "Wheel rebinds"),
        cat("run", "pink", "Ран", "Run"),
        cat("misc", "grey", "Разное", "Misc"),
    ]
}

/// Оружие из стандартного autoexec Ghosting идёт первым: им пользуются после Yesclip.
const GHOSTING_WEAPON_ORDER: [&str; 6] = [
    "weapon_crowbar",
    "weapon_pistol",
    "weapon_smg1",
    "weapon_shotgun",
    "weapon_frag",
    "weapon_rpg",
];

fn ghosting_presets(platform: Platform) -> Vec<Preset> {
    let mut list = source_movement();
    list.extend(source_combat());
    list.push(p("weapons", "bug_swap", "Феропод ↔ оружие", "Bugbait swap"));
    for name in GHOSTING_WEAPON_ORDER {
        let weapon = HL2_WEAPONS
            .iter()
            .find(|item| item.0 == name)
            .expect("оружие из порядка Ghosting");
        list.push(use_weapon(weapon));
    }
    list.extend(
        HL2_WEAPONS
            .iter()
            .filter(|item| !GHOSTING_WEAPON_ORDER.contains(&item.0))
            .map(use_weapon),
    );
    let required = |preset: Preset| preset.note("Обязателен в ране.", "Required in a run.");
    list.extend([
        p("saves", "save quick", "Быстрое сохранение", "Quick save"),
        p("saves", "load quick", "Быстрая загрузка", "Quick load"),
        p("saves", "save x; load x", "Сейвлоад", "Save & load").note(
            "Сохранение и загрузка одним нажатием, квиксейв не трогает.",
            "Saves and loads with one key press and keeps your quick save.",
        ),
        p("saves", "load autosave", "Загрузить автосейв", "Load autosave"),
        p("saves", "load quick01", "Позапрошлый сейв", "Previous quick save").note(
            "Обязательный бинд: загружает позапрошлый квиксейв.",
            "Required bind: loads the quick save before the last one.",
        ),
        p("saves", &ghosting_save_deletion(platform), "Удаление сейва", "Save deletion"),
        p("saves", "reload", "Перезагрузить последний сейв", "Reload last save"),
        required(p(
            "rebinds",
            "bind mwheeldown +duck; bind mwheelup +duck",
            "Колесо: присед (Duckroll)",
            "Wheel: duck (Duckroll)",
        )),
        required(p(
            "rebinds",
            "bind mwheeldown pause; bind mwheelup pause",
            "Колесо: пауза (Pauseboost)",
            "Wheel: pause (Pauseboost)",
        )),
        required(p(
            "rebinds",
            "bind mwheeldown +use; bind mwheelup +use",
            "Колесо: использовать (Wall Climb)",
            "Wheel: use (Wall Climb)",
        )),
        p(
            "rebinds",
            "bind mwheeldown +jump; bind mwheelup +jump",
            "Колесо: прыжок (AHop)",
            "Wheel: jump (AHop)",
        ),
        p("run", "load start;sv_cheats 0", "Начать ран", "Start run"),
        p("run", "sv_cheats 0", "Читы выключить", "Cheats off").note(
            "В конце игры отключает ускорение катсцены.",
            "At the end of the game, turns off the cutscene speedup.",
        ),
        p("run", "r_portalsopenall 1", "Открыть всю карту", "Open all portals")
            .note("Риск краша игры.", "May crash the game."),
        p("run", "exec autoexec", "Применить autoexec", "Run autoexec").note(
            "Заново выполняет autoexec.cfg со всеми настройками.",
            "Executes autoexec.cfg again with all settings.",
        ),
        p("misc", "gh_spray", "Спрей Ghosting", "Ghosting spray").note(
            "Рисует свою картинку на поверхности, куда смотришь. Картинка выбирается в настройках мода: «Эффекты и спреи».",
            "Sprays your picture on the surface you are looking at. Pick the picture in the mod settings: Effects and sprays.",
        ),
    ]);
    list.extend(source_misc());
    list
}

// ------------------------------------------------------------------ Half-Life 2

fn hl2_categories() -> Vec<Category> {
    vec![
        cat("movement", "blue", "Движение", "Movement"),
        cat("weapons", "orange", "Оружие", "Weapons"),
        cat("saves", "green", "Сохранения", "Saves"),
        cat("fps", "yellow", "FPS", "FPS"),
        cat("misc", "grey", "Разное", "Misc"),
    ]
}

fn hl2_presets(game_id: &str, platform: Platform) -> Vec<Preset> {
    let steampipe = game_id == "hl2-5288";
    let mut list = source_movement();
    list.extend(source_combat());
    list.extend(HL2_WEAPONS.iter().map(use_weapon));
    list.extend([
        p("saves", "save quick", "Быстрое сохранение", "Quick save"),
        p("saves", "load quick", "Быстрая загрузка", "Quick load"),
        p("saves", "save sl;load sl", "Сейвлоад", "Save & load").note(
            "Тик-в-тик и не трогает квиксейв.",
            "Tick-perfect and keeps your quick save.",
        ),
        p(
            "saves",
            "load sl",
            "Загрузить сейвлоад",
            "Load save & load save",
        ),
        p(
            "saves",
            "save quick;load quick",
            "Сейвлоад через квиксейв",
            "Quick save & load",
        ),
    ]);
    list.push(if steampipe {
        p(
            "saves",
            &steampipe_save_deletion(platform),
            "Удаление сейва",
            "Save deletion",
        )
    } else {
        p(
            "saves",
            &pre_steampipe_save_deletion(),
            "Удаление сейва",
            "Save deletion",
        )
        .note(
            "Способ для версий до Steampipe.",
            "The method for pre-Steampipe versions.",
        )
    });
    list.extend([
        p(
            "saves",
            "reload",
            "Перезагрузить последний сейв",
            "Reload last save",
        ),
        p(
            "saves",
            "load autosave",
            "Загрузить автосейв",
            "Load autosave",
        ),
        p("fps", "save fps;load fps;fps_max 30", "30 FPS", "30 FPS").note(
            "Для войдклипов и сложных прыжков.",
            "For voidclips and hard jumps.",
        ),
        p("fps", "save fps;load fps;fps_max 300", "300 FPS", "300 FPS"),
        p(
            "fps",
            "reload;fps_max 30",
            "Перезагрузка с 30 FPS",
            "Reload at 30 FPS",
        ),
        p(
            "fps",
            "reload;fps_max 300",
            "Перезагрузка с 300 FPS",
            "Reload at 300 FPS",
        ),
    ]);
    list.extend(source_misc());
    list
}

// ------------------------------------------------------------------- Каталог

/// Ключ сравнения команд: регистр и пробелы вокруг «;» движку не важны.
pub fn command_key(command: &str) -> String {
    // .toLowerCase().replace(/\s*;\s*/g, ";").replace(/\s+/g, " ").trim()
    let lower = command.to_lowercase();
    let mut tight = String::with_capacity(lower.len());
    let mut pending = String::new();
    let mut after_semicolon = false;
    for c in lower.chars() {
        if js::is_whitespace(c) {
            if !after_semicolon {
                pending.push(c);
            }
        } else if c == ';' {
            pending.clear();
            tight.push(';');
            after_semicolon = true;
        } else {
            tight.push_str(&pending);
            pending.clear();
            tight.push(c);
            after_semicolon = false;
        }
    }
    tight.push_str(&pending);
    let mut spaced = String::with_capacity(tight.len());
    let mut in_run = false;
    for c in tight.chars() {
        if js::is_whitespace(c) {
            if !in_run {
                spaced.push(' ');
            }
            in_run = true;
        } else {
            spaced.push(c);
            in_run = false;
        }
    }
    js::trim(&spaced).to_owned()
}

fn build(
    game_id: &str,
    categories: Vec<Category>,
    presets: Vec<Preset>,
    run_modes: bool,
) -> Catalog {
    let mut by_command = JsObject::new();
    for (index, preset) in presets.iter().enumerate() {
        let key = command_key(&preset.command);
        if !by_command.contains_key(&key) {
            by_command.insert(key, index);
        }
    }
    Catalog {
        game_id: game_id.to_owned(),
        engine: engine_for(game_id).expect("у каталога есть движок"),
        categories,
        presets,
        by_command,
        run_modes,
    }
}

fn build_all(platform: Platform) -> Vec<Catalog> {
    vec![
        build("hl1-won", hl1_categories(), hl1_presets(), true),
        build(
            "ghosting-renewed",
            ghosting_categories(),
            ghosting_presets(platform),
            false,
        ),
        build(
            "hl2-4260",
            hl2_categories(),
            hl2_presets("hl2-4260", platform),
            false,
        ),
        build(
            "hl2-5288",
            hl2_categories(),
            hl2_presets("hl2-5288", platform),
            false,
        ),
        build(
            "hl2-2707",
            hl2_categories(),
            hl2_presets("hl2-2707", platform),
            false,
        ),
    ]
}

static LINUX: LazyLock<Vec<Catalog>> = LazyLock::new(|| build_all(Platform::Linux));
static WIN32: LazyLock<Vec<Catalog>> = LazyLock::new(|| build_all(Platform::Win32));

/// Заготовки игры для ОС; игры без каталога — `None`.
pub fn catalog_for(game_id: &str, platform: Platform) -> Option<&'static Catalog> {
    let catalogs = match platform {
        Platform::Linux => &*LINUX,
        Platform::Win32 => &*WIN32,
    };
    catalogs.iter().find(|catalog| catalog.game_id == game_id)
}

impl Catalog {
    /// Заготовка команды: регистр и пробелы вокруг «;» не важны.
    pub fn preset_of(&self, command: &str) -> Option<&Preset> {
        self.by_command
            .get(&command_key(command))
            .map(|&index| &self.presets[index])
    }
}

/// Alias скриптов для назначенных команд, без повторов, в порядке заготовок.
pub fn aliases_for(
    game_id: &str,
    platform: Platform,
    overrides: &Overrides,
) -> Vec<(String, String)> {
    let Some(catalog) = catalog_for(game_id, platform) else {
        return Vec::new();
    };
    let used: HashSet<String> = overrides
        .values()
        .filter(|entry| !entry.command.is_empty())
        .map(|entry| command_key(&entry.command))
        .collect();
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for preset in &catalog.presets {
        if !used.contains(&command_key(&preset.command)) {
            continue;
        }
        for (name, value) in &preset.aliases {
            if seen.insert(name.as_str()) {
                result.push((name.clone(), value.clone()));
            }
        }
    }
    result
}

/// Скрипты гайда, которые ждут «auto_alias» в game.cfg (см. `hl1_guide`).
pub fn needs_auto_alias(game_id: &str, platform: Platform, overrides: &Overrides) -> bool {
    let Some(catalog) = catalog_for(game_id, platform) else {
        return false;
    };
    overrides
        .values()
        .filter(|entry| !entry.command.is_empty())
        .any(|entry| {
            catalog
                .preset_of(&entry.command)
                .is_some_and(|preset| preset.auto_alias == Some(true))
        })
}

/// Команды, разрешённые только в Scripted (белый список команд GoldSrc на
/// speedrun.com/hl1): ожидание, TAS-функции BXT и bxt_autojump*. В 0.4.7 это
/// `/^(wait|w|[+-]?_?bxt_tas_.*|bxt_autojump.*)$/i`: `.` не берёт переводы строк, а
/// флаг `i` без `u` сводит регистр только у ASCII-букв, поэтому выражение записано
/// вручную.
fn is_script_only_command(verb: &str) -> bool {
    fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
        let head = text.get(..prefix.len())?;
        head.eq_ignore_ascii_case(prefix)
            .then(|| &text[prefix.len()..])
    }
    if verb.eq_ignore_ascii_case("wait") || verb.eq_ignore_ascii_case("w") {
        return true;
    }
    let signed = verb.strip_prefix(['+', '-']).unwrap_or(verb);
    let signed = signed.strip_prefix('_').unwrap_or(signed);
    strip_prefix_ci(signed, "bxt_tas_")
        .or_else(|| strip_prefix_ci(verb, "bxt_autojump"))
        .is_some_and(|rest| !rest.contains(['\n', '\r', '\u{2028}', '\u{2029}']))
}

const SCRIPTED_DEPTH: usize = 16;

/// Разбор alias игрока, нужный [`is_scripted`]: клавиатура спрашивает про каждую
/// клавишу, поэтому считается один раз на набор alias.
#[derive(Debug, Clone, Default)]
pub struct AliasInfo {
    pub map: AliasMap,
    pub reassigned: HashSet<String>,
}

impl AliasInfo {
    pub fn new(aliases: &JsObject<String>) -> AliasInfo {
        let map = alias_map(aliases);
        let reassigned = reassigned_aliases(&map);
        AliasInfo { map, reassigned }
    }
}

/// Скрипт ли команда: известная заготовка — по её отметке, своя команда — по
/// правилам Scriptless: больше одной команды на бинд, ожидание, TAS-функции BXT,
/// alias на +/- команду. `aliases` — alias из cfg игрока: одно слово в бинде часто
/// прячет скрипт («bind f fps4», «bind alt re4»). Alias, который переопределяют
/// другие alias, — шаг или переключатель скрипта.
pub fn is_scripted(catalog: Option<&Catalog>, command: &str, aliases: &AliasInfo) -> bool {
    if command.is_empty() {
        return false;
    }
    if let Some(preset) = catalog.and_then(|catalog| catalog.preset_of(command)) {
        return preset.scripted;
    }
    fn scripted(
        text: &str,
        depth: usize,
        aliases: &AliasInfo,
        visited: &mut HashSet<String>,
    ) -> bool {
        let commands = parse_cfg(text);
        if commands.len() > 1 {
            return true;
        }
        let Some(args) = commands.first() else {
            return false;
        };
        if depth > SCRIPTED_DEPTH {
            return false;
        }
        let verb = args[0].to_lowercase();
        if is_script_only_command(&verb) {
            return true;
        }
        // Перебинд и alias выполнят свою команду позже: скрипт ли она.
        if (verb == "bind" || verb == "alias") && args.len() >= 3 {
            return scripted(&args[2..].join(" "), depth + 1, aliases, visited);
        }
        let Some(body) = aliases.map.get(&verb) else {
            return false;
        };
        if verb.starts_with(['+', '-']) || aliases.reassigned.contains(&verb) {
            return true;
        }
        if !visited.insert(verb) {
            return false;
        }
        scripted(body, depth + 1, aliases, visited)
    }
    scripted(command, 0, aliases, &mut HashSet::new())
}
