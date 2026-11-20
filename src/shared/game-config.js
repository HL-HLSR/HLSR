"use strict";

// Редактор конфигов во вкладке «Конфиги» настроек игры: понятные настройки
// («Чувствительность», «Громкость») вместо переменных движка.
//
// Значение берётся оттуда, где движок его задаёт последним: цепочка cfg
// выполняется так же, как при запуске игры (config.cfg, потом autoexec.cfg
// или userconfig.cfg, со всеми exec). Новое значение пишется в ту же строку
// того же файла: правка остаётся у игрока на своём месте, с комментарием.
// Если переменная нигде не задана или задана не в файле игрока, она пишется в
// раздел settings файла игрока (userconfig.cfg / autoexec.cfg): он выполняется
// последним и перекрывает config.cfg. config.cfg и WON.cfg лаунчер не правит.
//
// Переменные, которые пишет персонализация лаунчера (прицел, HUD, FOV,
// настройки Ghosting), здесь не показываются: их значения всё равно
// перекрывает hlsr-personalization.cfg.
//
// Значения по умолчанию — из эталонных cfg: config.cfg пакета Half-Life
// 2005 WON для HL1, стоковые значения движка для Source. Для HL1 здесь
// только переменные из белого списка категорий speedrun.com/hl1
// (brightness по таблице разрешён с 07.11, но в каталог этой вкладки не входит).

const L = (ru, en) => ({ ru, en });

// Файлы, значения из которых не правятся на месте: WON.cfg пакета HL1 ставит
// fps_max 100 и clockwindow 0, без которых ран не засчитают («Don't mess with this»).
const GENERATED_FILES = new Set(["won.cfg"]);

// Порядок, в котором движок выполняет cfg игрока при запуске. В GoldSrc
// autoexec.cfg идёт раньше config.cfg, поэтому Valve и добавила userconfig.cfg.
const ENGINE_ROOTS = {
  goldsrc: ["autoexec.cfg", "config.cfg", "userconfig.cfg"],
  source: ["config.cfg", "autoexec.cfg"],
  "source-old": ["config.cfg", "autoexec.cfg"],
};

const EXEC_FILES = { goldsrc: "userconfig.cfg", source: "autoexec.cfg", "source-old": "autoexec.cfg" };

const SECTIONS = [
  { id: "controls", icon: "fa-mouse", title: L("Управление", "Controls") },
  { id: "video", icon: "fa-desktop", title: L("Изображение", "Video") },
  { id: "audio", icon: "fa-volume-up", title: L("Звук", "Audio") },
  { id: "game", icon: "fa-gamepad", title: L("Игра", "Game") },
];

const GOLDSRC = ["goldsrc"];
const SOURCE = ["source", "source-old"];
const ALL = GOLDSRC.concat(SOURCE);

const off = (value) => ({ value, label: L("Выключено", "Off") });

