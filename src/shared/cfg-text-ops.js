"use strict";

// Операции текстового редактора cfg (GameCfgFiles.vue): поиск и замена,
// работа со строками. Каждая правка — одна замена участка текста
// { start, end, insert } и новое выделение { selStart, selEnd }: окно
// применяет её через insertText, и Ctrl+Z отменяет правку целиком.

const MAX_MATCHES = 10000;

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// RegExp для поиска или { error } для неверного выражения.
function buildPattern(query, options = {}) {
  if (typeof query !== "string" || !query) return { error: "EMPTY" };
  let source = options.regex ? query : escapeRegExp(query);
  if (options.wholeWord) source = `(?<![\\w])(?:${source})(?![\\w])`;
  try {
    return { pattern: new RegExp(source, options.caseSensitive ? "gu" : "giu") };
  } catch (error) {
    return { error: "BAD_REGEX", message: error.message };
  }
}

// Все совпадения [{ start, end }]; пустые совпадения регулярки пропускаются.
function findAll(text, query, options) {
  const built = buildPattern(query, options);
  if (built.error) return { matches: [], error: built.error === "EMPTY" ? null : built.error };
  const matches = [];
  const pattern = built.pattern;
  let found;
  while ((found = pattern.exec(text)) && matches.length < MAX_MATCHES) {
    if (found[0].length === 0) {
      pattern.lastIndex += 1;
      continue;
    }
    matches.push({ start: found.index, end: found.index + found[0].length });
  }
  return { matches, error: null, capped: matches.length >= MAX_MATCHES };
}

// Номер совпадения для курсора: первое, что начинается не раньше from
// (backward — последнее, что кончается не позже from). По кругу.
function nearestMatch(matches, from, backward) {
  if (!matches.length) return -1;
  if (backward) {
    for (let i = matches.length - 1; i >= 0; i -= 1) if (matches[i].end <= from && matches[i].start < from) return i;
    return matches.length - 1;
  }
  for (let i = 0; i < matches.length; i += 1) if (matches[i].start >= from) return i;
  return 0;
}

// $$, $&, $1…$99 и $<имя> в тексте замены — как у String.prototype.replace.
function expandReplacement(replacement, found) {
  return replacement.replace(/\$(\$|&|<([^>]*)>|(\d{1,2}))/g, (token, kind, name, digits) => {
    if (kind === "$") return "$";
    if (kind === "&") return found[0];
    if (name !== undefined) return found.groups && found.groups[name] !== undefined ? found.groups[name] : "";
    const index = Number(digits);
    if (index > 0 && index < found.length) return found[index] === undefined ? "" : found[index];
    return token;
  });
}

// Текст замены для одного совпадения: в режиме регулярки работают $1, $&.
function replacementFor(text, match, query, replacement, options) {
  if (!options || !options.regex) return replacement;
  const built = buildPattern(query, options);
  if (built.error) return replacement;
  const sticky = new RegExp(built.pattern.source, built.pattern.flags.replace("g", "") + "y");
  sticky.lastIndex = match.start;
  const found = sticky.exec(text);
  return found ? expandReplacement(replacement, found) : replacement;
}

function replaceOne(text, match, query, replacement, options) {
  const insert = replacementFor(text, match, query, replacement, options);
  return { start: match.start, end: match.end, insert, selStart: match.start, selEnd: match.start + insert.length };
}

// Замена всех совпадений одной правкой на весь текст.
function replaceAll(text, query, replacement, options) {
  const { matches, error } = findAll(text, query, options);
  if (error || !matches.length) return { count: 0, error };
  let out = "";
  let at = 0;
  for (const match of matches) {
    out += text.slice(at, match.start) + replacementFor(text, match, query, replacement, options);
    at = match.end;
  }
  out += text.slice(at);
  return { count: matches.length, edit: { start: 0, end: text.length, insert: out, selStart: 0, selEnd: 0 } };
}

// Границы строк, которых касается выделение: [начало первой, конец последней)
// без завершающего перевода строки. Выделение, кончающееся в начале строки,
// эту строку не захватывает.
function lineSpan(text, selStart, selEnd) {
  let end = selEnd;
  if (end > selStart && text[end - 1] === "\n") end -= 1;
  const start = text.lastIndexOf("\n", selStart - 1) + 1;
  let stop = text.indexOf("\n", end);
  if (stop === -1) stop = text.length;
  return { start, end: stop };
}

