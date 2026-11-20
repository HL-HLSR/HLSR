"use strict";

// Бинды игр лаунчера: имена клавиш движков, разбор cfg и переназначения.
//
// GoldSrc (HL1) выполняет config.cfg, а он последней строкой — userconfig.cfg.
// Source (HL2, Ghosting) выполняет cfg/config.cfg, потом cfg/autoexec.cfg.
// Оба движка перезаписывают config.cfg при выходе: в начале unbindall,
// потом все бинды. Переназначения лаунчера пишутся в hlsr-binds.cfg и
// подключаются последней строкой userconfig.cfg или autoexec.cfg, поэтому
// всегда главнее.
//
// После первого запуска игра сохранит переназначенный бинд в config.cfg, и
// прежнее значение оттуда пропадёт. Поэтому у каждого переназначения хранится
// base — что было на клавише до лаунчера. Отмена переназначения не удаляет
// запись, а превращает её в «возврат»: command = base. Такая запись снова
// пишет в игру прежний бинд и удаляется, когда config.cfg его уже содержит.

const BINDS_FILE = "hlsr-binds.cfg";
const BINDS_EXEC_LINE = `exec ${BINDS_FILE}`;

const MAX_COMMAND = 255;
const MAX_OVERRIDES = 300;

// Имена из gfx/shell/kb_keys.lst HL1, без джойстика и AUX. Старый Source
// (HL2 2707) называет клавиши так же.
const GOLDSRC_KEYS = [
  "TAB", "ENTER", "ESCAPE", "SPACE", "BACKSPACE",
  "UPARROW", "DOWNARROW", "LEFTARROW", "RIGHTARROW",
  "ALT", "CTRL", "SHIFT", "CAPSLOCK", "PAUSE",
  "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
  "INS", "DEL", "PGDN", "PGUP", "HOME", "END",
  "KP_HOME", "KP_UPARROW", "KP_PGUP", "KP_LEFTARROW", "KP_5", "KP_RIGHTARROW",
  "KP_END", "KP_DOWNARROW", "KP_PGDN", "KP_ENTER", "KP_INS", "KP_DEL",
  "KP_SLASH", "KP_MINUS", "KP_PLUS",
  "MOUSE1", "MOUSE2", "MOUSE3", "MOUSE4", "MOUSE5", "MWHEELUP", "MWHEELDOWN",
];
// Source с inputsystem (HL2 4260, Ghosting) различает правые модификаторы,
// Win, Num Lock и Scroll Lock, а Num * зовёт KP_MULTIPLY.
const SOURCE_KEYS = GOLDSRC_KEYS.concat([
  "RSHIFT", "RCTRL", "RALT", "LWIN", "RWIN", "APP",
  "NUMLOCK", "SCROLLLOCK", "KP_MULTIPLY",
]);
const NAMED = new Set(SOURCE_KEYS);

// Где движок ищет cfg (относительно папки игры), с чего начинает их
// выполнять и в какой файл лаунчер дописывает exec своего hlsr-binds.cfg.
const ENGINES = {
  goldsrc: { keys: GOLDSRC_KEYS, cfgDir: "", roots: ["config.cfg"], execFile: "userconfig.cfg", encoding: "latin1" },
  source: { keys: SOURCE_KEYS, cfgDir: "cfg", roots: ["config.cfg", "autoexec.cfg"], execFile: "autoexec.cfg", encoding: "utf8" },
  "source-old": { keys: GOLDSRC_KEYS, cfgDir: "cfg", roots: ["config.cfg", "autoexec.cfg"], execFile: "autoexec.cfg", encoding: "utf8" },
};

function engineOf(name) {
  return ENGINES[name] || ENGINES.goldsrc;
}

// Имя клавиши как его понимает движок: одиночный символ без учёта регистра
// (буквы в нижнем), длинные имена — в верхнем. Кавычку забиндить нельзя:
// cfg не умеет её экранировать.
function normalizeKey(name) {
  if (typeof name !== "string") return null;
  const raw = name.trim();
  if (raw.length === 1) {
    const code = raw.charCodeAt(0);
    if (code < 33 || code > 126 || raw === '"') return null;
    return raw.toLowerCase();
  }
  const upper = raw.toUpperCase();
  if (upper === "SEMICOLON") return ";";
  return NAMED.has(upper) ? upper : null;
}

