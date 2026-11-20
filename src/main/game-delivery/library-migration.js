"use strict";

const fs = require("fs");
const path = require("path");

const { resolveLibrary, assertInside, isInside } = require("./paths");
const { readJsonSafe, writeJsonAtomic } = require("./atomic");
const { DeliveryError } = require("./errors");

const INSTALLED_FILE = "installed.json";

function detectLegacyLayout(libraryRoot, gameIds) {
  const paths = resolveLibrary(libraryRoot);
  const found = [];

  for (const gameId of gameIds || []) {
    if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(String(gameId || ""))) continue;

    const legacyDir = path.join(paths.root, gameId);
    const targetDir = path.join(paths.games, gameId);

    let installed = null;
    try {
      installed = readJsonSafe(path.join(legacyDir, INSTALLED_FILE));
    } catch (e) {
      installed = null;
    }

    if (!installed || installed.gameId !== gameId) continue;
    if (fs.existsSync(targetDir)) continue;

    found.push({ gameId, from: legacyDir, to: targetDir, version: installed.version || null });
  }

  return found;
}

function migrateLegacyGame(libraryRoot, gameId) {
  const paths = resolveLibrary(libraryRoot);

  const from = assertInside(paths.root, path.join(paths.root, gameId), "Старая папка игры");
  const to = assertInside(paths.games, path.join(paths.games, gameId), "Новая папка игры");

  for (const reserved of [paths.games, paths.downloads, paths.backups, paths.internal])
    if (from.toLowerCase() === reserved.toLowerCase())
      throw new DeliveryError("PATH_INVALID", `Отказ мигрировать служебный каталог ${from}`);

  const installedFile = path.join(from, INSTALLED_FILE);
  const installed = readJsonSafe(installedFile);

  if (!installed || installed.gameId !== gameId)
    throw new DeliveryError(
      "PATH_INVALID",
      `В ${from} нет installed.json игры ${gameId} — мигрировать нечего`
    );

  if (fs.existsSync(to))
    throw new DeliveryError(
      "PATH_INVALID",
      `Целевой каталог уже существует: ${to} — миграция вручную`
    );

  fs.mkdirSync(paths.games, { recursive: true });

  fs.renameSync(from, to);

  try {
    const updated = Object.assign({}, installed, { installPath: to });
    writeJsonAtomic(path.join(to, INSTALLED_FILE), updated);
  } catch (error) {
    try {
      fs.renameSync(to, from);
    } catch (e) {
      throw new DeliveryError(
        "INSTALL_FAILED",
        `Перенос выполнен, но installed.json не обновлён и откат не удался: ${e.message}`
      );
    }

    throw new DeliveryError(
      "INSTALL_FAILED",
      `Не удалось обновить installed.json: ${error.message} — перенос отменён`
    );
  }

  return { ok: true, gameId, from, to };
}

module.exports = { detectLegacyLayout, migrateLegacyGame, INSTALLED_FILE, isInside };
