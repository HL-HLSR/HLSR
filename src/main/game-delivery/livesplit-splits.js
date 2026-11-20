"use strict";

// Сплиты LiveSplit для каждой игры. У Ghosting это файлы из архива LiveSplit
// (по главам и по картам), для остальных игр лаунчер пишет файлы сам в
// splits/hlsr/<gameId>/ папки LiveSplit. Файл пишется, только если его ещё
// нет: LiveSplit хранит в нём личные рекорды и историю попыток.
//
// Категории — как на speedrun.com: HL1 WON (Scripted / Scriptless), HL2 Old
// Engine для 2707 и 4231–4260 (With Scripts / Scriptless) и New Engine для
// 5288 (Any% / Any% No Voidclip). Сплиты HL2 ставит SourceSplit (настройки в
// самих сплитах), HL1 — BunnySplit через Bunnymod XT (компонент в раскладке).
//
// LiveSplit открывает сплиты и раскладку из аргументов -s и -l. Пути
// относительные, от папки LiveSplit: на Linux он запущен под Wine, и
// относительный путь не нужно переводить в путь Wine.
//
// BunnySplit (resources/livesplit-components) собран из
// github.com/YaLTeR/LiveSplit.BunnySplit (8024493) Roslyn (csc.exe из Visual
// Studio Build Tools) против настоящих сборок .NET Framework 4.8.1, с
// LiveSplit.Core.dll и UpdateManager.dll из архива LiveSplit. Прежняя сборка
// mcs из wine-mono привязывала Split('|') к Split(char, StringSplitOptions),
// которого нет в .NET Framework, и SetSettings падал с MissingMethodException.
// Из папки исходников (FW = %WINDIR%\Microsoft.NET\Framework64\v4.0.30319,
// LS = папка архива LiveSplit); при той же команде байты совпадают:
//   csc.exe -nologo -target:library -optimize+ -deterministic -nostdlib
//     -pathmap:<корень сборки>=. -out:LiveSplit.BunnySplit.dll
//     -r:FW\mscorlib.dll -r:LS\LiveSplit.Core.dll -r:LS\UpdateManager.dll
//     -r:FW\System.Windows.Forms.dll -r:FW\System.Drawing.dll
//     -r:FW\System.Xml.dll -r:FW\System.Xml.Linq.dll -r:FW\System.Core.dll
//     -r:FW\Microsoft.CSharp.dll -r:FW\System.dll -r:FW\System.Data.dll
//     Component.cs ComponentSettings.cs ComponentSettings.Designer.cs
//     EditableListBox.cs Factory.cs Properties\AssemblyInfo.cs
// Проверка на .NET Framework: scripts/livesplit-overlay/bunnysplit-netfx-check.ps1.

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { writeFileAtomic } = require("./atomic");

const COMPONENTS_DIRECTORY = "livesplit-components";
const BUNNYSPLIT = "LiveSplit.BunnySplit.dll";
const BUNNYSPLIT_VERSION = "2.0.2.0";
const BUNNYSPLIT_SHA256 = "8e76fd5b006638b6e4bd76881ffaaff34bc36553a33b7bef92b061340445a3aa";
// Копии, собранные mcs: у пользователей они падают на .NET Framework. Только
// их лаунчер заменяет, чужую версию не трогает.
const BUNNYSPLIT_BROKEN_SHA256 = ["0c6a8d199f8ef6c5e3bc32239569f017568220b03314a5424a56acd714279794"];
const SETTINGS_FILE = "settings.cfg";
// Раскладки из архива LiveSplit, на их основе пишутся раскладки игр.
const BASE_LAYOUTS = { chapters: "splits/chapters/chapters.lsl", maps: "splits/maps/maps.lsl" };

