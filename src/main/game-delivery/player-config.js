"use strict";

const nodeFs = require("fs");
const path = require("path");
const crypto = require("crypto");

const cfgDocument = require("../../shared/cfg-document");
const gameBinds = require("../../shared/game-binds");
const bindPresets = require("../../shared/bind-presets");
const cfgSearch = require("./cfg-search");
const hl1Default = require("./hl1-default-userconfig");
const ghostingDefault = require("./ghosting-default-autoexec");
const ghostingPersonalization = require("./ghosting-personalization");
const goldsrcPersonalization = require("./goldsrc-personalization");
const { cvarsOf, adoptImported } = require("../../shared/goldsrc-import-adopt");
const launchArgs = require("./launch-args");

const { STOCK_AUTOEXEC_SHA256 } = ghostingPersonalization;

// Единый файл игрока: HL1 — userconfig.cfg, Ghosting и HL2 — autoexec.cfg.
// Лаунчер правит его на месте по разделам (cfg-document), а не держит рядом
// слои hlsr-*.cfg.

class PlayerConfigError extends Error {
  constructor(code, message) {
    super(message || code);
    this.name = "PlayerConfigError";
    this.code = code;
  }
}

const ORDER = ["rules", "settings", "personalization", "binds", "scripts", "own"];
const LAUNCHER_EXEC_LINE = /^exec\s+hlsr-[a-z-]+\.cfg$/i;
const sha256Of = (bytes) => crypto.createHash("sha256").update(bytes).digest("hex");

// Файл игрока читается и пишется байт в байт у всех движков (ruling R17): его текст — байты
// как latin1, UTF-8 и cp1251 переживают любую правку. Свои строки (шаблон, персонализация,
// бинды) лаунчер кладёт так, как их писала 0.4.8: GoldSrc — latin1, Source — UTF-8 (в шаблоне
// Ghosting и в gh_name бывает кириллица). fromFileText — обратно, для значений в окне.
const FILE_ENCODING = "latin1";
const encodingOf = (gameId) => gameBinds.engineOf(bindPresets.engineFor(gameId)).encoding;
const toFileText = (gameId, text) => Buffer.from(String(text), encodingOf(gameId)).toString(FILE_ENCODING);
const fromFileText = (gameId, text) => Buffer.from(String(text), FILE_ENCODING).toString(encodingOf(gameId));

const HL2_SETTINGS = ["sv_max_queries_sec 9", "save_screenshot 0", "cl_showpos 1", "cl_showfps 1"];

function bodies(gameId) {
  if (gameId === "hl1-won") {
    const [first, ...rest] = hl1Default.TEXT.split("\r\n");
    // Хвост-подсказка игроку («Write your own…») — в раздел игрока, в settings только команды.
    const hint = rest.findIndex((line) => line.startsWith("// Write your own"));
    return { rules: [first], settings: trimBlank(rest.slice(0, hint)), own: trimBlank(rest.slice(hint)) };
  }
  if (gameId === "ghosting-renewed") return { settings: trimBlank(ghostingDefault.split("\r\n")) };
  if (bindPresets.engineFor(gameId)) return { settings: HL2_SETTINGS };
  return {};
}

function trimBlank(lines) {
  let from = 0;
  let to = lines.length;
  while (from < to && lines[from] === "") from += 1;
  while (to > from && lines[to - 1] === "") to -= 1;
  return lines.slice(from, to);
}

// Разделы шаблона по порядку names: заголовок и стандартные строки, между разделами пустая строка.
function templateLines(gameId, names) {
  const body = bodies(gameId);
  return names.flatMap((name, index) => [...(index ? [""] : []), cfgDocument.SECTIONS[name], ...(body[name] || [])]);
}

// Свежий файл игрока: все шесть разделов по порядку, только заголовки и стандартные строки.
function template(gameId) {
  return templateLines(gameId, ORDER).join("\r\n") + "\r\n";
}

