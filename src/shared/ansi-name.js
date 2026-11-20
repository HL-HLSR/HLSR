"use strict";

// Имена файлов от игр на Source и GoldSrc. Движок держит строки в UTF-8, а файл
// создаёт ANSI-функцией Windows, поэтому на русской системе (cp1251) запись
// «спидран_005.run» ложится на диск как «СЃРїРёРґСЂР°РЅ_005.run». Переименовывать
// файл нельзя: игра ищет его по тому же испорченному имени. Поэтому чиним только
// то, что видит игрок.
// shortcut: только cp1251 (русская и украинская Windows); другие кодовые страницы — если игроки пришлют такие имена.
const cp1251 = new TextDecoder("windows-1251");
const utf8 = new TextDecoder("utf-8", { fatal: true });
const BYTE_OF = new Map();
for (let byte = 0x80; byte <= 0xff; byte++) BYTE_OF.set(cp1251.decode(Uint8Array.of(byte)), byte);

// Имя как его записала игра; не похожее на UTF-8, прочитанный как cp1251, — без изменений.
function readableName(name) {
  if (typeof name !== "string" || !/[^\x00-\x7f]/.test(name)) return name;
  const bytes = [];
  for (const char of name) {
    const code = char.codePointAt(0);
    if (code < 0x80) bytes.push(code);
    else if (BYTE_OF.has(char)) bytes.push(BYTE_OF.get(char));
    else return name; // символа нет в cp1251 — имя настоящее
  }
  try {
    return utf8.decode(Uint8Array.from(bytes));
  } catch (error) {
    return name; // настоящая кириллица в cp1251 почти никогда не складывается в UTF-8
  }
}

module.exports = { readableName };
