"use strict";

const nodeFs = require("fs");
const path = require("path");
const crypto = require("crypto");

const playerConfig = require("./player-config");
const cfgSearch = require("./cfg-search");
const gameBindsInstall = require("./game-binds-install");
const goldsrcPersonalization = require("./goldsrc-personalization");
const ghostingPersonalization = require("./ghosting-personalization");
const hl1Default = require("./hl1-default-userconfig");
const ghostingDefault = require("./ghosting-default-autoexec");
const cfgDocument = require("../../shared/cfg-document");
const cfgImport = require("../../shared/cfg-import");
const gameBinds = require("../../shared/game-binds");
const gameConfig = require("../../shared/game-config");
const bindPresets = require("../../shared/bind-presets");

// Переход с 0.4.8 (§9 замысла): файл игрока, hlsr-*.cfg лаунчера и импорт один раз
// сливаются в файл игрока. Итог в игре тот же, что дала бы 0.4.8 при следующем
// запуске: каждая строка exec заменяется текстом файла на том же месте, порядок
// выполнения не меняется. Прежние файлы переносятся в .hlsr/backup/migration-<дата>/,
// последней ставится отметка .hlsr/config-setup.json.

const SETUP_FILE = path.join(".hlsr", "config-setup.json");
const BACKUP_DIR = path.join(".hlsr", "backup");
const BACKUP_NAME = /^migration-\d{8}-\d{6}$/;
const LAUNCHER_FILE = /^hlsr-[^/]*\.cfg$/i;
const LAUNCHER_EXEC_LINE = /^exec\s+hlsr-[a-z-]+\.cfg$/i;
// Файлы, которые пишет 0.4.8, и раздел файла игрока для их текста.
const SECTION_OF = {
  [goldsrcPersonalization.PERSONALIZATION_FILE]: "personalization",
  [gameBinds.BINDS_FILE]: "binds",
  "hlsr-config.cfg": "settings",
};
const RUN_TEMPLATE_LINE = `alias run "${bindPresets.HL1_RUN_ALIAS}"`;
const HEADERS = new Set(Object.values(cfgDocument.SECTIONS));
const MAX_DEPTH = 8;
// Стандартный файл 0.4.8 и стоковые, которые она заменяет им перед запуском.
const DEFAULTS = {
  "hl1-won": { text: hl1Default.TEXT, stock: hl1Default.REPLACEABLE_SHA256 },
  "ghosting-renewed": { text: ghostingDefault, stock: ghostingPersonalization.STOCK_AUTOEXEC_SHA256 },
};

const sha256Of = (bytes) => crypto.createHash("sha256").update(bytes).digest("hex");
const linesOf = (text) => cfgDocument.parse(text).lines.map((line) => line.text);

function contextOf(installPath, gameId, gameDirectory, options) {
  const found = playerConfig.locate(installPath, gameId, gameDirectory, options);
  if (!found) return null;
  const fs = options.fs || nodeFs;
  const record = gameBindsInstall.readImportRecord(installPath, { fs });
  const lower = (names) => new Set(names.map((name) => String(name).toLowerCase()));
  return {
    installPath, gameId, gameDirectory, options, fs, found,
    engine: found.engine,
    files: cfgSearch.filesOf(installPath, gameDirectory, found.engine, options),
    // exec импорта в файле игрока и файлы записи, которые раскрываются внутри
    // (config.cfg, userconfig.cfg и autoexec.cfg выполняет сама игра).
    execs: lower(record ? record.execs : []),
    recordFiles: lower(record ? record.files.map((item) => item.path)
      .filter((name) => !cfgImport.ENGINE_RUN.test(name.split("/").pop())) : []),
  };
}

// Тексты файлов — байты как latin1, как у файла игрока (R17): переход не меняет ни одного байта игрока.
function readText(ctx, name) {
  try {
    return ctx.fs.readFileSync(gameBindsInstall.importTarget(ctx.files, name, ctx.fs), "latin1");
  } catch (error) {
    if (error && (error.code === "ENOENT" || error.code === "EISDIR")) return null;
    throw error;
  }
}

// Строка, в которой только exec одного файла: { key, name }, как его найдёт движок.
function execTargetOf(text, engine) {
  const commands = gameConfig.lineCommands(text);
  if (commands.length !== 1 || commands[0].args.length !== 2) return null;
  const [verb, arg] = commands[0].args;
  return verb.value.toLowerCase() === "exec" ? cfgImport.execTarget(arg.value, engine) : null;
}