function commandError(command) {
  if (typeof command !== "string") return "INVALID";
  if (command.length > MAX_COMMAND) return "TOO_LONG";
  // Кавычки cfg не экранирует, управляющие символы рвут строку.
  for (let i = 0; i < command.length; i += 1) {
    const code = command.charCodeAt(i);
    if (code === 34 || code < 32 || code === 127) return "FORBIDDEN_CHARS";
  }
  return null;
}

function cleanCommand(command) {
  return typeof command === "string" ? command.trim().replace(/\s+/g, " ") : command;
}

// Разбор cfg так же, как это делает движок: команды разделяются переводом
// строки или «;» вне кавычек, «//» вне кавычек — комментарий до конца
// строки, кавычки не экранируются.
function parseCfg(text) {
  const commands = [];
  let args = [];
  let token = null;
  let quoted = false;
  const endToken = () => {
    if (token !== null) args.push(token);
    token = null;
  };
  const endCommand = () => {
    endToken();
    if (args.length) commands.push(args);
    args = [];
  };
  const source = String(text || "");
  for (let i = 0; i < source.length; i += 1) {
    const ch = source[i];
    if (quoted) {
      if (ch === '"') {
        quoted = false;
        args.push(token);
        token = null;
      } else if (ch === "\n") {
        quoted = false;
        endCommand();
      } else {
        token += ch;
      }
      continue;
    }
    if (ch === '"') {
      endToken();
      quoted = true;
      token = "";
    } else if (ch === "\n" || ch === ";") {
      endCommand();
    } else if (ch === "/" && source[i + 1] === "/") {
      while (i < source.length && source[i] !== "\n") i += 1;
      endCommand();
    } else if (ch === " " || ch === "\t" || ch === "\r") {
      endToken();
    } else {
      token = (token || "") + ch;
    }
  }
  if (quoted) {
    args.push(token);
    token = null;
  }
  endCommand();
  return commands;
}

// Имя cfg, которое движок выполнит exec (и подпапки мода: exec scripts/jump.cfg).
const CFG_NAME = /^[A-Za-z0-9_-][A-Za-z0-9._ -]*(\/[A-Za-z0-9_-][A-Za-z0-9._ -]*){0,2}\.cfg$/;

// Бинды, которые появляются только по нажатию: клавиша зовёт exec (bind k "exec tc_n.cfg",
// в том числе через alias), а файл назначает другие клавиши. Стартовые файлы (startupFiles)
// уже учтены в binds и не читаются. Бинд, совпадающий со стартовым, не считается.
// { key: [{ command, file, trigger }] } — file null, если bind прямо в alias/команде.
function runtimeBinds(binds, aliases, readFile, startupFiles) {
  const runtime = Object.create(null);
  const startup = new Set((startupFiles || []).map((name) => String(name).toLowerCase()));
  // Каждый файл читается и разбирается один раз, сколько бы клавиш его ни звали.
  const parsed = new Map();
  const commandsOf = (name) => {
    if (!parsed.has(name)) {
      const text = readFile(name);
      parsed.set(name, text === null || text === undefined ? null : parseCfg(text));
    }
    return parsed.get(name);
  };
  let budget = 5000; // команд на все клавиши: огромный или зацикленный конфиг не вешает окно
  const record = (rawKey, command, ctx) => {
    const key = normalizeKey(rawKey);
    if (!key || (binds[key] && binds[key].command === command)) return;
    const list = runtime[key] || (runtime[key] = []);
    if (list.length < 4 && !list.some((entry) => entry.command === command && entry.trigger === ctx.trigger)) {
      list.push({ command, file: ctx.file, trigger: ctx.trigger });
    }
  };
  const walk = (commands, ctx, depth) => {
    if (depth > 8) return;
    for (const args of commands) {
      if ((budget -= 1) < 0) return;
      const verb = args[0].toLowerCase();
      if (verb === "bind" && args.length >= 3) {
        record(args[1], cleanCommand(args.slice(2).join(" ")), ctx);
      } else if (verb === "exec" && args.length >= 2) {
        const name = String(args[1]).replace(/\\/g, "/");
        const lower = name.toLowerCase();
        if (startup.has(lower) || ctx.files.has(lower) || lower === BINDS_FILE || !CFG_NAME.test(name)) continue;
        ctx.files.add(lower);
        const commands = commandsOf(name);
        // called — свой у файла: alias, вызванный из двух файлов, назовёт оба.
        if (commands) walk(commands, Object.assign({}, ctx, { file: name, called: new Set(ctx.called) }), depth + 1);
      } else if (verb === "alias" && args.length >= 2) {
        ctx.aliases.set(args[1].toLowerCase(), args.slice(2).join(" "));
      } else if (ctx.aliases.has(verb) && !ctx.called.has(verb)) {
        ctx.called.add(verb);
        walk(parseCfg(ctx.aliases.get(verb)), ctx, depth + 1);
      }
    }
  };
  const known = new Map(Object.entries(aliases).map(([name, value]) => [name.toLowerCase(), value]));
  for (const [trigger, bound] of Object.entries(binds)) {
    walk(parseCfg(bound.command), { trigger, file: null, files: new Set(), called: new Set(), aliases: new Map(known) }, 0);
  }
  return runtime;
}

