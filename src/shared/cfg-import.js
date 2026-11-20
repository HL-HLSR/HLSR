"use strict";

// Импорт конфига с цепочкой exec (вкладка «Клавиатура»).
//
// Конфиг игрока часто вызывает другие cfg: сразу (exec на верхнем уровне) или
// по клавише (exec внутри alias и bind). Лаунчер просит выбрать каждый такой
// файл и подключает их вместе с конфигом. В HL1 файлы ложатся в папку мода
// под теми же именами и по тем же путям, что в exec: так конфиг работает как
// у игрока, без переименований. Файлы, которые импорт заменил, лаунчер
// сохраняет, и «Убрать конфиг» возвращает их (game-binds-install.js).
//
// Проверки: файл, который выполняется сам из себя (цикл exec), игра крутила
// бы бесконечно — такой exec закомментирован. Один и тот же файл, выбранный
// под двумя именами, в отчёте. Бинды, alias и переменные, которые цепочка
// задаёт несколько раз по-разному, попадают в отчёт.

const gameBinds = require("./game-binds");
const gameConfig = require("./game-config");

const MAX_FILES = 32;
const MAX_FLAT_COMMANDS = 20000;
// Файлы, которые выполняет сама игра или лаунчер: их не спрашиваем.
// hlsr-import*.cfg — импорт прошлых версий лаунчера, его тоже спрашиваем.
const SYSTEM_NAMES = /^(config|valve|game|language|joystick|skill|listenserver|server)\.(cfg|rc)$|^hlsr-(?!import)/;
// cfg, которые движок выполняет сам: импортированный под этим именем файл
// не подключается строкой exec, он и так выполнится.
const ENGINE_RUN = /^(config|userconfig|autoexec)\.cfg$/i;
// Имя файла импорта в папке мода: как в exec, если оно безопасно (до двух
// подпапок, без «..»), и это не файл лаунчера или движка.
const SAFE_NAME = /^[a-z0-9_][a-z0-9_. -]*(\/[a-z0-9_][a-z0-9_. -]*){0,2}\.(cfg|rc)$/i;
const RESERVED_NAME = /^(hlsr-.*|(valve|game|language|joystick|skill|listenserver|server)\.(cfg|rc))$/i;
// Файлы игрока, которые движок выполняет сам: config.cfg последней строкой
// вызывает userconfig.cfg, valve.rc — autoexec.cfg. Их импорт просит как
// обычные файлы цепочки, а невыбранные на верхнем уровне не выполняет:
// иначе выполнился бы userconfig.cfg лаунчера, а он снова вызывает импорт.
const PLAYER_AUTO = /^(userconfig|autoexec)\.cfg$/;

// Имя файла из аргумента exec, как его найдёт движок: ключ (без регистра,
// прямые слэши) и имя для показа. null — движок такой exec не выполнит.
function execTarget(arg, engine) {
  let name = String(arg || "").trim().replace(/\\/g, "/").replace(/^\/+/, "");
  if (!name || name.includes("..") || name.includes(":")) return null;
  if (!/\.(cfg|rc)$/i.test(name)) {
    // GoldSrc выполняет только .cfg и .rc, Source сам дописывает .cfg.
    if (engine === "goldsrc") return null;
    name += ".cfg";
  }
  const key = name.toLowerCase();
  const base = key.split("/").pop();
  return { key, name, system: SYSTEM_NAMES.test(base), auto: PLAYER_AUTO.test(base) };
}

// exec в командах: верхний уровень и внутри значений alias и bind.
function execsOf(text, engine) {
  const result = [];
  const visit = (args, runtime, depth) => {
    if (!args.length || depth > 4) return;
    const verb = args[0].toLowerCase();
    if (verb === "exec" && args.length >= 2) {
      const target = execTarget(args[1], engine);
      if (target) result.push(Object.assign({ runtime }, target));
    } else if ((verb === "alias" || verb === "bind") && args.length >= 3) {
      for (const inner of gameBinds.parseCfg(args.slice(2).join(" "))) visit(inner, true, depth + 1);
    }
  };
  for (const args of gameBinds.parseCfg(text)) visit(args, false, 0);
  return result;
}