// type:
//   range   — число с ползунком (min, max, step);
//   percent — доля 0…1 в cfg, проценты 0…100 в окне;
//   toggle  — 0/1;
//   invert  — знак m_pitch: инверсия мыши без потери величины;
//   select  — один из вариантов options.
// archived — движок сам сохраняет переменную в config.cfg.
// skip — сборки, где переменной нет или её пишет персонализация.
const FIELDS = [
  {
    cvar: "sensitivity", section: "controls", type: "range", min: 0.1, max: 10, step: 0.01, default: 3,
    engines: ALL, archived: true,
    label: L("Чувствительность", "Sensitivity"),
    hint: L("Скорость поворота камеры мышью.", "How fast the mouse turns the camera."),
  },
  {
    cvar: "zoom_sensitivity_ratio", section: "controls", type: "range", min: 0.1, max: 4, step: 0.05, default: 1,
    engines: SOURCE, archived: true,
    label: L("Чувствительность в приближении", "Zoomed sensitivity"),
    hint: L("Множитель чувствительности при приближении костюма и прицела.", "Sensitivity multiplier while zoomed in."),
  },
  {
    cvar: "m_pitch", section: "controls", type: "invert", default: false, engines: ALL, archived: true,
    label: L("Инверсия мыши", "Invert mouse"),
    hint: L("Мышь от себя — взгляд вниз.", "Pushing the mouse forward looks down."),
  },
  {
    cvar: "m_filter", section: "controls", type: "toggle", default: false, engines: ALL, archived: true,
    label: L("Сглаживание мыши", "Mouse smoothing"),
    hint: L("Усредняет движение за два кадра. Плавнее, но с задержкой.", "Averages movement over two frames: smoother, but delayed."),
  },
  {
    cvar: "m_customaccel", section: "controls", type: "toggle", default: false, engines: SOURCE, archived: true,
    label: L("Ускорение мыши", "Mouse acceleration"),
    hint: L("Чем быстрее движение, тем дальше поворот.", "Faster movement turns further."),
  },

  {
    cvar: "fps_max", section: "video", type: "range", min: 20, max: 100, step: 1, default: 100,
    engines: GOLDSRC, archived: true,
    label: L("Ограничение FPS", "FPS limit"),
    hint: L("От FPS в Half-Life зависит физика. По правилам WON выше 100 нельзя.", "In Half-Life the physics depend on FPS. WON rules allow at most 100."),
  },
  {
    cvar: "fps_max", section: "video", type: "range", min: 30, max: 999, step: 1, default: 300,
    engines: SOURCE, archived: true,
    label: L("Ограничение FPS", "FPS limit"),
  },
  {
    cvar: "gamma", section: "video", type: "range", min: 1.8, max: 3, step: 0.05, default: 2.5,
    engines: GOLDSRC, archived: true,
    label: L("Гамма", "Gamma"),
    hint: L("Меняется после перезапуска игры.", "Takes effect after restarting the game."),
  },
  {
    cvar: "mat_monitorgamma", section: "video", type: "range", min: 1.6, max: 2.6, step: 0.05, default: 2.2,
    engines: SOURCE, archived: true,
    label: L("Яркость", "Brightness"),
    scale: [L("Светлее", "Brighter"), L("Темнее", "Darker")],
  },
  {
    cvar: "cl_showfps", section: "video", type: "select", default: 0, engines: SOURCE, archived: false,
    skip: ["ghosting-renewed"],
    label: L("Счётчик FPS", "FPS counter"),
    options: [off(0), { value: 1, label: L("FPS", "FPS") }, { value: 2, label: L("FPS и время кадра", "FPS and frame time") }],
  },
  {
    cvar: "net_graph", section: "video", type: "select", default: 0, engines: GOLDSRC, archived: true,
    label: L("Сетевой график", "Net graph"),
    hint: L("Показывает FPS, пинг и потери пакетов в углу экрана.", "Shows FPS, ping and packet loss in a corner of the screen."),
    options: [off(0), { value: 1, label: L("Кратко", "Brief") }, { value: 2, label: L("Подробно", "Detailed") }, { value: 3, label: L("Только цифры", "Numbers only") }],
  },

  {
    cvar: "volume", section: "audio", type: "percent", default: 1, engines: SOURCE, archived: true,
    label: L("Общая громкость", "Master volume"),
  },
  {
    cvar: "volume", section: "audio", type: "percent", default: 0.4, engines: GOLDSRC, archived: true,
    label: L("Общая громкость", "Master volume"),
  },
  {
    cvar: "snd_musicvolume", section: "audio", type: "percent", default: 1, engines: ["source"], archived: true,
    label: L("Громкость музыки", "Music volume"),
  },
  {
    cvar: "MP3Volume", section: "audio", type: "percent", default: 0.8, engines: GOLDSRC, archived: true,
    label: L("Громкость музыки", "Music volume"),
  },
  {
    cvar: "suitvolume", section: "audio", type: "percent", default: 0.25, engines: GOLDSRC, archived: true,
    label: L("Голос костюма HEV", "HEV suit voice"),
  },
  {
    cvar: "closecaption", section: "audio", type: "toggle", default: false, engines: SOURCE, archived: true,
    label: L("Субтитры", "Captions"),
  },
  {
    cvar: "cc_subtitles", section: "audio", type: "toggle", default: false, engines: SOURCE, archived: true,
    requires: "closecaption",
    label: L("Только речь", "Dialogue only"),
    hint: L("Без подписей к звукам.", "No captions for sound effects."),
  },

  {
    cvar: "hud_fastswitch", section: "controls", type: "toggle", default: false, engines: ALL, archived: true,
    label: L("Быстрая смена оружия", "Fast weapon switch"),
    hint: L("Оружие берётся сразу, без меню выбора.", "Weapons are switched at once, without the selection menu."),
  },
  {
    cvar: "skill", section: "game", type: "select", default: 1, engines: SOURCE, archived: true,
    label: L("Сложность", "Difficulty"),
    options: [
      { value: 1, label: L("Лёгкая", "Easy") },
      { value: 2, label: L("Средняя", "Normal") },
      { value: 3, label: L("Сложная", "Hard") },
    ],
  },
  {
    cvar: "crosshair", section: "game", type: "toggle", default: true, engines: SOURCE, archived: true,
    label: L("Прицел", "Crosshair"),
  },
  {
    cvar: "hud_quickinfo", section: "game", type: "toggle", default: true, engines: SOURCE, archived: true,
    skip: ["ghosting-renewed"],
    label: L("Индикаторы у прицела", "Crosshair indicators"),
    hint: L("Дуги здоровья и патронов вокруг прицела.", "Health and ammo arcs around the crosshair."),
  },
];