// Переменные, значения которых нужны лаунчеру из cfg игрока.
const READ_CVARS = new Set(["sensitivity", "m_pitch"]);

// Выполняет цепочку cfg движка (roots по порядку) и собирает итоговые бинды,
// alias и нужные переменные (READ_CVARS). Свой hlsr-binds.cfg лаунчер
// пропускает: переназначения он знает и так. readFile(name) возвращает текст
// или null, если файла нет.
function readBinds(readFile, roots = ENGINES.goldsrc.roots) {
  const binds = Object.create(null);
  const aliases = Object.create(null);
  const cvars = Object.create(null);
  const files = [];
  const visited = new Set();

  const run = (raw, depth) => {
    // Файлы в подпапках мода тоже выполняются (exec scripts/jump.cfg).
    const name = String(raw).replace(/\\/g, "/");
    const lower = name.toLowerCase();
    if (depth > 8 || visited.has(lower) || lower === BINDS_FILE) return;
    if (!CFG_NAME.test(name)) return;
    const text = readFile(name);
    if (text === null || text === undefined) return;
    visited.add(lower);
    files.push(name);
    for (const args of parseCfg(text)) {
      const verb = args[0].toLowerCase();
      if (verb === "unbindall") {
        for (const key of Object.keys(binds)) delete binds[key];
      } else if (verb === "bind" && args.length >= 3) {
        const key = normalizeKey(args[1]);
        if (key) binds[key] = { command: cleanCommand(args.slice(2).join(" ")), source: name };
      } else if (verb === "unbind" && args.length >= 2) {
        const key = normalizeKey(args[1]);
        if (key) delete binds[key];
      } else if (verb === "alias" && args.length >= 2) {
        aliases[args[1]] = args.slice(2).join(" ");
      } else if (verb === "exec" && args.length >= 2) {
        run(args[1], depth + 1);
      } else if (READ_CVARS.has(verb) && args.length >= 2) {
        cvars[verb] = args[1];
      }
    }
  };

  for (const root of roots) run(root, 0);
  return { binds, aliases, cvars, files, runtime: runtimeBinds(binds, aliases, readFile, files) };
}

function sanitizeOverrides(input) {
  const result = {};
  if (!input || typeof input !== "object" || Array.isArray(input)) return result;
  let count = 0;
  for (const [rawKey, entry] of Object.entries(input)) {
    if (count >= MAX_OVERRIDES) break;
    const key = normalizeKey(rawKey);
    if (!key || !entry || typeof entry !== "object") continue;
    const command = cleanCommand(entry.command);
    if (commandError(command)) continue;
    let base = entry.base === null || entry.base === undefined ? "" : cleanCommand(entry.base);
    if (commandError(base)) base = "";
    result[key] = { command, base };
    count += 1;
  }
  return result;
}

// Возврат (command = base) больше не нужен, когда игра уже хранит base сама.
function pruneOverrides(overrides, fileBinds) {
  const result = {};
  for (const [key, entry] of Object.entries(sanitizeOverrides(overrides))) {
    const current = fileBinds && fileBinds[key] ? fileBinds[key].command : "";
    if (entry.command === entry.base && current === entry.base) continue;
    result[key] = entry;
  }
  return result;
}

