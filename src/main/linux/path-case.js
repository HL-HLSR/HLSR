"use strict";

const fs = require("fs");
const path = require("path");

// Wine ищет файлы без учёта регистра, а Source и GoldSrc обращаются к своим
// путям в нижнем регистре. Если рядом с папкой на пути к игре лежит другая,
// отличающаяся только регистром (~/Games/HLSR и ~/Games/hlsr), Wine уходит
// в неё и игра падает на старте. Ищем такую пару до запуска.
function findCaseConflict(target, fsModule = fs) {
  const parts = [];
  for (let dir = path.resolve(target); path.dirname(dir) !== dir; dir = path.dirname(dir)) parts.unshift(dir);
  for (const folder of parts) {
    const parent = path.dirname(folder);
    const name = path.basename(folder);
    let entries;
    try { entries = fsModule.readdirSync(parent); } catch (error) { continue; }
    const twin = entries.find((entry) => entry !== name && entry.toLowerCase() === name.toLowerCase());
    if (twin) return { folder, twin: path.join(parent, twin) };
  }
  return null;
}

module.exports = { findCaseConflict };