const key = (name) => String(name || "").toLowerCase();

// Настройки сборки: по движку и без тех, что пишет персонализация.
function catalogFor(gameId, engine) {
  if (!ENGINE_ROOTS[engine]) return null;
  const fields = FIELDS.filter((field) => field.engines.includes(engine) && !(field.skip || []).includes(gameId));
  const sections = SECTIONS.map((section) => Object.assign({}, section, {
    fields: fields.filter((field) => field.section === section.id),
  })).filter((section) => section.fields.length);
  return { engine, sections, fields, byCvar: new Map(fields.map((field) => [key(field.cvar), field])) };
}

const round = (value, step) => {
  const digits = Math.max(0, Math.min(6, Math.ceil(-Math.log10(step || 1))));
  return Number(Number(value).toFixed(digits));
};

const numberOf = (raw) => {
  if (typeof raw === "number") return Number.isFinite(raw) ? raw : NaN;
  const text = String(raw == null ? "" : raw).trim();
  return /^[+-]?(\d+\.?\d*|\.\d+)$/.test(text) ? Number(text) : NaN;
};

// Значение из cfg → значение в окне. undefined — в cfg что-то не то, и в
// окне показывается значение по умолчанию.
function fromCfg(field, raw) {
  if (raw === undefined || raw === null) return undefined;
  const n = numberOf(raw);
  switch (field.type) {
    case "toggle": return Number.isNaN(n) ? undefined : n !== 0;
    case "invert": return Number.isNaN(n) ? undefined : n < 0;
    case "percent": return Number.isNaN(n) ? undefined : Math.max(0, Math.min(100, Math.round(n * 100)));
    case "range": return Number.isNaN(n) ? undefined : n;
    case "select": {
      const option = field.options.find((item) => item.value === n);
      return option ? option.value : undefined;
    }
    default: return undefined;
  }
}

// Значение по умолчанию в единицах окна.
function defaultValue(field) {
  return field.type === "percent" ? Math.round(field.default * 100) : field.default;
}

function formatNumber(value) {
  return String(Number(Number(value).toFixed(6)));
}

// Значение из окна → строка для cfg; null — значение не годится.
// previous — что стоит в cfg сейчас: у инверсии сохраняется величина m_pitch.
function toCfg(field, value, previous) {
  switch (field.type) {
    case "toggle":
      return typeof value === "boolean" ? (value ? "1" : "0") : null;
    case "invert": {
      if (typeof value !== "boolean") return null;
      const magnitude = Math.abs(numberOf(previous)) || 0.022;
      return formatNumber(value ? -magnitude : magnitude);
    }
    case "percent": {
      const n = numberOf(value);
      if (Number.isNaN(n) || n < 0 || n > 100) return null;
      return formatNumber(Math.round(n) / 100);
    }
    case "range": {
      const n = numberOf(value);
      if (Number.isNaN(n) || n < field.min || n > field.max) return null;
      return formatNumber(round(n, field.step));
    }
    case "select": {
      const n = numberOf(value);
      return field.options.some((item) => item.value === n) ? formatNumber(n) : null;
    }
    default:
      return null;
  }
}

// Команды одной строки cfg с их местом в строке. Разбор как у движка:
// команды делит «;» вне кавычек, «//» вне кавычек — комментарий до конца
// строки, кавычки не экранируются и закрываются концом строки.
function lineCommands(line) {
  const commands = [];
  let start = 0;
  let quoted = false;
  let end = line.length;
  for (let i = 0; i < line.length; i += 1) {
    const ch = line[i];
    if (quoted) {
      if (ch === '"') quoted = false;
      continue;
    }
    if (ch === '"') quoted = true;
    else if (ch === "/" && line[i + 1] === "/") { end = i; break; }
    else if (ch === ";") {
      commands.push({ start, end: i });
      start = i + 1;
    }
  }
  commands.push({ start, end });
  return commands.map((command) => Object.assign(command, { args: tokens(line.slice(command.start, command.end)) }))
    .filter((command) => command.args.length);
}