// «Сбросить» в окне настроек: все переназначения становятся возвратами.
function resetOverrides(overrides) {
  const result = {};
  for (const [key, entry] of Object.entries(sanitizeOverrides(overrides))) {
    result[key] = { command: entry.base, base: entry.base };
  }
  return result;
}

// Source не принимает «;» в имени клавиши даже в кавычках.
const quoteKey = (key, engine) => `"${key === ";" && engine !== "goldsrc" ? "SEMICOLON" : key}"`;

// aliases — [[имя, значение]] скриптов из заготовок, которые нужны
// назначенным командам. Пишутся до биндов: alias должен существовать к
// нажатию клавиши. Пустое значение — пустой alias, как «alias _taubo» в cfg.
function toCfgLines(overrides, options = {}) {
  const engine = options.engine || "goldsrc";
  const clean = sanitizeOverrides(overrides);
  const lines = [
    "// Generated by HLSR Launcher. Do not edit: overwritten before every launch.",
    `// Executed from ${engineOf(engine).execFile}. Change binds in the launcher: game settings, Keyboard.`,
  ];
  for (const [name, value] of options.aliases || []) {
    lines.push(value ? `alias ${name} "${value}"` : `alias ${name}`);
  }
  for (const key of Object.keys(clean).sort()) {
    const { command } = clean[key];
    lines.push(command ? `bind ${quoteKey(key, engine)} "${command}"` : `unbind ${quoteKey(key, engine)}`);
  }
  lines.push("");
  return lines.join("\r\n");
}

// Импорт cfg на вкладке «Клавиатура»: переназначения, после которых клавиши
// станут такими же, как в файле. Файл разбирается сам по себе, его exec не
// выполняются. unbindall в файле значит, что клавиш вне файла быть не должно.
// Бинд на alias, объявленный только в этом файле, пропускается: в игре такого
// alias нет. knownCommand(command) — команда из заготовок лаунчера, их alias
// он пишет сам; aliases — alias, которые уже есть в cfg игры.
function importOverrides(text, options = {}) {
  const fileBinds = options.fileBinds || {};
  const current = sanitizeOverrides(options.overrides);
  const knownCommand = typeof options.knownCommand === "function" ? options.knownCommand : () => false;
  const gameAliases = new Set(Object.keys(options.aliases || {}).map((name) => name.toLowerCase()));
  const imported = new Map();
  const fileAliases = new Set();
  const stats = { changed: 0, unbound: 0, unchanged: 0, skippedAlias: 0, skippedInvalid: 0, skippedKey: 0, found: 0 };
  let clearAll = false;

  for (const args of parseCfg(text)) {
    const verb = args[0].toLowerCase();
    if (verb === "unbindall") {
      clearAll = true;
      imported.clear();
    } else if ((verb === "bind" && args.length >= 3) || (verb === "unbind" && args.length >= 2)) {
      const key = normalizeKey(args[1]);
      if (!key) {
        stats.skippedKey += 1;
        continue;
      }
      imported.set(key, verb === "bind" ? cleanCommand(args.slice(2).join(" ")) : "");
    } else if (verb === "alias" && args.length >= 2) {
      fileAliases.add(args[1].toLowerCase());
    }
  }

  const commandOf = (key) => (current[key] ? current[key].command : fileBinds[key] ? fileBinds[key].command : "");
  const baseOf = (key) => (current[key] ? current[key].base : fileBinds[key] ? fileBinds[key].command : "");
  const usesFileAlias = (command) =>
    command.split(";").some((part) => {
      const name = part.trim().split(/\s+/)[0].toLowerCase();
      return !!name && fileAliases.has(name) && !gameAliases.has(name);
    });

  const next = Object.assign({}, current);
  for (const [key, command] of imported) {
    stats.found += 1;
    if (commandError(command)) {
      stats.skippedInvalid += 1;
    } else if (command && usesFileAlias(command) && !knownCommand(command)) {
      stats.skippedAlias += 1;
    } else if (command === commandOf(key)) {
      stats.unchanged += 1;
    } else {
      next[key] = { command, base: baseOf(key) };
      if (command) stats.changed += 1;
      else stats.unbound += 1;
    }
  }
  if (clearAll) {
    const keys = new Set(Object.keys(fileBinds).concat(Object.keys(current)));
    for (const key of keys) {
      if (imported.has(key) || !commandOf(key)) continue;
      next[key] = { command: "", base: baseOf(key) };
      stats.unbound += 1;
    }
  }
  return Object.assign({ overrides: sanitizeOverrides(next) }, stats);
}

