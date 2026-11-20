#!/usr/bin/env node
"use strict";

// Импорт скриптов из гайда https://github.com/yavats/hl1-scripts для вкладки
// «Скрипты» Half-Life. Гайд читается на зафиксированном коммите, результат —
// src/shared/hl1-guide-data.json: код скриптов, клавиши из гайда, видео,
// авторы и английский текст. Переводы и подписи действий лежат отдельно, в
// src/shared/hl1-guide-text.js.
//
//   node scripts/import-hl1-guide.js            скачать README и обновить JSON
//   node scripts/import-hl1-guide.js README.md  взять README с диска
//   node scripts/import-hl1-guide.js --check    сверить JSON с гайдом
//
// Что импортёр меняет в коде скриптов и почему:
// - разные версии одного трюка объявляют alias с одинаковыми именами; такие
//   скрипты получают свой префикс (NAMESPACES), иначе назначенные вместе они
//   бы перезаписали друг друга;
// - скрипты, которые сами перебиндивают клавиши 1–3 на время трюка
//   (SELF_BINDING), в лаунчере стоят на выбранных игроком клавишах постоянно:
//   их «bind»-alias становятся пустыми, а клавиши — действиями;
// - «sensitivity 3» в alias, возвращающих чувствительность после трюка,
//   заменяется на {sensitivity}: лаунчер подставит чувствительность игрока;
// - «m_pitch 0.022» на выходе из трюка заменяется на {m_pitch}, иначе после
//   скрипта пропадает инверсия мыши игрока;
// - alias, которые после трюка возвращают FOV и прицел BXT значениями автора
//   гайда («deffov», «defcross»), вместо этого заново выполняют настройки
//   игры из лаунчера.

const fs = require("fs");
const path = require("path");
const https = require("https");

const REPO = "yavats/hl1-scripts";
const COMMIT = "64b83c58a31253de7b7caa75036f31f73a882423";
const OUT = path.join(__dirname, "..", "src", "shared", "hl1-guide-data.json");

// Файл настроек игры, который лаунчер пишет перед каждым запуском
// (goldsrc-personalization.js).
const RESTORE_PERSONALIZATION = "exec hlsr-personalization.cfg";

// Справочная таблица ожиданий: её не назначают на клавишу.
const SKIP = new Set(["wait-table"]);

const NAMESPACES = {
  "testchamber-b": "tcb",
  "testchamber-n": "tcn",
  "testchamber-d": "tcd",
  "testchamber-d-v2": "tcd2",
  "captured-v1": "ct1",
  "captured-v12": "ct12",
  "pipe2pipe-script-v3": "p2p3",
  "pipe2pipe-script-v4": "p2p4",
  "pipe2pipe-script-v4-2-key-edition": "p2p4k",
};

const SELF_BINDING = new Set([
  "pipe2pipe-script-v4",
  "pipe2pipe-script-v4-2-key-edition",
  "fully-automatic-xen-script",
]);

const MARKS = { "🟢": "green", "🟡": "yellow", "🟠": "orange", "🔴": "red", "🟣": "purple", "⚪": "white" };

function fetchText(url) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { headers: { "User-Agent": "hlsr-guide-import" } }, (res) => {
        if (res.statusCode !== 200) {
          reject(new Error(`${url}: HTTP ${res.statusCode}`));
          res.resume();
          return;
        }
        let body = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => (body += chunk));
        res.on("end", () => resolve(body));
      })
      .on("error", reject);
  });
}

// Как якоря GitHub: без знаков препинания, пробелы — дефисы. Тогда ссылки
// гайда вида [v2](#-elevator-reverse-script-v2) ведут на скрипт по id.
function slug(text) {
  return text
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .trim()
    .replace(/[\s-]+/g, "-");
}

// Заголовок без эмодзи и номера, отметка сложности — по цветному кружку.
function splitTitle(raw) {
  let text = raw.replace(/^\d+\.\s*/, "").trim();
  let mark = null;
  for (const [emoji, name] of Object.entries(MARKS)) {
    if (text.includes(emoji)) {
      mark = name;
      text = text.replace(emoji, "");
    }
  }
  text = text.replace(/[\p{Extended_Pictographic}⬛️]/gu, "").replace(/\s+/g, " ").trim();
  return { title: text, mark };
}

