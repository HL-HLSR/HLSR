"use strict";

// Заготовки биндов для вкладки «Клавиатура»: у каждой игры свои категории.
//
// Заготовка — команда для бинда, подпись и, если это скрипт, alias, без
// которых команда не работает. Alias пишутся в hlsr-binds.cfg, только пока
// команда назначена на клавишу.
//
// scripted — команда автоматизирует нажатия (alias, wait, несколько команд,
// TAS-функции BXT). В HL1 такие бинды допустимы только в категориях Scripted,
// поэтому клавиатура HL1 прячет их в режиме Scriptless.
// practice — для тренировки: в ране запрещено.
//
// Источники:
// - HL1: gfx/shell/kb_act.lst сборки, гайд https://github.com/yavats/hl1-scripts
//   (базовые скрипты) и команды Bunnymod XT из BunnymodXT.dll;
// - Ghosting: стандартный autoexec.cfg лаунчера (main/game-delivery/ghosting-default-autoexec.js);
// - HL2: гайд New Engine на speedrun.com/hl2 (раздел Key bindings).

const hl1Guide = require("./hl1-guide");
const gameBinds = require("./game-binds");

// В renderer нет process: там платформу сообщает preload.
const IS_LINUX = typeof process !== "undefined" && process.platform
  ? process.platform === "linux"
  : typeof window !== "undefined" && !!window.hlsrElectron && window.hlsrElectron.platform === "linux";

// Удаление сейва в Ghosting и Steampipe-сборках HL2. Под Linux (Wine)
// «save :» сейв не удаляет, там работает «save /».
const SAVE_DELETION_NAME = IS_LINUX ? "/" : ":";
const GHOSTING_SAVE_DELETION = `save ${SAVE_DELETION_NAME}; reload`;
const STEAMPIPE_SAVE_DELETION = `save ${SAVE_DELETION_NAME};reload`;

const L = (ru, en) => ({ ru, en });

// Тон задаёт цвет категории на клавиатуре.
const cat = (id, tone, title) => ({ id, tone, title });

// [команда, подпись, доп. поля]
// listed: false — заготовку не предлагают в списке клавиатуры (её назначают
// в другой вкладке), но клавиатура узнаёт её на клавише.
const p = (category, command, label, extra) =>
  Object.assign({ category, command, label, scripted: false, practice: false, listed: true, aliases: [] }, extra || {});

const GAME_ENGINES = {
  "hl1-won": "goldsrc",
  "ghosting-renewed": "source",
  "hl2-4260": "source",
  "hl2-5288": "source",
  "hl2-2707": "source-old",
};

// ---------------------------------------------------------------- Half-Life 1

const HL1_CATEGORIES = [
  cat("movement", "blue", L("Движение", "Movement")),
  cat("weapons", "orange", L("Оружие", "Weapons")),
  cat("saves", "green", L("Сохранения", "Saves")),
  cat("fps", "yellow", L("FPS", "FPS")),
  cat("scripts", "purple", L("Скрипты", "Scripts")),
  cat("bxt", "pink", L("Bunnymod XT", "Bunnymod XT")),
  cat("practice", "red", L("Тренировка", "Practice")),
  cat("misc", "grey", L("Разное", "Misc")),
  cat("guide", "purple", L("Скрипты гайда", "Guide scripts")),
];

const HL1_WEAPONS = [
  ["weapon_crowbar", L("Монтировка", "Crowbar")],
  ["weapon_9mmhandgun", L("Пистолет", "Glock")],
  ["weapon_357", L("Револьвер .357", ".357 Magnum")],
  ["weapon_9mmAR", L("Автомат MP5", "MP5")],
  ["weapon_shotgun", L("Дробовик", "Shotgun")],
  ["weapon_crossbow", L("Арбалет", "Crossbow")],
  ["weapon_rpg", L("Ракетница", "RPG")],
  ["weapon_gauss", L("Гаусс", "Gauss")],
  ["weapon_egon", L("Глюонная пушка", "Gluon gun")],
  ["weapon_hornetgun", L("Улейная пушка", "Hivehand")],
  ["weapon_handgrenade", L("Граната", "Grenade")],
  ["weapon_satchel", L("Сумка со взрывчаткой", "Satchel")],
  ["weapon_tripmine", L("Мина-растяжка", "Tripmine")],
  ["weapon_snark", L("Снарки", "Snarks")],
];