// Импорт готового конфига Half-Life 1 целиком: файлы ложатся в папку мода
// под своими именами и подключаются из userconfig.cfg перед остальными
// файлами лаунчера. Так работают и alias из него, и бинды на эти alias, и
// перебинды внутри скриптов; переназначения лаунчера всё равно главнее.
// IMPORT_FILE — так импорт подключали прошлые версии лаунчера; в
// предпросмотре ещё не сохранённого импорта это метка его биндов.
const IMPORT_FILE = "hlsr-import.cfg";
const IMPORT_EXEC_LINE = `exec ${IMPORT_FILE}`;
const MAX_IMPORT_BYTES = 256 * 1024;
// exec файлов, которые и так выполняет игра или лаунчер: из импорта они
// зациклили бы выполнение (config.cfg снова выполнит userconfig.cfg лаунчера,
// а тот — импорт) или выполнили бы файлы лаунчера раньше времени.
// userconfig.cfg и autoexec.cfg цепочка импорта (src/shared/cfg-import.js)
// просит как файлы игрока и пропускает их здесь (keepPlayerFiles).
const IMPORT_SKIPPED_EXEC = /^"?(?:\.\/)?(config|valve|game|language|joystick|hlsr-(?!import)[a-z-]+)\.(cfg|rc)"?$/i;
const PLAYER_FILES_EXEC = /^"?(?:\.\/)?(userconfig|autoexec)\.cfg"?$/i;

// Текст cfg, прочитанный побайтно (latin1): BOM UTF-8 убирается, UTF-16
// (Блокнот «Юникод») перекодируется. Символы вне latin1 после UTF-16
// становятся байтами UTF-8, как если бы файл был сохранён в UTF-8.
function cfgText(raw) {
  let text = String(raw || "");
  if (text.startsWith("\xef\xbb\xbf")) return text.slice(3);
  let little = null;
  if (text.startsWith("\xff\xfe")) { little = true; text = text.slice(2); }
  else if (text.startsWith("\xfe\xff")) { little = false; text = text.slice(2); }
  else if (text.length >= 4) {
    // Без BOM: в ASCII-тексте UTF-16 каждый второй байт нулевой.
    let even = 0, odd = 0;
    const n = Math.min(text.length, 4096) & ~1;
    for (let i = 0; i < n; i += 2) {
      if (text.charCodeAt(i) === 0) even += 1;
      if (text.charCodeAt(i + 1) === 0) odd += 1;
    }
    if (odd > n / 2 * 0.6 && even === 0) little = true;
    else if (even > n / 2 * 0.6 && odd === 0) little = false;
  }
  if (little === null) return text;
  let decoded = "";
  for (let i = 0; i + 1 < text.length; i += 2) {
    const lo = text.charCodeAt(little ? i : i + 1), hi = text.charCodeAt(little ? i + 1 : i);
    decoded += String.fromCharCode(lo | (hi << 8));
  }
  // eslint-disable-next-line no-control-regex
  if (!/[^\x00-\xff]/.test(decoded)) return decoded;
  let bytes = "";
  for (const byte of new TextEncoder().encode(decoded)) bytes += String.fromCharCode(byte);
  return bytes;
}

// Команды строки верхнего уровня с их исходным текстом: «;» и «//» внутри
// кавычек не делят строку. { parts: [текст команды], comment } .
function splitLine(line) {
  const parts = [];
  let start = 0, quoted = false, comment = "";
  for (let i = 0; i < line.length; i += 1) {
    const ch = line[i];
    if (ch === '"') quoted = !quoted;
    else if (!quoted && ch === ";") { parts.push(line.slice(start, i)); start = i + 1; }
    else if (!quoted && ch === "/" && line[i + 1] === "/") { comment = line.slice(i); line = line.slice(0, i); break; }
  }
  parts.push(line.slice(start));
  return { parts, comment };
}