function locate(installPath, gameId, gameDirectory, options = {}) {
  const engine = bindPresets.engineFor(gameId);
  if (!engine || !gameDirectory) return null;
  const files = cfgSearch.filesOf(installPath, gameDirectory, engine, options);
  const { execFile, encoding } = gameBinds.engineOf(engine);
  // Нового файла нет — он создаётся там, где игра держит config.cfg (языковая папка GoldSrc).
  const file = files.exists(execFile) ? files.pathOf(execFile) : path.join(files.dirs[0], execFile);
  return { file, modDir: path.dirname(file), engine, encoding };
}

function read(installPath, gameId, gameDirectory, options = {}) {
  const fs = options.fs || nodeFs;
  const found = locate(installPath, gameId, gameDirectory, options);
  if (!found) throw new PlayerConfigError("CONFIG_UNSUPPORTED", `Нет файла игрока для ${gameId}`);
  let bytes = null;
  try {
    bytes = fs.readFileSync(found.file);
  } catch (error) {
    if (!error || error.code !== "ENOENT") throw error;
  }
  if (!bytes) return { file: found.file, exists: false, text: "", sha256: null };
  return { file: found.file, exists: true, text: bytes.toString(FILE_ENCODING), sha256: sha256Of(bytes) };
}

// Пишет через временный файл рядом и rename. expectedSha256 — отпечаток, который
// вызывающий видел при чтении (null — файла не было); не совпал — файл не трогаем.
function write(installPath, gameId, gameDirectory, text, options = {}) {
  const fs = options.fs || nodeFs;
  const current = read(installPath, gameId, gameDirectory, options);
  if (options.expectedSha256 !== undefined && options.expectedSha256 !== current.sha256) {
    throw new PlayerConfigError("CONFIG_CHANGED", "Файл изменён с момента чтения");
  }
  const bytes = Buffer.from(String(text), FILE_ENCODING);
  fs.mkdirSync(path.dirname(current.file), { recursive: true });
  const temp = `${current.file}.hlsr-${process.pid}-${crypto.randomBytes(4).toString("hex")}.tmp`;
  try {
    fs.writeFileSync(temp, bytes);
    fs.renameSync(temp, current.file);
  } catch (error) {
    try {
      fs.rmSync(temp, { force: true });
    } catch (ignored) {
      // временный файл уже убран
    }
    throw error;
  }
  return { file: current.file, sha256: sha256Of(bytes) };
}

// Стоковый или шаблонный файл лаунчер вправе заменить; файл игрока — нет.
// text — как его даёт read (байты в latin1).
function isStock(gameId, text) {
  const source = String(text || "");
  if (source.trim() === "" || source === toFileText(gameId, template(gameId))) return true;
  const own = source
    .replace(/\r\n/g, "\n")
    .split("\n")
    .filter((line) => !LAUNCHER_EXEC_LINE.test(line.trim()))
    .join("\n")
    .replace(/\s+$/, "");
  const hash = sha256Of(Buffer.from(own, FILE_ENCODING));
  // Свой стандартный файл 0.4.8 тоже заменяем: в нём ещё нет разделов.
  const previous = { "hl1-won": hl1Default.TEXT, "ghosting-renewed": ghostingDefault }[gameId];
  if (previous && own === toFileText(gameId, previous).replace(/\r\n/g, "\n").replace(/\s+$/, "")) return true;
  if (gameId === "hl1-won") return hl1Default.REPLACEABLE_SHA256.has(hash);
  if (gameId === "ghosting-renewed") return STOCK_AUTOEXEC_SHA256.has(hash);
  return false;
}

function ensure(installPath, gameId, gameDirectory, options = {}) {
  const current = read(installPath, gameId, gameDirectory, options);
  if (current.exists && !isStock(gameId, current.text)) return { file: current.file, created: false };
  try {
    write(installPath, gameId, gameDirectory, toFileText(gameId, template(gameId)), { ...options, expectedSha256: current.sha256 });
  } catch (error) {
    // Файл успел создать или изменить кто-то другой — это нормально, запуск не ломаем.
    if (error instanceof PlayerConfigError && error.code === "CONFIG_CHANGED") return { file: current.file, created: false };
    throw error;
  }
  return { file: current.file, created: true };
}