// Скрипты из гайда hl1-scripts. «w N» — встроенное ожидание Bunnymod XT.
// Старт рана из стокового userconfig.cfg пакета Half-Life 2005 WON: так же
// его даёт гайд «FAQ & Setup» на speedrun.com/hl1.
const HL1_RUN_ALIAS = "skill 1;map c1a0;bxt_hud_timer 1;bxt_timer_reset;bxt_timer_start;bxt_autorecord run";

const HL1_PRESETS = [
  p("movement", "+forward", L("Вперёд", "Forward")),
  p("movement", "+back", L("Назад", "Back")),
  p("movement", "+moveleft", L("Влево", "Strafe left")),
  p("movement", "+moveright", L("Вправо", "Strafe right")),
  p("movement", "+jump", L("Прыжок", "Jump")),
  p("movement", "+duck", L("Присесть", "Duck")),
  p("movement", "+speed", L("Шаг", "Walk")),
  p("movement", "+use", L("Использовать", "Use")),
  p("movement", "+moveup", L("Всплыть", "Swim up")),
  p("movement", "+movedown", L("Погрузиться", "Swim down")),
  p("movement", "+left", L("Поворот влево", "Turn left")),
  p("movement", "+right", L("Поворот вправо", "Turn right")),
  p("movement", "+lookup", L("Взгляд вверх", "Look up")),
  p("movement", "+lookdown", L("Взгляд вниз", "Look down")),
  p("movement", "centerview", L("Взгляд прямо", "Center view")),
  p("movement", "+strafe", L("Стрейф", "Strafe")),
  p("movement", "+mlook", L("Обзор мышью", "Mouse look")),
  p("movement", "+klook", L("Обзор клавишами", "Keyboard look")),

  p("weapons", "+attack", L("Огонь", "Fire")),
  p("weapons", "+attack2", L("Альтернативный огонь", "Secondary fire")),
  p("weapons", "+reload", L("Перезарядка", "Reload")),
  p("weapons", "slot1", L("Слот 1", "Slot 1")),
  p("weapons", "slot2", L("Слот 2", "Slot 2")),
  p("weapons", "slot3", L("Слот 3", "Slot 3")),
  p("weapons", "slot4", L("Слот 4", "Slot 4")),
  p("weapons", "slot5", L("Слот 5", "Slot 5")),
  p("weapons", "invprev", L("Предыдущее оружие", "Previous weapon")),
  p("weapons", "invnext", L("Следующее оружие", "Next weapon")),
  p("weapons", "lastinv", L("Последнее оружие", "Last weapon")),
  ...HL1_WEAPONS.map(([command, label]) => p("weapons", command, label)),
  p("weapons", "impulse 100", L("Фонарик", "Flashlight")),
  p("weapons", "impulse 201", L("Спрей", "Spray")),

  p("saves", "save quick", L("Быстрое сохранение", "Quick save")),
  p("saves", "load quick", L("Быстрая загрузка", "Quick load")),
  p("saves", "save x; load x", L("Сейвлоад", "Save & load"), {
    scripted: true,
    note: L("Сохранение и загрузка одним нажатием, квиксейв не трогает.", "Saves and loads with one key press and keeps your quick save."),
  }),

  p("fps", "fps_max 100", L("100 FPS", "100 FPS"), {
    note: L("Максимум по правилам HL1.", "The maximum allowed by HL1 rules."),
  }),
  p("fps", "fps_max 20", L("20 FPS", "20 FPS")),
  p("fps", "+fps_20_hold", L("20 FPS, пока держишь", "20 FPS while held"), {
    scripted: true,
    note: L("Держите клавишу — 20 FPS, отпустите — снова 100.", "Hold for 20 FPS, release to go back to 100."),
    aliases: [
      ["+fps_20_hold", "fps_max 20; alias -fps_20_hold fps_20_reset"],
      ["fps_20_reset", "fps_max 100; alias -fps_20_hold"],
    ],
  }),
  p("fps", "fps_20_toggle", L("20 / 100 FPS переключатель", "20 / 100 FPS toggle"), {
    scripted: true,
    note: L("Нажатие — 20 FPS, ещё нажатие — 100.", "Press for 20 FPS, press again for 100."),
    aliases: [
      ["fps_20_toggle", "fps_20_toggle1"],
      ["fps_20_toggle1", "fps_max 20; alias fps_20_toggle fps_20_toggle2"],
      ["fps_20_toggle2", "fps_max 100; alias fps_20_toggle fps_20_toggle1"],
    ],
  }),
  p("fps", "4fpsbind", L("Кадр на 4 FPS", "One 4 FPS frame"), {
    scripted: true,
    note: L("Один кадр на 4 FPS, потом снова 100.", "One frame at 4 FPS, then back to 100."),
    aliases: [["4fpsbind", "fps_max 4; w; fps_max 100"]],
  }),

  p("scripts", "+usespam", L("Спам «Использовать»", "Use spam"), {
    scripted: true,
    note: L("Пока клавиша зажата, жмёт +use через кадр.", "Presses +use every other frame while held."),
    aliases: [
      ["+usespam", "alias _zspecial @use;@use1"],
      ["-usespam", "alias _zspecial;-use"],
      ["@use1", "+use;alias @use @use2;bxt_append _zspecial"],
      ["@use2", "w;alias @use @use3;bxt_append _zspecial"],
      ["@use3", "-use;alias @use @use4;bxt_append _zspecial"],
      ["@use4", "w;alias @use @use1;bxt_append _zspecial"],
    ],
  }),
  p("scripts", "+duckroll", L("Даккролл", "Duckroll"), {
    scripted: true,
    note: L("Спам приседа для водных бустов. Для движения лучше +bxt_tas_ducktap.", "Duck spam for water boosts. For movement +bxt_tas_ducktap is better."),
    aliases: [
      ["+duckroll", "alias _zspecial @duck;@dr1"],
      ["-duckroll", "alias _zspecial;-duck"],
      ["@dr1", "+duck;alias @duck @dr2;bxt_append _zspecial"],
      ["@dr2", "w;alias @duck @dr3;bxt_append _zspecial"],
      ["@dr3", "-duck;alias @duck @dr4;bxt_append _zspecial"],
      ["@dr4", "w;alias @duck @dr1;bxt_append _zspecial"],
    ],
  }),
  p("scripts", "+tau", L("Гаусс-буст 180°", "180° gauss boost"), {
    scripted: true,
    note: L("Держите — заряд, отпустите — выстрел себе под ноги.", "Hold to charge, release to boost."),
    aliases: [
      ["_taubo", ""],
      ["_taubofunc", "cl_pitchup 180;cl_pitchdown -180;-attack2;wait;cl_pitchup 89;cl_pitchdown 89;force_centerview"],
      ["+tau", "+attack2;alias _taubo _taubofunc"],
      ["-tau", "_taubo;alias _taubo"],
    ],
  }),
  p("scripts", "slowgauss", L("Медленный гаусс", "Slow gauss"), {
    scripted: true,
    note: L("Заряд на ПКМ, сразу ЛКМ, через ~3 с — эта клавиша. Нужно 1–2 патрона.", "Charge with MOUSE2, switch to MOUSE1, after ~3 s press this key. Needs 1–2 ammo."),
    aliases: [["slowgauss", "cl_pitchup 180;cl_pitchdown -180;-attack;w 50;cl_pitchup 89;cl_pitchdown 89;force_centerview"]],
  }),
  p("scripts", "tauswitch", L("Переключатель ПКМ: гаусс-буст", "MOUSE2 gauss boost switch"), {
    scripted: true,
    note: L("Меняет бинд ПКМ между +tau и +attack2.", "Switches MOUSE2 between +tau and +attack2."),
    aliases: [
      ["_taubo", ""],
      ["_taubofunc", "cl_pitchup 180;cl_pitchdown -180;-attack2;wait;cl_pitchup 89;cl_pitchdown 89;force_centerview"],
      ["+tau", "+attack2;alias _taubo _taubofunc"],
      ["-tau", "_taubo;alias _taubo"],
      ["tauswitch", "tauswitch1"],
      ["tauswitch1", "bind mouse2 +tau; say gaussboost_ON; alias tauswitch tauswitch2"],
      ["tauswitch2", "bind mouse2 +attack2; say gaussboost_OFF; alias tauswitch tauswitch1"],
    ],
  }),
  ...[
    ["obbo2000", "2000", 11],
    ["obbo1600", "1600", 10],
    ["obbo1300", "1300", 9],
    ["obbo1000", "1000", 8],
    ["obbo800", "800", 7],
  ].map(([name, ups, wait]) =>
    p("scripts", name, L(`Буст от объекта ${ups} UPS`, `Object boost ${ups} UPS`), {
      scripted: true,
      aliases: [[name, `+use;w ${wait};-use;+jump;w;-jump`]],
    })
  ),
  p("scripts", "obboshoot", L("Буст от объекта вперёд", "Forward object boost"), {
    scripted: true,
    note: L("~1500–1700 UPS вперёд.", "~1500–1700 UPS forward."),
    aliases: [["obboshoot", "+use;w 10;-use;+jump;w;-jump;+attack;w;-attack"]],
  }),
  p("scripts", "app_obbo", L("Обжбуст с дактапом", "Obbo ducktap"), {
    scripted: true,
    note: L("Apprehension: спрыгнуть, зажать A, нажать скрипт.", "Apprehension: drop down, hold A, press the script."),
    aliases: [["app_obbo", "+use; w 9; -use; w; +duck; w 8; -duck"]],
  }),

  p("bxt", "+bxt_tas_ducktap", L("Дактап (BXT)", "Ducktap (BXT)"), { scripted: true }),
  p("bxt", "+bxt_tas_jumpbug", L("Джампбаг (BXT)", "Jumpbug (BXT)"), { scripted: true }),
  p("bxt", "+bxt_tas_autojump", L("TAS-автопрыжок (BXT)", "TAS autojump (BXT)"), {
    scripted: true,
    note: L("Держите клавишу — BXT прыгает в первый кадр на земле.", "Hold the key and BXT jumps on the first frame on the ground."),
  }),
  // Обычный автопрыжок 0.4.3: в списке его больше нет, но на клавише узнаётся.
  p("bxt", "+autojump", L("Автопрыжок (BXT)", "Autojump (BXT)"), {
    scripted: true,
    listed: false,
    note: L("Держите клавишу — BXT прыгает сам при каждом приземлении.", "Hold the key and BXT jumps on every landing."),
    aliases: [
      ["+autojump", "bxt_autojump 1; +jump"],
      ["-autojump", "-jump; bxt_autojump 0"],
    ],
  }),
  p("bxt", "bxt_timer_reset", L("Сбросить таймер", "Reset timer")),
  p("bxt", "bxt_timer_start", L("Запустить таймер", "Start timer")),
  p("bxt", "bxt_timer_stop", L("Остановить таймер", "Stop timer")),
  p("bxt", "run", L("Начать ран", "Start run"), {
    note: L("Сложность, c1a0, таймер BXT и запись демок, как в стандартном userconfig.cfg.",
      "Difficulty, c1a0, BXT timer and demo recording, as in the standard userconfig.cfg."),
    aliases: [["run", HL1_RUN_ALIAS]],
  }),
  p("bxt", "stop", L("Остановить запись демки", "Stop demo recording")),

  p("practice", "sv_cheats 1", L("Читы включить", "Cheats on"), { practice: true }),
  p("practice", "sv_cheats 0", L("Читы выключить", "Cheats off")),
  p("practice", "noclip", L("Полёт сквозь стены", "Noclip"), { practice: true }),
  p("practice", "god", L("Бессмертие", "God mode"), { practice: true }),
  p("practice", "notarget", L("Невидимость для врагов", "Notarget"), { practice: true }),
  p("practice", "impulse 101", L("Всё оружие", "All weapons"), { practice: true }),

  p("misc", "toggleconsole", L("Консоль", "Console")),
  p("misc", "cancelselect", L("Отмена / меню", "Cancel / menu")),
  p("misc", "pause", L("Пауза", "Pause")),
  p("misc", "snapshot", L("Скриншот", "Screenshot")),
  p("misc", "+showscores", L("Таблица очков", "Scoreboard")),
  p("misc", "messagemode", L("Чат", "Chat")),
  p("misc", "messagemode2", L("Командный чат", "Team chat")),
  p("misc", "+voicerecord", L("Голосовой чат", "Voice chat")),
  p("misc", "sizeup", L("Больше экран", "Screen size up")),
  p("misc", "sizedown", L("Меньше экран", "Screen size down")),
  p("misc", "quit prompt", L("Выход из игры", "Quit")),
];