// Переписывает команды верхнего уровня строки. fn(args, текст) возвращает
// undefined — оставить, строку — новый текст, { drop: причина } — убрать
// (команда уходит в комментарий «// HLSR: причина: команда» в конце строки).
// mapKept(текст) применяется к оставшимся командам.
function rewriteTopLevel(line, fn, mapKept = (part) => part) {
  const { parts, comment } = splitLine(line);
  let changed = false;
  const kept = [], dropped = [];
  for (const part of parts) {
    const args = parseCfg(part)[0];
    const next = args ? fn(args, part) : undefined;
    if (next && typeof next === "object") {
      changed = true;
      dropped.push((next.drop ? `${next.drop}: ` : "") + part.trim());
      continue;
    }
    const text = mapKept(next === undefined ? part : next);
    if (text !== part) changed = true;
    kept.push(text);
  }
  if (!changed) return line;
  const body = kept.filter((part) => part.trim()).join(";");
  const note = dropped.length ? `// HLSR: ${dropped.join("; ")}` : "";
  return [body, note, comment].filter(Boolean).join(" ");
}

// Текст файла для записи: exec файлов игры и лаунчера на верхнем уровне
// убраны в комментарий (остальные команды строки остаются), NUL и прочие
// управляющие символы (кроме табуляции и переводов строк) убраны.
function importFileText(text, { keepPlayerFiles = false } = {}) {
  const skipped = (name) => IMPORT_SKIPPED_EXEC.test(name) || (!keepPlayerFiles && PLAYER_FILES_EXEC.test(name));
  return cfgText(text)
    // eslint-disable-next-line no-control-regex
    .replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/g, "")
    .split("\n")
    .map((line) => {
      const cr = line.endsWith("\r") ? "\r" : "";
      const body = cr ? line.slice(0, -1) : line;
      return rewriteTopLevel(body, (args) =>
        args[0].toLowerCase() === "exec" && args.length >= 2 && skipped(args[1]) ? { drop: "" } : undefined) + cr;
    })
    .join("\n");
}

// Бинд из подключённого импорта: из его файлов (imported — пути из
// keyBinds.imported) или из hlsr-import*.cfg прошлых версий лаунчера.
function isImportSource(source, imported) {
  const lower = String(source || "").replace(/\\/g, "/").toLowerCase();
  if (!lower) return false;
  if (/^hlsr-import(-[a-z0-9_-]+)?\.cfg$/.test(lower)) return true;
  return Array.isArray(imported) && imported.includes(lower);
}

// Что увидит игра после импорта: бинды, alias и cvar из readBinds() плюс
// команды файла поверх них (файл выполняется последним из cfg игрока).
function withImport(read, text) {
  const binds = Object.assign(Object.create(null), (read && read.binds) || {});
  const aliases = Object.assign(Object.create(null), (read && read.aliases) || {});
  const cvars = Object.assign(Object.create(null), (read && read.cvars) || {});
  // Бинды прошлого импорта уходят: новый импорт его заменяет.
  const fromImport = (source) => isImportSource(source, read && read.imported);
  for (const key of Object.keys(binds)) if (fromImport(binds[key].source)) delete binds[key];
  for (const args of parseCfg(importFileText(text))) {
    const verb = args[0].toLowerCase();
    if (verb === "unbindall") {
      for (const key of Object.keys(binds)) delete binds[key];
    } else if (verb === "bind" && args.length >= 3) {
      const key = normalizeKey(args[1]);
      if (key) binds[key] = { command: cleanCommand(args.slice(2).join(" ")), source: IMPORT_FILE };
    } else if (verb === "unbind" && args.length >= 2) {
      const key = normalizeKey(args[1]);
      if (key) delete binds[key];
    } else if (verb === "alias" && args.length >= 2) {
      aliases[args[1]] = args.slice(2).join(" ");
    } else if (READ_CVARS.has(verb) && args.length >= 2) {
      cvars[verb] = args[1];
    }
  }
  const files = ((read && read.files) || []).filter((name) => !fromImport(name));
  files.push(IMPORT_FILE);
  // Файлы импорта, которые зовут клавиши, ещё не на диске: что они назначат — видно после
  // «Сохранить». С диска остаются только бинды по нажатию клавиш, которые импорт не тронул.
  const runtime = Object.create(null);
  for (const [key, list] of Object.entries((read && read.runtime) || {})) {
    const kept = list.filter((item) => {
      const before = read.binds && read.binds[item.trigger];
      return !!before && !!binds[item.trigger] && binds[item.trigger].command === before.command;
    });
    if (kept.length) runtime[key] = kept;
  }
  return { binds, aliases, cvars, files, runtime };
}

