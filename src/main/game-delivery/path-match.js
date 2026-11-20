"use strict";

function normalize(relativePath) {
  return String(relativePath || "")
    .replace(/\\/g, "/")
    .replace(/^\.\//, "")
    .replace(/\/+$/, "");
}

function toRegExp(pattern) {
  const normalized = normalize(pattern);
  let source = "";

  for (let i = 0; i < normalized.length; i++) {
    const char = normalized[i];

    if (char === "*") {
      if (normalized[i + 1] === "*") {
        if (normalized[i + 2] === "/") {
          source += "(?:[^/]+/)*";
          i += 2;
        } else {
          source += ".*";
          i += 1;
        }
      } else {
        source += "[^/]*";
      }
      continue;
    }

    source += char.replace(/[.+?^${}()|[\]\\]/g, "\\$&");
  }

  return new RegExp(`^${source}$`, "i");
}

function createMatcher(patterns) {
  const list = (patterns || []).map(normalize).filter(Boolean);
  const exact = new Set(list.filter((item) => item.indexOf("*") === -1).map((s) => s.toLowerCase()));
  const prefixes = Array.from(exact).map((item) => item + "/");
  const masks = list.filter((item) => item.indexOf("*") !== -1).map(toRegExp);

  const match = (relativePath) => {
    const value = normalize(relativePath).toLowerCase();
    if (!value) return false;
    if (exact.has(value)) return true;
    for (const prefix of prefixes) if (value.startsWith(prefix)) return true;
    for (const mask of masks) if (mask.test(value)) return true;
    return false;
  };

  match.patterns = list;
  return match;
}

module.exports = { normalize, toRegExp, createMatcher };