function plainCredit(raw) {
  return raw
    .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

function unquote(value) {
  const v = value.trim();
  return v.length >= 2 && v.startsWith('"') && v.endsWith('"') ? v.slice(1, -1) : v;
}

// Первый проход: заголовки, текст и блоки кода каждого скрипта.
function parseSections(markdown) {
  const lines = markdown.replace(/\r\n/g, "\n").split("\n");
  const scripts = [];
  let chapter = null;
  let chapterTitle = null;
  let section = null;
  let current = null;
  let fence = null;

  const flush = () => {
    if (current && current.blocks.some((b) => b.kind === "code")) scripts.push(current);
    current = null;
  };
  const start = (rawTitle) => {
    flush();
    const { title, mark } = splitTitle(rawTitle);
    current = { chapter, chapterTitle, section, title, mark, blocks: [] };
  };

  for (const raw of lines) {
    const line = raw.replace(/\s+$/, "");
    if (/^\s*```/.test(line)) {
      if (fence) {
        if (current) current.blocks.push({ kind: "code", lines: fence });
        fence = null;
      } else fence = [];
      continue;
    }
    if (fence) {
      fence.push(line);
      continue;
    }
    let m;
    if ((m = /^## (?:\*\*)?([A-Z])\. (?:\*\*)?(.+?)(?:\*\*)?$/.exec(line))) {
      flush();
      chapterTitle = m[2].replace(/\*/g, "").trim();
      chapter = slug(chapterTitle);
      section = null;
      continue;
    }
    if ((m = /^### (.+)$/.exec(line))) {
      section = splitTitle(m[1]).title;
      start(m[1]);
      continue;
    }
    if ((m = /^#### (.+)$/.exec(line))) {
      start(m[1]);
      continue;
    }
    if (current) current.blocks.push({ kind: "text", line });
  }
  flush();
  return scripts;
}

// Блок кода — код скрипта, если в нём есть alias или bind; иначе это
// последовательность нажатий из описания.
function isScriptCode(block) {
  return block.lines.some((l) => /^\s*(alias|bind)\s/i.test(l.replace(/\/\/.*$/, "")));
}

function parseCode(lines) {
  const aliases = [];
  const binds = [];
  const commands = [];
  for (let line of lines) {
    line = line.replace(/\/\/.*$/, "").trim();
    if (!line) continue;
    let m;
    // Имя alias и клавиша тоже бывают в кавычках: alias "faf_d" "...".
    if ((m = /^alias\s+(\S+)\s*(.*)$/i.exec(line))) aliases.push([unquote(m[1]), unquote(m[2])]);
    else if ((m = /^bind\s+(\S+)\s+(.*)$/i.exec(line))) binds.push([unquote(m[1]), unquote(m[2])]);
    else commands.push(line);
  }
  return { aliases, binds, commands };
}

// Второй проход: из блоков — код, действия, ссылки и текст.
function buildScript(raw) {
  const id = slug(raw.title);
  const text = [];
  const notes = [];
  const examples = [];
  let credit = null;
  let video = null;
  let save = null;
  let aliases = [];
  let binds = [];
  let commands = [];
  let seenCode = false;
  let inDetails = false;
  let detailsKind = null;

  for (const block of raw.blocks) {
    if (block.kind === "code") {
      if (isScriptCode(block)) {
        const code = parseCode(block.lines);
        aliases = aliases.concat(code.aliases);
        binds = binds.concat(code.binds);
        commands = commands.concat(code.commands);
        seenCode = true;
      } else {
        text.push("```", ...block.lines.map((l) => l.trim()), "```");
      }
      continue;
    }
    // Обратная косая черта в конце строки — перенос строки в markdown.
    const line = block.line.replace(/\\$/, "");
    let m;
    if (/<details>/.test(line)) {
      inDetails = true;
      detailsKind = null;
      continue;
    }
    if ((m = /<summary>(.*)<\/summary>/.exec(line))) {
      detailsKind = /usage/i.test(m[1]) ? "usage" : "script";
      continue;
    }
    if (/<\/details>/.test(line)) {
      inDetails = false;
      detailsKind = null;
      continue;
    }
    if ((m = /^\s*\*Made by (.+?)\*\s*$/i.exec(line))) {
      if (!credit) credit = plainCredit(m[1]);
      continue;
    }
    if (/\[Video link/.test(line)) {
      // «Video link (TODO)» ведёт на google.com — видео ещё нет.
      const videoMatch = /\[Video link[^\]]*\]\(https:\/\/youtu\.be\/([\w-]{11})(?:\?t=(\d+))?\)/.exec(line);
      if (videoMatch && !video) video = { id: videoMatch[1], start: videoMatch[2] ? Number(videoMatch[2]) : 0 };
      continue;
    }
    if ((m = /\[Practice save\]\((https:\/\/github\.com\/[^)\s]+)\)/.exec(line))) {
      save = m[1];
      continue;
    }
    if (/^Example:/.test(line)) {
      for (const ex of line.matchAll(/`bind key ("?)([^`]+?)\1`/g)) examples.push(unquote(ex[2]));
      continue;
    }
    if (!seenCode && !inDetails && /^- /.test(line)) {
      notes.push(line.slice(2).trim());
      continue;
    }
    if (detailsKind === "script") continue;
    const cleaned = line.replace(/^Usage:\s*/, "");
    text.push(cleaned);
  }

  // Пустые строки по краям и повторы пустых строк не нужны.
  const usage = text
    .join("\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();

  return {
    id,
    chapter: raw.chapter,
    chapterTitle: raw.chapterTitle,
    section: raw.section,
    title: raw.title,
    mark: raw.mark,
    credit,
    notes,
    usage,
    video,
    save,
    aliases,
    binds,
    commands,
    examples,
  };
}

// Глобальный хук гайда: его вызывает game.cfg, имя переименовывать нельзя.
const SHARED_NAMES = new Set(["auto_alias"]);

// Имена alias скрипта: объявленные в конфиге и те, что скрипт задаёт на ходу
// («alias xenv xenv1» внутри другого alias).
function definedNames(script) {
  const names = new Set(script.aliases.map(([name]) => name.toLowerCase()));
  for (const [, value] of script.aliases)
    for (const m of value.matchAll(/(?:^|;)\s*alias\s+([^\s;"]+)/gi)) names.add(m[1].toLowerCase());
  for (const name of SHARED_NAMES) names.delete(name);
  return names;
}

function renameTokens(text, map) {
  return text.replace(/[^\s;"]+/g, (token) => map.get(token.toLowerCase()) || token);
}

function namespace(script, prefix) {
  const map = new Map();
  for (const name of definedNames(script)) {
    const sigil = /^[+\-@_]/.test(name) ? name[0] : "";
    map.set(name, `${sigil}${prefix}_${name.slice(sigil.length)}`);
  }
  script.aliases = script.aliases.map(([name, value]) => [map.get(name.toLowerCase()), renameTokens(value, map)]);
  script.binds = script.binds.map(([key, command]) => [key, renameTokens(command, map)]);
  script.commands = script.commands.map((command) => renameTokens(command, map));
  script.examples = script.examples.map((command) => renameTokens(command, map));
  script.namespace = prefix;
}

// Скрипт, который сам ставит свои действия на 1–3 и потом возвращает туда
// оружие: действия — это то, что он биндит, а сами bind-alias пустеют.
function unbindSelfBinding(script) {
  const own = definedNames(script);
  const actionKeys = new Map();
  script.aliases = script.aliases.map(([name, value]) => {
    const m = /^bind\s+(\S+)\s+(\S+)$/i.exec(value.trim());
    if (!m) return [name, value];
    if (own.has(m[2].toLowerCase()) && !actionKeys.has(m[2])) actionKeys.set(m[2], m[1]);
    return [name, ""];
  });
  script.binds = script.binds.concat([...actionKeys].map(([command, key]) => [key, command]));
  // Строка вызова этих alias в конце конфига больше не нужна.
  script.commands = [];
}

function parametrizeSensitivity(script) {
  script.aliases = script.aliases.map(([name, value]) =>
    /sens$/i.test(name) && /^sensitivity\s+[0-9.]+$/i.test(value.trim()) ? [name, "sensitivity {sensitivity}"] : [name, value]
  );
}

function restorePlayerSettings(script) {
  script.aliases = script.aliases.map(([name, value]) => {
    const text = value.trim();
    if (/^def/i.test(name) && /^(default_fov|bxt_cross)\s+[0-9.]+$/i.test(text)) return [name, RESTORE_PERSONALIZATION];
    return [name, value.replace(/\bm_pitch\s+0\.022\b/gi, "m_pitch {m_pitch}")];
  });
}

// Пример из гайда годится, только если вызывает alias этого скрипта: в
// гайде бывают опечатки (obbo200 вместо obbo2000).
function validExample(script, command) {
  const own = definedNames(script);
  const words = command.split(/[\s;]+/).filter(Boolean);
  if (words[0].toLowerCase() === "alias") return own.has((words[2] || "").toLowerCase());
  return words.some((word) => own.has(word.toLowerCase()));
}

function actionsOf(script) {
  const actions = [];
  const seen = new Set();
  const add = (command, guideKey) => {
    const key = command.toLowerCase();
    if (seen.has(key)) return;
    seen.add(key);
    actions.push({ command, guideKey: guideKey && guideKey !== "key" ? guideKey.toUpperCase() : null });
  };
  for (const [key, command] of script.binds) add(command, key);
  for (const command of script.examples) if (validExample(script, command)) add(command, null);
  // Без примера — те alias, которые не вызывает ни один другой alias.
  if (!actions.length) {
    const used = new Set();
    for (const [, value] of script.aliases)
      for (const token of value.split(/[\s;]+/)) used.add(token.toLowerCase());
    for (const [name] of script.aliases) {
      const lower = name.toLowerCase();
      if (lower.startsWith("-")) continue;
      if (!used.has(lower)) add(name, null);
    }
  }
  return actions;
}

function convert(markdown) {
  const scripts = parseSections(markdown)
    .map(buildScript)
    .filter((script) => !SKIP.has(script.id));

  const ids = new Set();
  for (const script of scripts) {
    if (ids.has(script.id)) throw new Error(`повтор id ${script.id}`);
    ids.add(script.id);
  }
  for (const [id, prefix] of Object.entries(NAMESPACES)) {
    const script = scripts.find((s) => s.id === id);
    if (!script) throw new Error(`нет скрипта ${id} для префикса`);
    namespace(script, prefix);
  }
  for (const id of SELF_BINDING) {
    const script = scripts.find((s) => s.id === id);
    if (!script) throw new Error(`нет скрипта ${id}`);
    unbindSelfBinding(script);
  }

  // После префиксов одно имя alias в разных скриптах означает одно и то же.
  const values = new Map();
  for (const script of scripts) {
    for (const [name, value] of script.aliases) {
      const key = name.toLowerCase();
      const prev = values.get(key);
      if (prev && prev.value !== value && prev.id !== script.id)
        throw new Error(`alias ${name}: разный код в ${prev.id} и ${script.id}, нужен префикс в NAMESPACES`);
      if (!prev) values.set(key, { id: script.id, value });
    }
  }

  // Ссылки на другие скрипты — к виду #<id>. В гайде якоря бывают с номером
  // раздела (#28--simple-saveload-script) и с лишними дефисами.
  const linkTarget = (anchor) => {
    const bare = anchor.replace(/^#/, "").replace(/-+/g, "-").replace(/^-|-$/g, "");
    if (ids.has(bare)) return bare;
    const unnumbered = bare.replace(/^\d+-/, "");
    if (ids.has(unnumbered)) return unnumbered;
    throw new Error(`ссылка ${anchor} не ведёт ни на один скрипт`);
  };
  const fixLinks = (text) => text.replace(/\]\((#[^)\s]+)\)/g, (_, anchor) => `](#${linkTarget(anchor)})`);
  for (const script of scripts) {
    script.usage = fixLinks(script.usage);
    script.notes = script.notes.map(fixLinks);
  }

  const chapters = [];
  const result = [];
  for (const script of scripts) {
    parametrizeSensitivity(script);
    restorePlayerSettings(script);
    if (!chapters.some((c) => c.id === script.chapter))
      chapters.push({ id: script.chapter, title: script.chapterTitle });
    const actions = actionsOf(script);
    const needsAutoAlias = script.binds.some(([, c]) => /\bauto_alias\b/i.test(c)) ||
      script.examples.some((c) => /\bauto_alias\b/i.test(c));
    result.push({
      id: script.id,
      chapter: script.chapter,
      section: script.section,
      title: script.title,
      mark: script.mark,
      credit: script.credit,
      notes: script.notes,
      usage: script.usage,
      video: script.video,
      save: script.save,
      namespace: script.namespace || null,
      aliases: script.aliases,
      commands: script.commands,
      actions,
      needsAutoAlias,
    });
  }

  return {
    source: { repo: `https://github.com/${REPO}`, commit: COMMIT },
    chapters,
    scripts: result,
  };
}

async function main() {
  const args = process.argv.slice(2);
  const check = args.includes("--check");
  const file = args.find((arg) => !arg.startsWith("--"));
  const markdown = file
    ? fs.readFileSync(file, "utf8")
    : await fetchText(`https://raw.githubusercontent.com/${REPO}/${COMMIT}/README.md`);
  const data = convert(markdown);
  const text = `${JSON.stringify(data, null, 2)}\n`;
  if (check) {
    const current = fs.readFileSync(OUT, "utf8");
    if (current !== text) {
      console.error("hl1-guide-data.json не совпадает с гайдом: запустите импорт заново");
      process.exit(1);
    }
    console.log(`hl1-guide-data.json совпадает с гайдом, скриптов ${data.scripts.length}`);
    return;
  }
  fs.writeFileSync(OUT, text);
  console.log(`скриптов ${data.scripts.length}, глав ${data.chapters.length} → ${path.relative(process.cwd(), OUT)}`);
}

if (require.main === module) {
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}

module.exports = { convert, COMMIT };