// Клавиши, которые импорт назначает или снимает сам (bind и unbind верхнего
// уровня). unbindall сюда не входит: он в начале каждого config.cfg и не
// значит, что игрок отказывается от переназначений лаунчера.
function importTouchedKeys(text) {
  const keys = new Set();
  for (const args of parseCfg(text)) {
    const verb = args[0].toLowerCase();
    if ((verb === "bind" && args.length >= 3) || (verb === "unbind" && args.length >= 2)) {
      const key = normalizeKey(args[1]);
      if (key) keys.add(key);
    }
  }
  return keys;
}

// Отдельные команды бинда, в том числе перебинды внутри него:
// «save x; bind mouse4 +tau; say TAO» → ["save x", "bind mouse4 +tau",
// "say TAO", "+tau"]. По ним вкладка «Скрипты» находит скрипты в биндах
// импортированного конфига.
function commandsOf(command) {
  const result = [];
  for (const args of parseCfg(String(command || ""))) {
    result.push(args.join(" "));
    if (args[0].toLowerCase() === "bind" && args.length >= 3) result.push(...commandsOf(args.slice(2).join(" ")));
  }
  return result;
}

// Alias по имени без учёта регистра: так их ищет движок.
function aliasMap(aliases) {
  const map = new Map();
  for (const [name, value] of Object.entries(aliases || {})) map.set(name.toLowerCase(), String(value || ""));
  return map;
}

// Alias, которые переопределяются внутри других alias («alias sst sst2»):
// это шаги и переключатели скриптов, их значение в cfg — только начальное.
function reassignedAliases(map) {
  const names = new Set();
  for (const value of map.values()) {
    for (const args of parseCfg(value)) {
      if (args[0].toLowerCase() === "alias" && args.length >= 2) names.add(args[1].toLowerCase());
    }
  }
  return names;
}

const EXPAND_DEPTH = 16;
const EXPAND_LIMIT = 400;

// Всё, что может выполнить бинд: commandsOf, а ещё тела alias, которые он
// вызывает или назначает («alias re4 re42»), и парный «-x» для «+x». Так
// вкладка «Скрипты» находит скрипт, назначенный цепочкой alias: «re4» →
// «re41» → «alias re4 re42» → «bind f obbo1000».
function expandCommand(command, aliases) {
  const map = aliases instanceof Map ? aliases : aliasMap(aliases);
  const result = [];
  const visited = new Set();
  const walk = (text, depth) => {
    if (depth > EXPAND_DEPTH) return;
    for (const args of parseCfg(text)) {
      if (result.length >= EXPAND_LIMIT) return;
      result.push(args.join(" "));
      const verb = args[0].toLowerCase();
      if ((verb === "bind" || verb === "alias") && args.length >= 3) {
        walk(args.slice(2).join(" "), depth + 1);
        continue;
      }
      const names = verb.startsWith("+") ? [verb, `-${verb.slice(1)}`] : [verb];
      for (const name of names) {
        if (!map.has(name) || visited.has(name)) continue;
        visited.add(name);
        walk(map.get(name), depth + 1);
      }
    }
  };
  walk(String(command || ""), 0);
  return result;
}

module.exports = {
  BINDS_FILE,
  IMPORT_FILE,
  IMPORT_EXEC_LINE,
  MAX_IMPORT_BYTES,
  cfgText,
  splitLine,
  rewriteTopLevel,
  importFileText,
  isImportSource,
  importTouchedKeys,
  withImport,
  commandsOf,
  aliasMap,
  reassignedAliases,
  expandCommand,
  BINDS_EXEC_LINE,
  MAX_COMMAND,
  GOLDSRC_KEYS,
  SOURCE_KEYS,
  ENGINES,
  engineOf,
  normalizeKey,
  commandError,
  cleanCommand,
  parseCfg,
  readBinds,
  runtimeBinds,
  sanitizeOverrides,
  pruneOverrides,
  resetOverrides,
  toCfgLines,
  importOverrides,
};