function lineCol(text, pos) {
  const before = text.slice(0, pos);
  const line = before.split("\n").length;
  return { line, col: pos - (before.lastIndexOf("\n") + 1) + 1 };
}

function lineStart(text, line) {
  let at = 0;
  for (let n = 1; n < line; n += 1) {
    const next = text.indexOf("\n", at);
    if (next === -1) return text.length;
    at = next + 1;
  }
  return at;
}

function lineCount(text) {
  let count = 1;
  for (let i = 0; i < text.length; i += 1) if (text.charCodeAt(i) === 10) count += 1;
  return count;
}

const COMMENT = /^(\s*)\/\/ ?/;

// Ctrl+/: закомментировать строки выделения «// » или снять комментарий,
// если закомментированы все непустые.
function toggleComment(text, selStart, selEnd) {
  const span = lineSpan(text, selStart, selEnd);
  const lines = text.slice(span.start, span.end).split("\n");
  const filled = lines.filter((line) => line.trim());
  const uncomment = filled.length > 0 && filled.every((line) => COMMENT.test(line));
  const indent = Math.min(...filled.map((line) => line.match(/^\s*/)[0].length).concat(filled.length ? [] : [0]));
  const out = lines.map((line) => {
    if (!line.trim()) return line;
    if (uncomment) return line.replace(COMMENT, "$1");
    return line.slice(0, indent) + "// " + line.slice(indent);
  });
  const insert = out.join("\n");
  return { start: span.start, end: span.end, insert, selStart: span.start, selEnd: span.start + insert.length };
}

// Ctrl+D: копия строк выделения ниже, выделение переходит на копию.
function duplicateLines(text, selStart, selEnd) {
  const span = lineSpan(text, selStart, selEnd);
  const block = text.slice(span.start, span.end);
  const shift = block.length + 1;
  return { start: span.end, end: span.end, insert: "\n" + block, selStart: selStart + shift, selEnd: selEnd + shift };
}

// Ctrl+Shift+K: удалить строки выделения.
function deleteLines(text, selStart, selEnd) {
  const span = lineSpan(text, selStart, selEnd);
  if (span.end < text.length) return { start: span.start, end: span.end + 1, insert: "", selStart: span.start, selEnd: span.start };
  const start = Math.max(0, span.start - 1);
  return { start, end: span.end, insert: "", selStart: lineSpan(text, start, start).start, selEnd: lineSpan(text, start, start).start };
}

// Alt+↑/↓: переставить строки выделения с соседней. null — двигать некуда.
function moveLines(text, selStart, selEnd, direction) {
  const span = lineSpan(text, selStart, selEnd);
  const block = text.slice(span.start, span.end);
  if (direction < 0) {
    if (span.start === 0) return null;
    const prev = lineSpan(text, span.start - 1, span.start - 1);
    const above = text.slice(prev.start, prev.end);
    const delta = above.length + 1;
    return { start: prev.start, end: span.end, insert: block + "\n" + above, selStart: selStart - delta, selEnd: selEnd - delta };
  }
  if (span.end >= text.length) return null;
  const next = lineSpan(text, span.end + 1, span.end + 1);
  const below = text.slice(next.start, next.end);
  const delta = below.length + 1;
  return { start: span.start, end: next.end, insert: below + "\n" + block, selStart: selStart + delta, selEnd: selEnd + delta };
}

// Tab / Shift+Tab по выделению в несколько строк: отступ табуляцией.
function indentLines(text, selStart, selEnd, outdent) {
  const span = lineSpan(text, selStart, selEnd);
  const lines = text.slice(span.start, span.end).split("\n");
  const out = lines.map((line) => {
    if (!outdent) return line ? "\t" + line : line;
    if (line.startsWith("\t")) return line.slice(1);
    return line.replace(/^ {1,4}/, "");
  });
  const insert = out.join("\n");
  return { start: span.start, end: span.end, insert, selStart: span.start, selEnd: span.start + insert.length };
}

module.exports = {
  MAX_MATCHES,
  buildPattern,
  findAll,
  nearestMatch,
  replaceOne,
  replaceAll,
  lineSpan,
  lineCol,
  lineStart,
  lineCount,
  toggleComment,
  duplicateLines,
  deleteLines,
  moveLines,
  indentLines,
};
