#!/usr/bin/env node
"use strict";

// Таблица разрешённых команд HL1 с speedrun.com (Google Sheet, вкладки Main и
// Skill) → src/shared/hl1-whitelist.json. Содержимое таблицы — только данные.
//
//   node scripts/build-hl1-whitelist.js --fetch
//   node scripts/build-hl1-whitelist.js <main.csv> <skill.csv> [<main-colors.json>]
//
// Правило записи в таблице — это цвет ячейки с именем (зелёный разрешено,
// красный запрещено, жёлтый зависит от категории) и текст рядом. Цвета в CSV
// не попадают, поэтому --fetch скачивает ещё и xlsx и сохраняет цвета первого
// столбца в main-colors.json рядом с CSV (по умолчанию он берётся из папки
// main.csv). Результат детерминирован: ключи по алфавиту, LF. Время загрузки
// пишется только при --fetch, иначе берётся из HLSR_WHITELIST_FETCHED_AT или null.

const fs = require("fs");
const https = require("https");
const path = require("path");
const zlib = require("zlib");

const SHEET = "1sbVfoW7LAOdwgXzZH3vcA7D-djr2Y8qAQ_c4WezwhRA";
const OUT = path.join(__dirname, "..", "src", "shared", "hl1-whitelist.json");
const FIXTURES = path.join(__dirname, "fixtures", "hl1-whitelist");

// Вкладка Main первая (gid 0): экспорт CSV в gviz отбрасывает текст из
// числовых столбцов, а export — нет. Skill в gviz читается без потерь.
const URLS = {
  main: `https://docs.google.com/spreadsheets/d/${SHEET}/export?format=csv&gid=0`,
  skill: `https://docs.google.com/spreadsheets/d/${SHEET}/gviz/tq?tqx=out:csv&sheet=Skill`,
  xlsx: `https://docs.google.com/spreadsheets/d/${SHEET}/export?format=xlsx`,
};

// Переходы только на https и хосты Google, не больше 5, с таймаутом.
const allowedHost = (url) => url.protocol === "https:" && /(^|\.)(google\.com|googleusercontent\.com)$/.test(url.hostname);

function download(url, depth = 0) {
  return new Promise((resolve, reject) => {
    const target = new URL(url);
    if (!allowedHost(target)) { reject(new Error(`запрещённый адрес: ${target.origin}`)); return; }
    const req = https.get(target, { timeout: 30000 }, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        res.resume();
        if (depth >= 5) { reject(new Error("слишком много переадресаций")); return; }
        resolve(download(new URL(res.headers.location, target).toString(), depth + 1));
        return;
      }
      if (res.statusCode !== 200) { res.resume(); reject(new Error(`HTTP ${res.statusCode}: ${target.origin}`)); return; }
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => resolve(Buffer.concat(chunks)));
    });
    req.on("timeout", () => req.destroy(new Error("таймаут загрузки")));
    req.on("error", reject);
  });
}

// CSV с кавычками → строки ячеек (хвостовые пустые ячейки отброшены).
function parseCsv(text) {
  const rows = [];
  let row = [];
  let cell = "";
  let quoted = false;
  const src = String(text).replace(/^\ufeff/, "");
  for (let i = 0; i < src.length; i += 1) {
    const ch = src[i];
    if (quoted) {
      if (ch === '"') {
        if (src[i + 1] === '"') { cell += '"'; i += 1; } else quoted = false;
      } else cell += ch;
    } else if (ch === '"') quoted = true;
    else if (ch === ",") { row.push(cell); cell = ""; }
    else if (ch === "\n") { row.push(cell); rows.push(row); row = []; cell = ""; }
    else if (ch !== "\r") cell += ch;
  }
  if (cell || row.length) { row.push(cell); rows.push(row); }
  return rows.map((cells) => {
    const trimmed = cells.map((c) => c.replace(/[\t\xa0]+/g, " ").trim());
    while (trimmed.length && !trimmed[trimmed.length - 1]) trimmed.pop();
    return trimmed;
  });
}

// --- цвета из xlsx ---------------------------------------------------------

