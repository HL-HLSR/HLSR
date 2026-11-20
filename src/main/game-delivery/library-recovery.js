"use strict";

const nodeFs = require("fs");
const path = require("path");

const { resolveLibrary } = require("./paths");

const INSTALLED_FILE = "installed.json";

function installedGamesAt(root, options) {
  const opts = options || {};
  const fs = opts.fs || nodeFs;
  const gameIds = opts.gameIds || [];

  if (!root) return 0;

  let layout;
  try {
    layout = (opts.resolveLibrary || resolveLibrary)(root);
  } catch (error) {
    return 0;
  }

  let count = 0;

  for (const gameId of gameIds) {
    try {
      const data = JSON.parse(
        fs.readFileSync(path.join(layout.games, gameId, INSTALLED_FILE), "utf8")
      );
      if (data && data.state === "ready") count += 1;
    } catch (ignored) {
    }
  }

  return count;
}

function samePath(a, b) {
  return path.resolve(String(a)).toLowerCase() === path.resolve(String(b)).toLowerCase();
}

function recoverLibraryPath(current, options) {
  const opts = options || {};
  const fallback = opts.fallback;
  const logger = opts.logger || { warn() {} };
  const countAt = opts.countAt || ((root) => installedGamesAt(root, opts));

  if (!current || !fallback) return current;
  if (samePath(current, fallback)) return current;

  if (countAt(current) > 0) return current;

  const found = countAt(fallback);
  if (!found) return current;

  logger.warn(
    `[game-delivery] по сохранённому пути библиотеки игр нет (${current}), ` +
      `а в стандартном найдено ${found} — работаем с ${fallback}`
  );

  return fallback;
}

module.exports = { installedGamesAt, recoverLibraryPath, INSTALLED_FILE };