// Действия скриптов гайда hl1-scripts (вкладка «Скрипты»). Команды, которые
// уже есть выше (базовые скрипты гайда), остаются прежними заготовками: их
// alias совпадают с гайдом. Подписи у них — английские названия из гайда,
// как во вкладке «Скрипты». Общие команды BXT (+bxt_tas_jumpbug) не трогаются.
function hl1GuidePresets(existing) {
  const byKey = new Map(existing.map((preset) => [commandKey(preset.command), preset]));
  const known = new Set(byKey.keys());
  const result = [];
  for (const script of hl1Guide.SCRIPTS) {
    for (const action of script.actions) {
      const key = commandKey(action.command);
      const label = script.actions.length > 1 ? `${script.title}: ${action.command}` : script.title;
      const preset = byKey.get(key);
      if (preset && preset.category !== "bxt" && preset.aliases.length) {
        preset.label = L(label, label);
        if (!preset.guide) preset.guide = script.id;
      }
      if (known.has(key)) continue;
      known.add(key);
      result.push(
        p("guide", action.command, L(label, label), {
          scripted: true,
          listed: false,
          guide: script.id,
          autoAlias: !!script.needsAutoAlias,
          note: L(
            "Скрипт из гайда hl1-scripts. Видео и описание — во вкладке «Скрипты».",
            "A script from the hl1-scripts guide. See the Scripts tab for the video and description."
          ),
          aliases: script.aliases,
        })
      );
    }
  }
  return result;
}

