"use strict";

const REACTIONS = Object.freeze({ like: "👍", love: "❤️", fire: "🔥", wow: "😮", celebrate: "🎉" });
const validPostId = (id) => typeof id === "string" && /^[1-9][0-9]{0,18}$/.test(id);

function counts(raw) {
  const out = {};
  for (const key of Object.keys(REACTIONS)) {
    const n = raw && raw[key];
    out[key] = Number.isSafeInteger(n) && n >= 0 && n <= 1000000000 ? n : 0;
  }
  return out;
}

function mine(raw) {
  return Array.isArray(raw) ? [...new Set(raw.filter((key) => typeof key === "string" && Object.hasOwn(REACTIONS, key)))] : [];
}

module.exports = { REACTIONS, validPostId, counts, mine };