// Читает zip-архив: get(name) — текст файла или null.
function openZip(buf) {
  let eocd = buf.length - 22;
  while (eocd >= 0 && buf.readUInt32LE(eocd) !== 0x06054b50) eocd -= 1;
  if (eocd < 0) throw new Error("xlsx: не найден конец архива");
  let pos = buf.readUInt32LE(eocd + 16);
  const count = buf.readUInt16LE(eocd + 10);
  const entries = new Map();
  for (let i = 0; i < count; i += 1) {
    const nameLen = buf.readUInt16LE(pos + 28);
    const name = buf.toString("utf8", pos + 46, pos + 46 + nameLen);
    entries.set(name, { method: buf.readUInt16LE(pos + 10), size: buf.readUInt32LE(pos + 20), local: buf.readUInt32LE(pos + 42) });
    pos += 46 + nameLen + buf.readUInt16LE(pos + 30) + buf.readUInt16LE(pos + 32);
  }
  return {
    get(name) {
      const e = entries.get(name);
      if (!e) return null;
      const start = e.local + 30 + buf.readUInt16LE(e.local + 26) + buf.readUInt16LE(e.local + 28);
      const data = buf.subarray(start, start + e.size);
      return (e.method === 8 ? zlib.inflateRawSync(data) : data).toString("utf8");
    },
  };
}

const unxml = (s) => s.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, "&");
const COLORS = { FFB6D7A8: "green", FFEA9999: "red", FFFFD966: "yellow", FF000000: "black" };
const attr = (tag, name) => { const m = new RegExp(`\\b${name}="([^"]*)"`).exec(tag); return m ? unxml(m[1]) : null; };

// Порядок цветов темы в ссылках theme="N": lt1, dk1, lt2, dk2, accent1..6, hlink, folHlink.
const THEME_ORDER = ["lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6", "hlink", "folHlink"];

function themeColors(theme) {
  const result = [];
  if (!theme) return result;
  for (const name of THEME_ORDER) {
    const m = new RegExp(`<a:${name}>\\s*<a:(?:srgbClr val|sysClr[^>]*lastClr)="(\\w{6})"`).exec(theme);
    result.push(m ? `FF${m[1].toUpperCase()}` : null);
  }
  return result;
}

// Цвет заливки: ARGB или null (неразрешимо) либо "none" без заливки.
function fillRgb(fillXml, theme) {
  const tag = /<fgColor\b[^>]*>/.exec(fillXml);
  if (!tag) return "none";
  const rgb = attr(tag[0], "rgb");
  if (rgb) return rgb.toUpperCase();
  const index = attr(tag[0], "theme");
  const tint = Number(attr(tag[0], "tint") || 0);
  if (index !== null && !tint && theme[+index]) return theme[+index];
  return null; // indexed и тема с оттенком не разрешаем
}

// Столбец A листа «Main»: [[имя, цвет]] в порядке строк, только непустые.
// warn(text) — всё, что не удалось понять однозначно.
function columnColors(xlsx, warn = console.warn) {
  const zip = openZip(xlsx);
  const need = (name) => { const text = zip.get(name); if (text === null) throw new Error(`xlsx: нет файла ${name}`); return text; };
  // Лист по имени: workbook.xml + связи.
  const sheetTag = [...need("xl/workbook.xml").matchAll(/<sheet\b[^>]*>/g)].map((m) => m[0]).find((tag) => attr(tag, "name") === "Main");
  if (!sheetTag) throw new Error("xlsx: нет листа Main");
  const relId = attr(sheetTag, "r:id");
  const rel = [...need("xl/_rels/workbook.xml.rels").matchAll(/<Relationship\b[^>]*>/g)].map((m) => m[0]).find((tag) => attr(tag, "Id") === relId);
  if (!rel) throw new Error("xlsx: нет связи листа Main");
  const target = attr(rel, "Target");
  const sheetPath = target.startsWith("/") ? target.slice(1) : `xl/${target}`;
  const sheet = need(sheetPath);
  if (/<conditionalFormatting\b/.test(sheet)) warn("xlsx: на листе Main есть условное форматирование, цвета ячеек могут не совпадать с видимыми");
  const styles = need("xl/styles.xml");
  const theme = themeColors(zip.get("xl/theme/theme1.xml"));
  const fills = [...styles.matchAll(/<fill>([\s\S]*?)<\/fill>/g)].map((m) => fillRgb(m[1], theme));
  const cellXfs = /<cellXfs[^>]*>([\s\S]*?)<\/cellXfs>/.exec(styles)[1];
  const xfFill = [...cellXfs.matchAll(/<xf\b[^>]*>/g)].map((m) => +(attr(m[0], "fillId") || 0));
  const strings = [...need("xl/sharedStrings.xml").matchAll(/<si>([\s\S]*?)<\/si>/g)]
    .map((m) => unxml([...m[1].matchAll(/<t[^>]*>([\s\S]*?)<\/t>/g)].map((x) => x[1]).join("")));
  const result = [];
  for (const m of sheet.matchAll(/<c r="A(\d+)"([^>]*?)(?:\/>|>([\s\S]*?)<\/c>)/g)) {
    const style = +(/\bs="(\d+)"/.exec(m[2]) || [0, 0])[1];
    const value = m[3] && /<v>(.*?)<\/v>/.exec(m[3]);
    if (!value) continue;
    const text = (/\bt="s"/.test(m[2]) ? strings[+value[1]] : unxml(value[1])).replace(/\s+/g, " ").trim();
    if (!text) continue;
    const rgb = fills[xfFill[style]];
    if (rgb === null) warn(`xlsx: строка ${m[1]} «${text}»: цвет заливки не разрешён (theme с оттенком или indexed)`);
    result.push([text, COLORS[rgb] || "other"]);
  }
  return result;
}