const callsLauncher = (ctx, text) => cfgDocument.parse(text).lines.some((line) => {
  const target = execTargetOf(line.text, ctx.engine);
  return Boolean(target) && LAUNCHER_FILE.test(target.key);
});

// Текст файла 0.4.8 таким, каким она записала бы его перед запуском (её
// генераторы, как в applyBinds и applyToInstall); null — генератора нет или нет настроек.
function generatedText(ctx, key) {
  const { settings } = ctx.options;
  if (!settings) return null;
  if (key === gameBinds.BINDS_FILE) {
    const clean = gameBinds.sanitizeOverrides(settings.binds);
    const { aliases } = gameBindsInstall.scriptAliases(ctx.installPath, ctx.gameId, ctx.gameDirectory, ctx.engine, clean, ctx.fs);
    return gameBinds.toCfgLines(settings.binds, { engine: ctx.engine, aliases });
  }
  if (key === goldsrcPersonalization.PERSONALIZATION_FILE) {
    if (ctx.gameId === "hl1-won") return goldsrcPersonalization.toCfgLines(settings);
    if (ctx.gameId === "ghosting-renewed" && settings.personalization)
      return ghostingPersonalization.toCfgLines(ghostingPersonalization.sanitizePersonalization(settings.personalization));
  }
  return null;
}

// Текст генератора — байтами, какими 0.4.8 записала бы файл (Source — UTF-8).
function launcherText(ctx, key) {
  const generated = generatedText(ctx, key);
  return generated === null ? readText(ctx, key) : playerConfig.toFileText(ctx.gameId, generated);
}

// Пояснения 0.4.8 в начале её файлов («Generated by HLSR Launcher…») в файле игрока не к месту.
function withoutHeader(lines) {
  const first = lines.findIndex((line) => !line.trim().startsWith("//"));
  return first < 0 ? [] : lines.slice(first);
}

// Хвост syncImportTrailer: повтор exec лаунчера после импорта. 0.4.8 пересобирает
// его перед каждым запуском, а те же файлы файл игрока выполняет ниже.
function withoutTrailer(lines) {
  const mark = lines.findIndex((line) => line.trim() === gameBindsInstall.IMPORT_TRAILER_MARK);
  if (mark < 0) return lines;
  let end = mark;
  while (end > 0 && lines[end - 1].trim() === "") end -= 1;
  return lines.slice(0, end);
}

// Строки вместо exec: заголовок и текст файла, exec в нём раскрыты так же.
// [] — строку убрать (файла нет, игра его и не выполнит), null — оставить.
function expand(ctx, target, stack, topLevel) {
  const section = SECTION_OF[target.key];
  if (section) {
    const text = launcherText(ctx, target.key);
    return text === null ? [] : [cfgDocument.SECTIONS[section], ...withoutHeader(linesOf(text))];
  }
  const launcher = LAUNCHER_FILE.test(target.key);
  if (!launcher && !(topLevel ? ctx.execs : ctx.recordFiles).has(target.key)) return null;
  if (stack.includes(target.key) || stack.length >= MAX_DEPTH) return null;
  const text = readText(ctx, target.name);
  if (text === null) return launcher ? [] : null;
  const inner = withoutTrailer(linesOf(text)).flatMap((line) => {
    const next = execTargetOf(line, ctx.engine);
    return (next && expand(ctx, next, stack.concat(target.key), false)) || [line];
  });
  return [`// ===== from ${target.name} =====`, ...inner];
}

// alias run шаблона 0.4.8 перекрыт alias run ниже (из hlsr-binds.cfg) — остаётся последний.
function withoutShadowedRun(lines) {
  const isRun = (line) => gameConfig.lineCommands(line.text).some((command) => cfgDocument.keyOf(command.args) === "alias:run");
  return lines.filter((line, index) => line.text.trim() !== RUN_TEMPLATE_LINE || !lines.slice(index + 1).some(isRun));
}

// Строки игрока остаются как есть (те же байты и переводы строк), меняются только строки exec.
function mergedText(ctx, text) {
  const doc = cfgDocument.parse(text);
  const lines = [];
  for (const line of doc.lines) {
    const target = execTargetOf(line.text, ctx.engine);
    const expanded = target && expand(ctx, target, [], true);
    if (!expanded) {
      lines.push(line);
      continue;
    }
    if (expanded.length && lines.length && lines[lines.length - 1].text.trim() !== "") lines.push({ text: "", eol: doc.eol });
    expanded.forEach((item, index) => lines.push({ text: item, eol: index === expanded.length - 1 ? line.eol : doc.eol }));
  }
  return cfgDocument.serialize(Object.assign({}, doc, { lines: withoutShadowedRun(lines) }));
}

