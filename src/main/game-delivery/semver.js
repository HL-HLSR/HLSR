"use strict";

function parse(version) {
  const match = String(version || "").match(/^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?/);
  if (!match) return null;

  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4] || null,
  };
}

function compare(a, b) {
  const left = parse(a);
  const right = parse(b);

  if (!left && !right) return 0;
  if (!left) return -1;
  if (!right) return 1;

  for (const key of ["major", "minor", "patch"]) {
    if (left[key] !== right[key]) return left[key] < right[key] ? -1 : 1;
  }

  if (left.prerelease === right.prerelease) return 0;
  if (!left.prerelease) return 1;
  if (!right.prerelease) return -1;
  return left.prerelease < right.prerelease ? -1 : 1;
}

const lt = (a, b) => compare(a, b) < 0;
const gte = (a, b) => compare(a, b) >= 0;

module.exports = { parse, compare, lt, gte };