function tokens(text) {
  const result = [];
  const re = /"([^"]*)"?|(\S+)/g;
  let match;
  while ((match = re.exec(text))) {
    if (match[1] !== undefined) result.push({ value: match[1], quoted: true });
    else result.push({ value: match[2].replace(/"/g, ""), quoted: false });
  }
  return result;
}

const splitLines = (text) => String(text || "").split("\n");

// Выполняет cfg по порядку (roots), как движок, и запоминает, где каждая
// переменная из cvars задана последней. readFile(name) — текст или null.
function execute(readFile, roots, cvars) {
  const wanted = new Set(Array.from(cvars, key));
  const found = Object.create(null);
  const files = [];
  const visited = new Set();

  const run = (name, depth) => {
    let file = String(name || "");
    if (!/\.[a-z0-9]+$/i.test(file)) file += ".cfg";
    const lower = file.toLowerCase();
    if (depth > 8 || visited.has(lower)) return;
    if (!/^[A-Za-z0-9._-]+\.cfg$/.test(file)) return;
    const text = readFile(file);
    if (text === null || text === undefined) return;
    visited.add(lower);
    files.push(file);
    const generated = GENERATED_FILES.has(lower);
    splitLines(text).forEach((rawLine, line) => {
      lineCommands(rawLine.replace(/\r$/, "")).forEach((command, index) => {
        const verb = key(command.args[0].value);
        if (verb === "exec" && command.args.length >= 2) run(command.args[1].value, depth + 1);
        else if (wanted.has(verb) && command.args.length >= 2)
          found[verb] = { value: command.args[1].value, file, line, index, generated };
      });
    });
  };

  for (const root of roots) run(root, 0);
  return { found, files };
}

// Импорт чужого cfg во вкладку: последнее значение каждой переменной
// каталога в единицах окна. text — команды в порядке выполнения (цепочка exec
// уже раскрыта, см. src/shared/cfg-import.js). Число вне диапазона окна
// прижимается к краю, непонятное значение пропускается.
function importValues(catalog, text) {
  const raw = Object.create(null);
  for (const line of splitLines(text)) {
    for (const command of lineCommands(line.replace(/\r$/, ""))) {
      const verb = key(command.args[0].value);
      if (catalog.byCvar.has(verb) && command.args.length >= 2) raw[verb] = command.args[1].value;
    }
  }
  const values = {};
  const skipped = [];
  for (const [cvar, value] of Object.entries(raw)) {
    const field = catalog.byCvar.get(cvar);
    let next = fromCfg(field, value);
    if (next === undefined) {
      skipped.push(field.cvar);
      continue;
    }
    if (field.type === "range") next = Math.max(field.min, Math.min(field.max, round(next, field.step)));
    values[field.cvar] = next;
  }
  return { values, skipped };
}

// Новое значение на месте старого: та же строка, та же команда, отступы,
// написание имени, кавычки и комментарий остаются.
function replaceAt(text, location, value) {
  const lines = splitLines(text);
  const raw = lines[location.line];
  if (raw === undefined) return null;
  const cr = raw.endsWith("\r") ? "\r" : "";
  const line = cr ? raw.slice(0, -1) : raw;
  const command = lineCommands(line)[location.index];
  if (!command || command.args.length < 2) return null;
  const segment = line.slice(command.start, command.end);
  const lead = /^\s*/.exec(segment)[0];
  const trail = /\s*$/.exec(segment)[0];
  const quoted = command.args[1].quoted || /\s/.test(value);
  const next = `${lead}${command.args[0].value} ${quoted ? `"${value}"` : value}${trail}`;
  lines[location.line] = line.slice(0, command.start) + next + line.slice(command.end) + cr;
  return lines.join("\n");
}

// Строка в конец файла с тем же переводом строки, что в файле.
function appendLine(text, line, defaultEol) {
  const source = String(text || "");
  const eol = source.includes("\r\n") ? "\r\n" : source.includes("\n") ? "\n" : defaultEol || "\n";
  const suffix = source.length === 0 || source.endsWith("\n") ? "" : eol;
  return `${source}${suffix}${line}${eol}`;
}

module.exports = {
  GENERATED_FILES,
  ENGINE_ROOTS,
  EXEC_FILES,
  SECTIONS,
  FIELDS,
  catalogFor,
  fromCfg,
  toCfg,
  defaultValue,
  importValues,
  lineCommands,
  execute,
  replaceAt,
  appendLine,
};
