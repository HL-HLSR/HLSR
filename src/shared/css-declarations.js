"use strict";

// Проверка CSS, который админка выдаёт игрокам (ник, рамка аватара): только
// объявления «свойство: значение» из своего списка, без url() и всего, что
// загружает что-то из сети или выходит за пределы одного элемента. Та же
// проверка на сервере — server/backend/src/Profile/CssDeclarations.php.

// Всё, через что CSS выходит за пределы оформления: загрузка из сети,
// вложенные правила, экранирование, строки и комментарии. Управляющие
// символы в классе — нарочно: их тоже нельзя пускать в CSS.
// eslint-disable-next-line no-control-regex
const FORBIDDEN = /url\s*\(|image\s*\(|image-set\s*\(|element\s*\(|cross-fade\s*\(|expression|javascript:|@|[{}<>\\"'`]|\/\*|[\x00-\x1f\x7f]/i;

// Нормализованная строка объявлений, "" для пустой или null, если хоть одно
// объявление не прошло проверку.
function sanitize(value, properties, max) {
  if (typeof value !== "string") return null;
  const text = value.trim();
  if (!text) return "";
  if (text.length > max || FORBIDDEN.test(text)) return null;
  let depth = 0;
  for (const char of text) {
    if (char === "(") depth++;
    if (char === ")" && --depth < 0) return null;
  }
  if (depth !== 0) return null;
  const out = [];
  for (const part of text.split(";")) {
    if (!part.trim()) continue;
    const colon = part.indexOf(":");
    if (colon === -1) return null;
    const property = part.slice(0, colon).trim().toLowerCase();
    const body = part.slice(colon + 1).trim();
    if (properties.indexOf(property) === -1 || !body || body.length > 300) return null;
    out.push(property + ": " + body);
  }
  return out.join("; ");
}

module.exports = { FORBIDDEN, sanitize };
