"use strict";

// Проверка конфига HL1 по таблице разрешённых команд speedrun.com
// (src/shared/hl1-whitelist.json, собирается scripts/build-hl1-whitelist.js).
//
// Чистая функция: тексты файлов приходят снаружи. Проход как у движка:
// команды корневого файла по порядку, exec уходит в файл сразу. Плюс всё,
// что может выполниться позже: тела alias, команды биндов, bind внутри bind
// и alias, exec внутри них (эти файлы проверяются тоже).
//
// Уровни: error — запрещено или нет в таблице; warning — зависит от
// категории или значение не то; info — только для старта рана и файлы,
// которых нет.

const { lineCommands } = require("./game-config");
const { parseCfg } = require("./game-binds");
const whitelist = require("./hl1-whitelist.json");

const RUN_START_ANCHOR = "map c1a0";
const WARN_RULES = new Set(["scripted", "category", "thirdperson", "steam"]);

const RULE_TEXT = {
  scripted: "только в scripted-категориях",
  category: "зависит от категории",
  thirdperson: "только в категории с thirdperson",
  steam: "только в Steam-версии",
};

const lower = (value) => String(value).toLowerCase();

// Имя файла в exec как его ищет движок: без каталогов-обратных слэшей,
// в нижнем регистре, расширение .cfg по умолчанию.
function execName(raw) {
  let name = lower(raw).replace(/\\/g, "/").replace(/^\.\//, "");
  if (!/\.[a-z0-9]+$/.test(name)) name += ".cfg";
  return name;
}

function sameValue(a, b) {
  const x = Number(a);
  const y = Number(b);
  return Number.isNaN(x) || Number.isNaN(y) || a === "" || b === "" ? lower(a) === lower(b) : x === y;
}

// Запись таблицы для имени: точное имя команды ("map c1a0" раньше "map"),
// cvar, команда, потом самый длинный префикс.
function lookup(args) {
  const verb = lower(args[0]);
  if (args.length > 1) {
    const withArg = `${verb} ${lower(args[1])}`;
    if (whitelist.commands[withArg]) return { entry: whitelist.commands[withArg], key: withArg, isCvar: false };
  }
  if (whitelist.cvars[verb]) return { entry: whitelist.cvars[verb], key: verb, isCvar: true };
  if (whitelist.commands[verb]) return { entry: whitelist.commands[verb], key: verb, isCvar: false };
  let best = null;
  for (const pattern of whitelist.patterns) {
    if (verb.startsWith(pattern.prefix) && (!best || pattern.prefix.length > best.prefix.length)) best = pattern;
  }
  return best ? { entry: best, key: best.prefix, isCvar: best.kind === "cvars" } : null;
}

function checkConfig({ files = {}, root, category = "scriptless" }) {
  category = category === "scripted" ? "scripted" : "scriptless";
  const findings = [];
  const aliases = new Set();
  const stats = { category, files: 0, commands: 0, errors: 0, warnings: 0, infos: 0 };
  let collecting = true;
  let visited = new Set();

  const isOwnName = (name) => {
    const base = name.replace(/^[+-]/, "");
    return aliases.has(name) || aliases.has(base) || aliases.has(`+${base}`) || aliases.has(`-${base}`);
  };

  const report = (site, args, level, rule, note) => {
    if (collecting) return;
    findings.push({
      file: site.file,
      line: site.line,
      command: args.join(" "),
      level,
      rule,
      note,
      context: site.path.length ? site.path.join(" > ") : "top level",
      // То же структурой для окна: [{ kind: "alias"|"bind", name }, { kind: "exec", via: [...] }]; текст строит окно.
      trail: site.trail.slice(),
    });
    stats[level === "error" ? "errors" : level === "warning" ? "warnings" : "infos"] += 1;
  };

  // runStartSafe — команда в теле alias/bind, где есть и map c1a0.
  const check = (args, site, sequence) => {
    if (!collecting) stats.commands += 1;
    const verb = lower(args[0]);
    const value = args.length > 1 ? args[1] : undefined;

    if (verb.startsWith("sk_")) {
      if (!(verb in whitelist.skill)) report(site, args, "error", "unknown", "нет в таблице разрешённых команд (раздел HL1 вкладки Skill)");
      else if (value !== undefined && !sameValue(value, whitelist.skill[verb])) {
        report(site, args, "warning", "skill", `значение должно быть по умолчанию: ${whitelist.skill[verb]}`);
      }
      return;
    }

    const found = lookup(args);
    if (!found) {
      if (!isOwnName(verb)) report(site, args, "error", "unknown", "нет в таблице разрешённых команд");
      return;
    }
    const { entry } = found;
    const note = entry.note || "";
    if (entry.rule === "banned") {
      // Запрещённая переменная со значением по умолчанию таблицы не нарушение.
      if (found.isCvar && entry.default && value !== undefined && sameValue(value, entry.default)) return;
      report(site, args, "error", "banned", note || "запрещено правилами");
    } else if (entry.rule === "scripted-only") {
      if (category !== "scripted") report(site, args, "error", "scripted-only", "В Scriptless нельзя (только Scripted)");
    } else if (entry.rule === "no-script-seq") {
      // Одиночная команда в теле alias/bind и верхний уровень не флагуются: запрещена только связка из нескольких команд.
      if (site.body && sequence.length >= 2) report(site, args, "error", "no-script-seq", "Нельзя в последовательности команд (alias, bind)");
    } else if (WARN_RULES.has(entry.rule)) report(site, args, "warning", entry.rule, note || RULE_TEXT[entry.rule]);
    else if (entry.rule === "values") {
      // Как у banned: значение по умолчанию таблицы (например, bxt_hud_viewangles 0 — выключено) не нарушение.
      const isDefault = found.isCvar && entry.default && value !== undefined && sameValue(value, entry.default);
      if (value !== undefined && !isDefault && !entry.values.some((v) => sameValue(value, v))) {
        report(site, args, "warning", "values", `допустимые значения: ${entry.values.join(", ")}`);
      }
    } else if (entry.rule === "run-start") {
      // Старт рана: bind/alias, где есть map c1a0, — так и задумано.
      const nested = !!site.body;
      const isAnchor = found.key === RUN_START_ANCHOR;
      const withAnchor = sequence.some((other) => other.length && `${lower(other[0])} ${lower(other[1] || "")}` === RUN_START_ANCHOR);
      if (!(nested && (isAnchor || withAnchor))) report(site, args, "info", "run-start", note);
    }
  };

  const runSequence = (sequence, site) => {
    for (const args of sequence) {
      const verb = lower(args[0]);
      if (verb === "alias" && args.length >= 2) {
        aliases.add(lower(args[1]));
        const body = args.slice(2).join(" ");
        if (body) runSequence(parseCfg(body), Object.assign({}, site, { body: true, path: site.path.concat(`alias ${args[1]}`), trail: site.trail.concat({ kind: "alias", name: args[1] }) }));
      } else if (verb === "bind" && args.length >= 3) {
        runSequence(parseCfg(args.slice(2).join(" ")), Object.assign({}, site, { body: true, path: site.path.concat(`bind ${args[1]}`), trail: site.trail.concat({ kind: "bind", name: args[1] }) }));
      } else if (verb === "exec" && args.length >= 2) {
        runFile(args[1], site, args);
      }
      check(args, site, sequence);
    }
  };

  const runFile = (rawName, from, execArgs) => {
    const name = execName(rawName);
    if (visited.has(name)) return;
    visited.add(name);
    const text = files[name];
    if (text === undefined || text === null) {
      const site = from || { file: name, line: 0, path: [], trail: [] };
      report(site, execArgs || ["exec", rawName], "info", "missing", "файл не найден");
      return;
    }
    if (!collecting) stats.files += 1;
    // Файл из exec внутри bind/alias выполнится позже, но его команды —
    // верхний уровень этого файла.
    const via = from && from.path.length ? [`exec из ${from.path.join(" > ")}`] : [];
    const viaTrail = from && from.trail.length ? [{ kind: "exec", via: from.trail }] : [];
    String(text).split("\n").forEach((rawLine, index) => {
      const sequence = lineCommands(rawLine.replace(/\r$/, "")).map((command) => command.args.map((token) => token.value));
      if (sequence.length) runSequence(sequence, { file: name, line: index + 1, path: via.slice(), trail: viaTrail.slice(), body: false });
    });
  };

  // Первый проход собирает имена alias игрока, второй проверяет.
  const start = () => { visited = new Set(); runFile(root, null, null); };
  start();
  collecting = false;
  start();
  return { findings, stats };
}

module.exports = { checkConfig };