function rootKey(root, engine) {
  const target = execTarget(root && root.name, engine);
  return target ? target.key : "";
}

// Следующий файл цепочки, который нужно выбрать: { key, name, from } или null.
// files — уже выбранные { key: { name, text } }, missing — ключи, от которых
// игрок отказался.
// topLevelOnly — только exec, которые выполняются сразу, без alias и bind.
function nextWanted({ engine, root, files = {}, missing = [], topLevelOnly = false }) {
  const skip = new Set(missing.map((key) => String(key).toLowerCase()));
  skip.add(rootKey(root, engine));
  const queue = [{ name: root.name, text: root.text }];
  const seen = new Set();
  while (queue.length) {
    const file = queue.shift();
    for (const target of execsOf(file.text, engine)) {
      if (seen.has(target.key)) continue;
      // Выбранный игроком файл (несколько файлов сразу, rootOfPicked)
      // выполняется, даже если зовётся как файл игры (config.cfg).
      if (files[target.key]) {
        seen.add(target.key);
        queue.push(files[target.key]);
        continue;
      }
      if (target.system || (topLevelOnly && target.runtime)) continue;
      seen.add(target.key);
      if (!skip.has(target.key)) return { key: target.key, name: target.name, from: file.name };
    }
  }
  return null;
}

function isSafeImportName(name) {
  const value = String(name || "");
  return SAFE_NAME.test(value) && !value.split("/").some((part) => /^\.+$/.test(part.trim())) &&
    !RESERVED_NAME.test(value.split("/").pop());
}

// Имя в папке мода: как у игрока, а небезопасное — очищенное. Повтор
// имени (два config.cfg из разных папок) получает номер.
function outputName(name, used) {
  let candidate = String(name || "").trim().replace(/\\/g, "/").replace(/^\/+/, "");
  if (!isSafeImportName(candidate)) {
    // Импорт прошлых версий (hlsr-import-jump.cfg) возвращает себе имя игрока.
    const stem = candidate.split("/").pop().toLowerCase().replace(/\.(cfg|rc)$/, "")
      .replace(/^hlsr-import(-|$)/, "").replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "") || "player";
    candidate = `${stem}.cfg`;
    if (!isSafeImportName(candidate)) candidate = `cfg-${stem}.cfg`;
  }
  const dot = candidate.lastIndexOf(".");
  let result = candidate;
  for (let n = 2; used.has(result.toLowerCase()); n += 1) result = `${candidate.slice(0, dot)}-${n}${candidate.slice(dot)}`;
  used.add(result.toLowerCase());
  return result;
}

const quote = (token) => `"${token}"`;