const HL1_CHAPTERS = [
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

const HL2_CHAPTERS = [
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

// Карты с подсплитами по главам, как в сплитах Ghosting (splits/maps).
const HL2_MAPS = [
  "-d1_trainstation_01", "-d1_trainstation_02", "-d1_trainstation_03", "{Point Insertion} d1_trainstation_04",
  "-d1_trainstation_05", "{A Red Letter Day} d1_trainstation_06",
  "-d1_canals_01", "-d1_canals_01a", "-d1_canals_02", "-d1_canals_03", "{Route Kanal} d1_canals_05",
  "-d1_canals_06", "-d1_canals_07", "-d1_canals_08", "-d1_canals_09", "-d1_canals_10", "-d1_canals_11",
  "-d1_canals_12", "{Water Hazard} d1_canals_13",
  "-d1_eli_01", "{Black Mesa East} d1_eli_02",
  "-d1_town_01", "-d1_town_01a", "-d1_town_02", "-d1_town_02a", "-d1_town_04", "{We Don't Go To Ravenholm} d1_town_05",
  "-d2_coast_01", "-d2_coast_03", "-d2_coast_04", "-d2_coast_05", "{Highway 17} d2_coast_07",
  "-d2_coast_09", "-d2_coast_10", "-d2_coast_11", "-d2_coast_12", "{Sandtraps} d2_prison_01",
  "-d2_prison_02", "-d2_prison_03", "-d2_prison_04", "{Nova Prospekt} d2_prison_05",
  "-d2_prison_06", "-d2_prison_07", "-d2_prison_08", "{Entanglement} d3_c17_01",
  "-d3_c17_02", "-d3_c17_03", "-d3_c17_04", "-d3_c17_05", "-d3_c17_06a", "-d3_c17_06b", "-d3_c17_07",
  "{Anticitizen One} d3_c17_08",
  "-d3_c17_09", "-d3_c17_10a", "-d3_c17_10b", "-d3_c17_11", "-d3_c17_12", "-d3_c17_12b", "{Follow Freeman!} d3_c17_13",
  "-d3_citadel_01", "-d3_citadel_02", "-d3_citadel_03", "-d3_citadel_04", "{Our Benefactors} d3_citadel_05",
  "{Dark Energy} Dark Energy",
];

// Настройки SourceSplit из сплитов Ghosting: по главам сплит только на
// переходах между главами (список MTL), по картам — на каждом переходе.
function sourceSplitSettings(kind) {
  const chapters = kind === "chapters";
  return [
    ["Version", chapters ? "3.3.4.6" : "3.3.4.5"],
    ["AutoSplitEnabled", "True"],
    ["SplitInterval", "1"],
    ["UseSplitInterval", "True"],
    ["UseMTL", "True"],
    ["MTLModeSetting", chapters ? "Allow" : "Disallow"],
    ["AutoSplitOnLevelTrans", "True"],
    ["AutoSplitOnGenericMap", "False"],
    ["AutoSplitOnSpecial", "True"],
    ["AutoStartEnabled", "True"],
    ["AutoStopEnabled", "True"],
    ["AutoResetEnabled", "True"],
    ["FirstMapAutoReset", "True"],
    ["StartMap", ""],
    ["AddAutoStartType", "NewGame"],
    ["AllowAdditionalAutoStart", "False"],
    ["ShowGameTime", "True"],
    ["ShowAltTime", "False"],
    ["ShowTickCount", "False"],
    ["ShowCurDemo", "False"],
    ["GameTimeDecimalPlaces", "6"],
    ["HoldUntilPause", "True"],
    ["RTAStartOffset", "True"],
    ["ServerInitialTicks", "False"],
    ["SLPenalty", chapters ? "1" : "0"],
    ["SplitInstead", "False"],
    ["ResetMapTransitions", "False"],
    ["CountEngineTicks", "True"],
    ["CountPauses", "True"],
    ["CountDisconnects", "False"],
    ["CountInactive", "False"],
    ["CountAutomatic", "True"],
    ["CountDemoInterop", "False"],
    ["PrintDemoInfo", "False"],
    ["DemoParserPath", ""],
    ["SetGameTimeOnLaunch", "True"],
    [
      "MapTransitionList",
      chapters
        ? "d1_trainstation_06,d1_canals_01|d1_trainstation_04,d1_trainstation_05|d1_canals_05,d1_canals_06|" +
          "d1_canals_13,d1_eli_01|d1_eli_02,d1_town_01|d1_town_05,d2_coast_01|d2_coast_07,d2_coast_09|" +
          "d2_prison_01,d2_prison_02|d3_c17_01,d3_c17_02|d2_prison_05,d2_prison_06|d3_c17_08,d3_c17_09|" +
          "d3_c17_13,d3_citadel_01|d3_citadel_05,d3_breen_01"
        : "d1_canals_07,d1_canals_08",
    ],
  ];
}

// BunnySplit: сплит при первом входе в главу HL1 и на смерти Нихиланта,
// старт и сброс — по таймеру BXT.
const BUNNYSPLIT_SETTINGS = [
  ["Version", BUNNYSPLIT_VERSION],
  ["EnableAutoSplit", "True"],
  ["SplitOnGameEnd", "True"],
  ["SplitOnHL1Chapters", "True"],
  ["SplitOnOP4Chapters", "False"],
  ["SplitOnBSChapters", "False"],
  ["SplitOnGMCChapters", "False"],
  ["SplitOnMaps", "False"],
  ["EnableAutoReset", "True"],
  ["EnableAutoStart", "True"],
];

function hl1(id, scripts) {
  return {
    id,
    kind: "chapters",
    category: scripts,
    gameName: "Half-Life",
    categoryName: "WON",
    variables: { Scripts: scripts, Segments: "RTA" },
    segments: HL1_CHAPTERS,
    autosplitter: "bunnysplit",
  };
}

function hl2(id, kind, category, categoryName, variables) {
  return {
    id,
    kind,
    category,
    gameName: "Half-Life 2",
    categoryName,
    variables,
    segments: kind === "chapters" ? HL2_CHAPTERS : HL2_MAPS,
    autosplitter: "sourcesplit",
  };
}

function oldEngine(build) {
  const variables = (scripts) => ({ Scripts: scripts, Build: build });
  return [
    hl2("scripts-chapters", "chapters", "With Scripts", "Old Engine", variables("With Scripts")),
    hl2("scriptless-chapters", "chapters", "Scriptless", "Old Engine", variables("Scriptless")),
    hl2("scripts-maps", "maps", "With Scripts", "Old Engine", variables("With Scripts")),
    hl2("scriptless-maps", "maps", "Scriptless", "Old Engine", variables("Scriptless")),
  ];
}

function newEngine(build) {
  const variables = (voidclip) => ({ "With/Without Voidclip": voidclip, Build: build });
  return [
    hl2("any-chapters", "chapters", "Any%", "New Engine", variables("Any%")),
    hl2("novoidclip-chapters", "chapters", "Any% No Voidclip", "New Engine", variables("Any% No Voidclip")),
    hl2("any-maps", "maps", "Any%", "New Engine", variables("Any%")),
    hl2("novoidclip-maps", "maps", "Any% No Voidclip", "New Engine", variables("Any% No Voidclip")),
  ];
}

const PRESETS = {
  // Готовые файлы из архива LiveSplit (в них же рекорды прежних версий).
  "ghosting-renewed": [
    { id: "chapters", kind: "chapters", category: null, bundled: true,
      splits: "splits/chapters/chapters.lss", layout: "splits/chapters/chapters.lsl" },
    { id: "maps", kind: "maps", category: null, bundled: true,
      splits: "splits/maps/maps.lss", layout: "splits/maps/maps.lsl" },
  ],
  "hl1-won": [hl1("scripted", "Scripted"), hl1("scriptless", "Scriptless")],
  "hl2-2707": oldEngine("2707"),
  "hl2-4260": oldEngine("4231-4260"),
  "hl2-5288": newEngine("5288"),
};

function own(table, key) {
  return Object.prototype.hasOwnProperty.call(table, key) ? table[key] : null;
}

function presetsOf(gameId) {
  return own(PRESETS, String(gameId || "")) || [];
}

// Набор по id, без него (или с чужим id) — первый набор игры.
function presetOf(gameId, presetId) {
  const presets = presetsOf(gameId);
  return presets.find((preset) => preset.id === presetId) || presets[0] || null;
}

// Для окна выбора: id, вид (главы или карты) и категория speedrun.com.
function describe(gameId) {
  return presetsOf(gameId).map((preset) => ({ id: preset.id, kind: preset.kind, category: preset.category }));
}

// Пути внутри папки LiveSplit, через «\»: так их пишет и сам LiveSplit.
function filesOf(gameId, preset) {
  if (preset.bundled) return { splits: preset.splits, layout: preset.layout };
  const base = `splits/hlsr/${gameId}/${preset.id}`;
  return { splits: `${base}.lss`, layout: `${base}.lsl` };
}

function windowsPath(relative) {
  return relative.split("/").join("\\");
}

function escapeXml(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function settingsXml(entries, indent) {
  return entries
    .map(([name, value]) => (value === "" ? `${indent}<${name}>\r\n${indent}</${name}>` : `${indent}<${name}>${escapeXml(value)}</${name}>`))
    .join("\r\n");
}

function splitsXml(preset) {
  const variables = Object.keys(preset.variables || {})
    .map((name) => `      <Variable name="${escapeXml(name)}">${escapeXml(preset.variables[name])}</Variable>`)
    .join("\r\n");
  const segments = preset.segments
    .map((name) =>
      [
        "    <Segment>",
        `      <Name>${escapeXml(name)}</Name>`,
        "      <Icon />",
        "      <SplitTimes>",
        '        <SplitTime name="Personal Best" />',
        "      </SplitTimes>",
        "      <BestSegmentTime />",
        "      <SegmentHistory />",
        "    </Segment>",
      ].join("\r\n")
    )
    .join("\r\n");
  const autosplitter =
    preset.autosplitter === "sourcesplit"
      ? `  <AutoSplitterSettings>\r\n${settingsXml(sourceSplitSettings(preset.kind), "    ")}\r\n    <Misc />\r\n  </AutoSplitterSettings>`
      : "  <AutoSplitterSettings />";
  return [
    '﻿<?xml version="1.0" encoding="UTF-8"?>',
    '<Run version="1.7.0">',
    "  <GameIcon />",
    `  <GameName>${escapeXml(preset.gameName)}</GameName>`,
    `  <CategoryName>${escapeXml(preset.categoryName)}</CategoryName>`,
    "  <LayoutPath>",
    "  </LayoutPath>",
    "  <Metadata>",
    '    <Run id="" />',
    '    <Platform usesEmulator="False">PC</Platform>',
    "    <Region>",
    "    </Region>",
    variables ? `    <Variables>\r\n${variables}\r\n    </Variables>` : "    <Variables />",
    "    <CustomVariables />",
    "  </Metadata>",
    "  <Offset>00:00:00</Offset>",
    "  <AttemptCount>0</AttemptCount>",
    "  <AttemptHistory />",
    "  <Segments>",
    segments,
    "  </Segments>",
    autosplitter,
    "</Run>",
    "",
  ].join("\r\n");
}

// Раскладка игры — раскладка Ghosting того же вида; для HL1 в неё
// добавляется BunnySplit.
function layoutXml(base, preset) {
  if (preset.autosplitter !== "bunnysplit") return base;
  const component = [
    "    <Component>",
    `      <Path>${BUNNYSPLIT}</Path>`,
    "      <Settings>",
    settingsXml(BUNNYSPLIT_SETTINGS, "        "),
    "      </Settings>",
    "    </Component>",
    "  </Components>",
  ].join("\r\n");
  const end = base.lastIndexOf("</Components>");
  if (end < 0) return null;
  const lineStart = base.lastIndexOf("\n", end) + 1;
  return base.slice(0, lineStart) + component + base.slice(end + "</Components>".length);
}

function sha256Of(fsModule, file) {
  return crypto.createHash("sha256").update(fsModule.readFileSync(file)).digest("hex");
}

class LiveSplitSplits {
  constructor(options = {}) {
    this.fs = options.fs || fs;
    this.resourcesPath = options.resourcesPath || process.resourcesPath;
    this.logger = options.logger || { info() {}, warn() {} };
    this.brokenSha256 = options.brokenSha256 || BUNNYSPLIT_BROKEN_SHA256;
  }

  writeIfMissing(file, content) {
    if (this.fs.existsSync(file)) return false;
    this.fs.mkdirSync(path.dirname(file), { recursive: true });
    writeFileAtomic(file, content);
    return true;
  }

  // BunnySplit в Components, если его там нет или это известная сломанная
  // сборка: версию, которую пользователь положил сам, лаунчер не трогает.
  installBunnySplit(root) {
    const source = path.join(this.resourcesPath, COMPONENTS_DIRECTORY, BUNNYSPLIT);
    const target = path.join(root, "Components", BUNNYSPLIT);
    if (!this.fs.existsSync(source)) throw new Error(`${BUNNYSPLIT} не найден в ресурсах лаунчера`);
    const exists = this.fs.existsSync(target);
    if (exists && !this.brokenSha256.includes(sha256Of(this.fs, target))) return false;
    const temporary = `${target}.${process.pid}.tmp`;
    try {
      this.fs.copyFileSync(source, temporary);
      this.fs.renameSync(temporary, target);
    } finally {
      try { this.fs.rmSync(temporary, { force: true }); } catch (error) { /* временный файл не критичен */ }
    }
    this.logger.info(
      `[tools] livesplit: ${exists ? "заменён сломанный BunnySplit, установлен" : "установлен"} ${BUNNYSPLIT} (${sha256Of(this.fs, target).slice(0, 12)})`
    );
    return true;
  }

  // Сплиты новые для LiveSplit: в settings.cfg они попадают с игровым
  // временем, иначе LiveSplit показал бы реальное. Автосплиттер Half-Life 2
  // (SourceSplit) включается, если его выключили.
  seedSettings(root, preset, splitsPath) {
    const file = path.join(root, SETTINGS_FILE);
    let text;
    try {
      text = this.fs.readFileSync(file, "utf8");
    } catch (error) {
      return false;
    }
    let next = text;
    const entry =
      `<SplitsFile gameName="${escapeXml(preset.gameName)}" categoryName="${escapeXml(preset.categoryName)}" ` +
      `lastTimingMethod="GameTime" lastHotkeyProfile="Default">${escapeXml(splitsPath)}</SplitsFile>`;
    if (!next.includes(`>${escapeXml(splitsPath)}</SplitsFile>`)) {
      if (/<RecentSplits\s*\/>/.test(next)) next = next.replace(/<RecentSplits\s*\/>/, `<RecentSplits>\r\n    ${entry}\r\n  </RecentSplits>`);
      else if (next.includes("<RecentSplits>")) next = next.replace("<RecentSplits>", `<RecentSplits>\r\n    ${entry}`);
    }
    if (preset.autosplitter === "sourcesplit" && !next.includes(`<AutoSplitter>${preset.gameName}</AutoSplitter>`)) {
      const item = `<AutoSplitter>${escapeXml(preset.gameName)}</AutoSplitter>`;
      if (/<ActiveAutoSplitters\s*\/>/.test(next)) next = next.replace(/<ActiveAutoSplitters\s*\/>/, `<ActiveAutoSplitters>\r\n    ${item}\r\n  </ActiveAutoSplitters>`);
      else if (next.includes("<ActiveAutoSplitters>")) next = next.replace("<ActiveAutoSplitters>", `<ActiveAutoSplitters>\r\n    ${item}`);
    }
    if (next === text) return false;
    writeFileAtomic(file, next);
    return true;
  }

  // «Время из LiveSplit» в Ghosting (gh_timer_mirror) спрашивает время у
  // LiveSplit Server по TCP на 127.0.0.1. Сервер встроен в LiveSplit, но по
  // умолчанию выключен, а под Proton меню Control найти непросто. Перед
  // запуском LiveSplit включаем старт TCP-сервера и порт из настроек игры.
  // Открытый LiveSplit файл перезапишет при выходе, поэтому только до старта.
  // ServerStartup: 0 — выкл, 1 — TCP, 2 — WebSocket, 3 — как в прошлый раз.
  enableServer(root, port) {
    const file = path.join(root, SETTINGS_FILE);
    let text;
    try {
      text = this.fs.readFileSync(file, "utf8");
    } catch (error) {
      return false;
    }
    let next = text;
    const startup = /<ServerStartup>(\d+)<\/ServerStartup>/.exec(next);
    const state = /<ServerState>(\d+)<\/ServerState>/.exec(next);
    const startsTcp = startup && (startup[1] === "1" || (startup[1] === "3" && state && state[1] === "1"));
    if (!startsTcp) {
      if (startup) next = next.replace(startup[0], "<ServerStartup>1</ServerStartup>");
      else next = next.replace(/<\/Settings>\s*$/, "  <ServerStartup>1</ServerStartup>\r\n</Settings>\r\n");
    }
    const portTag = `<ServerPort>${port}</ServerPort>`;
    if (/<ServerPort>\d*<\/ServerPort>/.test(next)) next = next.replace(/<ServerPort>\d*<\/ServerPort>/, portTag);
    else next = next.replace(/<\/Settings>\s*$/, `  ${portTag}\r\n</Settings>\r\n`);
    if (next === text) return false;
    writeFileAtomic(file, next);
    this.logger.info(`[tools] livesplit: TCP-сервер включён на порту ${port} (время из LiveSplit в Ghosting)`);
    return true;
  }

  // Готовит сплиты и раскладку игры и возвращает аргументы LiveSplit.
  // Без сплитов для игры — пустой список: LiveSplit откроет последние.
  prepare(root, gameId, presetId) {
    const preset = presetOf(gameId, presetId);
    if (!preset) return { args: [], preset: null };
    const files = filesOf(gameId, preset);
    const splitsFile = path.join(root, files.splits);
    const layoutFile = path.join(root, files.layout);

    if (!preset.bundled) {
      if (this.writeIfMissing(splitsFile, splitsXml(preset)))
        this.logger.info(`[tools] livesplit: созданы сплиты ${files.splits}`);
      if (!this.fs.existsSync(layoutFile)) {
        let base = null;
        try {
          base = this.fs.readFileSync(path.join(root, BASE_LAYOUTS[preset.kind]), "utf8");
        } catch (error) {
          this.logger.warn(`[tools] livesplit: нет раскладки ${BASE_LAYOUTS[preset.kind]}`);
        }
        const layout = base && layoutXml(base, preset);
        if (layout) this.writeIfMissing(layoutFile, layout);
      }
      if (preset.autosplitter === "bunnysplit") this.installBunnySplit(root);
      this.seedSettings(root, preset, windowsPath(files.splits));
    }

    if (!this.fs.existsSync(splitsFile)) {
      this.logger.warn(`[tools] livesplit: сплиты ${files.splits} не найдены`);
      return { args: [], preset };
    }
    const args = ["-s", windowsPath(files.splits)];
    if (this.fs.existsSync(layoutFile)) args.push("-l", windowsPath(files.layout));
    return { args, preset };
  }
}

module.exports = LiveSplitSplits;
module.exports.PRESETS = PRESETS;
module.exports.presetsOf = presetsOf;
module.exports.presetOf = presetOf;
module.exports.describe = describe;
module.exports.filesOf = filesOf;
module.exports.splitsXml = splitsXml;
module.exports.layoutXml = layoutXml;
module.exports.BUNNYSPLIT = BUNNYSPLIT;
module.exports.BUNNYSPLIT_SHA256 = BUNNYSPLIT_SHA256;
module.exports.BUNNYSPLIT_BROKEN_SHA256 = BUNNYSPLIT_BROKEN_SHA256;
module.exports.COMPONENTS_DIRECTORY = COMPONENTS_DIRECTORY;
