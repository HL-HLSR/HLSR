"use strict";

// Документ cfg игрока: разбор по строкам и точечная правка на месте. Текст —
// уже декодированная строка (кодировку выбирает вызывающий, для GoldSrc это
// latin1, байт в байт). Все функции чистые: возвращают новый документ.

const { lineCommands } = require("./game-config");

const SECTIONS = {
  rules: "// ===== HLSR: category rules =====",
  settings: "// ===== HLSR: settings =====",
  personalization: "// ===== HLSR: personalization =====",
  binds: "// ===== HLSR: binds =====",
  scripts: "// ===== HLSR: scripts =====",
  own: "// ===== Your own =====",
};

const HEADERS = new Set(Object.values(SECTIONS));
// Глаголы, у которых второй аргумент — не значение переменной.
const NOT_CVAR = new Set(["exec", "echo", "say", "say_team", "map", "changelevel", "connect", "wait", "toggle"]);

const blockBegin = (id) => `// HLSR script ${id} begin`;
const blockEnd = (id) => `// HLSR script ${id} end`;

function parse(text) {
  let source = String(text || "");
  const bom = source.charCodeAt(0) === 0xfeff;
  if (bom) source = source.slice(1);
  const lines = [];
  const re = /([^\r\n]*)(\r\n|\n|\r|$)/g;
  let match;
  while ((match = re.exec(source)) && (match[1] !== "" || match[2] !== "")) {
    lines.push({ text: match[1], eol: match[2] });
  }
  const first = lines.find((line) => line.eol);
  return { bom, eol: first ? first.eol : "\r\n", lines };
}

function serialize(doc) {
  return (doc.bom ? "﻿" : "") + doc.lines.map((line) => line.text + line.eol).join("");
}

// Ключ команды: cvar:<имя>, bind:<клавиша>, alias:<имя>; иначе null.
function keyOf(args) {
  if (!args.length) return null;
  const verb = args[0].value.toLowerCase();
  if (verb === "bind") return args.length >= 3 ? `bind:${args[1].value.toLowerCase()}` : null;
  if (verb === "unbind") return args.length >= 2 ? `bind:${args[1].value.toLowerCase()}` : null;
  if (verb === "alias") return args.length >= 3 ? `alias:${args[1].value.toLowerCase()}` : null;
  if (/^[+-]/.test(verb) || NOT_CVAR.has(verb)) return null;
  return args.length >= 2 ? `cvar:${verb}` : null;
}

// Номер аргумента со значением: у bind и alias — третий, у cvar — второй.
const valueArg = (key) => (key.startsWith("cvar:") ? 1 : 2);

// Все команды документа с ключом: { line, index, command, key }.
function commandsOf(doc) {
  const result = [];
  doc.lines.forEach((line, lineNo) => {
    lineCommands(line.text).forEach((command, index) => {
      result.push({ line: lineNo, index, command, key: keyOf(command.args) });
    });
  });
  return result;
}

function get(doc, key) {
  const found = commandsOf(doc).filter((item) => item.key === key).pop();
  if (!found || isUnbind(found.command)) return null;
  return { value: found.command.args[valueArg(key)].value, line: found.line, index: found.index };
}

const isUnbind = (command) => command.args[0].value.toLowerCase() === "unbind";

// Позиции слов команды внутри строки: { start, end, quoted }.
function spans(line, command) {
  const result = [];
  const re = /"[^"]*"?|\S+/g;
  const segment = line.slice(command.start, command.end);
  let match;
  while ((match = re.exec(segment))) {
    result.push({ start: command.start + match.index, end: command.start + match.index + match[0].length, quoted: match[0][0] === '"' });
  }
  return result;
}

function withLines(doc, lines) {
  return Object.assign({}, doc, { lines });
}

const section = (doc, name) => {
  const header = doc.lines.findIndex((line) => line.text.trim() === SECTIONS[name]);
  if (header < 0) return null;
  let end = header + 1;
  while (end < doc.lines.length && !HEADERS.has(doc.lines[end].text.trim())) end += 1;
  // Пустые строки перед следующим заголовком остаются разделителем.
  while (end - 1 > header && doc.lines[end - 1].text.trim() === "" && end < doc.lines.length) end -= 1;
  return { header, end };
};

// Вставляет строки в конец раздела; нет заголовка — создаёт раздел перед «Your own» или в конце.
function insertLines(doc, name, texts) {
  const lines = doc.lines.map((line) => ({ ...line }));
  const make = (list) => list.map((text) => ({ text, eol: doc.eol }));
  const found = section(doc, name);
  if (found) {
    if (found.end === lines.length && lines.length && !lines[lines.length - 1].eol) lines[lines.length - 1].eol = doc.eol;
    lines.splice(found.end, 0, ...make(texts));
    return withLines(doc, lines);
  }
  const created = [SECTIONS[name], ...texts];
  const own = name === "own" ? -1 : lines.findIndex((line) => line.text.trim() === SECTIONS.own);
  if (own >= 0) {
    lines.splice(own, 0, ...make([...created, ""]));
    return withLines(doc, lines);
  }
  if (lines.length && !lines[lines.length - 1].eol) lines[lines.length - 1].eol = doc.eol;
  if (lines.length && lines[lines.length - 1].text.trim() !== "") created.unshift("");
  lines.push(...make(created));
  return withLines(doc, lines);
}