// Собирает импорт: файлы для папки игры, строки exec для userconfig.cfg,
// плоский текст в порядке выполнения (для предпросмотра и импорта биндов) и
// отчёт о проверках. root.virtual — несколько выбранных файлов: самого
// корня на диске нет, его exec идут в userconfig.cfg.
function buildImport({ engine, root, files = {}, missing = [] }) {
  const sources = new Map();
  const used = new Set();
  const byText = new Map();
  const duplicateFiles = [];
  const keep = { keepPlayerFiles: true };
  const rootEntry = {
    key: root.virtual ? "" : rootKey(root, engine),
    name: root.name,
    // exec выбранных файлов, даже config.cfg: их строит rootOfPicked.
    text: root.virtual ? String(root.text) : gameBinds.importFileText(root.text, keep),
    virtual: !!root.virtual,
  };
  rootEntry.output = rootEntry.virtual ? null : outputName(root.name, used);
  if (!rootEntry.virtual) byText.set(rootEntry.text, rootEntry);
  for (const [key, file] of Object.entries(files)) {
    if (!file || typeof file.text !== "string") continue;
    const lower = key.toLowerCase();
    const text = gameBinds.importFileText(file.text, keep);
    const same = byText.get(text);
    if (same) duplicateFiles.push([same.name, file.name || key]);
    const entry = { key: lower, name: file.name || key, text, output: outputName(file.target || key, used) };
    if (!same) byText.set(text, entry);
    sources.set(lower, entry);
  }
  if (rootEntry.key && !sources.has(rootEntry.key)) sources.set(rootEntry.key, rootEntry);
  const childOf = (target) => (target && (!target.system || files[target.key]) ? sources.get(target.key) : null);

  // Порядок выполнения: exec верхнего уровня раскрывается на месте. Exec на
  // файл, который уже выполняется выше по цепочке, — цикл.
  const flat = [];
  const cycles = [];
  const cut = new Set();
  const idOf = (entry) => entry.output || "";
  const run = (entry, stack) => {
    for (const args of gameBinds.parseCfg(entry.text)) {
      if (flat.length >= MAX_FLAT_COMMANDS) return;
      if (args[0].toLowerCase() === "exec" && args.length >= 2) {
        const target = execTarget(args[1], engine);
        const child = childOf(target);
        if (child && stack.includes(child)) {
          const id = `${idOf(entry)}\n${target.key}`;
          if (!cut.has(id)) cycles.push([entry.name, child.name]);
          cut.add(id);
          continue;
        }
        if (child) {
          run(child, stack.concat(child));
          continue;
        }
      }
      flat.push({ args, file: entry.name });
    }
  };
  run(rootEntry, [rootEntry]);

  // Тексты для папки игры: exec остаются как у игрока, меняется только имя
  // файла, которое пришлось очистить. exec цикла и невыбранного файла на
  // верхнем уровне закомментированы.
  const skipped = new Set(missing.map((key) => String(key).toLowerCase()));
  const renamed = (text) => text.replace(/(\bexec\s+)("?)([^\s";]+)\2/gi, (match, verb, q, arg) => {
    const target = execTarget(arg, engine);
    const child = childOf(target);
    return child && child.output.toLowerCase() !== target.key ? `${verb}${q}${child.output}${q}` : match;
  });
  const rewrite = (entry) => entry.text.split("\n").map((line) => gameBinds.rewriteTopLevel(line, (args) => {
    if (args[0].toLowerCase() !== "exec" || args.length < 2) return undefined;
    const target = execTarget(args[1], engine);
    if (!target || (target.system && !files[target.key])) return undefined;
    if (cut.has(`${idOf(entry)}\n${target.key}`)) return { drop: "exec loop" };
    // userconfig.cfg и autoexec.cfg без файла игрока выполнили бы файлы
    // лаунчера, а те снова вызывают импорт.
    if (!sources.has(target.key) && (skipped.has(target.key) || target.auto)) return { drop: "not imported" };
    return undefined;
  }, renamed)).join("\n");

  const outputs = [];
  if (!rootEntry.virtual) outputs.push({ file: rootEntry.output, text: rewrite(rootEntry), root: true });
  for (const entry of new Set(sources.values())) if (entry !== rootEntry) outputs.push({ file: entry.output, text: rewrite(entry) });
  // Что подключить из userconfig.cfg: корень или выбранные файлы по порядку.
  // Файлы, которые движок выполняет сам, строки не получают.
  const execs = [];
  const execLines = rootEntry.virtual ? rootEntry.text : `exec "${rootEntry.output}"`;
  for (const args of gameBinds.parseCfg(execLines)) {
    if (args[0].toLowerCase() !== "exec" || args.length < 2) continue;
    const target = execTarget(args[1], engine);
    const entry = rootEntry.virtual ? childOf(target) : rootEntry;
    if (entry && !ENGINE_RUN.test(entry.output) && !execs.includes(entry.output)) execs.push(entry.output);
  }

  return {
    outputs,
    execs,
    flatText: flat.map((item) => item.args.map(quote).join(" ")).join("\n"),
    bytes: outputs.reduce((sum, item) => sum + item.text.length, 0),
    report: {
      files: outputs.filter((item) => !item.root).map((item) => item.file),
      missing: [...skipped],
      cycles,
      duplicateFiles,
      duplicates: duplicatesOf(flat),
    },
  };
}