// --- разбор вкладок --------------------------------------------------------

const SECTION = {
  "key cvars": "cvars",
  "remaning cvars": "cvars",
  "remaining cvars": "cvars",
  "key commands": "commands",
  "remaning commands": "commands",
  "remaining commands": "commands",
};

// Правило по тексту рядом с именем; без понятного текста — по цвету.
function ruleOfText(note, color) {
  const t = note.toLowerCase();
  let m = /both\s+(\d+)\s+and\s+(\d+)\s+allowed/.exec(t);
  if (m) return { rule: "values", values: [m[1], m[2]] };
  const numbers = [...t.matchAll(/(?:^|\|)\s*(\d+)\s*\(/g)].map((x) => x[1]);
  if (numbers.length) return { rule: "values", values: numbers };
  if (/run-start/.test(t)) return { rule: "run-start" };
  if (/thirdperson/.test(t)) return { rule: "thirdperson" };
  if (/steam version/.test(t)) return { rule: "steam" };
  if (/category extension|individual levels/.test(t)) return { rule: "category" };
  // Две разные надписи таблицы: «только в Scripted» и «нельзя в последовательностях Scripted».
  if (/banned in scripted/.test(t)) return { rule: "no-script-seq" };
  if (/only allowed in scripted/.test(t)) return { rule: "scripted-only" };
  if (/scripted/.test(t)) return { rule: "scripted" };
  if (color === "green") return { rule: "allowed" };
  if (color === "red") return { rule: "banned" };
  if (color === "yellow") return { rule: "category" };
  throw new Error(`цвет «${color}» без понятного текста: ${note}`);
}

// Цвет и текст должны говорить одно и то же; расхождение не скрываем.
const COLOR_RULES = {
  green: ["allowed", "values"],
  red: ["banned"],
  yellow: ["scripted", "scripted-only", "no-script-seq", "category", "thirdperson", "steam", "run-start"],
};

function buildMain(rows, colors, warn = console.warn) {
  const ruleOf = (note, color, name) => {
    const result = ruleOfText(note, color);
    if (!(COLOR_RULES[color] || []).includes(result.rule)) {
      warn(`ЦВЕТ/ТЕКСТ: «${name}»: цвет ${color}, текст «${note}», выбрано ${result.rule}`);
    }
    return result;
  };
  const named = rows.filter((cells) => cells[0]);
  if (named.length !== colors.length) throw new Error(`имён в CSV ${named.length}, цветов ${colors.length}`);
  const key = (s) => s.replace(/\s+/g, " ").trim().toLowerCase();
  const cvars = {};
  const commands = {};
  const patterns = [];
  let section = null;
  named.forEach((cells, index) => {
    const [text, color] = colors[index];
    if (key(text) !== key(cells[0])) throw new Error(`цвета не сошлись с CSV на «${cells[0]}» / «${text}»`);
    const first = key(cells[0]);
    if (SECTION[first]) { section = SECTION[first]; return; }
    if (!section) return;
    // Столбец D — легенда цветов («Allowed», «Banned»), не данные.
    const def = section === "cvars" ? cells[1] || "" : "";
    const note = (section === "cvars" ? [cells[2]] : [cells[1], cells[2]]).filter(Boolean).join("; ");
    const all = /^all\s+(.+)$/.exec(first);
    if (all) {
      const m = /^(.*?)\s*(cvars|commands)$/.exec(all[1]);
      patterns.push(Object.assign({ prefix: m ? m[1] : all[1], kind: m ? m[2] : section, note }, ruleOf(note, color, first)));
      return;
    }
    // «weapon_» в таблице — все weapon_*, а не одна команда.
    if (first.endsWith("_") && !first.includes(" ")) {
      patterns.push(Object.assign({ prefix: first, kind: section, note }, ruleOf(note, color, first)));
      return;
    }
    const entry = Object.assign(section === "cvars" ? { default: def } : {}, ruleOf(note, color, first), { note });
    (section === "cvars" ? cvars : commands)[first] = entry;
  });
  const order = (p) => `${p.kind} ${p.prefix}`;
  patterns.sort((a, b) => (order(a) < order(b) ? -1 : order(a) > order(b) ? 1 : 0));
  return { cvars, commands, patterns };
}

// Для HL1 WON действует только раздел «HL1».
function buildSkill(rows) {
  const skill = {};
  let inHl1 = false;
  for (const cells of rows) {
    if (!cells[0]) continue;
    if (!/^sk_/i.test(cells[0])) { inHl1 = cells[0].toLowerCase() === "hl1"; continue; }
    if (inHl1) skill[cells[0].toLowerCase()] = cells[1] || "";
  }
  return skill;
}

const sortObject = (value) => {
  if (Array.isArray(value)) return value.map(sortObject);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.keys(value).sort().map((k) => [k, sortObject(value[k])]));
  }
  return value;
};