function newLine(key, value) {
  const name = key.slice(key.indexOf(":") + 1);
  if (key.startsWith("bind:")) return `bind "${name}" "${value}"`;
  if (key.startsWith("alias:")) return `alias ${name} "${value}"`;
  return `${name} "${value}"`;
}

const isUnbindall = (item) => item.command.args.length > 0 && item.command.args[0].value.toLowerCase() === "unbindall";
const isAfter = (item, other) => item.line > other.line || (item.line === other.line && item.index > other.index);

function set(doc, key, value, sectionName) {
  const commands = commandsOf(doc);
  let found = commands.filter((item) => item.key === key).pop();
  // Бинд выше unbindall (шаблон над импортированным config.cfg) в игре снят: правка на месте
  // ничего бы не дала. Новая строка идёт после последнего unbindall (в раздел, если его нет или он ниже),
  // а следующая правка меняет её на месте: мёртвых строк не копится.
  if (found && key.startsWith("bind:")) {
    const last = commands.filter(isUnbindall).pop();
    if (last && isAfter(last, found)) {
      const own = section(doc, sectionName);
      if (!own || own.end > last.line) return insertLines(doc, sectionName, [newLine(key, value)]);
      const lines = doc.lines.map((line) => ({ ...line }));
      if (!lines[last.line].eol) lines[last.line].eol = doc.eol;
      lines.splice(last.line + 1, 0, { text: newLine(key, value), eol: doc.eol });
      return withLines(doc, lines);
    }
  }
  if (!found) return insertLines(doc, sectionName, [newLine(key, value)]);
  const lines = doc.lines.map((line) => ({ ...line }));
  const text = lines[found.line].text;
  if (isUnbind(found.command)) {
    // Последний unbind заменяем на месте, чтобы новый bind остался последним.
    const words = spans(text, found.command);
    lines[found.line].text = text.slice(0, words[0].start) + newLine(key, value) + text.slice(words[words.length - 1].end);
    return withLines(doc, lines);
  }
  const span = spans(text, found.command)[valueArg(key)];
  const quoted = span.quoted || value === "" || /[\s;]/.test(value);
  lines[found.line].text = text.slice(0, span.start) + (quoted ? `"${value}"` : value) + text.slice(span.end);
  return withLines(doc, lines);
}

function removeKey(doc, key) {
  const lines = [];
  doc.lines.forEach((line, lineNo) => {
    const mine = lineCommands(line.text).filter((command) => keyOf(command.args) === key);
    if (!mine.length) return lines.push(line);
    const all = lineCommands(line.text);
    if (all.length === mine.length) return;
    // Вырезаем команду вместе с «;» после неё (у последней — с «;» перед ней).
    let text = line.text;
    for (const command of mine.reverse()) {
      const semi = text.indexOf(";", command.end);
      const next = all.find((other) => other.start > command.start);
      if (next && semi >= 0) text = text.slice(0, command.start) + text.slice(semi + 1).replace(command.start === 0 ? /^\s+/ : /^$/, "");
      else text = text.slice(0, command.start).replace(/;\s*$/, "") + text.slice(command.end);
    }
    lines.push({ text, eol: line.eol });
  });
  return withLines(doc, lines);
}

function findBlock(doc, id) {
  const texts = doc.lines.map((line) => line.text.trim());
  const begin = texts.indexOf(blockBegin(id));
  if (begin < 0) return null;
  const end = texts.indexOf(blockEnd(id), begin + 1);
  if (end < 0) return null;
  return { begin, end, body: doc.lines.slice(begin + 1, end).map((line) => line.text) };
}

function setBlock(doc, id, bodyLines, sectionName) {
  const block = findBlock(doc, id);
  if (!block) return insertLines(doc, sectionName, [blockBegin(id), ...bodyLines, blockEnd(id)]);
  const lines = doc.lines.map((line) => ({ ...line }));
  lines.splice(block.begin + 1, block.end - block.begin - 1, ...bodyLines.map((text) => ({ text, eol: doc.eol })));
  return withLines(doc, lines);
}

function removeBlock(doc, id) {
  const block = findBlock(doc, id);
  if (!block) return doc;
  const lines = doc.lines.map((line) => ({ ...line }));
  lines.splice(block.begin, block.end - block.begin + 1);
  return withLines(doc, lines);
}

// Все блоки скриптов лаунчера: id в порядке появления.
function blockIds(doc) {
  const ids = [];
  for (const line of doc.lines) {
    const match = /^\/\/ HLSR script (.+) begin$/.exec(line.text.trim());
    if (match && !ids.includes(match[1])) ids.push(match[1]);
  }
  return ids;
}

module.exports = { SECTIONS, commandsOf, parse, serialize, keyOf, get, set, removeKey, findBlock, setBlock, removeBlock, insertLines, blockIds };