// Строка, в которой только exec одного файла (на верхнем уровне): его цель или null.
function singleExec(line, engine) {
  const commands = gameConfig.lineCommands(line);
  if (commands.length !== 1 || commands[0].args.length !== 2) return null;
  const [verb, arg] = commands[0].args;
  return verb.value.toLowerCase() === "exec" ? execTarget(arg.value, engine) : null;
}

// Сколько чего задаёт текст: для ответа импорта. unknown — команды без значения
// (кроме unbindall и exec): скрипты вида +jump, echo, жесты.
function summaryOf(text) {
  const summary = { binds: 0, cvars: 0, aliases: 0, unknown: 0 };
  for (const line of String(text).split("\n")) {
    for (const { args } of gameConfig.lineCommands(line.replace(/\r$/, ""))) {
      const verb = args[0].value.toLowerCase();
      if (verb === "bind" && args.length >= 3) summary.binds += 1;
      else if (verb === "alias" && args.length >= 2) summary.aliases += 1;
      else if (verb === "exec" || verb === "unbindall" || verb === "unbind") continue;
      else if (args.length >= 2) summary.cvars += 1;
      else summary.unknown += 1;
    }
  }
  return summary;
}

// Импорт одним файлом (§8.1): корень, где каждый exec верхнего уровня заменён
// текстом своего файла на том же месте, в порядке выполнения. Сам файл в папке
// мода после этого не нужен. Файлы, которые зовут только alias и bind (во время
// игры), остаются файлами: runtimeFiles под их именами из exec. Проверки те же,
// что у buildImport: цикл exec обрывается комментарием, exec файла игрока
// (userconfig.cfg, autoexec.cfg) и отказанных файлов закомментирован, а exec
// лаунчера убирает importFileText; отчёт — report у buildImport.
// Исходная кодировка строк сохраняется, переводы строк — "\n".
// Имя файла в строке-комментарии, которую пишет лаунчер: только ASCII и без «;».
const asciiName = (name) => String(name).replace(/[^\x20-\x7e]|;/g, "_");

function flattenImport({ engine, root, files = {}, missing = [] }) {
  const keep = { keepPlayerFiles: true };
  const skipped = new Set(missing.map((key) => String(key).toLowerCase()));
  const rootEntry = root.virtual ? null : execTarget(root.name, engine);
  const rootKeyValue = rootEntry ? rootEntry.key : "";
  const clean = (text) => gameBinds.importFileText(text, keep);
  const sourceOf = (key) => {
    const file = files[key];
    if (file && typeof file.text === "string") return file;
    return key && key === rootKeyValue ? root : null;
  };
  const expand = (text, stack) => {
    const out = [];
    for (const raw of text.split("\n")) {
      const line = raw.replace(/\r$/, "");
      const target = singleExec(line, engine);
      const child = target && files[target.key] && typeof files[target.key].text === "string" ? files[target.key] : null;
      if (!target) {
        out.push(line);
      } else if (child && stack.includes(target.key)) {
        out.push(`// HLSR: exec ${asciiName(target.name)} skipped (cycle)`);
      } else if (child) {
        out.push(`// ===== from ${asciiName(target.name)} =====`, ...expand(clean(child.text), stack.concat(target.key)));
      } else if (target.key === rootKeyValue && stack.includes(rootKeyValue)) {
        out.push(`// HLSR: exec ${asciiName(target.name)} skipped (cycle)`);
      } else if (target.auto || skipped.has(target.key)) {
        out.push(`// HLSR: exec ${asciiName(target.name)} skipped (not imported)`);
      } else {
        out.push(line);
      }
    }
    return out;
  };

  const rootText = root.virtual ? String(root.text) : clean(root.text);
  const text = expand(rootText, rootKeyValue ? [rootKeyValue] : []).join("\n");

  const runtimeFiles = [];
  const unsafe = [];
  const seen = new Set();
  const queue = [text];
  while (queue.length) {
    for (const target of execsOf(queue.pop(), engine)) {
      const source = target.runtime && !seen.has(target.key) ? sourceOf(target.key) : null;
      if (!source || ENGINE_RUN.test(target.key.split("/").pop())) continue;
      seen.add(target.key);
      if (!isSafeImportName(target.name)) {
        unsafe.push(target.name);
        continue;
      }
      const body = expand(clean(source.text), [target.key]).join("\n");
      runtimeFiles.push({ file: target.name, text: body });
      queue.push(body);
    }
  }

  // Отчёт — как у buildImport (цепочка, цикл, повторы); files — что вошло в итог.
  const built = buildImport({ engine, root, files, missing });
  const inlined = Object.values(files).filter((file) => file && typeof file.text === "string")
    .map((file) => file.name || "");
  const report = Object.assign({}, built.report, { files: inlined.filter(Boolean), unsafe });
  const bytes = [text].concat(runtimeFiles.map((item) => item.text))
    .reduce((sum, item) => sum + Buffer.byteLength(item, "latin1"), 0);
  return { text, runtimeFiles, report, bytes, summary: Object.assign(summaryOf(text), { files: runtimeFiles.length + inlined.length }) };
}