HL1_PRESETS.push(...hl1GuidePresets(HL1_PRESETS));

// ------------------------------------------------------- Source: общие части

const SOURCE_MOVEMENT = [
  p("movement", "+forward", L("Вперёд", "Forward")),
  p("movement", "+back", L("Назад", "Back")),
  p("movement", "+moveleft", L("Влево", "Strafe left")),
  p("movement", "+moveright", L("Вправо", "Strafe right")),
  p("movement", "+jump", L("Прыжок", "Jump"), {
    note: L("Прыжок на колесо мыши нужен для AHop.", "Jump on the mouse wheel is needed for AHop."),
  }),
  p("movement", "+duck", L("Присесть", "Duck")),
  p("movement", "toggle_duck", L("Присесть (переключатель)", "Toggle duck"), {
    note: L("Меняет присед во время AHop без потери скорости.", "Changes crouch state while AHopping without losing speed."),
  }),
  p("movement", "+speed", L("Бег", "Sprint")),
  p("movement", "+walk", L("Шаг", "Walk")),
  p("movement", "+use", L("Использовать", "Use")),
  p("movement", "+zoom", L("Зум", "Zoom")),
  p("movement", "+strafe", L("Стрейф", "Strafe")),
];

const SOURCE_COMBAT = [
  p("weapons", "+attack", L("Огонь", "Fire")),
  p("weapons", "+attack2", L("Альтернативный огонь", "Secondary fire")),
  p("weapons", "+reload", L("Перезарядка", "Reload")),
  ...[1, 2, 3, 4, 5, 6].map((n) => p("weapons", `slot${n}`, L(`Слот ${n}`, `Slot ${n}`))),
  p("weapons", "invprev", L("Предыдущее оружие", "Previous weapon")),
  p("weapons", "invnext", L("Следующее оружие", "Next weapon")),
  p("weapons", "lastinv", L("Последнее оружие", "Last weapon")),
  p("weapons", "phys_swap", L("Грави-пушка ↔ оружие", "Gravity gun swap")),
  p("weapons", "impulse 100", L("Фонарик", "Flashlight")),
];