function build(mainCsv, skillCsv, colors, fetchedAt = null, warn = console.warn) {
  const main = buildMain(parseCsv(mainCsv), colors, warn);
  const data = {
    source: { sheet: SHEET, tabs: ["Main", "Skill"], fetchedAt },
    cvars: main.cvars,
    commands: main.commands,
    patterns: main.patterns,
    skill: buildSkill(parseCsv(skillCsv)),
  };
  return `${JSON.stringify(sortObject(data), null, 2)}\n`;
}

async function main() {
  const args = process.argv.slice(2);
  let mainCsv, skillCsv, colors, fetchedAt;
  if (args[0] === "--fetch") {
    mainCsv = (await download(URLS.main)).toString("utf8");
    skillCsv = (await download(URLS.skill)).toString("utf8");
    colors = columnColors(await download(URLS.xlsx));
    fetchedAt = new Date().toISOString();
    fs.mkdirSync(FIXTURES, { recursive: true });
    fs.writeFileSync(path.join(FIXTURES, "main.csv"), mainCsv);
    fs.writeFileSync(path.join(FIXTURES, "skill.csv"), skillCsv);
    fs.writeFileSync(path.join(FIXTURES, "main-colors.json"), `${JSON.stringify(colors)}\n`);
  } else if (args.length >= 2) {
    mainCsv = fs.readFileSync(args[0], "utf8");
    skillCsv = fs.readFileSync(args[1], "utf8");
    colors = JSON.parse(fs.readFileSync(args[2] || path.join(path.dirname(args[0]), "main-colors.json"), "utf8"));
    fetchedAt = process.env.HLSR_WHITELIST_FETCHED_AT || null;
  } else {
    console.error("Использование: node scripts/build-hl1-whitelist.js [--fetch | <main.csv> <skill.csv> [<main-colors.json>]]");
    process.exit(2);
  }
  fs.writeFileSync(OUT, build(mainCsv, skillCsv, colors, fetchedAt));
  console.log(`записано: ${OUT}`);
}

if (require.main === module) {
  main().catch((error) => { console.error(error.message); process.exit(1); });
}

module.exports = { parseCsv, build, columnColors, FIXTURES };