// Под Linux (Wine) «save :» сейв не удаляет. Строка — бинд GHOSTING_SAVE_DELETION из
// bind-presets под Linux: там значение зависит от платформы самого лаунчера.
const LINUX_SAVE_DELETION_BIND = 'bind h "save /; reload"';

// Что 0.4.8 сделала бы с файлом игрока перед запуском: стоковый — на свой стандартный
// со строками exec (ensureDefaultUserconfig, ensureDefaultAutoexec), под Linux у Ghosting —
// рабочий бинд удаления сейва (fixSaveDeletionBind), недостающие exec — в конец
// (ensureExecLine). Без настроек exec дописываются только для файлов, что лежат на диске.
function launchedText(ctx, current) {
  const { settings } = ctx.options;
  let text = current.text;
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const defaults = DEFAULTS[ctx.gameId];
  if (defaults) {
    const own = lines.filter((line) => !LAUNCHER_EXEC_LINE.test(line.trim())).join("\n").replace(/\s+$/, "");
    if (!current.exists || defaults.stock.has(sha256Of(Buffer.from(own, "latin1")))) {
      const execLines = lines.map((line) => line.trim()).filter((line) => LAUNCHER_EXEC_LINE.test(line));
      text = playerConfig.toFileText(ctx.gameId, defaults.text) + execLines.map((line) => `${line}\r\n`).join("");
    }
  }
  if (ctx.gameId === "ghosting-renewed" && (ctx.options.platform || process.platform) === "linux") {
    text = text.split("\n").map((line) => {
      if (line.trim() !== ghostingPersonalization.WINDOWS_SAVE_DELETION_BIND) return line;
      return line.slice(0, line.indexOf("bind")) + LINUX_SAVE_DELETION_BIND + (line.endsWith("\r") ? "\r" : "");
    }).join("\n");
  }
  // Порядок, в каком 0.4.8 дописывает exec: HL1 — персонализация, потом бинды
  // (applyToInstall); Source — бинды, потом персонализация Ghosting (LaunchService).
  const execs = [gameBinds.BINDS_EXEC_LINE];
  if (ctx.gameId === "hl1-won") execs.unshift(goldsrcPersonalization.EXEC_LINE);
  if (ctx.gameId === "ghosting-renewed" && (!settings || settings.personalization)) execs.push(goldsrcPersonalization.EXEC_LINE);
  for (const execLine of execs) {
    if (!settings && !ctx.files.exists(execLine.slice("exec ".length))) continue;
    if (text.split(/\r?\n/).some((line) => line.trim().toLowerCase() === execLine)) continue;
    const eol = text.includes("\r\n") || ctx.found.encoding === "latin1" ? "\r\n" : "\n";
    text += `${text.length === 0 || text.endsWith("\n") ? "" : eol}${execLine}${eol}`;
  }
  return text;
}

// Переход прервали после записи нового файла: в нём уже разделы лаунчера и нет
// exec его файлов. Второй раз не сливаем — только доносим файлы и ставим отметку.
// Шаблон не в счёт: его мог поставить ensure вместо стандартного файла 0.4.8, а
// exec 0.4.8 дописала бы в конец (launchedText).
const isMerged = (ctx, text) => !playerConfig.isStock(ctx.gameId, text) &&
  cfgDocument.parse(text).lines.some((line) => HEADERS.has(line.text.trim())) && !callsLauncher(ctx, text);

function launcherFilesOf(ctx) {
  return ctx.files.dirs.flatMap((dir) => {
    let names = [];
    try {
      names = ctx.fs.readdirSync(dir);
    } catch (error) {
      if (error.code !== "ENOENT" && error.code !== "ENOTDIR") throw error;
    }
    return names.filter((name) => LAUNCHER_FILE.test(name)).map((name) => ({ name, file: path.join(dir, name) }));
  });
}

// hlsr-*.cfg, которые файл игрока ещё зовёт (alias "exec hlsr-import-x.cfg" импорта
// 0.4.3), остаются на месте: без них скрипт игрока не сработает.
function stillUsed(ctx, text, candidates) {
  const used = new Set();
  const queue = [text];
  while (queue.length) {
    for (const target of cfgImport.execsOf(queue.pop(), ctx.engine)) {
      if (used.has(target.key) || !candidates.some((item) => item.name.toLowerCase() === target.key)) continue;
      used.add(target.key);
      queue.push(withoutTrailer(linesOf(readText(ctx, target.key) || "")).join("\n"));
    }
  }
  return used;
}