// После квикклипа оружие листать нельзя: нужен бинд на каждое.
const HL2_WEAPONS = [
  ["weapon_crowbar", L("Монтировка", "Crowbar")],
  ["weapon_physcannon", L("Грави-пушка", "Gravity gun")],
  ["weapon_pistol", L("Пистолет", "Pistol")],
  ["weapon_357", L("Револьвер .357", ".357 Magnum")],
  ["weapon_smg1", L("Автомат SMG", "SMG")],
  ["weapon_ar2", L("Импульсная винтовка", "Pulse rifle")],
  ["weapon_shotgun", L("Дробовик", "Shotgun")],
  ["weapon_crossbow", L("Арбалет", "Crossbow")],
  ["weapon_frag", L("Граната", "Grenade")],
  ["weapon_rpg", L("Ракетница", "RPG")],
  ["weapon_bugbait", L("Феропод", "Bugbait")],
];
const useWeapon = ([weapon, label]) =>
  p("weapons", `use ${weapon}`, label, {
    note: L("Работает и после квикклипа.", "Works after a quickclip too."),
  });

const SOURCE_MISC = [
  p("misc", "toggleconsole", L("Консоль", "Console")),
  p("misc", "cancelselect", L("Отмена / меню", "Cancel / menu")),
  p("misc", "pause", L("Пауза", "Pause")),
  p("misc", "jpeg", L("Скриншот", "Screenshot")),
  p("misc", "quit", L("Выход из игры", "Quit")),
];