// Перед первыми записями: один раз переход с 0.4.8, иначе — шаблон вместо отсутствующего.
function prepare(installPath, gameId, gameDirectory, options = {}) {
  // Внутри функции: config-migration сам зовёт этот модуль.
  const configMigration = require("./config-migration");
  if (configMigration.needsMigration(installPath, gameId, gameDirectory, options)) {
    const result = configMigration.migrate(installPath, gameId, gameDirectory, options);
    return { file: result.file, migrated: result.status === "migrated", created: false };
  }
  const result = ensure(installPath, gameId, gameDirectory, options);
  return { file: result.file, migrated: false, created: result.created };
}

const SETUP_FILE = path.join(".hlsr", "config-setup.json");
const SETUP_CHOICES = ["scratch", "own", "skip"];

function readSetupMark(installPath, fs) {
  try {
    const value = JSON.parse(fs.readFileSync(path.join(installPath, SETUP_FILE), "utf8"));
    return value && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch (error) {
    return null;
  }
}

// Опрос «с нуля или свой конфиг» нужен, пока нет отметки .hlsr/config-setup.json, файла
// игрока нет или он стоковый, и установке 0.4.8 не нужен переход (его сделает запуск).
// Ничего не пишет: создание файла спрятало бы состояние «конфига ещё нет».
function setupState(installPath, gameId, gameDirectory, options = {}) {
  const fs = options.fs || nodeFs;
  if (readSetupMark(installPath, fs) || fs.existsSync(path.join(installPath, SETUP_FILE))) return { needed: false };
  const configMigration = require("./config-migration");
  if (configMigration.needsMigration(installPath, gameId, gameDirectory, options)) return { needed: false };
  const current = read(installPath, gameId, gameDirectory, options);
  return { needed: !current.exists || isStock(gameId, current.text) };
}

// Ответ на опрос: «с нуля» и «пропустить» готовят файл игрока, «свой» его не трогает
// (импорт запишет сам). Отметка дописывается к уже стоящей (там может быть migrated).
function setupDone(installPath, gameId, gameDirectory, choice, options = {}) {
  if (!SETUP_CHOICES.includes(choice)) throw new PlayerConfigError("CONFIG_SETUP_CHOICE", `Неизвестный ответ: ${choice}`);
  const fs = options.fs || nodeFs;
  if (choice !== "own") prepare(installPath, gameId, gameDirectory, options);
  const mark = Object.assign(readSetupMark(installPath, fs) || {}, { choice, at: new Date().toISOString() });
  const target = path.join(installPath, SETUP_FILE);
  const temp = `${target}.${process.pid}-${crypto.randomBytes(4).toString("hex")}.tmp`;
  fs.mkdirSync(path.dirname(target), { recursive: true });
  try {
    fs.writeFileSync(temp, JSON.stringify(mark, null, 2));
    fs.renameSync(temp, target);
  } catch (error) {
    try {
      fs.rmSync(temp, { force: true });
    } catch (ignored) {
      // временный файл уже убран
    }
    throw error;
  }
  return { choice };
}

// Персонализация живёт в файле игрока: лаунчер читает оттуда значения и пишет
// только те, что игрок изменил в окне. Строки берутся из генераторов
// toCfgLines (их вывод не меняется).
const PERSONALIZED = new Set(["hl1-won", "ghosting-renewed"]);
const SNAPSHOT_EXEC = /exec\s+hlsr-personalization\.cfg/i;
// Команды extras Ghosting, которые не cvar (без значения или повторяются: Twitch и YouTube):
// пишутся отдельным блоком раздела personalization, который переписывается целиком.
const EXTRAS_BLOCK = "ghosting-extras";
const BLOCK_VERB = /^(gh_spray_set|gh_spray_clear|gh_chat_connect|gh_chat_disconnect)$/i;
const BLOCK_LINE = /^\s*(gh_spray_set|gh_spray_clear|gh_chat_connect|gh_chat_disconnect)(\s|$)|^\s*\/\/ HLSR_YOUTUBE_URL /i;
const ASSET_EXTRAS = ["gh_spray_file", "gh_chat_connect"];

// Строки генератора в виде текста файла (toFileText): [cvar-строки, строки блока].
function generatedLines(gameId, settings) {
  if (gameId === "hl1-won") return [toFileText(gameId, goldsrcPersonalization.toCfgLines(settings)).split(/\r?\n/), []];
  if (gameId !== "ghosting-renewed") return [[], []];
  const cvars = [];
  const block = [];
  for (const line of toFileText(gameId, ghostingPersonalization.toCfgLines(settings && settings.personalization)).split("\n")) {
    const verb = /^\s*([^\s"]+)(?:\s+(\S.*))?$/.exec(line);
    if (/^\s*\/\/ HLSR_YOUTUBE_URL /.test(line) || (verb && (BLOCK_VERB.test(verb[1]) || (!verb[2] && !verb[1].startsWith("//"))))) block.push(line);
    else cvars.push(line);
  }
  return [cvars, block];
}

// cvar -> значение
function personalizationPairs(gameId, settings) {
  return cvarsOf(generatedLines(gameId, settings)[0].join("\n"));
}

function extrasBlock(gameId, settings) {
  return generatedLines(gameId, settings)[1];
}

// Настройки stored, поверх которых лежит файл игрока. Поля, которые задаёт cvar, берутся из файла,
// а если cvar в файле нет - это значение игры по умолчанию (не из settings.json). Поля без cvar
// (паки, файлы спрея и YouTube) остаются из stored.
function overlayPersonalization(gameId, stored, text) {
  if (!PERSONALIZED.has(gameId) || !stored) return stored;
  const defaults = launchArgs.defaultSettings();
  const fromFile = cvarsOf(text);
  const current = personalizationPairs(gameId, stored);
  const base = personalizationPairs(gameId, defaults);
  if (gameId === "hl1-won") {
    const lines = Object.keys(current)
      .map((name) => [name, name in fromFile ? fromFile[name] : base[name]])
      .filter(([name, value]) => value !== undefined && value !== current[name])
      .map(([name, value]) => `${name} "${value}"`);
    if (!lines.length) return stored;
    const adopted = adoptImported(lines.join("\n"), { personalization: stored.personalization, bxt: stored.bxt });
    return { ...stored, personalization: adopted.personalization, bxt: adopted.bxt };
  }
  const core = Object.keys(base).map((name) => `${name} "${name in fromFile ? fromFile[name] : base[name]}"`);
  const imported = ghostingPersonalization.importFromConfigCfg(null, { text: fromFileText(gameId, `${core.join("\n")}\n${text}`) });
  if (!imported) return stored;
  const own = ghostingPersonalization.sanitizePersonalization(stored.personalization);
  for (const key of ASSET_EXTRAS) if (!(key in imported.extra) && key in own.extra) imported.extra[key] = own.extra[key];
  const crossOf = (p) => JSON.stringify(Object.entries(personalizationPairs(gameId, { personalization: p })).filter(([name]) => name.startsWith("gh_cross")));
  const initial = ghostingPersonalization.defaultPersonalization();
  const crossPreset = crossOf(imported) === crossOf(own) ? own.crossPreset
    : crossOf(imported) === crossOf(initial) ? initial.crossPreset : "custom";
  const result = ghostingPersonalization.sanitizePersonalization({ ...imported, crossPreset });
  return JSON.stringify(result) === JSON.stringify(own) ? stored : { ...stored, personalization: result };
}

// { settings, sha256 }: sha256 файла игрока, по которому построены настройки (null - файла нет).
// Отпечаток есть у любой игры с файлом игрока (HL2 тоже: «Клавиатура» пишет со сверкой).
function inspectPersonalization(installPath, gameId, gameDirectory, stored, options = {}) {
  if (!bindPresets.engineFor(gameId) || !gameDirectory) return { settings: stored, sha256: undefined };
  const current = read(installPath, gameId, gameDirectory, options);
  return { settings: overlayPersonalization(gameId, stored, current.text), sha256: current.sha256 };
}

function readPersonalization(installPath, gameId, gameDirectory, stored, options = {}) {
  return inspectPersonalization(installPath, gameId, gameDirectory, stored, options).settings;
}

// Блок extras заменяется целиком; такие же команды вне блока (перенос с 0.4.8) убираются,
// иначе старая строка gh_spray_set осталась бы и выполнялась при каждом запуске.
function replaceExtrasBlock(doc, lines) {
  let out = lines.length ? cfgDocument.setBlock(doc, EXTRAS_BLOCK, lines, "personalization") : cfgDocument.removeBlock(doc, EXTRAS_BLOCK);
  const block = cfgDocument.findBlock(out, EXTRAS_BLOCK);
  const kept = out.lines.filter((line, index) => !BLOCK_LINE.test(line.text) || (block && index >= block.begin && index <= block.end));
  out = { ...out, lines: kept };
  return out;
}

// Правит документ на месте: в раздел personalization идут только значения, которые
// отличаются у previous (каким его видело окно, то есть с учётом файла) и next.
// options.explicit («Сбросить», §5): все значения next пишутся явными строками, даже равные
// показанному умолчанию, — иначе в игре осталось бы то, что она сохранила в config.cfg.
function applyPersonalization(doc, gameId, previous, next, text, options = {}) {
  if (!PERSONALIZED.has(gameId)) return { doc, changed: [] };
  const seen = overlayPersonalization(gameId, previous, text);
  const before = personalizationPairs(gameId, seen);
  const after = personalizationPairs(gameId, next);
  const changed = [];
  let out = doc;
  for (const name of Object.keys(after)) {
    if (!options.explicit && before[name] === after[name]) continue;
    const present = cfgDocument.get(out, `cvar:${name}`);
    if (present && present.value === after[name]) continue;
    out = cfgDocument.set(out, `cvar:${name}`, after[name], "personalization");
    changed.push(name);
  }
  const wanted = extrasBlock(gameId, next);
  if (wanted.join("\n") !== extrasBlock(gameId, seen).join("\n")) {
    out = replaceExtrasBlock(out, wanted);
    changed.push(EXTRAS_BLOCK);
  }
  return { doc: out, changed };
}
// hlsr-personalization.cfg остаётся снимком для скриптов гайда HL1 (exec в игре).
// Переписывается, если лежит в папке мода или на него ссылается файл игрока (force).
function syncSnapshot(installPath, gameId, gameDirectory, text, settings, options = {}) {
  if (bindPresets.engineFor(gameId) !== "goldsrc" || !settings) return false;
  const fs = options.fs || nodeFs;
  const files = cfgSearch.filesOf(installPath, gameDirectory, "goldsrc", options);
  const name = goldsrcPersonalization.PERSONALIZATION_FILE;
  if (!options.force && !files.exists(name) && !SNAPSHOT_EXEC.test(text)) return false;
  fs.mkdirSync(files.modDir, { recursive: true });
  fs.writeFileSync(files.pathOf(name), goldsrcPersonalization.toCfgLines(settings), "latin1");
  return true;
}

// previous - настройки до сохранения, next - после. -> { file, changed: [cvar] }
function writePersonalization(installPath, gameId, gameDirectory, previous, next, options = {}) {
  const current = read(installPath, gameId, gameDirectory, options);
  const { doc, changed } = applyPersonalization(cfgDocument.parse(current.text), gameId, previous, next, current.text);
  const out = cfgDocument.serialize(doc);
  let file = current.file;
  let sha256 = current.sha256;
  if (out !== current.text) {
    const expectedSha256 = options.expectedSha256 !== undefined ? options.expectedSha256 : current.sha256;
    ({ file, sha256 } = write(installPath, gameId, gameDirectory, out, { ...options, expectedSha256 }));
  }
  syncSnapshot(installPath, gameId, gameDirectory, out, next, options);
  return { file, changed, sha256 };
}

module.exports = {
  PlayerConfigError, locate, template, templateLines, toFileText, read, write, isStock, ensure, prepare, setupState, setupDone,
  SNAPSHOT_EXEC, EXTRAS_BLOCK, personalizationPairs, extrasBlock, overlayPersonalization, inspectPersonalization, readPersonalization,
  applyPersonalization, syncSnapshot, writePersonalization,
};