function stamp(now) {
  const date = new Date(now === undefined ? Date.now() : now);
  const two = (value) => String(value).padStart(2, "0");
  return `${date.getFullYear()}${two(date.getMonth() + 1)}${two(date.getDate())}-` +
    `${two(date.getHours())}${two(date.getMinutes())}${two(date.getSeconds())}`;
}

// Доводя прерванный переход, файлы кладём к копии прежнего файла игрока.
function backupDirOf(ctx, recovering) {
  const root = path.join(ctx.installPath, BACKUP_DIR);
  if (recovering) {
    let names = [];
    try {
      names = ctx.fs.readdirSync(root).filter((name) => BACKUP_NAME.test(name)).sort();
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    if (names.length) return path.join(root, names[names.length - 1]);
  }
  return path.join(root, `migration-${stamp(ctx.options.now)}`);
}

function readSetup(installPath, fs) {
  try {
    const value = JSON.parse(fs.readFileSync(path.join(installPath, SETUP_FILE), "utf8"));
    return value && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch (error) {
    return null;
  }
}

function needsMigration(installPath, gameId, gameDirectory, options = {}) {
  const ctx = contextOf(installPath, gameId, gameDirectory, options);
  if (!ctx) return false;
  const setup = readSetup(installPath, ctx.fs);
  if (setup && "migrated" in setup) return false;
  if (launcherFilesOf(ctx).length || ctx.fs.existsSync(path.join(installPath, gameBindsInstall.IMPORT_RECORD))) return true;
  return callsLauncher(ctx, playerConfig.read(installPath, gameId, gameDirectory, options).text);
}

// Порядок: копия прежнего файла игрока → новый файл → перенос → отметка. Копия
// раньше нового файла: сбой между ними не теряет прежний файл.
function migrate(installPath, gameId, gameDirectory, options = {}) {
  const ctx = contextOf(installPath, gameId, gameDirectory, options);
  if (!ctx || !needsMigration(installPath, gameId, gameDirectory, options))
    return { status: "skipped", file: ctx ? ctx.found.file : null, backupDir: null };
  const { fs } = ctx;
  const current = playerConfig.read(installPath, gameId, gameDirectory, options);
  const recovering = isMerged(ctx, current.text);
  const backupDir = backupDirOf(ctx, recovering);
  const backupOf = (file) => path.join(backupDir, path.relative(installPath, file));
  let text = current.text;
  if (!recovering) {
    text = mergedText(ctx, launchedText(ctx, current));
    if (current.exists) {
      fs.mkdirSync(path.dirname(backupOf(current.file)), { recursive: true });
      fs.copyFileSync(current.file, backupOf(current.file));
    }
    if (text !== current.text)
      playerConfig.write(installPath, gameId, gameDirectory, text, { ...options, expectedSha256: current.sha256 });
  }
  const candidates = launcherFilesOf(ctx);
  const used = stillUsed(ctx, text, candidates);
  for (const item of candidates) {
    const key = item.name.toLowerCase();
    if (!used.has(key)) {
      fs.mkdirSync(path.dirname(backupOf(item.file)), { recursive: true });
      fs.renameSync(item.file, backupOf(item.file));
      continue;
    }
    // Оставленный файл лаунчера — снимок для скриптов, которые зовут его во время игры
    // (alias deffov "exec hlsr-personalization.cfg" гайда): пишем его, как 0.4.8 перед
    // запуском, туда, откуда его выполнит игра. Прежний текст — в копию (один раз).
    const generated = generatedText(ctx, key);
    if (generated === null || item.file !== ctx.files.pathOf(key)) continue;
    if (!fs.existsSync(backupOf(item.file))) {
      fs.mkdirSync(path.dirname(backupOf(item.file)), { recursive: true });
      fs.copyFileSync(item.file, backupOf(item.file));
    }
    fs.writeFileSync(item.file, Buffer.from(generated, ctx.found.encoding));
  }
  const setup = readSetup(installPath, fs) || {};
  fs.mkdirSync(path.join(installPath, ".hlsr"), { recursive: true });
  fs.writeFileSync(path.join(installPath, SETUP_FILE),
    JSON.stringify(Object.assign(setup, { migrated: options.launcherVersion || null }), null, 2));
  return { status: "migrated", file: current.file, backupDir };
}

module.exports = { needsMigration, migrate, stamp, BACKUP_DIR };