// ------------------------------------------------------------------ Ghosting

const GHOSTING_CATEGORIES = [
  cat("movement", "blue", L("Движение", "Movement")),
  cat("weapons", "orange", L("Оружие", "Weapons")),
  cat("saves", "green", L("Сохранения", "Saves")),
  cat("rebinds", "yellow", L("Рибинды колеса", "Wheel rebinds")),
  cat("run", "pink", L("Ран", "Run")),
  cat("misc", "grey", L("Разное", "Misc")),
];

// Оружие из стандартного autoexec Ghosting идёт первым: им пользуются после Yesclip.
const GHOSTING_WEAPON_ORDER = ["weapon_crowbar", "weapon_pistol", "weapon_smg1", "weapon_shotgun", "weapon_frag", "weapon_rpg"];

const GHOSTING_PRESETS = [
  ...SOURCE_MOVEMENT,
  ...SOURCE_COMBAT,
  p("weapons", "bug_swap", L("Феропод ↔ оружие", "Bugbait swap")),
  ...GHOSTING_WEAPON_ORDER.map((name) => HL2_WEAPONS.find((item) => item[0] === name)).map(useWeapon),
  ...HL2_WEAPONS.filter((item) => !GHOSTING_WEAPON_ORDER.includes(item[0])).map(useWeapon),

  p("saves", "save quick", L("Быстрое сохранение", "Quick save")),
  p("saves", "load quick", L("Быстрая загрузка", "Quick load")),
  p("saves", "save x; load x", L("Сейвлоад", "Save & load"), {
    note: L("Сохранение и загрузка одним нажатием, квиксейв не трогает.", "Saves and loads with one key press and keeps your quick save."),
  }),
  p("saves", "load autosave", L("Загрузить автосейв", "Load autosave")),
  p("saves", "load quick01", L("Позапрошлый сейв", "Previous quick save"), {
    note: L("Обязательный бинд: загружает позапрошлый квиксейв.", "Required bind: loads the quick save before the last one."),
  }),
  p("saves", GHOSTING_SAVE_DELETION, L("Удаление сейва", "Save deletion")),
  p("saves", "reload", L("Перезагрузить последний сейв", "Reload last save")),

  p("rebinds", "bind mwheeldown +duck; bind mwheelup +duck", L("Колесо: присед (Duckroll)", "Wheel: duck (Duckroll)"), {
    note: L("Обязателен в ране.", "Required in a run."),
  }),
  p("rebinds", "bind mwheeldown pause; bind mwheelup pause", L("Колесо: пауза (Pauseboost)", "Wheel: pause (Pauseboost)"), {
    note: L("Обязателен в ране.", "Required in a run."),
  }),
  p("rebinds", "bind mwheeldown +use; bind mwheelup +use", L("Колесо: использовать (Wall Climb)", "Wheel: use (Wall Climb)"), {
    note: L("Обязателен в ране.", "Required in a run."),
  }),
  p("rebinds", "bind mwheeldown +jump; bind mwheelup +jump", L("Колесо: прыжок (AHop)", "Wheel: jump (AHop)")),

  p("run", "load start;sv_cheats 0", L("Начать ран", "Start run")),
  p("run", "sv_cheats 0", L("Читы выключить", "Cheats off"), {
    note: L("В конце игры отключает ускорение катсцены.", "At the end of the game, turns off the cutscene speedup."),
  }),
  p("run", "r_portalsopenall 1", L("Открыть всю карту", "Open all portals"), {
    note: L("Риск краша игры.", "May crash the game."),
  }),
  p("run", "exec autoexec", L("Применить autoexec", "Run autoexec"), {
    note: L("Заново выполняет autoexec.cfg со всеми настройками.", "Executes autoexec.cfg again with all settings."),
  }),

  p("misc", "gh_spray", L("Спрей Ghosting", "Ghosting spray"), {
    note: L("Рисует свою картинку на поверхности, куда смотришь. Картинка выбирается в настройках мода: «Эффекты и спреи».", "Sprays your picture on the surface you are looking at. Pick the picture in the mod settings: Effects and sprays."),
  }),
  ...SOURCE_MISC,
];