// Что цепочка задаёт несколько раз с разными значениями: в игре действует
// последнее. unbindall сбрасывает историю биндов.
function duplicatesOf(flat) {
  const seen = new Map();
  const note = (kind, name, value, file) => {
    const id = `${kind}\n${name}`;
    const list = seen.get(id) || [];
    list.push({ value, file });
    seen.set(id, list);
  };
  for (const { args, file } of flat) {
    const verb = args[0].toLowerCase();
    if (verb === "unbindall") {
      for (const id of [...seen.keys()]) if (id.startsWith("bind\n")) seen.delete(id);
    } else if (verb === "bind" && args.length >= 3) {
      const key = gameBinds.normalizeKey(args[1]);
      if (key) note("bind", key, gameBinds.cleanCommand(args.slice(2).join(" ")), file);
    } else if (verb === "alias" && args.length >= 2) {
      note("alias", args[1].toLowerCase(), args.slice(2).join(" "), file);
    } else if (args.length === 2 && /^[a-z_][a-z0-9_]*$/i.test(verb) && verb !== "exec" && verb !== "unbind") {
      note("cvar", verb, args[1], file);
    }
  }
  const result = [];
  for (const [id, list] of seen) {
    if (list.length < 2 || new Set(list.map((item) => item.value)).size < 2) continue;
    const [kind, name] = id.split("\n");
    result.push({ kind, name, files: list.map((item) => item.file), value: list[list.length - 1].value });
  }
  return result;
}

// Файлы игрока, которые движок выполнил бы сам рядом с выбранными: к
// config.cfg — autoexec.cfg (valve.rc выполняет его раньше config.cfg).
// userconfig.cfg отдельно не нужен: его вызывает последняя строка
// config.cfg, и цепочка спросит его сама. [{ name, before }]
function companionsOf(names) {
  const lower = new Set(names.map((name) => String(name).toLowerCase()));
  if (!lower.has("config.cfg") || lower.has("autoexec.cfg")) return [];
  return [{ name: "autoexec.cfg", from: "config.cfg", before: true }];
}

// Несколько выбранных файлов — один импорт: userconfig.cfg выполняет их в
// порядке выбора, каждый под своим именем. picked — [{ name, text }]. Второй
// файл с тем же именем (два config.cfg из разных папок) получает номер.
function rootOfPicked(picked) {
  if (picked.length === 1) return { root: { name: picked[0].name, text: picked[0].text }, files: {} };
  const files = {};
  const lines = [];
  const used = new Set();
  for (const file of picked) {
    const name = outputName(file.name, used);
    files[name.toLowerCase()] = { name: file.name, target: name, text: file.text };
    lines.push(`exec "${name}"`);
  }
  return { root: { name: picked.map((file) => file.name).join(", "), text: lines.join("\n"), virtual: true }, files };
}

module.exports = {
  MAX_FILES,
  ENGINE_RUN,
  execTarget,
  execsOf,
  nextWanted,
  buildImport,
  flattenImport,
  rootOfPicked,
  companionsOf,
  isSafeImportName,
};