// -------------------------------------------------------------- Half-Life 2

const HL2_CATEGORIES = [
  cat("movement", "blue", L("Движение", "Movement")),
  cat("weapons", "orange", L("Оружие", "Weapons")),
  cat("saves", "green", L("Сохранения", "Saves")),
  cat("fps", "yellow", L("FPS", "FPS")),
  cat("misc", "grey", L("Разное", "Misc")),
];

// Удаление сейва в версиях до Steampipe (5135 и старше): имя сейва ровно
// такой длины. В Steampipe (5288 и новее) — STEAMPIPE_SAVE_DELETION.
const PRE_STEAMPIPE_SAVE_DELETION = `save ${"a".repeat(224)};reload`;

function hl2Presets(gameId) {
  const steampipe = gameId === "hl2-5288";
  return [
    ...SOURCE_MOVEMENT,
    ...SOURCE_COMBAT,
    ...HL2_WEAPONS.map(useWeapon),

    p("saves", "save quick", L("Быстрое сохранение", "Quick save")),
    p("saves", "load quick", L("Быстрая загрузка", "Quick load")),
    p("saves", "save sl;load sl", L("Сейвлоад", "Save & load"), {
      note: L("Тик-в-тик и не трогает квиксейв.", "Tick-perfect and keeps your quick save."),
    }),
    p("saves", "load sl", L("Загрузить сейвлоад", "Load save & load save")),
    p("saves", "save quick;load quick", L("Сейвлоад через квиксейв", "Quick save & load")),
    steampipe
      ? p("saves", STEAMPIPE_SAVE_DELETION, L("Удаление сейва", "Save deletion"))
      : p("saves", PRE_STEAMPIPE_SAVE_DELETION, L("Удаление сейва", "Save deletion"), {
          note: L("Способ для версий до Steampipe.", "The method for pre-Steampipe versions."),
        }),
    p("saves", "reload", L("Перезагрузить последний сейв", "Reload last save")),
    p("saves", "load autosave", L("Загрузить автосейв", "Load autosave")),

    p("fps", "save fps;load fps;fps_max 30", L("30 FPS", "30 FPS"), {
      note: L("Для войдклипов и сложных прыжков.", "For voidclips and hard jumps."),
    }),
    p("fps", "save fps;load fps;fps_max 300", L("300 FPS", "300 FPS")),
    p("fps", "reload;fps_max 30", L("Перезагрузка с 30 FPS", "Reload at 30 FPS")),
    p("fps", "reload;fps_max 300", L("Перезагрузка с 300 FPS", "Reload at 300 FPS")),

    ...SOURCE_MISC,
  ];
}

// ------------------------------------------------------------------- Каталог

// Ключ сравнения команд: регистр и пробелы вокруг «;» движку не важны.
function commandKey(command) {
  return String(command).toLowerCase().replace(/\s*;\s*/g, ";").replace(/\s+/g, " ").trim();
}

function build(gameId, categories, presets, extra) {
  const byCommand = Object.create(null);
  for (const preset of presets) {
    const key = commandKey(preset.command);
    if (!byCommand[key]) byCommand[key] = preset;
  }
  return Object.assign(
    { gameId, engine: GAME_ENGINES[gameId], categories, presets, byCommand, runModes: false },
    extra || {}
  );
}

const CATALOGS = {
  "hl1-won": build("hl1-won", HL1_CATEGORIES, HL1_PRESETS, { runModes: true }),
  "ghosting-renewed": build("ghosting-renewed", GHOSTING_CATEGORIES, GHOSTING_PRESETS),
  "hl2-4260": build("hl2-4260", HL2_CATEGORIES, hl2Presets("hl2-4260")),
  "hl2-5288": build("hl2-5288", HL2_CATEGORIES, hl2Presets("hl2-5288")),
  "hl2-2707": build("hl2-2707", HL2_CATEGORIES, hl2Presets("hl2-2707")),
};

function catalogFor(gameId) {
  return CATALOGS[gameId] || null;
}

function engineFor(gameId) {
  return GAME_ENGINES[gameId] || null;
}

function presetOf(catalog, command) {
  if (!catalog || typeof command !== "string") return null;
  return catalog.byCommand[commandKey(command)] || null;
}

// Alias скриптов для назначенных команд, без повторов, в порядке заготовок.
function aliasesFor(gameId, overrides) {
  const catalog = catalogFor(gameId);
  if (!catalog || !overrides) return [];
  const used = new Set();
  for (const entry of Object.values(overrides)) {
    if (entry && entry.command) used.add(commandKey(entry.command));
  }
  const result = [];
  const seen = new Set();
  for (const preset of catalog.presets) {
    if (!used.has(commandKey(preset.command))) continue;
    for (const [name, value] of preset.aliases) {
      if (seen.has(name)) continue;
      seen.add(name);
      result.push([name, value]);
    }
  }
  return result;
}

// Скрипты гайда, которые ждут «auto_alias» в game.cfg (см. hl1-guide.js).
function needsAutoAlias(gameId, overrides) {
  const catalog = catalogFor(gameId);
  if (!catalog || !overrides) return false;
  return Object.values(overrides).some((entry) => {
    const preset = entry && entry.command ? presetOf(catalog, entry.command) : null;
    return !!(preset && preset.autoAlias);
  });
}

// Команды, разрешённые только в Scripted (белый список команд GoldSrc на
// speedrun.com/hl1): ожидание, TAS-функции BXT и bxt_autojump*.
const SCRIPT_ONLY_COMMAND = /^(wait|w|[+-]?_?bxt_tas_.*|bxt_autojump.*)$/i;

const SCRIPTED_DEPTH = 16;

// Кэш разбора alias игрока: клавиатура спрашивает про каждую клавишу.
const aliasCache = new WeakMap();

function aliasInfo(aliases) {
  if (!aliases || typeof aliases !== "object") return { map: new Map(), reassigned: new Set() };
  let info = aliasCache.get(aliases);
  if (!info) {
    const map = gameBinds.aliasMap(aliases);
    info = { map, reassigned: gameBinds.reassignedAliases(map) };
    aliasCache.set(aliases, info);
  }
  return info;
}

// Скрипт ли команда: известная заготовка — по её отметке, своя команда —
// по правилам Scriptless: больше одной команды на бинд, ожидание, TAS-функции
// BXT, alias на +/- команду. aliases — alias из cfg игрока ({ имя: значение }):
// одно слово в бинде часто прячет скрипт («bind f fps4», «bind alt re4»).
// Alias, который переопределяют другие alias, — шаг или переключатель
// скрипта.
function isScripted(catalog, command, aliases) {
  if (!command) return false;
  const preset = presetOf(catalog, command);
  if (preset) return preset.scripted;
  const { map, reassigned } = aliasInfo(aliases);
  const visited = new Set();
  const scripted = (text, depth) => {
    const commands = gameBinds.parseCfg(text);
    if (commands.length > 1) return true;
    if (!commands.length || depth > SCRIPTED_DEPTH) return false;
    const args = commands[0];
    const verb = args[0].toLowerCase();
    if (SCRIPT_ONLY_COMMAND.test(verb)) return true;
    // Перебинд и alias выполнят свою команду позже: скрипт ли она.
    if ((verb === "bind" || verb === "alias") && args.length >= 3) return scripted(args.slice(2).join(" "), depth + 1);
    if (!map.has(verb)) return false;
    if (/^[+-]/.test(verb) || reassigned.has(verb)) return true;
    if (visited.has(verb)) return false;
    visited.add(verb);
    return scripted(map.get(verb), depth + 1);
  };
  return scripted(String(command), 0);
}

module.exports = {
  GAME_ENGINES,
  CATALOGS,
  PRE_STEAMPIPE_SAVE_DELETION,
  GHOSTING_SAVE_DELETION,
  STEAMPIPE_SAVE_DELETION,
  HL1_RUN_ALIAS,
  commandKey,
  catalogFor,
  engineFor,
  presetOf,
  aliasesFor,
  needsAutoAlias,
  isScripted,
};
